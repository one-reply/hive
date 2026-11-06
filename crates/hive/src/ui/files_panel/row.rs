use super::*;
use crate::ui::hover::SmoothHoverBg as _;
use gpui_kit::base::motion::{Transition, transition};
use gpui_kit::component::{Icon, list::ListItem, tree::TreeEntry};
use hive_core::{files::decorations::Decoration, git::status::Change, settings::FilesSettings};

/// A quarter turn: the right-pointing chevron points down when its folder is open.
const CHEVRON_OPEN: f32 = std::f32::consts::FRAC_PI_2;
const CHEVRON_DURATION: Duration = Duration::from_millis(160);
/// Matches the selected worktree card in the sidebar: a neutral tint of the foreground
/// instead of the theme's blue `list_active`, so selection reads the same across panels.
const SELECTED_BACKGROUND_OPACITY: f32 = 0.09;
const SELECTED_BORDER_OPACITY: f32 = 0.14;

pub(super) fn render(
    ix: usize,
    entry: &TreeEntry,
    selected: bool,
    panel: &Entity<FilesPanel>,
    settings: &FilesSettings,
    window: &mut Window,
    cx: &mut App,
) -> ListItem {
    let path = PathBuf::from(entry.item().id.as_ref());
    let files = panel.read(cx).files.clone();
    let state = files.read(cx);
    let folder = entry.is_folder();
    let missing = state.missing.contains(&path);
    let unsupported = state.entry(&path).is_some_and(|e| e.unsupported);
    let ignored = state.ignored.contains(&path);
    let dirty = state.dirty.contains(&path);
    let decoration = settings
        .git_decorations
        .then(|| state.decorations.get(&path))
        .flatten()
        .cloned();
    let target = state.target.clone();
    let busy = state.busy;
    let rename_field = panel
        .read(cx)
        .edit
        .as_ref()
        .filter(|edit| edit.renaming() == Some(path.as_path()))
        .map(|edit| edit.field(busy, settings.font_size(), window, cx));
    let renaming = rename_field.is_some();
    let scale = settings.font_size() / LABEL_SIZE;
    let row_key: SharedString = format!("files-row:{:?}:{}", target, path.display()).into();
    let hover = Hover::new(row_key.clone(), window, cx);
    let motion = panel.read(cx).motion.sample(ix);
    if motion.is_some() {
        window.request_animation_frame();
    }
    let chevron_turn = transition(
        (row_key.clone(), "chevron"),
        if entry.is_expanded() {
            CHEVRON_OPEN
        } else {
            0.
        },
        Transition::new(CHEVRON_DURATION),
        window,
        cx,
    );
    let row_height = settings.row_height() / 16.;
    let foreground = cx.theme().foreground;
    let sidebar = cx.theme().sidebar;
    let background = if selected {
        sidebar.blend(foreground.opacity(SELECTED_BACKGROUND_OPACITY))
    } else {
        sidebar
    };
    let detail = description(
        &path,
        decoration.as_ref(),
        dirty,
        ignored,
        missing,
        unsupported,
    );
    let mut content = h_flex()
        .id(row_key)
        .relative()
        .w_full()
        .min_w_0()
        .h(rems(row_height))
        .gap(rems(4. / 16.))
        .pl(rems((4. + entry.depth() as f32 * INDENT * scale) / 16.))
        .pr_2()
        .rounded_sm()
        .border_1()
        .border_color(if selected {
            sidebar.blend(foreground.opacity(SELECTED_BORDER_OPACITY))
        } else {
            background
        })
        .bg(background)
        .cursor_pointer()
        .text_size(rems(settings.font_size() / 16.))
        .text_color(if missing || ignored || entry.is_disabled() {
            cx.theme().muted_foreground
        } else {
            foreground
        })
        .when(!renaming, |row| {
            row.tooltip(move |window, cx| Tooltip::new(detail.clone()).build(window, cx))
        })
        .children((0..entry.depth()).map(|depth| {
            div()
                .absolute()
                .left(rems((10. + depth as f32 * INDENT * scale) / 16.))
                .top_0()
                .bottom_0()
                .w(px(1.))
                .bg(cx.theme().border.opacity(0.6))
        }))
        .child(
            div()
                .w(rems(12. * scale / 16.))
                .flex_none()
                .when(folder, |slot| {
                    slot.child(
                        Icon::new(Lucide::ChevronRight)
                            .rotate(radians(chevron_turn))
                            .size(rems(12. * scale / 16.))
                            .text_color(cx.theme().muted_foreground),
                    )
                }),
        )
        .when(!entry.is_disabled(), |row| {
            row.child(icons::render(
                &path,
                folder,
                entry.is_expanded(),
                settings.icons,
                rems(ICON_SIZE * scale / 16.),
                cx,
            ))
        })
        .child(match rename_field {
            Some(field) => div()
                .key_context(editing::EDIT_CONTEXT)
                .flex_1()
                .min_w_0()
                // The press belongs to the field: it must not select, toggle or drag the row.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(field),
            None => div()
                .flex_1()
                .min_w_0()
                .truncate()
                .when(missing, |label| label.line_through())
                .child(entry.item().label.clone()),
        })
        .when(dirty, |row| {
            row.child(
                div()
                    .flex_none()
                    .size(rems(5. * scale / 16.))
                    .rounded_full()
                    .bg(foreground),
            )
        })
        .when_some(decoration, |row, decoration| {
            row.child(badge(&decoration, folder, scale, cx))
        })
        .hover_bg(
            &hover,
            background,
            if selected {
                background
            } else {
                background.blend(foreground.opacity(0.06))
            },
        );
    if folder && !entry.is_disabled() {
        let (drop_panel, hover_panel, destination) = (panel.clone(), panel.clone(), path.clone());
        let hover_destination = path.clone();
        content = content
            .drag_over::<moving::DraggedFile>(|style, _, _, cx| {
                style
                    .bg(cx.theme().drop_target)
                    .border_1()
                    .border_color(cx.theme().drag_border)
            })
            .on_drag_move(
                move |event: &DragMoveEvent<moving::DraggedFile>, window, cx| {
                    if event.bounds.contains(&event.event.position) {
                        let drag = event.drag(cx).clone();
                        hover_panel.update(cx, |p, cx| {
                            p.drag_expand(hover_destination.clone(), &drag, window, cx)
                        });
                    } else {
                        hover_panel.update(cx, |p, _| {
                            if p.drag_hover
                                .as_ref()
                                .is_some_and(|(path, _)| *path == hover_destination)
                            {
                                p.drag_hover = None;
                                p._drag_expand = None;
                            }
                        });
                    }
                },
            )
            .on_drop(move |drag: &moving::DraggedFile, window, cx| {
                drop_panel.update(cx, |p, cx| p.drop_file(drag, &destination, window, cx));
            });
    }
    if !missing
        && !unsupported
        && !entry.is_disabled()
        && !busy
        && !renaming
        && let Some(target) = target
    {
        let dragged = moving::DraggedFile {
            target,
            path: path.clone(),
        };
        content = content.on_drag(dragged, |drag, _, _, cx| cx.new(|_| drag.clone()));
    }
    ListItem::new(ix)
        .px_0()
        .py_0()
        .rounded_sm()
        .when_some(motion, |item, motion| {
            item.top(rems(motion.offset * row_height))
                .opacity(motion.opacity)
        })
        .child(content)
        .on_click(move |event, _, cx| {
            if !folder && !missing && !unsupported && !renaming {
                files.update(cx, |files, cx| {
                    files.open(path.clone(), event.click_count() < 2, cx)
                });
            }
        })
}

