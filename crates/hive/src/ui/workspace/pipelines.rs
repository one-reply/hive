//! The drawer covers the selected project's dock and inspector; its view survives project switches.
use super::*;
use crate::state::pipelines::RunStatus;
use crate::ui::text_field::TextField;
use gpui_kit::component::{
    Size as ButtonSize,
    input::{Input, InputState, Textarea, TextareaState},
    menu::{DropdownMenu as _, PopupMenuItem},
};
use hive_core::pipeline::{Run, Template};
use std::path::Path;

mod editor;
mod inspector;
mod launch;
pub(crate) use launch::task_key_bindings;
mod library;
mod map;
mod run;

mod drawer;
pub(super) use drawer::PanelSlot;
mod navigation;
use navigation::{Destination, Unsaved};
mod overview;
pub(crate) use overview::PipelinesOverview;

enum Screen {
    Library,
    Editor(Entity<editor::TemplateEditor>),
    Launch(Entity<launch::LaunchForm>),
    Run(Entity<run::RunView>),
}

pub(super) struct PipelinePanel {
    project: PathBuf,
    pub(super) focus: FocusHandle,
    screen: Screen,
    draft: Option<Entity<launch::LaunchForm>>,
    selected_template: Option<String>,
    /// Scroll position and measured rows of the library page.
    history: ListState,
    /// What `history` lists, to tell rows that moved from rows that were replaced.
    entries: Rc<[library::Entry]>,
    /// The pending repaint that words the ages of the listed runs anew.
    age_tick: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl PipelinePanel {
    fn new(project: PathBuf, cx: &mut Context<Self>) -> Self {
        let app = AppState::get(cx);
        let store = app.store.clone();
        let pipelines = app.pipelines.clone();
        let agents = app.agents.clone();
        // The panel is drawn from a cache, so it repaints only when it is told to. It and
        // the screens inside it show nothing from another entity that is not observed here
        // or by the screen itself.
        let subscriptions = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.observe(&pipelines, |_, _, cx| cx.notify()),
            // A run's status follows what its agent last reported.
            cx.observe(&agents, |_, _, cx| cx.notify()),
        ];
        Self {
            project,
            focus: cx.focus_handle(),
            screen: Screen::Library,
            draft: None,
            selected_template: None,
            history: Self::history_list(),
            entries: Rc::from([]),
            age_tick: None,
            _subscriptions: subscriptions,
        }
    }
    fn history_list() -> ListState {
        ListState::new(0, ListAlignment::Top, library::HISTORY_OVERDRAW)
    }
    /// Back to the library once the screen in view is done with: saved or discarded.
    fn home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.go(Destination::Library, window, cx);
    }
    /// Shows run `id`, on the live session of `attempt` when one is named.
    pub(super) fn show_run(
        &mut self,
        id: u64,
        attempt: Option<hive_core::pipeline::Ticket>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate(Destination::Run { id, attempt }, window, cx);
    }
    /// Shows the launch form with the draft saved for this project.
    pub(super) fn show_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.navigate(Destination::Draft, window, cx);
    }
    /// The launch form started run `id`. The form is finished, so nothing is asked.
    fn run_started(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.draft = None;
        self.go(Destination::Run { id, attempt: None }, window, cx);
    }
    fn edit(&mut self, original: Option<Template>, window: &mut Window, cx: &mut Context<Self>) {
        let parent = cx.weak_entity();
        self.screen =
            Screen::Editor(cx.new(|cx| editor::TemplateEditor::new(original, parent, window, cx)));
        cx.notify();
    }
    fn discard_draft(&mut self, cx: &mut Context<Self>) {
        self.draft = None;
        AppState::get(cx).store.clone().update(cx, |s, cx| {
            s.edit_pipelines(cx, |l| l.drafts.retain(|d| d.owner != self.project))
        });
    }
    /// The recipe of the draft this project would resume: an open form the user has changed,
    /// or one saved earlier. A form left as it opened is not a draft.
    fn draft_template(&self, cx: &App) -> Option<Template> {
        if let Some(form) = self.draft.as_ref().map(|form| form.read(cx))
            && form.is_modified(cx)
        {
            return Some(form.template().clone());
        }
        let drafts = &AppState::get(cx).store.read(cx).state.pipelines.drafts;
        drafts
            .iter()
            .find(|d| d.owner == self.project)
            .map(|d| d.template.clone())
    }
    /// Start from the task, recipe and repositories saved in this run.
    fn launch_again(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.draft_template(cx).is_some() {
            let panel = cx.weak_entity();
            window.open_alert_dialog(cx, move |alert, _, _| {
                let panel = panel.clone();
                let alert = alert.confirm().title("Replace the current run draft?");
                crate::ui::destructive::confirmation(alert, "Replace draft", move |w, cx| {
                    let _ = panel.update(cx, |p, cx| {
                        // The replaced form must not write itself back afterwards.
                        if let Some(form) = &p.draft {
                            form.update(cx, |form, _| form.abandon());
                        }
                        p.discard_draft(cx);
                        p.launch_again(id, w, cx);
                    });
                    true
                })
            });
            return;
        }
        let library = &AppState::get(cx).store.read(cx).state.pipelines;
        let Some(run) = library.runs.iter().find(|run| run.id() == id).cloned() else {
            return;
        };
        let template = run.template().clone();
        let parent = cx.weak_entity();
        let project = self.project.clone();
        // An untouched form may still be open; the repeated run takes its place.
        let draft = cx.new(|cx| {
            let mut form = launch::LaunchForm::new(project, template, parent, window, cx);
            form.repeat(&run, window, cx);
            form
        });
        self.screen = Screen::Launch(draft.clone());
        self.draft = Some(draft);
        cx.notify();
    }
    fn launch(&mut self, template: Template, window: &mut Window, cx: &mut Context<Self>) {
        let parent = cx.weak_entity();
        let project = self.project.clone();
        // A form left as it opened holds nothing to resume: the chosen recipe starts anew.
        if self.draft_template(cx).is_none() {
            self.draft = None;
        }
        let draft = self.draft.get_or_insert_with(|| {
            let saved = AppState::get(cx)
                .store
                .read(cx)
                .state
                .pipelines
                .drafts
                .iter()
                .find(|d| d.owner == project)
                .cloned();
            cx.new(|cx| {
                let mut form = launch::LaunchForm::new(project, template, parent, window, cx);
                if let Some(saved) = saved {
                    form.restore_draft(saved, window, cx);
                }
                form
            })
        });
        self.screen = Screen::Launch(draft.clone());
        cx.notify();
    }
}

