//! Create a worktree and optionally launch its first agent.
use super::*;
use crate::ui::agent_logo;
use gpui_kit::component::{
    Disableable as _, Icon, IndexPath,
    searchable_list::{SearchableListItem, SearchableVec},
    select::{Select, SelectState},
};
use hive_core::agent::{
    AgentKind,
    launch::{self, Launch},
};

const WIDTH: Pixels = px(480.);
const DEFAULT_BASE: &str = "Default branch";

/// A "Start with" choice: a plain terminal (`None`) or an agent.
#[derive(Clone, IntoElement)]
struct StartItem(Option<AgentKind>);

impl StartItem {
    fn name(&self) -> &'static str {
        self.0.map_or("Terminal", AgentKind::name)
    }
}

impl SearchableListItem for StartItem {
    type Value = Option<AgentKind>;
    fn title(&self) -> SharedString {
        self.name().into()
    }
    fn display_title(&self) -> Option<AnyElement> {
        Some(self.clone().into_any_element())
    }
    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.clone()
    }
    fn value(&self) -> &Option<AgentKind> {
        &self.0
    }
}

impl RenderOnce for StartItem {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let icon = match self.0 {
            Some(kind) => agent_logo::icon(kind, cx),
            None => Icon::new(Lucide::SquareTerminal),
        };
        h_flex()
            .min_w_0()
            .items_center()
            .gap_2()
            .child(icon.size_4().flex_none())
            .child(div().min_w_0().truncate().child(self.name()))
    }
}

struct CreateWorktree {
    projects: Entity<Projects>,
    project: PathBuf,
    host: Host,
    name: Entity<InputState>,
    base: Entity<SelectState<SearchableVec<String>>>,
    agent: Entity<SelectState<SearchableVec<StartItem>>>,
    error: Option<String>,
    busy: bool,
    _load: Task<()>,
    _create: Option<Task<()>>,
}

impl CreateWorktree {
    fn new(
        projects: Entity<Projects>,
        project: PathBuf,
        host: Host,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name =
            cx.new(|cx| InputState::new(window, cx).placeholder("Task name, or leave empty"));
        let base = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(vec![DEFAULT_BASE.to_owned()]),
                Some(IndexPath::default()),
                window,
                cx,
            )
            .searchable(true)
        });
        let agent = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(
                    std::iter::once(StartItem(None))
                        .chain(launch::AGENTS.map(|kind| StartItem(Some(kind))))
                        .collect::<Vec<_>>(),
                ),
                Some(IndexPath::default()),
                window,
                cx,
            )
        });
        let task = projects.read(cx).bases(&host, &project, cx);
        let load = cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(mut branches) => {
                        branches.insert(0, DEFAULT_BASE.to_owned());
                        this.base.update(cx, |base, cx| {
                            base.set_items(SearchableVec::new(branches), window, cx)
                        });
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        });
        name.focus_handle(cx).focus(window, cx);
        Self {
            projects,
            project,
            host,
            name,
            base,
            agent,
            error: None,
            busy: false,
            _load: load,
            _create: None,
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let name = self.name.read(cx).value().to_string();
        let base = self
            .base
            .read(cx)
            .selected_value()
            .filter(|s| s.as_str() != DEFAULT_BASE)
            .cloned()
            .unwrap_or_default();
        let launch = self
            .agent
            .read(cx)
            .selected_value()
            .copied()
            .flatten()
            .map(Launch::New);
        self.busy = true;
        self.error = None;
        let task = self.projects.update(cx, |projects, cx| {
            projects.create_worktree(&self.host, &self.project, name, base, launch.clone(), cx)
        });
        self._create = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(path) => {
                        window.close_dialog(cx);
                        let launch = launch.clone();
                        let host = this.host.clone();
                        window.defer(cx, move |_, cx| {
                            AppState::get(cx)
                                .agents
                                .clone()
                                .update(cx, |agents, cx| agents.start(host, path, launch, cx));
                        });
                    }
                    Err(error) => {
                        tracing::warn!(%error, "could not create and start worktree");
                        this.error = Some(error);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}

impl Render for CreateWorktree {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let create_hover = Hover::new("create-start", window, cx);
        let cancel_hover = Hover::new("cancel-create", window, cx);
        v_flex()
            .gap_3()
            .child("Task name")
            .child(TextField::input(
                Input::new(&self.name).disabled(self.busy),
                &self.name,
                cx,
            ))
            .child("Base branch")
            .child(Select::new(&self.base).disabled(self.busy).w_full())
            .child("Start with")
            .child(Select::new(&self.agent).disabled(self.busy).w_full())
            .when_some(self.error.clone(), |view, error| {
                view.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("cancel-create")
                            .cursor_pointer()
                            .label("Cancel")
                            .disabled(self.busy)
                            .on_click(|_, window, cx| window.close_dialog(cx))
                            .smooth_hover(ButtonLook::Ghost, &cancel_hover, cx),
                    )
                    .child(
                        Button::new("create-start")
                            .cursor_pointer()
                            .label("Create & Start")
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
                            .smooth_hover(ButtonLook::Outline, &create_hover, cx),
                    ),
            )
    }
}

pub(super) fn open(
    projects: Entity<Projects>,
    project: &Project,
    window: &mut Window,
    cx: &mut App,
) {
    let title = format!("New worktree in {}", project.name);
    let form = cx.new(|cx| {
        CreateWorktree::new(
            projects,
            project.path.clone(),
            project.host.clone(),
            window,
            cx,
        )
    });
    let initial_focus = std::cell::Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if initial_focus.replace(false) {
            let name = form.read(cx).name.clone();
            window.defer(cx, move |window, cx| {
                name.focus_handle(cx).focus(window, cx)
            });
        }
        // Closing drops the form and cancels its task, so dismissal waits until Git finishes.
        let dismissable = !form.read(cx).busy;
        dialog
            .title(title.clone())
            .w(WIDTH)
            .close_button(false)
            .overlay_closable(dismissable)
            .keyboard(dismissable)
            .child(form.clone())
    });
}
