//! Text fields with an animated focus frame. gpui-component's `Input` snaps its border and
//! ring on and off; here the field renders without its own chrome and the frame around it
//! fades between the resting and the focused look. The resting border is pulled towards the
//! foreground so it reads on both themes (lighter on dark, darker on light).

use std::time::Duration;

use gpui_kit::base::motion::{Transition, transition};
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Colorize as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

const FOCUS_DURATION: Duration = Duration::from_millis(160);
/// How far the resting border moves from `theme.input` towards the foreground.
const BORDER_CONTRAST: f32 = 0.12;
/// Same geometry and opacity as gpui-component's own focus ring, so fields match other controls.
const RING_WIDTH: Pixels = px(3.);
const RING_OPACITY: f32 = 0.5;

/// A single- or multi-line text field inside the animated focus frame.
#[derive(IntoElement)]
pub(crate) struct TextField {
    field: AnyElement,
    focus: FocusHandle,
    /// Keys the focus transition; the state entity outlives re-renders, the element does not.
    key: EntityId,
}

impl TextField {
    pub(crate) fn input(input: Input, state: &Entity<InputState>, cx: &App) -> Self {
        Self {
            field: input.appearance(false).into_any_element(),
            focus: state.focus_handle(cx),
            key: state.entity_id(),
        }
    }

    pub(crate) fn textarea(textarea: Textarea, state: &Entity<TextareaState>, cx: &App) -> Self {
        Self {
            field: textarea.appearance(false).into_any_element(),
            focus: state.focus_handle(cx),
            key: state.entity_id(),
        }
    }
}

impl RenderOnce for TextField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let t = transition(
            ElementId::from(("text-field-focus", self.key)),
            if focused { 1.0_f32 } else { 0. },
            Transition::new(FOCUS_DURATION).ease(ease_in_out),
            window,
            cx,
        );
        let theme = cx.theme();
        let resting = theme.input.mix_oklab(theme.foreground, BORDER_CONTRAST);
        let border = resting.mix_oklab(theme.ring, t);
        let ring = theme.ring.alpha(RING_OPACITY * t);
        let radius = theme.radius;
        let show_ring = theme.focus_ring && t > 0.;
        div()
            .relative()
            .w_full()
            .bg(theme.input_background())
            .rounded(radius)
            .child(self.field)
            // The frame is an overlay so the field keeps the exact box it has without one.
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .border_1()
                    .border_color(border)
                    .rounded(radius),
            )
            .when(show_ring, |frame| {
                frame.child(
                    div()
                        .absolute()
                        .inset(-RING_WIDTH)
                        .border(RING_WIDTH)
                        .border_color(ring)
                        .rounded(radius + RING_WIDTH),
                )
            })
    }
}