fn description(
    path: &Path,
    status: Option<&Decoration>,
    dirty: bool,
    ignored: bool,
    missing: bool,
    unsupported: bool,
) -> String {
    let mut value = path.display().to_string();
    if let Some(status) = status {
        if let Some(change) = status.staged {
            value.push_str(&format!("\nStaged: {}", label(change)));
        }
        if let Some(change) = status.unstaged {
            value.push_str(&format!("\nUnstaged: {}", label(change)));
        }
        if status.descendants {
            value.push_str(if status.conflicted {
                "\nContains merge conflicts"
            } else {
                "\nContains uncommitted changes"
            });
        }
    }
    if dirty {
        value.push_str("\nUnsaved changes in editor");
    }
    if ignored {
        value.push_str("\nIgnored by Git");
    }
    if missing {
        value.push_str("\nDeleted on disk · Show Changes to review");
    }
    if unsupported {
        value.push_str("\nSymlinks and special files cannot be opened or modified");
    }
    value
}
fn label(change: Change) -> &'static str {
    match change {
        Change::Modified => "Modified",
        Change::Added => "Added",
        Change::Deleted => "Deleted",
        Change::Renamed => "Renamed",
        Change::Copied => "Copied",
        Change::TypeChanged => "Type changed",
        Change::Untracked => "Untracked",
        Change::Conflicted => "Conflict",
    }
}
fn code(change: Option<Change>) -> &'static str {
    match change {
        Some(Change::Modified) => "M",
        Some(Change::Added) => "A",
        Some(Change::Deleted) => "D",
        Some(Change::Renamed) => "R",
        Some(Change::Copied) => "C",
        Some(Change::TypeChanged) => "T",
        Some(Change::Untracked) => "?",
        Some(Change::Conflicted) => "!",
        None => " ",
    }
}
fn color(change: Option<Change>, cx: &App) -> Hsla {
    match change {
        Some(Change::Added | Change::Untracked) => cx.theme().green,
        Some(Change::Deleted | Change::Conflicted) => cx.theme().danger,
        Some(Change::Renamed | Change::Copied) => cx.theme().blue,
        _ => cx.theme().yellow,
    }
}
fn badge(status: &Decoration, folder: bool, scale: f32, cx: &App) -> AnyElement {
    if folder && status.descendants {
        return Icon::new(if status.conflicted {
            Lucide::CircleAlert
        } else {
            Lucide::Circle
        })
        .size(rems(9. * scale / 16.))
        .flex_none()
        .text_color(if status.conflicted {
            cx.theme().danger
        } else {
            cx.theme().yellow
        })
        .into_any_element();
    }
    h_flex()
        .flex_none()
        .text_size(rems(META_SIZE * scale / 16.))
        .font_family(cx.theme().mono_font_family.clone())
        .child(
            div()
                .w(rems(9. * scale / 16.))
                .text_color(color(status.staged, cx))
                .child(code(status.staged)),
        )
        .child(
            div()
                .w(rems(9. * scale / 16.))
                .text_color(color(status.unstaged, cx))
                .child(code(status.unstaged)),
        )
        .into_any_element()
}
