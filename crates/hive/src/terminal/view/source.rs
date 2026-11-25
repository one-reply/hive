use super::*;
use crate::{app::AppState, state::documents::OpenFile};
use gpui_kit::component::{
    IndexPath,
    dialog::DialogButtonProps,
    input::Position,
    searchable_list::{SearchableListItem, SearchableVec},
    select::{Select, SelectState},
};
use hive_core::{projects::Target, terminal::SourceLink};

impl TerminalView {
    pub(super) fn open_source(
        &self,
        link: SourceLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((target, task)) = self.session.read(cx).source_files(&link.path, cx) else {
            window.push_notification(
                Notification::info("This terminal has no project worktree."),
                cx,
            );
            return;
        };
        let origin = self.session.entity_id();
        let position = Position::new(link.position.line, link.position.column);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if !this.focus_handle.is_focused(window)
                    || this.session.entity_id() != origin
                    || !AppState::get(cx).projects.read(cx).contains_target(&target)
                {
                    return;
                }
                match result {
                    Ok(paths) if paths.is_empty() => window.push_notification(
                        Notification::info(format!(
                            "No source file matches {} in this worktree.",
                            link.path.display()
                        )),
                        cx,
                    ),
                    Ok(mut paths) if paths.len() == 1 => {
                        open(&target, paths.remove(0), position, cx)
                    }
                    Ok(paths) => choose(&target, paths, position, window, cx),
                    Err(error) => window.push_notification(Notification::error(error), cx),
                }
            });
        })
        .detach();
    }
}

fn open(target: &Target, path: PathBuf, position: Position, cx: &mut App) {
    AppState::get(cx).documents.clone().update(cx, |docs, cx| {
        docs.request(
            OpenFile {
                target: target.clone(),
                path,
                position: Some(position),
                preview: false,
                placement: Default::default(),
            },
            cx,
        )
    });
}

#[derive(Clone)]
struct Choice(PathBuf);
impl SearchableListItem for Choice {
    type Value = PathBuf;
    fn title(&self) -> SharedString {
        self.0.display().to_string().into()
    }
    fn value(&self) -> &PathBuf {
        &self.0
    }
}

fn choose(
    target: &Target,
    paths: Vec<PathBuf>,
    position: Position,
    window: &mut Window,
    cx: &mut App,
) {
    let target = target.clone();
    let labels = paths.into_iter().map(Choice).collect::<Vec<_>>();
    let select = cx.new(|cx| {
        SelectState::new(
            SearchableVec::new(labels),
            Some(IndexPath::default()),
            window,
            cx,
        )
    });
    window.open_alert_dialog(cx, move |dialog, _, _| {
        let (select, target) = (select.clone(), target.clone());
        dialog
            .title("Choose source file")
            .child(Select::new(&select).w_full())
            .button_props(
                DialogButtonProps::default()
                    .ok_text("Open")
                    .show_cancel(true)
                    .on_ok(move |_, window, cx| {
                        let Some(path) = select.read(cx).selected_value().cloned() else {
                            return false;
                        };
                        let target = target.clone();
                        window.defer(cx, move |_, cx| open(&target, path, position, cx));
                        true
                    }),
            )
    });
}
