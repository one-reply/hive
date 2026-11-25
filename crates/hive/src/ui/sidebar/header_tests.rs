use super::{CARD_HEIGHT, ROW_GAP, Sidebar};
use crate::state::{projects::Projects, store::Store};
use crate::ui::hover::Hover;
use gpui_kit as gpui;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::{
    Context, Entity, IntoElement, KeyDownEvent, KeyUpEvent, Keystroke, Render, TestAppContext,
    Window, div, point, px,
};
use hive_core::{
    host::Host,
    identity::ProjectIdentity,
    projects::{Head, Project, Worktree},
    state::{StateError, WorktreeFilter},
};
use std::collections::{HashMap, HashSet};

struct Headers(Entity<Sidebar>);

impl Render for Headers {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.0.update(cx, |sidebar, cx| {
            let items = sidebar.projects.read(cx).items.clone();
            v_flex().w(px(300.)).children(
                items
                    .iter()
                    .enumerate()
                    .map(|(ix, project)| {
                        let open = !sidebar
                            .collapsed
                            .contains(&(project.host.clone(), project.path.clone()));
                        let hover = Hover::new(("project", ix), window, cx);
                        sidebar
                            .render_header(ix, project, open, &hover, window, cx)
                            .into_any_element()
                    })
                    .collect::<Vec<_>>(),
            )
        })
    }
}

fn project(name: &str) -> Project {
    Project {
        name: name.into(),
        path: format!("/{name}").into(),
        host: Host::Local,
        filter: WorktreeFilter::All,
        identity: ProjectIdentity::default(),
        worktrees: [format!("/{name}"), format!("/{name}-feature")]
            .into_iter()
            .map(|path| Worktree {
                path: path.into(),
                head: Head::Branch("main".into()),
            })
            .collect(),
    }
}

#[gpui::test]
fn project_names_expand_on_selection_and_toggle_when_active(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let store = cx.new(|cx| {
        Store::new(
            Err(StateError::Io {
                operation: "test",
                path: "/unused".into(),
                source: std::io::Error::other("persistence disabled for test"),
            }),
            hive_core::state::Access::Held,
            cx,
        )
    });
    store.update(cx, |store, _| {
        store.state.projects = ["/alpha", "/beta"]
            .into_iter()
            .map(|path| hive_core::state::ProjectRef::new(path.into(), Host::Local))
            .collect();
    });
    let projects = cx.new(|cx| Projects::new(&store, cx));
    cx.run_until_parked();
    projects.update(cx, |projects, _| {
        projects.items = vec![project("alpha"), project("beta")]
    });
    let sidebar = cx.new(|cx| Sidebar {
        projects: projects.clone(),
        store: store.clone(),
        collapsed: HashSet::from([
            (Host::Local, "/alpha".into()),
            (Host::Local, "/beta".into()),
        ]),
        toggled: HashMap::new(),
        agents_open: HashSet::new(),
        working: crate::ui::stepped_spinner::SteppedSpinner::new(cx),
        _observers: vec![
            cx.observe(&projects, |_, _, cx| cx.notify()),
            cx.observe(&store, |_, _, _| {}),
            cx.observe(&store, |_, _, _| {}),
            cx.observe(&store, |this: &mut Sidebar, _, cx| {
                this.forget_removed_projects(cx)
            }),
        ],
    });
    let (_, cx) = cx.add_window_view(|_, _| Headers(sidebar.clone()));
    for (x, y, selected, folded) in [
        (100., 14., "/alpha", [false, true]),
        (100., 14., "/alpha", [true, true]),
        (100., 14., "/alpha", [false, true]),
        (100., 42., "/beta", [false, false]),
        (100., 14., "/alpha", [false, false]),
        (100., 42., "/beta", [false, false]),
        (280., 14., "/beta", [true, false]),
        (100., 14., "/alpha", [false, false]),
        (280., 14., "/alpha", [true, false]),
        (280., 14., "/alpha", [false, false]),
    ] {
        cx.update(|window, cx| {
            window.activate_window();
            window.draw(cx).clear(cx);
        });
        cx.simulate_click(point(px(x), px(y)), Default::default());
        cx.run_until_parked();
        cx.read(|cx| {
            assert_eq!(
                projects.read(cx).target().unwrap().path,
                std::path::Path::new(selected)
            );
            for (path, folded) in ["/alpha", "/beta"].into_iter().zip(folded) {
                assert_eq!(
                    sidebar
                        .read(cx)
                        .collapsed
                        .contains(&(Host::Local, path.into())),
                    folded,
                    "{path} after clicking ({x}, {y})"
                );
            }
        });
    }
    cx.read(|cx| {
        let state = &store.read(cx).state;
        assert_eq!(
            state.projects[0].expansion,
            hive_core::state::ProjectExpansion::Expanded
        );
        let restored: hive_core::state::State =
            serde_json::from_slice(&serde_json::to_vec(state).unwrap()).unwrap();
        assert!(restored.projects[0].expanded_on_start(&restored.workspace));
    });
    projects.update(cx, |projects, cx| {
        projects.select(
            &Host::Local,
            "/alpha".as_ref(),
            Some("/alpha-feature".as_ref()),
            cx,
        )
    });
    for folded in [true, false] {
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        cx.simulate_click(point(px(100.), px(14.)), Default::default());
        cx.run_until_parked();
        cx.read(|cx| {
            assert_eq!(
                projects.read(cx).target().unwrap().path,
                std::path::Path::new("/alpha-feature")
            );
            assert_eq!(
                sidebar
                    .read(cx)
                    .collapsed
                    .contains(&(Host::Local, "/alpha".into())),
                folded
            );
            assert_eq!(
                store.read(cx).state.projects[0].expanded_on_start(&Default::default()),
                !folded
            );
        });
    }

    projects.update(cx, |projects, cx| {
        projects.select_project(&Host::Local, "/beta".as_ref(), cx)
    });
    sidebar.update(cx, |sidebar, cx| {
        sidebar.toggle_collapsed(&(Host::Local, "/alpha".into()), cx)
    });
    for (key, folded) in [("enter", false), ("space", true), ("space", false)] {
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        let keystroke =
            cx.update(|_, cx| Keystroke::parse_with_os(key, cx.platform_info().os).unwrap());
        cx.simulate_event(KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(KeyUpEvent { keystroke });
        cx.run_until_parked();
        cx.read(|cx| {
            assert_eq!(
                projects.read(cx).target().unwrap().path,
                std::path::Path::new("/alpha")
            );
            assert_eq!(
                sidebar
                    .read(cx)
                    .collapsed
                    .contains(&(Host::Local, "/alpha".into())),
                folded
            );
        });
    }
    sidebar.update(cx, |sidebar, cx| {
        sidebar.toggle_collapsed(&(Host::Local, "/alpha".into()), cx)
    });
    store.update(cx, |store, cx| {
        store.remove_project(&Host::Local, "/alpha".as_ref(), cx)
    });
    cx.run_until_parked();
    store.update(cx, |store, cx| {
        store.add_project(Host::Local, "/alpha".into(), cx)
    });
    cx.run_until_parked();
    cx.read(|cx| {
        assert!(
            !sidebar
                .read(cx)
                .collapsed
                .contains(&(Host::Local, "/alpha".into()))
        );
        assert_eq!(
            store.read(cx).state.projects.last().unwrap().expansion,
            hive_core::state::ProjectExpansion::Automatic
        );
    });
}

struct ProjectGroups(Entity<Sidebar>);

impl Render for ProjectGroups {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.0.update(cx, |sidebar, cx| {
            let items = sidebar.projects.read(cx).items.clone();
            v_flex().w(px(300.)).pl_1().pr_2().children(
                items
                    .iter()
                    .enumerate()
                    .map(|(ix, project)| {
                        let open = !sidebar
                            .collapsed
                            .contains(&(project.host.clone(), project.path.clone()));
                        let list = open.then(|| {
                            v_flex()
                                .children(project.worktrees.iter().enumerate().map(|(wt_ix, _)| {
                                    div()
                                        .debug_selector(move || {
                                            format!("project-{ix}-worktree-{wt_ix}")
                                        })
                                        .mt(ROW_GAP)
                                        .h(CARD_HEIGHT)
                                }))
                                .into_any_element()
                        });
                        sidebar.project_group(ix, project, open, list, window, cx)
                    })
                    .collect::<Vec<_>>(),
            )
        })
    }
}

