use super::Sidebar;
use crate::state::{projects::Projects, store::Store};
use gpui_kit as gpui;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::{
    Context, Entity, IntoElement, KeyDownEvent, KeyUpEvent, Keystroke, Render, TestAppContext,
    Window, point, px,
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
                        sidebar
                            .render_header(ix, project, open, window, cx)
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
        _observers: [
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
