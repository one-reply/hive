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
fn project_names_select_and_only_the_disclosure_button_folds(cx: &mut TestAppContext) {
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
    let projects = cx.new(|cx| Projects::new(&store, cx));
    cx.run_until_parked();
    projects.update(cx, |projects, _| {
        projects.items = vec![project("alpha"), project("beta")]
    });
    let sidebar = cx.new(|cx| Sidebar {
        projects: projects.clone(),
        store: store.clone(),
        collapsed: HashSet::new(),
        toggled: HashMap::new(),
        agents_open: HashSet::new(),
        working: crate::ui::stepped_spinner::SteppedSpinner::new(cx),
        _observers: [
            cx.observe(&projects, |_, _, cx| cx.notify()),
            cx.observe(&store, |_, _, _| {}),
            cx.observe(&store, |_, _, _| {}),
        ],
    });
    let (_, cx) = cx.add_window_view(|_, _| Headers(sidebar.clone()));
    for (x, y, selected, folded) in [
        (100., 14., "/alpha", false),
        (100., 42., "/beta", false),
        (280., 14., "/beta", true),
        (100., 14., "/alpha", true),
        (280., 14., "/alpha", false),
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
            assert_eq!(
                sidebar
                    .read(cx)
                    .collapsed
                    .contains(&(Host::Local, "/alpha".into())),
                folded
            );
        });
    }
    projects.update(cx, |projects, cx| {
        projects.select(
            &Host::Local,
            "/alpha".as_ref(),
            Some("/alpha-feature".as_ref()),
            cx,
        )
    });
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    cx.simulate_click(point(px(100.), px(14.)), Default::default());
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            projects.read(cx).target().unwrap().path,
            std::path::Path::new("/alpha-feature")
        )
    });

    projects.update(cx, |projects, cx| {
        projects.select_project(&Host::Local, "/beta".as_ref(), cx)
    });
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let keystroke =
        cx.update(|_, cx| Keystroke::parse_with_os("enter", cx.platform_info().os).unwrap());
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
        )
    });
}
