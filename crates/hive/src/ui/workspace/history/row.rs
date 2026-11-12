//! One conversation in the history list: logo, title, agent and age, and an "Open" pill.
use std::time::Duration;

use gpui_kit::base::animation::Lerp as _;
use gpui_kit::base::motion::{Transition, transition};
use gpui_kit::component::{ActiveTheme as _, Selectable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use hive_core::agent::AgentKind;

use crate::ui::agent_logo;
use crate::ui::hover::{Hover, SmoothHoverBg as _};

// List measures one item for every row, so every row has this height.
pub(super) const ROW_HEIGHT: Rems = rems(3.25);
const SELECTION_DURATION: Duration = Duration::from_millis(200);
const PILL_DURATION: Duration = Duration::from_millis(180);
/// How far below its place a row starts rising from as the list appears.
const ENTRANCE_RISE: Pixels = px(4.);
/// Lines the age up with the title: the logo's width plus the gap after it.
const DETAIL_INDENT: Rems = rems(1.375);

#[derive(IntoElement)]
pub(super) struct Row {
    /// Stable per conversation, so hover and selection state follow it through a search.
    key: SharedString,
    kind: AgentKind,
    title: SharedString,
    detail: SharedString,
    open: bool,
    /// Eased progress of the first appearance; `None` once the row rests.
    entrance: Option<f32>,
    selected: bool,
}

impl Row {
    pub(super) fn new(
        key: SharedString,
        kind: AgentKind,
        title: SharedString,
        detail: SharedString,
    ) -> Self {
        Self {
            key,
            kind,
            title,
            detail,
            open: false,
            entrance: None,
            selected: false,
        }
    }

    pub(super) fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    pub(super) fn entrance(mut self, progress: Option<f32>) -> Self {
        self.entrance = progress;
        self
    }
}

impl Selectable for Row {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for Row {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let hover = Hover::new(
            SharedString::from(format!("{}-hover", self.key)),
            window,
            cx,
        );
        let selection = transition(
            (self.key.clone(), "selected"),
            if self.selected { 1.0_f32 } else { 0. },
            Transition::new(SELECTION_DURATION).ease(ease_in_out),
            window,
            cx,
        );
        let pill = transition(
            (self.key.clone(), "open"),
            if self.open { 1.0_f32 } else { 0. },
            Transition::new(PILL_DURATION).ease(ease_in_out),
            window,
            cx,
        );
        let theme = cx.theme();
        let rest = theme.list_active.alpha(0.);
        let idle = rest.lerp(&theme.list_active, selection);
        let hovered = theme.list_hover.lerp(&theme.list_active, selection);
        let entrance = self.entrance.unwrap_or(1.);
        v_flex()
            .id(SharedString::from(format!("{}-row", self.key)))
            .w_full()
            .h(ROW_HEIGHT)
            .px_3()
            .justify_center()
            .gap_0p5()
            .rounded(theme.radius)
            .cursor_pointer()
            .relative()
            .top(ENTRANCE_RISE * (1. - entrance))
            .opacity(entrance)
            .hover_bg(&hover, idle, hovered)
            .child(
                h_flex()
                    .gap_2()
                    .min_w_0()
                    .child(agent_logo::icon(self.kind, cx).size_3p5().flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(self.title),
                    )
                    .when(pill > 0., |row| {
                        row.child(
                            div()
                                .flex_none()
                                .px_1p5()
                                .rounded_full()
                                .text_xs()
                                .bg(theme.success.opacity(0.15 * pill))
                                .text_color(theme.success.opacity(pill))
                                .child("Open"),
                        )
                    }),
            )
            .child(
                div()
                    .pl(DETAIL_INDENT)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .truncate()
                    .child(self.detail),
            )
    }
}
