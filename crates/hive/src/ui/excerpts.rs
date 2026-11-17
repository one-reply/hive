use crate::{
    state::excerpts::{Excerpts, Query},
    ui::hover::{ButtonLook, Hover, SmoothButton as _, SmoothHoverBg as _},
};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _, button::Button, dialog::DialogButtonProps,
    h_flex, scroll::ScrollableElement as _, v_flex,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use hive_core::projects::Target;

const HEIGHT: Pixels = px(560.);
const WIDTH: Pixels = px(1040.);
const ROW_HEIGHT: Pixels = px(26.);

pub(crate) fn open(
    query: Query,
    target: Target,
    title: &'static str,
    window: &mut Window,
    cx: &mut App,
) {
    let state = cx.new(|cx| Excerpts::new(query, target, cx));
    let view = cx.new(|cx| {
        let observer = cx.observe(&state, |_, _, cx| cx.notify());
        Results {
            state,
            selected: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            _observer: observer,
        }
    });
    let focus = view.read(cx).focus.clone();
    window.open_alert_dialog(cx, move |dialog, _, _| {
        dialog
            .title(title)
            .w(WIDTH)
            .max_w(WIDTH)
            .child(div().h(HEIGHT).child(view.clone()))
            .button_props(DialogButtonProps::default().ok_text("Close"))
    });
    window.defer(cx, move |window, cx| focus.focus(window, cx));
}

struct Results {
    state: Entity<Excerpts>,
    selected: Option<usize>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    _observer: Subscription,
}

impl Results {
    fn open_selected(&self, window: &mut Window, cx: &mut App) {
        if let Some(row) = self
            .selected
            .and_then(|i| self.state.read(cx).rows.get(i))
            .copied()
        {
            window.close_dialog(cx);
            self.state.update(cx, |state, cx| state.open(row, cx));
        }
    }
}

impl Render for Results {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let refresh = Hover::new("excerpts-refresh", window, cx);
        let state = self.state.read(cx);
        let status = if state.loading {
            "Searching…".into()
        } else if let Some(error) = &state.error {
            error.clone()
        } else {
            format!(
                "{} files · {}{}",
                state.files.len(),
                if state.stale {
                    "Source may have changed — refresh"
                } else {
                    "Read-only snapshot"
                },
                if state.truncated {
                    " · results limited; narrow the query"
                } else {
                    ""
                }
            )
        };
        let count = state.rows.len();
        v_flex().size_full().min_h_0().gap_2()
            .child(h_flex().gap_2().child(div().flex_1().text_sm().text_color(cx.theme().muted_foreground).child(status))
                .child(Button::new("excerpts-refresh").small().cursor_pointer().label("Refresh")
                    .tooltip("Refresh the snapshot; references use the source editor's cursor")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.selected = None; this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                        this.state.update(cx, |state, cx| state.refresh(cx));
                    })).smooth_hover(ButtonLook::Ghost, &refresh, cx)))
            .child(div().id("excerpts-list").role(Role::List).aria_label("Read-only source excerpts").track_focus(&self.focus).flex_1().min_h_0()
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    let count = this.state.read(cx).rows.len();
                    if count == 0 { return; }
                    match event.keystroke.key.as_str() {
                        "up" => this.selected = Some(this.selected.unwrap_or(0).saturating_sub(1)),
                        "down" => this.selected = Some(this.selected.map_or(0, |i| (i + 1).min(count - 1))),
                        "enter" => { this.open_selected(window, cx); cx.stop_propagation(); return; },
                        _ => return,
                    }
                    this.scroll.scroll_to_item(this.selected.unwrap_or(0), ScrollStrategy::Nearest);
                    cx.stop_propagation(); cx.notify();
                }))
                .child(uniform_list("excerpts", count, cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                    let state = this.state.clone();
                    let visible = range.clone();
                    cx.defer(move |cx| state.update(cx, |state, cx| state.load_visible(visible, cx)));
                    range.filter_map(|index| {
                        let row = *this.state.read(cx).rows.get(index)?;
                        let (text, message, matched) = this.state.read(cx).line(row);
                        let hover = Hover::new(("excerpt", index), window, cx);
                        Some(h_flex().id(("excerpt", index)).role(Role::ListItem).aria_label(format!("{} {text} {message}", row.line.map(|n| (n as u64 + 1).to_string()).unwrap_or_default())).h(ROW_HEIGHT).w_full().px_2().gap_2()
                            .cursor_pointer().text_sm().overflow_hidden()
                            .bg(if this.selected == Some(index) { cx.theme().selection } else if row.line.is_none() { cx.theme().muted } else { cx.theme().background })
                            .when(matched, |row| row.text_color(cx.theme().accent_foreground))
                            .child(div().w(px(10.)).flex_shrink_0().child(if matched { "›" } else { "" }))
                            .child(div().w(px(48.)).flex_shrink_0().text_color(cx.theme().muted_foreground).child(row.line.map(|n| (n as u64 + 1).to_string()).unwrap_or_default()))
                            .child(div().min_w_0().flex_1().font_family(cx.theme().mono_font_family.clone()).truncate().child(text))
                            .when(!message.is_empty(), |row| row.child(div().max_w(px(360.)).truncate().text_color(cx.theme().danger).child(message)))
                            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                                this.selected = Some(index); this.focus.focus(window, cx);
                                if event.click_count() >= 2 { this.open_selected(window, cx); }
                                cx.notify();
                            }))
                            .hover_bg(&hover, if this.selected == Some(index) { cx.theme().selection } else if row.line.is_none() { cx.theme().muted } else { cx.theme().background }, cx.theme().muted))
                    }).collect::<Vec<_>>()
                })).track_scroll(&self.scroll).size_full())
                .vertical_scrollbar(&self.scroll))
            .child(div().text_xs().text_color(cx.theme().muted_foreground).child("Double-click or Enter to open a line · Context: 3 lines before and after each match"))
    }
}