#[gpui::test]
fn active_project_line_highlights_selected_project_and_all_its_worktrees(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_reduce_motion(true);
    });
    let store = cx.new(|cx| {
        Store::new(
            Err(StateError::Io {
                operation: "test",
                path: "/unused".into(),
                source: std::io::Error::other("persistence disabled for test"),
            }),
            hive_core::state::Access::Held,
            cx,
        )
    });
    store.update(cx, |store, _| {
        store.state.projects = ["/alpha", "/beta"]
            .into_iter()
            .map(|path| hive_core::state::ProjectRef::new(path.into(), Host::Local))
            .collect();
    });
    let projects = cx.new(|cx| Projects::new(&store, cx));
    cx.run_until_parked();
    projects.update(cx, |projects, cx| {
        projects.items = vec![project("alpha"), project("beta")];
        projects.select_project(&Host::Local, "/alpha".as_ref(), cx);
    });
    let sidebar = cx.new(|cx| Sidebar {
        projects: projects.clone(),
        store: store.clone(),
        collapsed: HashSet::new(),
        toggled: HashMap::new(),
        agents_open: HashSet::new(),
        working: crate::ui::stepped_spinner::SteppedSpinner::new(cx),
        _observers: vec![cx.observe(&projects, |_, _, cx| cx.notify())],
    });
    let (_, cx) = cx.add_window_view(|_, _| ProjectGroups(sidebar.clone()));
    cx.update(|window, cx| {
        window.activate_window();
        window.draw(cx).clear(cx);
    });
    let alpha_bar = cx
        .debug_bounds("project-active-bar-0")
        .expect("active project bar for alpha");
    assert!(cx.debug_bounds("project-active-bar-1").is_none());
    let alpha_last_worktree = cx
        .debug_bounds("project-0-worktree-1")
        .expect("last worktree of alpha");
    assert_eq!(alpha_bar.size.width, px(2.));
    assert!(alpha_bar.bottom() > alpha_last_worktree.top());
    assert!((alpha_bar.bottom() - (alpha_last_worktree.bottom() - px(2.))).abs() <= px(0.5));

    projects.update(cx, |projects, cx| {
        projects.select_project(&Host::Local, "/beta".as_ref(), cx);
    });
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert!(cx.debug_bounds("project-active-bar-0").is_none());
    let beta_bar = cx
        .debug_bounds("project-active-bar-1")
        .expect("active project bar for beta");
    let beta_last_worktree = cx
        .debug_bounds("project-1-worktree-1")
        .expect("last worktree of beta");
    assert!(beta_bar.bottom() > beta_last_worktree.top());

    sidebar.update(cx, |sidebar, cx| {
        sidebar.toggle_collapsed(&(Host::Local, "/beta".into()), cx);
    });
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let collapsed_beta_bar = cx
        .debug_bounds("project-active-bar-1")
        .expect("active project bar remains on collapsed beta");
    assert!(collapsed_beta_bar.size.height < beta_bar.size.height);
}