impl Render for PipelinePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = match &self.screen {
            Screen::Editor(_) => "Template editor",
            Screen::Launch(_) => "New run",
            Screen::Run(_) => "Run",
            Screen::Library => "Library",
        };
        let content = match &self.screen {
            Screen::Library => None,
            Screen::Editor(view) => Some(view.clone().into_any_element()),
            Screen::Launch(view) => Some(view.clone().into_any_element()),
            Screen::Run(view) => Some(view.clone().into_any_element()),
        };
        let content = content.unwrap_or_else(|| self.library(cx));
        let home_hover = Hover::new("pipeline-home", window, cx);
        let bar = h_flex()
            .h(TAB_BAR_HEIGHT)
            .flex_none()
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("pipeline-home")
                    .cursor_pointer()
                    .small()
                    .icon(Lucide::Workflow)
                    .label("Pipelines")
                    .tooltip("Back to project pipelines")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.navigate(Destination::Library, window, cx)
                    }))
                    .smooth_hover(ButtonLook::Outline, &home_hover, cx),
            )
            .child(
                Icon::new(Lucide::ChevronRight)
                    .size_3()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(subtle(title, cx));
        v_flex()
            .size_full()
            .overflow_hidden()
            .track_focus(&self.focus)
            .child(bar)
            .child(content)
    }
}

