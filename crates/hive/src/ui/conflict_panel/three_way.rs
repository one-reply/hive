//! JetBrains-style 3-pane virtualized merge surface: Current | Result | Incoming.
use super::rows::DisplayRow;
use super::toolbar::{GUTTER_WIDTH, SideNames};
use super::*;
mod choices;
mod editor;
use gpui_kit::component::scroll::Scrollbar;
use hive_core::git::conflict_merge::{
    ConflictRegion, ConflictResolution, MergeDocument, MergeSegment,
};

const LINE_NUMBER_WIDTH: Pixels = px(42.);
const ROW_PADDING_X: Pixels = px(6.);
const SIDE_BG_OPACITY: f32 = 0.08;
const INLINE_DIFF_OPACITY: f32 = 0.24;
const RESOLVED_BG_OPACITY: f32 = 0.08;
const FILLER_BG_OPACITY: f32 = 0.22;
const CONNECTOR_OPACITY: f32 = 0.45;
const CONFLICT_HEADER_OPACITY: f32 = 0.16;
const UNDECIDED_OPACITY: f32 = 0.12;

#[derive(Clone, Copy)]
struct RowMetrics {
    font_size: Pixels,
    height: Pixels,
}

impl ConflictPanel {
    pub(super) fn three_way_body(
        &self,
        document: Arc<MergeDocument>,
        names: &SideNames,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let names = names.clone();
        let panel = cx.entity().downgrade();
        let list_state = self.list.clone();

        div()
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .bg(cx.theme().background)
            .child(
                list(list_state.clone(), move |ix, window, cx| {
                    panel
                        .update(cx, |this, cx| {
                            let Some(row) = this.rows.get(ix).cloned() else {
                                return div().into_any_element();
                            };
                            this.render_row(&row, &document, &names, window, cx)
                        })
                        .unwrap_or_else(|_| div().into_any_element())
                })
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(Scrollbar::vertical(&list_state).viewport_from_layout()),
            )
            .into_any_element()
    }

