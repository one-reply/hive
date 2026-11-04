//! The sweep of light across the commit message box while AI writes the message, in place of
//! a "Generating…" label under it.
use std::time::Duration;

use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::*;

const SWEEP_DURATION: Duration = Duration::from_millis(1600);
/// Width of the band of light, as a fraction of the box.
const BAND_WIDTH: f32 = 0.45;
/// Opacity of the band's brightest middle over the box.
const HIGHLIGHT_OPACITY: f32 = 0.08;

/// An overlay for the message box: a band sweeping left to right on a loop, or a still glow
/// when the system asks for reduced motion. It has no hitbox, so it never takes the pointer.
pub(super) fn sweep(cx: &App) -> AnyElement {
    let theme = cx.theme();
    let highlight = theme.foreground.alpha(HIGHLIGHT_OPACITY);
    let frame = div()
        .absolute()
        .inset_0()
        .rounded(theme.radius)
        .overflow_hidden();
    if cx.reduce_motion() {
        return frame
            .bg(highlight.alpha(HIGHLIGHT_OPACITY / 2.))
            .into_any_element();
    }
    // Fading to the same hue at zero alpha, not to transparent black, keeps the edges clean.
    let clear = highlight.alpha(0.);
    let band = h_flex()
        .absolute()
        .top_0()
        .bottom_0()
        .w(relative(BAND_WIDTH))
        .child(div().h_full().flex_1().bg(linear_gradient(
            90.,
            linear_color_stop(clear, 0.),
            linear_color_stop(highlight, 1.),
        )))
        .child(div().h_full().flex_1().bg(linear_gradient(
            90.,
            linear_color_stop(highlight, 0.),
            linear_color_stop(clear, 1.),
        )))
        .with_animation(
            "commit-message-sweep",
            Animation::new(SWEEP_DURATION)
                .repeat()
                .with_easing(ease_in_out),
            // From fully off the left edge to fully off the right one.
            |band, progress| band.left(relative(-BAND_WIDTH + (1. + BAND_WIDTH) * progress)),
        );
    frame.child(band).into_any_element()
}
