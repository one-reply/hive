//! Interruptible movement shared by drawers and folding lists.
use gpui_kit::base::motion::{Transition, TransitionId, transition};
use gpui_kit::{App, Window, ease_in_out};
use std::time::Duration;

const DURATION: Duration = Duration::from_millis(200);

/// Sample even while closed so the next toggle starts at the visible position.
pub(super) fn openness(
    id: impl Into<TransitionId>,
    open: bool,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    transition(
        id,
        if open { 1. } else { 0. },
        Transition::new(DURATION).ease(ease_in_out),
        window,
        cx,
    )
}

#[cfg(test)]
mod tests {
    use super::{DURATION, openness};
    use gpui_kit as gpui;
    use gpui_kit::{Context, Empty, IntoElement, Render, TestAppContext, Window, px, size};

    struct Drawer {
        open: bool,
        amount: f32,
    }

    impl Render for Drawer {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.amount = openness("drawer", self.open, window, cx);
            Empty
        }
    }

    #[gpui::test]
    fn a_drawer_restores_at_full_width_and_reverses_from_its_visible_width(
        cx: &mut TestAppContext,
    ) {
        let window = cx.open_window(size(px(320.), px(200.)), |_, _| Drawer {
            open: true,
            amount: 0.,
        });
        let sample = |open, cx: &mut TestAppContext| {
            window
                .update(cx, |drawer, window, _| {
                    drawer.open = open;
                    window.refresh();
                })
                .unwrap();
            cx.run_until_parked();
            window.update(cx, |drawer, _, _| drawer.amount).unwrap()
        };
        assert_eq!(sample(true, cx), 1.);
        assert_eq!(sample(false, cx), 1.);
        cx.executor().advance_clock(DURATION / 2);
        let middle = sample(false, cx);
        assert!(middle > 0. && middle < 1.);
        assert_eq!(sample(true, cx), middle);
        cx.executor().advance_clock(DURATION);
        assert_eq!(sample(true, cx), 1.);
        cx.update(|cx| cx.set_reduce_motion(true));
        assert_eq!(sample(false, cx), 0.);
        assert_eq!(sample(true, cx), 1.);
    }
}
