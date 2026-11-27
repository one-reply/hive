//! Create a worktree and optionally launch its first agent.
use super::*;
use crate::ui::{
    agent_logo,
    dialog::{field, initial_focus},
};
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
    primary: FocusHandle,
    cancel: FocusHandle,
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
        Self {
            projects,
            project,
            host,
            name,
            primary: cx.focus_handle(),
            cancel: cx.focus_handle(),
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

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.busy {
            return false;
        }
        if self.cancel.is_focused(window) {
            return true;
        }
        if self.name.focus_handle(cx).is_focused(window) || self.primary.is_focused(window) {
            self.submit(window, cx);
        }
        false
    }
}

impl Render for CreateWorktree {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let create_hover = Hover::new("create-start", window, cx);
        let cancel_hover = Hover::new("cancel-create", window, cx);
        v_flex()
            .gap_3()
            .child(field(
                "Task name",
                TextField::input(Input::new(&self.name).disabled(self.busy), &self.name, cx),
                cx,
            ))
            .child(field(
                "Base branch",
                Select::new(&self.base).disabled(self.busy).w_full(),
                cx,
            ))
            .child(field(
                "Start with",
                Select::new(&self.agent).disabled(self.busy).w_full(),
                cx,
            ))
            .when_some(self.error.clone(), |view, error| {
                view.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    // Git may sit in a checkout hook or a download for minutes, and the
                    // dialog cannot be dismissed meanwhile: Stop ends Git instead.
                    .child(if self.busy {
                        Button::new("stop-create")
                            .cursor_pointer()
                            .icon(Lucide::Square)
                            .label("Stop")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.projects
                                    .read(cx)
                                    .stop_creating(&this.host, &this.project)
                            }))
                            .smooth_hover(ButtonLook::Ghost, &cancel_hover, cx)
                    } else {
                        Button::new("cancel-create")
                            .cursor_pointer()
                            .track_focus(&self.cancel)
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx))
                            .smooth_hover(ButtonLook::Ghost, &cancel_hover, cx)
                    })
                    .child(
                        Button::new("create-start")
                            .cursor_pointer()
                            .track_focus(&self.primary)
                            .icon(Lucide::Play)
                            .label("Create & Start")
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
                            .smooth_hover(ButtonLook::Primary, &create_hover, cx),
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
    show_form(title, form, window, cx);
}

fn show_form(title: String, form: Entity<CreateWorktree>, window: &mut Window, cx: &mut App) {
    let focus_name = initial_focus(form.read(cx).name.focus_handle(cx));
    window.open_dialog(cx, move |dialog, window, cx| {
        focus_name(window, cx);
        // Closing drops the form and cancels its task, so dismissal waits until Git finishes.
        let dismissable = !form.read(cx).busy;
        dialog
            .title(title.clone())
            .w(WIDTH)
            .close_button(false)
            .overlay_closable(dismissable)
            .keyboard(dismissable)
            .child(form.clone())
            .on_ok({
                let form = form.clone();
                move |_, window, cx| form.update(cx, |form, cx| form.confirm(window, cx))
            })
    });
}

#[cfg(test)]
mod tests {
    use super::{CreateWorktree, Host, Projects, Store, show_form};
    use crate::ui::destructive::testing::{is_open, press, settle, window};
    use gpui_kit as gpui;
    use gpui_kit::{AppContext as _, Focusable as _, TestAppContext};

    #[gpui::test]
    fn enter_from_the_task_field_runs_creation_and_keeps_a_failed_form_open(
        cx: &mut TestAppContext,
    ) {
        let cx = window(cx);
        let form = cx.update(|window, cx| {
            let store = cx.new(Store::with_held_recovery);
            let projects = cx.new(|cx| Projects::new(&store, cx));
            let form = cx.new(|cx| {
                CreateWorktree::new(projects, "/removed".into(), Host::Local, window, cx)
            });
            show_form("New worktree".into(), form.clone(), window, cx);
            form
        });
        settle(cx);
        cx.update(|window, cx| {
            assert!(form.read(cx).name.focus_handle(cx).is_focused(window));
            // Clear the earlier base-branch lookup error to distinguish a new submission.
            form.update(cx, |form, _| form.error = None);
        });
        cx.simulate_input("feature");
        press("enter", cx);
        assert!(is_open(cx));
        cx.update(|window, cx| {
            let form = form.read(cx);
            assert_eq!(
                form.error.as_deref(),
                Some("Project is no longer available")
            );
            assert_eq!(form.name.read(cx).value().as_str(), "feature");
            window.focus(&form.cancel.clone(), cx);
        });
        settle(cx);
        press("enter", cx);
        assert!(!is_open(cx));
    }
}
