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
mod library;
mod map;
mod run;

mod drawer;
pub(super) use drawer::PanelSlot;

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
    selected_template: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl PipelinePanel {
    fn new(project: PathBuf, cx: &mut Context<Self>) -> Self {
        let app = AppState::get(cx);
        let store = app.store.clone();
        let pipelines = app.pipelines.clone();
        let subscriptions = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.observe(&pipelines, |_, _, cx| cx.notify()),
        ];
        Self {
            project,
            focus: cx.focus_handle(),
            screen: Screen::Library,
            selected_template: None,
            _subscriptions: subscriptions,
        }
    }
    fn home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.screen = Screen::Library;
        self.focus.focus(window, cx);
        cx.notify();
    }
    fn show_run(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.screen = Screen::Run(cx.new(|cx| run::RunView::new(id, window, cx)));
        cx.notify();
    }
    pub(super) fn show_attempt(
        &mut self,
        ticket: hive_core::pipeline::Ticket,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Screen::Run(view) = &self.screen {
            view.update(cx, |v, cx| v.terminal(ticket, w, cx));
        }
    }
    fn edit(&mut self, original: Option<Template>, window: &mut Window, cx: &mut Context<Self>) {
        let parent = cx.weak_entity();
        self.screen =
            Screen::Editor(cx.new(|cx| editor::TemplateEditor::new(original, parent, window, cx)));
        cx.notify();
    }
    fn launch(&mut self, template: Template, window: &mut Window, cx: &mut Context<Self>) {
        let parent = cx.weak_entity();
        let project = self.project.clone();
        self.screen = Screen::Launch(
            cx.new(|cx| launch::LaunchForm::new(project, template, parent, window, cx)),
        );
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
            Screen::Library => self.library(window, cx),
            Screen::Editor(view) => view.clone().into_any_element(),
            Screen::Launch(view) => view.clone().into_any_element(),
            Screen::Run(view) => view.clone().into_any_element(),
        };
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
                    .on_click(cx.listener(|this, _, window, cx| this.home(window, cx)))
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
    match run.phase() {
        Planning => "Planning".into(),
        ApprovePlan => "Plan approval".into(),
        Development => "Development".into(),
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
/// A focusable card or row: pointer, Enter and Space run the same transition.
fn on_activate<V: 'static, E: StatefulInteractiveElement>(
    element: E,
    cx: &Context<V>,
    activate: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> E {
    let click = std::rc::Rc::new(activate);
    let key = click.clone();
    element
        .on_click(cx.listener(move |v, _, w, cx| click(v, w, cx)))
        .on_key_down(cx.listener(move |v, event: &KeyDownEvent, w, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                cx.stop_propagation();
                key(v, w, cx);
            }
        }))
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
