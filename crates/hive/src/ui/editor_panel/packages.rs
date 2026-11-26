//! End-of-line version labels share the editor's post-paint geometry with inline blame.
use super::*;
use gpui_kit::component::{WindowExt as _, dialog::DialogButtonProps};

const LABEL_GAP: Pixels = px(20.);
const TRACK: Pixels = px(16.);

pub(super) fn overlay(doc: Entity<Document>) -> impl IntoElement {
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, cx| {
            let document = doc.read(cx);
            let editor = document.editor.read(cx);
            let mut hits = Vec::new();
            let mut paints = Vec::new();
            let Some(visible) = editor.visible_row_range() else {
                return;
            };
            let first = document
                .packages
                .rows
                .partition_point(|l| l.dependency.line < visible.start);
            let mut last = None;
            for (index, lens) in document
                .packages
                .rows
                .iter()
                .enumerate()
                .skip(first)
                .take_while(|(_, l)| l.dependency.line < visible.end)
            {
                let row = lens.dependency.line;
                if last == Some(row) {
                    continue;
                }
                last = Some(row);
                let Some(end) = blame::visible_line_end(editor, row) else {
                    continue;
                };
                let text = SharedString::from(lens.label.clone());
                let theme = cx.theme();
                let run = TextRun {
                    len: text.len(),
                    font: font(theme.mono_font_family.clone()),
                    color: if lens.advisories.is_empty() {
                        theme.muted_foreground
                    } else {
                        theme.danger
                    },
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let shaped =
                    window
                        .text_system()
                        .shape_line(text, theme.mono_font_size, &[run], None);
                let at = point(end.right() + LABEL_GAP, end.top());
                let rect = Bounds::new(at, size(shaped.width, end.size.height));
                if rect.left() < bounds.left()
                    || rect.right() > bounds.right() - TRACK
                    || rect.top() < bounds.top()
                    || rect.bottom() > bounds.bottom()
                {
                    continue;
                }
                paints.push((shaped, at, end.size.height));
                hits.push((rect, index));
            }
            for (shape, at, height) in paints {
                if let Err(e) = shape.paint(at, height, TextAlign::Left, None, window, cx) {
                    tracing::warn!(%e,"version label paint failed")
                }
            }
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture
                    || event.button != MouseButton::Left
                    || !hitbox.is_hovered(window)
                {
                    return;
                }
                if let Some((_, index)) =
                    hits.iter().find(|(rect, _)| rect.contains(&event.position))
                {
                    cx.stop_propagation();
                    menu(doc.clone(), Some(*index), window, cx)
                }
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

pub(super) fn menu(doc: Entity<Document>, index: Option<usize>, window: &mut Window, cx: &mut App) {
    let d = doc.read(cx);
    if !d.is_package_manifest() {
        return;
    }
    let revision = d.package_revision();
    let caret = d.editor.read(cx).cursor();
    let cursor_line = d.editor.read(cx).cursor_position().line as usize;
    let rows = &d.packages.rows;
    let selected = index
        .or_else(|| rows.iter().position(|l| l.dependency.pair.contains(&caret)))
        .or_else(|| rows.iter().position(|l| l.dependency.line == cursor_line));
    let lens = selected.and_then(|i| rows.get(i)).cloned();
    window.open_alert_dialog(cx, move |dialog, window, cx| {
        let mut dialog = dialog
            .title(lens.as_ref().map_or_else(
                || "Package versions".to_owned(),
                |l| l.dependency.name.clone(),
            ))
            .button_props(DialogButtonProps::default().ok_text("Close"));
        if let Some(lens) = &lens {
            dialog = dialog
                .child(format!("{} · {}", lens.dependency.spec, lens.label))
                .child(lens.audit.clone());
            for (i, (label, spec)) in lens.choices.iter().enumerate() {
                let doc = doc.clone();
                let (dep, spec) = (lens.dependency.clone(), spec.clone());
                let hover = Hover::new(("package-version", i), window, cx);
                dialog = dialog.child(
                    Button::new(("package-version", i))
                        .cursor_pointer()
                        .label(label.clone())
                        .on_click(move |_, window, cx| {
                            doc.update(cx, |d, cx| {
                                d.apply_package(revision, &dep, &spec, window, cx)
                            });
                            window.close_dialog(cx);
                            doc.read(cx)
                                .editor
                                .read(cx)
                                .focus_handle(cx)
                                .focus(window, cx);
                        })
                        .smooth_hover(ButtonLook::Outline, &hover, cx),
                );
            }
            for (i, id) in lens.advisories.iter().enumerate() {
                let url = format!("https://osv.dev/vulnerability/{id}");
                let hover = Hover::new(("package-advisory", i), window, cx);
                dialog = dialog.child(
                    Button::new(("package-advisory", i))
                        .cursor_pointer()
                        .label(id.clone())
                        .on_click(move |_, _, cx| cx.open_url(&url))
                        .smooth_hover(ButtonLook::Outline, &hover, cx),
                );
            }
        }
        let enabled = doc.read(cx).install_on_save(cx);
        let (sort, refresh, install) = (doc.clone(), doc.clone(), doc.clone());
        let [sh, rh, ih] = ["package-sort", "package-refresh", "package-install"]
            .map(|id| Hover::new(id, window, cx));
        dialog
            .child(
                Button::new("package-sort")
                    .cursor_pointer()
                    .label("Sort dependencies")
                    .on_click(move |_, window, cx| {
                        sort.update(cx, |d, cx| d.sort_packages(window, cx));
                        window.close_dialog(cx)
                    })
                    .smooth_hover(ButtonLook::Outline, &sh, cx),
            )
            .child(
                Button::new("package-refresh")
                    .cursor_pointer()
                    .label("Refresh versions")
                    .on_click(move |_, window, cx| {
                        refresh.update(cx, |d, cx| d.recheck_packages(cx));
                        window.close_dialog(cx)
                    })
                    .smooth_hover(ButtonLook::Outline, &rh, cx),
            )
            .child(
                Button::new("package-install")
                    .cursor_pointer()
                    .label(if enabled {
                        "Disable install on save for this project"
                    } else {
                        "Enable install on save for this project"
                    })
                    .on_click(move |_, window, cx| {
                        install.update(cx, |d, cx| d.toggle_package_install(cx));
                        window.close_dialog(cx)
                    })
                    .smooth_hover(ButtonLook::Outline, &ih, cx),
            )
    });
}
