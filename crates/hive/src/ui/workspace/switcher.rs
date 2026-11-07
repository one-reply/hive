//! One keyboard-first destination list for workspaces and agent attention.
use super::*;
use gpui_kit::component::{
    IndexPath,
    list::{List, ListDelegate, ListItem, ListState},
};

const WIDTH: Pixels = px(600.);
const HEIGHT: Pixels = px(420.);

struct Destination {
    host: Host,
    label: String,
    project: PathBuf,
    worktree: Option<PathBuf>,
    session: Option<EntityId>,
}

struct Destinations {
    attention: bool,
    query: String,
    entries: Vec<Destination>,
    selected: Option<IndexPath>,
}

impl Destinations {
    fn refresh(&mut self, cx: &App) {
        let app = AppState::get(cx);
        let agents = app.agents.read(cx);
        self.entries.clear();
        for project in &app.projects.read(cx).items {
            if !self.attention {
                self.entries.push(Destination {
                    host: project.host.clone(),
                    label: format!("Project · {}", project.name),
                    project: project.path.clone(),
                    worktree: project.worktrees.first().map(|w| w.path.clone()),
                    session: None,
                });
            }
            let paths: Vec<_> = if project.worktrees.is_empty() {
                vec![(&project.path, None)]
            } else {
                project
                    .worktrees
                    .iter()
                    .map(|w| (&w.path, Some(w.path.clone())))
                    .collect()
            };
            for (path, worktree) in paths {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if !self.attention && worktree.is_some() {
                    self.entries.push(Destination {
                        host: project.host.clone(),
                        label: format!("{} / {name}", project.name),
                        project: project.path.clone(),
                        worktree: worktree.clone(),
                        session: None,
                    });
                }
                for session in agents.sessions(&project.host, path) {
                    if self.attention && !session.unread {
                        continue;
                    }
                    let status = if !project.host.is_local() {
                        "SSH · status unavailable".to_owned()
                    } else if session.reported {
                        session.status.label().to_owned()
                    } else {
                        format!("{} · estimated", session.status.label())
                    };
                    let title = session.title.as_deref().unwrap_or(session.kind.name());
                    self.entries.push(Destination {
                        host: project.host.clone(),
                        label: format!("{} / {name} · {title} · {status}", project.name),
                        project: project.path.clone(),
                        worktree: worktree.clone(),
                        session: Some(session.session),
                    });
                }
            }
        }
        let query = self.query.to_lowercase();
        self.entries
            .retain(|entry| entry.label.to_lowercase().contains(&query));
    }
}

impl ListDelegate for Destinations {
    type Item = ListItem;
    fn items_count(&self, _: usize, _: &App) -> usize {
        self.entries.len()
    }
    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let entry = self.entries.get(ix.row)?;
        Some(
            ListItem::new(ix.row)
                .accessibility_label(entry.label.clone())
                .child(div().truncate().child(entry.label.clone())),
        )
    }
    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) {
        self.selected = ix;
    }
    fn perform_search(
        &mut self,
        query: &str,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.query = query.into();
        self.refresh(cx);
        Task::ready(())
    }
    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        let Some(entry) = self.selected.and_then(|ix| self.entries.get(ix.row)) else {
            return;
        };
        let project = entry.project.clone();
        let host = entry.host.clone();
        let worktree = entry.worktree.clone();
        let session = entry.session;
        window.close_dialog(cx);
        // Run after the dialog returns focus; otherwise it would steal focus from the pane.
        window.defer(cx, move |_, cx| {
            let app = AppState::get(cx);
            let (projects, agents) = (app.projects.clone(), app.agents.clone());
            projects.update(cx, |projects, cx| {
                projects.select(&host, &project, worktree.as_deref(), cx)
            });
            if let Some(session) = session {
                agents.update(cx, |agents, cx| agents.reveal(session, cx));
            }
        });
    }
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        window.close_dialog(cx);
    }
    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        div().p_4().child(if self.attention {
            "No agents need attention"
        } else {
            "No matching workspaces or agents"
        })
    }
}

pub(crate) fn open(attention: bool, window: &mut Window, cx: &mut App) {
    let mut delegate = Destinations {
        attention,
        query: String::new(),
        entries: Vec::new(),
        selected: None,
    };
    delegate.refresh(cx);
    let list = cx.new(|cx| ListState::new(delegate, window, cx).searchable(true));
    list.update(cx, |list, cx| {
        list.set_selected_index(Some(IndexPath::default()), window, cx);
    });
    let agents = AppState::get(cx).agents.clone();
    let weak = list.downgrade();
    // The dialog closure retains the subscription exactly as long as the list is visible.
    let subscription = cx.observe(&agents, move |_, cx| {
        let _ = weak.update(cx, |list, cx| {
            list.delegate_mut().refresh(cx);
            cx.notify();
        });
    });
    let initial_focus = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if initial_focus.replace(false) {
            let list = list.clone();
            window.defer(cx, move |window, cx| {
                list.update(cx, |list, cx| list.focus(window, cx))
            });
        }
        let _keep_alive = &subscription;
        dialog
            .title(if attention {
                "Needs attention"
            } else {
                "Go to workspace or agent"
            })
            .w(WIDTH)
            .child(
                List::new(&list)
                    .h(HEIGHT)
                    .search_placeholder("Search projects, worktrees, agents…"),
            )
    });
}
