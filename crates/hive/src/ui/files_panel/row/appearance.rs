use super::*;
use std::ops::Range;

/// Shared by the tree and its drag preview, including explicit styles outside the root view.
#[derive(Clone)]
pub(super) struct Appearance {
    pub path: PathBuf,
    pub label: SharedString,
    pub depth: usize,
    pub folder: bool,
    pub expanded: bool,
    pub disabled: bool,
    pub settings: FilesSettings,
    pub selected: bool,
    pub missing: bool,
    pub ignored: bool,
    pub dirty: bool,
    pub decoration: Option<Decoration>,
    pub highlights: Vec<(Range<usize>, HighlightStyle)>,
    pub chevron_turn: f32,
}

impl Appearance {
    pub(super) fn background(&self, cx: &App) -> Hsla {
        if self.selected {
            cx.theme()
                .sidebar
                .blend(cx.theme().foreground.opacity(SELECTED_BACKGROUND_OPACITY))
        } else {
            cx.theme().sidebar
        }
    }

    pub(super) fn render(&self, rename_field: Option<AnyElement>, cx: &App) -> Div {
        let settings = &self.settings;
        let scale = settings.font_size() / LABEL_SIZE;
        let row_height = settings.row_height() / 16.;
        let foreground = cx.theme().foreground;
        let sidebar = cx.theme().sidebar;
        let background = self.background(cx);
        h_flex()
            .relative()
            .w_full()
            .min_w_0()
            .h(rems(row_height))
            .gap(rems(4. / 16.))
            .pl(rems((4. + self.depth as f32 * INDENT * scale) / 16.))
            .pr_2()
            .rounded_sm()
            .border_1()
            .border_color(if self.selected {
                sidebar.blend(foreground.opacity(SELECTED_BORDER_OPACITY))
            } else {
                background
            })
            .bg(background)
            .font_family(cx.theme().font_family.clone())
            .text_size(rems(settings.font_size() / 16.))
            .text_color(if self.missing || self.ignored || self.disabled {
                cx.theme().muted_foreground
            } else {
                foreground
            })
            .children((0..self.depth).map(|depth| {
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
                    .when(self.folder, |slot| {
                        slot.child(
                            Icon::new(Lucide::ChevronRight)
                                .rotate(radians(self.chevron_turn))
                                .size(rems(12. * scale / 16.))
                                .text_color(cx.theme().muted_foreground),
                        )
                    }),
            )
            .when(!self.disabled, |row| {
                row.child(icons::render(
                    &self.path,
                    self.folder,
                    self.expanded,
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
                    .when(self.missing, |label| label.line_through())
                    .child(
                        StyledText::new(self.label.clone())
                            .with_highlights(self.highlights.iter().cloned()),
                    ),
            })
            .when(self.dirty, |row| {
                row.child(
                    div()
                        .flex_none()
                        .size(rems(5. * scale / 16.))
                        .rounded_full()
                        .bg(foreground),
                )
            })
            .when_some(self.decoration.as_ref(), |row, decoration| {
                row.child(badge(decoration, self.folder, scale, cx))
            })
    }
}