    fn render_row(
        &self,
        row: &DisplayRow,
        doc: &MergeDocument,
        names: &SideNames,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mono_size = window.rem_size() * (f32::from(cx.theme().mono_font_size) / 16.);
        let metrics = RowMetrics {
            font_size: mono_size,
            height: mono_size * 1.55,
        };

        match *row {
            DisplayRow::Common {
                segment,
                line,
                result_line,
            } => {
                let MergeSegment::Common(lines) = &doc.segments[segment] else {
                    return div().into_any_element();
                };
                let src = &lines[line];
                let styles = self.highlights.common_styles(segment, line);
                let text: SharedString = src.text().to_owned().into();
                let cell = |number| {
                    Self::code_cell(
                        number,
                        text.clone(),
                        styles.clone(),
                        None,
                        src.number,
                        metrics,
                        cx,
                    )
                };
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_stretch()
                    .child(cell(src.number))
                    .child(plain_gutter(cx))
                    .child(cell(result_line))
                    .child(plain_gutter(cx))
                    .child(cell(src.number))
                    .into_any_element()
            }
            DisplayRow::Fold { segment, ref range } => {
                let count = range.len();
                let expand_all = Hover::new(("fold-all", segment), window, cx);
                let theme = cx.theme();
                h_flex()
                    .w_full()
                    .h(metrics.height)
                    .px_3()
                    .gap_2()
                    .items_center()
                    .border_y_1()
                    .border_color(theme.border.opacity(0.6))
                    .bg(theme.muted.opacity(0.28))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(
                        small_action(
                            ("fold-all", segment),
                            format!("↕ {count} unchanged lines · Show all"),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.expand_fold(segment, cx);
                        }))
                        .smooth_hover(ButtonLook::Ghost, &expand_all, cx),
                    )
                    .into_any_element()
            }
            DisplayRow::ConflictHeader {
                segment,
                conflict: _,
            } => {
                let MergeSegment::Conflict(region) = &doc.segments[segment] else {
                    return div().into_any_element();
                };
                self.render_conflict_header(region, names, window, cx)
            }
            DisplayRow::ConflictBase {
                segment,
                conflict,
                line,
            } => {
                let MergeSegment::Conflict(region) = &doc.segments[segment] else {
                    return div().into_any_element();
                };
                let Some(base) = &region.base else {
                    return div().into_any_element();
                };
                let src = &base[line];
                let styles = self.highlights.base_styles(conflict, line);
                let text: SharedString = src.text().to_owned().into();
                let theme = cx.theme();
                let bg = theme.muted.opacity(0.32);
                let accent = theme.muted_foreground;
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_stretch()
                    .child(filler_cell(metrics.height, cx))
                    .child(plain_gutter(cx))
                    .child(Self::code_cell(
                        src.number,
                        text,
                        styles,
                        Some((bg, accent)),
                        src.number,
                        metrics,
                        cx,
                    ))
                    .child(plain_gutter(cx))
                    .child(filler_cell(metrics.height, cx))
                    .into_any_element()
            }
            DisplayRow::ConflictLine {
                segment,
                conflict,
                row,
                result_start,
            } => {
                let MergeSegment::Conflict(region) = &doc.segments[segment] else {
                    return div().into_any_element();
                };
                self.render_conflict_line(region, conflict, row, result_start, metrics, cx)
            }
            DisplayRow::ConflictEditor { segment, conflict } => {
                let MergeSegment::Conflict(region) = &doc.segments[segment] else {
                    return div().into_any_element();
                };
                self.render_conflict_editor(region, conflict, metrics, window, cx)
            }
        }
    }

    fn code_cell(
        line_number: usize,
        text: SharedString,
        styles: Vec<(std::ops::Range<usize>, HighlightStyle)>,
        tint: Option<(Hsla, Hsla)>,
        working_line: usize,
        metrics: RowMetrics,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        h_flex()
            .flex_1()
            .min_w_0()
            .min_h(metrics.height)
            .items_start()
            .overflow_hidden()
            .when_some(tint, |el, (background, border)| {
                el.bg(background).border_l_2().border_color(border)
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    if event.click_count >= 2 {
                        this.open_file(Some(working_line), cx);
                    }
                }),
            )
            .child(
                div()
                    .w(LINE_NUMBER_WIDTH)
                    .h(metrics.height)
                    .flex_none()
                    .pr_2()
                    .flex()
                    .items_center()
                    .justify_end()
                    .text_size(metrics.font_size)
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.muted_foreground)
                    .child(line_number.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px(ROW_PADDING_X)
                    .py(px(1.))
                    .text_size(metrics.font_size)
                    .line_height(metrics.height)
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.foreground)
                    .child(if text.is_empty() {
                        div().h(metrics.height).into_any_element()
                    } else {
                        StyledText::new(text)
                            .with_highlights(styles)
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }

    fn render_conflict_line(
        &self,
        region: &ConflictRegion,
        conflict_ix: usize,
        row: usize,
        result_start: usize,
        metrics: RowMetrics,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let resolution = self.resolutions.get(conflict_ix).and_then(Option::as_ref);

        let left_cell = self.side_cell(region, Side::Ours, row, metrics, cx);
        let right_cell = self.side_cell(region, Side::Theirs, row, metrics, cx);

        let center_cell = match resolution {
            _ if self
                .editing
                .as_ref()
                .is_some_and(|editor| editor.conflict == conflict_ix) =>
            {
                filler_cell(metrics.height, cx)
            }
            None if row == 0 => div()
                .flex_1()
                .min_w_0()
                .min_h(metrics.height)
                .flex()
                .items_center()
                .bg(theme.warning.opacity(0.06))
                .border_l_2()
                .border_color(theme.warning.opacity(0.5))
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(div().px_2().child("Choose a side or edit the result"))
                .into_any_element(),
            None => filler_cell(metrics.height, cx),
            Some(res) => match self
                .resolved_lines
                .get(conflict_ix)
                .and_then(|lines| lines.get(row))
            {
                Some(text) => {
                    let styles = self.highlights.result_styles(
                        region,
                        conflict_ix,
                        res,
                        row,
                        text,
                        theme.highlight_theme.as_ref(),
                    );
                    let accent = match res {
                        ConflictResolution::Current => theme.success,
                        ConflictResolution::Incoming => theme.info,
                        _ => theme.primary,
                    };
                    Self::code_cell(
                        result_start + row,
                        text.clone().into(),
                        styles,
                        Some((
                            accent.opacity(RESOLVED_BG_OPACITY),
                            accent.opacity(CONNECTOR_OPACITY),
                        )),
                        region.lines.start,
                        metrics,
                        cx,
                    )
                }
                None => filler_cell(metrics.height, cx),
            },
        };

        let left_active = matches!(
            resolution,
            Some(
                ConflictResolution::Current
                    | ConflictResolution::BothCurrentFirst
                    | ConflictResolution::BothIncomingFirst
            )
        );
        let right_active = matches!(
            resolution,
            Some(
                ConflictResolution::Incoming
                    | ConflictResolution::BothCurrentFirst
                    | ConflictResolution::BothIncomingFirst
            )
        );

        h_flex()
            .w_full()
            .min_w_0()
            .items_stretch()
            .child(left_cell)
            .child(connector_gutter(left_active.then_some(theme.success), cx))
            .child(center_cell)
            .child(connector_gutter(right_active.then_some(theme.info), cx))
            .child(right_cell)
            .into_any_element()
    }

    fn side_cell(
        &self,
        region: &ConflictRegion,
        side: Side,
        row: usize,
        metrics: RowMetrics,
        cx: &Context<Self>,
    ) -> AnyElement {
        let (lines, accent) = match side {
            Side::Ours => (&region.current, cx.theme().success),
            Side::Theirs => (&region.incoming, cx.theme().info),
        };
        let Some(line) = lines.get(row) else {
            return filler_cell(metrics.height, cx);
        };
        let background = accent.opacity(INLINE_DIFF_OPACITY);
        let styles = match side {
            Side::Ours => self
                .highlights
                .current_styles(region.index, row, background),
            Side::Theirs => self
                .highlights
                .incoming_styles(region.index, row, background),
        };
        Self::code_cell(
            line.number,
            line.text().to_owned().into(),
            styles,
            Some((
                accent.opacity(SIDE_BG_OPACITY),
                accent.opacity(CONNECTOR_OPACITY),
            )),
            line.number,
            metrics,
            cx,
        )
    }
}

fn plain_gutter(cx: &App) -> Div {
    div()
        .w(GUTTER_WIDTH)
        .flex_none()
        .border_x_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().muted.opacity(0.12))
}

fn connector_gutter(active: Option<Hsla>, cx: &App) -> Div {
    let base = plain_gutter(cx);
    match active {
        Some(color) => base.bg(color.opacity(0.24)),
        None => base,
    }
}

fn filler_cell(height: Pixels, cx: &App) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .min_h(height)
        .bg(cx.theme().muted.opacity(FILLER_BG_OPACITY))
        .into_any_element()
}

fn small_action(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
    Button::new(id).cursor_pointer().xsmall().label(label)
}
