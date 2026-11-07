use super::*;
use crate::ui::hover::SmoothHoverBg as _;
use gpui_kit::base::motion::{Transition, transition};
use gpui_kit::component::{Icon, list::ListItem, tree::TreeEntry};
use hive_core::{files::decorations::Decoration, git::status::Change, settings::FilesSettings};
use std::{cell::Cell, rc::Rc};

mod appearance;

/// A quarter turn: the right-pointing chevron points down when its folder is open.
const CHEVRON_OPEN: f32 = std::f32::consts::FRAC_PI_2;
const CHEVRON_DURATION: Duration = Duration::from_millis(160);
/// Matches the selected worktree card in the sidebar: a neutral tint of the foreground
/// instead of the theme's blue `list_active`, so selection reads the same across panels.
const SELECTED_BACKGROUND_OPACITY: f32 = 0.09;
const SELECTED_BORDER_OPACITY: f32 = 0.14;
const SEARCH_HIGHLIGHT_OPACITY: f32 = 0.15;

pub(super) fn render(
    ix: usize,
    entry: &TreeEntry,
    selected: bool,
    panel: &Entity<FilesPanel>,
    settings: &FilesSettings,
    window: &mut Window,
    cx: &mut App,
) -> ListItem {
    let path = tree::path(entry.item().id.as_ref());
    let files = panel.read(cx).files.clone();
    let state = files.read(cx);
    let folder = entry.is_folder();
    let missing = state.missing.contains(&path);
    let unsupported = state.entry(&path).is_some_and(|e| e.unsupported);
    let disabled = entry.is_disabled();
    let selected = if !folder && !missing && !unsupported && !disabled {
        panel.read(cx).selection.contains(&path)
    } else {
        selected
    };
    let ignored = state.ignored.contains(&path);
    let dirty = state.dirty.contains(&path);
    let decoration = settings
        .git_decorations
        .then(|| state.decorations.get(&path))
        .flatten()
        .cloned();
    let target = state.target.clone();
    let busy = state.busy;
    let highlights = state
        .search_highlights(&path)
        .iter()
        .cloned()
        .map(|range| {
            (
                range,
                HighlightStyle {
                    color: Some(cx.theme().primary),
                    background_color: Some(cx.theme().primary.opacity(SEARCH_HIGHLIGHT_OPACITY)),
                    font_weight: Some(FontWeight::SEMIBOLD),
                    ..Default::default()
                },
            )
        })
        .collect::<Vec<_>>();
    let rename_field = panel
        .read(cx)
        .edit
        .as_ref()
        .filter(|edit| edit.renaming() == Some(path.as_path()))
        .map(|edit| edit.field(busy, settings.font_size(), window, cx));
    let renaming = rename_field.is_some();
    let row_key: SharedString = format!("files-row:{target:?}:{}", entry.item().id).into();
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
    let detail = description(
        &path,
        decoration.as_ref(),
        dirty,
        ignored,
        missing,
        unsupported,
    );
    let appearance = appearance::Appearance {
        path: path.clone(),
        label: entry.item().label.clone(),
        depth: entry.depth(),
        folder,
        expanded: entry.is_expanded(),
        disabled: entry.is_disabled(),
        settings: settings.clone(),
        selected,
        missing,
        ignored,
        dirty,
        decoration,
        highlights,
        chevron_turn,
    };
    let background = appearance.background(cx);
    let mut content = appearance
        .render(rename_field, cx)
        .id(row_key)
        .cursor_pointer()
        .when(!renaming, |row| {
            row.tooltip(move |window, cx| Tooltip::new(detail.clone()).build(window, cx))
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
        let drop_target = target.clone();
        let drop_destination = path.clone();
        let (drop_panel, hover_panel, destination) = (panel.clone(), panel.clone(), path.clone());
        let hover_destination = path.clone();
        content = content
            .can_drop(move |value, _, _| {
                value
                    .downcast_ref::<moving::DraggedFiles>()
                    .is_some_and(|drag| drag.can_move_to(drop_target.as_ref(), &drop_destination))
            })
            .drag_over::<moving::DraggedFiles>(|style, _, _, cx| {
                style
                    .bg(cx.theme().drop_target)
                    .border_1()
                    .border_color(cx.theme().drag_border)
            })
            .on_drag_move(
                move |event: &DragMoveEvent<moving::DraggedFiles>, window, cx| {
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
            .on_drop(move |drag: &moving::DraggedFiles, window, cx| {
                drop_panel.update(cx, |p, cx| p.drop_file(drag, &destination, window, cx));
            });
    }
    let mut preview_width = None;
    if !missing
        && !unsupported
        && !entry.is_disabled()
        && !busy
        && !renaming
        && let Some(target) = target
    {
        let dragged = moving::DraggedFiles {
            target,
            paths: panel.read(cx).selection.drag_paths(&path),
        };
        let width = Rc::new(Cell::new(px(0.)));
        preview_width = Some(width.clone());
        content = content.on_drag(dragged, move |drag, _, _, cx| {
            cx.new(|_| DragPreview {
                appearance: appearance.clone(),
                width: width.get(),
                count: drag.paths.len(),
            })
        });
    }
    let content = div()
        .w_full()
        .when_some(preview_width, |row, width| {
            row.on_children_prepainted(move |bounds, _, _| {
                if let Some(bounds) = bounds.first() {
                    width.set(bounds.size.width);
                }
            })
        })
        .child(content);

    let click_panel = panel.clone();
    let context_panel = panel.clone();
    let context_path = path.clone();
    ListItem::new(ix)
        .px_0()
        .py_0()
        .rounded_sm()
        .when_some(motion, |item, motion| {
            item.top(rems(motion.offset * row_height))
                .opacity(motion.opacity)
        })
        .child(content)
        .on_mouse_down(MouseButton::Right, move |_, window, cx| {
            if !renaming && !disabled {
                context_panel.update(cx, |panel, cx| {
                    panel.select_row(&context_path, selection::Gesture::Replace, window, cx);
                });
            }
        })
        .on_click(move |event, window, cx| {
            if renaming || disabled {
                return;
            }
            let modifiers = event.modifiers();
            let gesture = selection::Gesture::from_modifiers(modifiers.shift, modifiers.platform);
            click_panel.update(cx, |panel, cx| panel.select_row(&path, gesture, window, cx));
            if modifiers.shift || modifiers.platform {
                cx.stop_propagation();
                return;
            }
            if !folder && !missing && !unsupported && !renaming {
                files.update(cx, |files, cx| {
                    files.open(path.clone(), event.click_count() < 2, cx)
                });
            }
        })
}

struct DragPreview {
    appearance: appearance::Appearance,
    width: Pixels,
    count: usize,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w(self.width)
            .child(self.appearance.render(None, cx))
            .when(self.count > 1, |preview| {
                preview.child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(cx.theme().sidebar)
                        .text_color(cx.theme().foreground)
                        .font_family(cx.theme().font_family.clone())
                        .text_size(rems(META_SIZE / 16.))
                        .child(format!("{} files", self.count)),
                )
            })
    }
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