fn phase_label(run: &Run) -> String {
    use hive_core::pipeline::Phase::*;
    if matches!(run.phase(), Planning | Development)
        && let Some(lane) = run
            .lanes()
            .iter()
            .find(|l| l.worktree.as_os_str().is_empty())
    {
        return if lane.setup.trim().is_empty() {
            format!("Preparing {}", lane.name)
        } else {
            format!("Preparing {} · {}", lane.name, lane.setup.trim())
        };
    }
    match run.phase() {
        Planning => "Planning".into(),
        ApprovePlan => "Plan approval".into(),
        Development => "Development".into(),
        Regression(step) => match step {
            hive_core::pipeline::RegressionStep::Choose => "Select regression criterion",
            hive_core::pipeline::RegressionStep::Author => "Writing regression test",
            hive_core::pipeline::RegressionStep::Baseline => "Reproducing on base",
            hive_core::pipeline::RegressionStep::Candidate => "Checking the fix",
            hive_core::pipeline::RegressionStep::Decision => "Regression needs a decision",
        }
        .into(),
        Review(i) => run
            .template()
            .reviews
            .get(i)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| "Review".into()),
        Commands(_) => "Required checks".into(),
        Decision(_) => "Decision needed".into(),
        Acceptance => "Ready to accept".into(),
        Accepted => "Accepted".into(),
        Cancelled => "Cancelled".into(),
    }
}
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
/// Status tints are washed out so the colored label on top stays readable.
const STATUS_TINT_OPACITY: f32 = 0.12;
fn status_color(status: RunStatus, cx: &App) -> Hsla {
    match status {
        RunStatus::Running => cx.theme().info,
        RunStatus::NeedsYou => cx.theme().warning,
        RunStatus::Failed => cx.theme().danger,
        RunStatus::Completed => cx.theme().success,
        RunStatus::Cancelled => cx.theme().muted_foreground,
    }
}
fn status_icon(status: RunStatus) -> Lucide {
    match status {
        RunStatus::Running => Lucide::Activity,
        RunStatus::NeedsYou => Lucide::UserRoundCheck,
        RunStatus::Failed => Lucide::CircleAlert,
        RunStatus::Completed => Lucide::CircleCheck,
        RunStatus::Cancelled => Lucide::CircleX,
    }
}
/// A tinted pill with a dot, so a status reads by color before its label is read.
fn status_pill(label: impl Into<SharedString>, color: Hsla) -> Div {
    h_flex()
        .flex_none()
        .gap_1p5()
        .px_2()
        .py_0p5()
        .rounded_full()
        .bg(color.opacity(STATUS_TINT_OPACITY))
        .child(div().size_1p5().rounded_full().bg(color))
        .child(div().text_xs().text_color(color).child(label.into()))
}
fn path_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}
fn heading(text: impl Into<SharedString>) -> Div {
    div()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}
fn subtle(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}
/// Marks a field that must be filled before its form can be submitted.
fn required_mark(cx: &App) -> Div {
    div().text_xs().text_color(cx.theme().danger).child("*")
}
fn card(cx: &App) -> Div {
    v_flex()
        .flex_none()
        .p_3()
        .gap_2()
        .border_1()
        .border_color(cx.theme().border)
        .rounded_md()
        .bg(cx.theme().sidebar)
}
fn scroll(id: &'static str, body: impl IntoElement) -> AnyElement {
    div()
        .id(id)
        .h_full()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .overflow_y_scroll()
        .child(v_flex().p_4().gap_3().child(body))
        .into_any_element()
}
fn button<V: 'static>(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    look: ButtonLook,
    window: &mut Window,
    cx: &mut Context<V>,
    click: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> Stateful<Div> {
    button_with_size(id, label, look, ButtonSize::Small, window, cx, click)
}
fn button_with_size<V: 'static>(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    look: ButtonLook,
    size: ButtonSize,
    window: &mut Window,
    cx: &mut Context<V>,
    click: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> Stateful<Div> {
    let id = id.into();
    let hover = Hover::new(id.clone(), window, cx);
    Button::new(id)
        .cursor_pointer()
        .with_size(size)
        .label(label)
        .on_click(cx.listener(move |this, _, w, cx| click(this, w, cx)))
        .smooth_hover(look, &hover, cx)
}
/// A small labeled button led by an icon that says what it does before the label is read.
fn button_with_icon<V: 'static>(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    icon: Lucide,
    look: ButtonLook,
    window: &mut Window,
    cx: &mut Context<V>,
    click: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> Stateful<Div> {
    let id = id.into();
    let hover = Hover::new(id.clone(), window, cx);
    Button::new(id)
        .cursor_pointer()
        .small()
        .icon(icon)
        .label(label)
        .on_click(cx.listener(move |this, _, w, cx| click(this, w, cx)))
        .smooth_hover(look, &hover, cx)
}
/// A focusable card or row: pointer, Enter and Space run the same transition. GPUI turns
/// Enter or Space released on a focused element into a click, so a key-down handler of our
/// own would run the transition a second time.
fn on_activate<V: 'static, E: StatefulInteractiveElement>(
    element: E,
    cx: &Context<V>,
    activate: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> E {
    element.on_click(cx.listener(move |v, _, w, cx| activate(v, w, cx)))
}
fn icon_action<V: 'static>(
    id: impl Into<ElementId>,
    label: &'static str,
    icon: Lucide,
    w: &mut Window,
    cx: &mut Context<V>,
    click: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> Stateful<Div> {
    let id = id.into();
    let hover = Hover::new(id.clone(), w, cx);
    Button::new(id)
        .cursor_pointer()
        .small()
        .icon(icon)
        .accessibility_label(label)
        .tooltip(label)
        .on_click(cx.listener(move |v, _, w, cx| click(v, w, cx)))
        .smooth_hover(ButtonLook::Ghost, &hover, cx)
}
fn input(
    value: &str,
    placeholder: &'static str,
    window: &mut Window,
    cx: &mut App,
) -> Entity<InputState> {
    cx.new(|cx| {
        InputState::new(window, cx)
            .default_value(value.to_owned())
            .placeholder(placeholder)
    })
}
fn area(
    value: &str,
    placeholder: &'static str,
    window: &mut Window,
    cx: &mut App,
) -> Entity<TextareaState> {
    cx.new(|cx| {
        TextareaState::new(window, cx)
            .rows(3)
            .default_value(value.to_owned())
            .placeholder(placeholder)
    })
}
fn field(label: &'static str, input: &Entity<InputState>, cx: &App) -> Div {
    v_flex()
        .gap_1()
        .child(subtle(label, cx))
        .child(TextField::input(Input::new(input), input, cx))
}
fn paragraph(label: &'static str, input: &Entity<TextareaState>, cx: &App) -> Div {
    v_flex()
        .gap_1()
        .child(subtle(label, cx))
        .child(TextField::textarea(Textarea::new(input), input, cx))
}

fn choice<V: 'static, T: Clone + PartialEq + 'static>(
    id: impl Into<ElementId>,
    current: &T,
    options: Vec<(T, String)>,
    cx: &mut Context<V>,
    select: impl Fn(&mut V, T, &mut Window, &mut Context<V>) + 'static,
) -> AnyElement {
    let label = options
        .iter()
        .find(|(v, _)| v == current)
        .map(|(_, s)| s.clone())
        .unwrap_or_else(|| "Select…".into());
    let options = options
        .into_iter()
        .map(|(v, label)| (label, &v == current, v))
        .collect();
    // Full width and outlined, so it lines up with the model picker as one form.
    options_menu(
        Button::new(id)
            .small()
            .outline()
            .w_full()
            .cursor_pointer()
            .dropdown_caret(true)
            .label(label),
        options,
        cx,
        select,
    )
}

fn options_menu<V: 'static, T: Clone + 'static>(
    trigger: Button,
    options: Vec<(String, bool, T)>,
    cx: &mut Context<V>,
    select: impl Fn(&mut V, T, &mut Window, &mut Context<V>) + 'static,
) -> AnyElement {
    let view = cx.weak_entity();
    let select = Rc::new(select);
    trigger
        .dropdown_menu(move |mut menu, _, _| {
            for (label, checked, item) in &options {
                let view = view.clone();
                let select = select.clone();
                let value = item.clone();
                menu = menu.item(
                    PopupMenuItem::new(label.clone())
                        .checked(*checked)
                        .on_click(move |_, w, cx| {
                            let _ = view.update(cx, |this, cx| {
                                select(this, value.clone(), w, cx);
                                cx.notify();
                            });
                        }),
                );
            }
            menu
        })
        .into_any_element()
}

#[cfg(test)]
mod tests;
