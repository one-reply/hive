//! A spinner for statuses that last minutes: it turns in steps instead of every frame.
//!
//! gpui-component's `Spinner` asks for a frame on every display refresh, and a notify in
//! gpui-pre dirties every ancestor view, so one spinner redraws the whole window at 120 Hz
//! for as long as it is on screen. That is fine for a short load and too much for an agent
//! that works for an hour. [`SteppedSpinner`] rotates the same icon a twelfth of a turn at a
//! time and wakes its view once per step, only while a view actually built the icon.
use std::cell::Cell;
use std::f32::consts::TAU;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::{Icon, IconName};
use gpui_kit::*;

/// Positions in one revolution, like the system activity indicator.
const STEPS: u32 = 12;
const STEP_INTERVAL: Duration = Duration::from_millis(80);

/// The position shown `elapsed` after the spinner's epoch.
fn spinner_step(elapsed: Duration) -> u32 {
    let steps = elapsed.as_nanos() / STEP_INTERVAL.as_nanos();
    (steps % u128::from(STEPS)) as u32
}

/// How long the position shown at `elapsed` stays current; never zero, so a timer set to it
/// always lands on a later step.
fn until_next_step(elapsed: Duration) -> Duration {
    let into_step = elapsed.as_nanos() % STEP_INTERVAL.as_nanos();
    STEP_INTERVAL - Duration::from_nanos(into_step as u64)
}

fn step_rotation(step: u32) -> Radians {
    radians(TAU * step as f32 / STEPS as f32)
}

/// The stepping clock of one view. The view builds its icons with [`Self::icon`] and calls
/// [`Self::keep_ticking`] once at the end of every render.
pub(crate) struct SteppedSpinner {
    /// Steps count from here, so every icon of the view turns in phase.
    epoch: Instant,
    /// The step of the icons built since the last `keep_ticking`; `None` when none were.
    shown: Cell<Option<u32>>,
    /// Whether `tick` still waits for its step, so a render in between sets no second timer.
    waiting: Rc<Cell<bool>>,
    tick: Option<Task<()>>,
}

impl SteppedSpinner {
    pub(crate) fn new(cx: &App) -> Self {
        Self {
            epoch: cx.background_executor().now(),
            shown: Cell::new(None),
            waiting: Rc::new(Cell::new(false)),
            tick: None,
        }
    }

    /// The loader icon at the current step; the caller sizes and colors it. With reduced
    /// motion it stands still and keeps no timer running.
    pub(crate) fn icon(&self, cx: &App) -> Icon {
        let icon = Icon::new(IconName::LoaderCircle);
        if cx.reduce_motion() {
            return icon;
        }
        // Icons of one render share a step even if the clock passes a boundary between them.
        let step = self
            .shown
            .get()
            .unwrap_or_else(|| spinner_step(self.elapsed(cx)));
        self.shown.set(Some(step));
        icon.rotate(step_rotation(step))
    }

    /// Wakes the view at the next step while this render built an icon, and stops otherwise.
    pub(crate) fn keep_ticking<V: 'static>(&mut self, cx: &mut Context<V>) {
        if self.shown.take().is_none() {
            self.tick = None;
            self.waiting.set(false);
            return;
        }
        if self.waiting.replace(true) {
            return;
        }
        let wait = until_next_step(self.elapsed(cx));
        let waiting = self.waiting.clone();
        self.tick = Some(cx.spawn(async move |view, cx| {
            cx.background_executor().timer(wait).await;
            waiting.set(false);
            // A view that is gone has nothing left to redraw.
            let _ = view.update(cx, |_, cx| cx.notify());
        }));
    }

    fn elapsed(&self, cx: &App) -> Duration {
        cx.background_executor()
            .now()
            .saturating_duration_since(self.epoch)
    }
}

#[cfg(test)]
mod tests {
    // Named imports: a glob of `super` would bring in GPUI's `test` attribute and shadow the
    // standard one.
    use super::{
        STEP_INTERVAL, STEPS, SteppedSpinner, spinner_step, step_rotation, until_next_step,
    };
    use gpui_kit as gpui;
    use gpui_kit::prelude::*;
    use gpui_kit::{
        Context, Entity, IntoElement, Render, TestAppContext, VisualTestContext, Window, div,
        radians,
    };
    use std::f32::consts::TAU;
    use std::time::Duration;

    #[test]
    fn step_advances_every_interval_and_wraps_after_a_revolution() {
        assert_eq!(spinner_step(Duration::ZERO), 0);
        assert_eq!(spinner_step(STEP_INTERVAL - Duration::from_nanos(1)), 0);
        assert_eq!(spinner_step(STEP_INTERVAL), 1);
        assert_eq!(spinner_step(STEP_INTERVAL * (STEPS - 1)), STEPS - 1);
        assert_eq!(spinner_step(STEP_INTERVAL * STEPS), 0);
        assert_eq!(spinner_step(STEP_INTERVAL * (STEPS * 7 + 3)), 3);
    }

    #[test]
    fn wait_for_the_next_step_ends_on_its_boundary_and_is_never_zero() {
        assert_eq!(until_next_step(Duration::ZERO), STEP_INTERVAL);
        assert_eq!(until_next_step(STEP_INTERVAL), STEP_INTERVAL);
        let into_step = Duration::from_millis(30);
        let elapsed = STEP_INTERVAL * 5 + into_step;
        assert_eq!(until_next_step(elapsed), STEP_INTERVAL - into_step);
        assert_eq!(
            spinner_step(elapsed + until_next_step(elapsed)),
            spinner_step(elapsed) + 1
        );
        let last_nanosecond = STEP_INTERVAL - Duration::from_nanos(1);
        assert_eq!(until_next_step(last_nanosecond), Duration::from_nanos(1));
    }

    #[test]
    fn steps_divide_the_circle_evenly() {
        assert_eq!(step_rotation(0), radians(0.));
        assert_eq!(step_rotation(STEPS / 4), radians(TAU / 4.));
        assert_eq!(step_rotation(STEPS / 2), radians(TAU / 2.));
    }

    struct Host {
        spinner: SteppedSpinner,
        working: bool,
        renders: usize,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.renders += 1;
            let icons = self
                .working
                .then(|| [self.spinner.icon(cx), self.spinner.icon(cx)]);
            self.spinner.keep_ticking(cx);
            div().children(icons.into_iter().flatten())
        }
    }

    fn open(working: bool, cx: &mut TestAppContext) -> (Entity<Host>, &mut VisualTestContext) {
        cx.update(gpui_kit::init);
        cx.add_window_view(|_, cx| Host {
            spinner: SteppedSpinner::new(cx),
            working,
            renders: 0,
        })
    }

    fn renders(view: &Entity<Host>, cx: &mut VisualTestContext) -> usize {
        view.read_with(cx, |host, _| host.renders)
    }

    /// Lets every due timer fire and the window redraw.
    fn pass(steps: u32, cx: &mut VisualTestContext) {
        for _ in 0..steps {
            cx.executor().advance_clock(STEP_INTERVAL);
            cx.run_until_parked();
        }
    }

    #[gpui::test]
    fn view_redraws_once_per_step_while_an_icon_is_built(cx: &mut TestAppContext) {
        let (view, cx) = open(true, cx);
        cx.run_until_parked();
        let before = renders(&view, cx);
        pass(5, cx);
        let drawn = renders(&view, cx) - before;
        // The test clock creeps forward on every poll, so a step may land one pass early.
        assert!((4..=6).contains(&drawn), "drew {drawn} times in 5 steps");
    }

    #[gpui::test]
    fn view_stops_redrawing_once_no_icon_is_built(cx: &mut TestAppContext) {
        let (view, cx) = open(true, cx);
        cx.run_until_parked();
        view.update(cx, |host, cx| {
            host.working = false;
            cx.notify();
        });
        cx.run_until_parked();
        let before = renders(&view, cx);
        pass(5, cx);
        assert_eq!(renders(&view, cx), before);
    }

    #[gpui::test]
    fn idle_view_never_starts_ticking(cx: &mut TestAppContext) {
        let (view, cx) = open(false, cx);
        cx.run_until_parked();
        let before = renders(&view, cx);
        pass(5, cx);
        assert_eq!(renders(&view, cx), before);
    }

    #[gpui::test]
    fn icons_of_one_render_share_a_step(cx: &mut TestAppContext) {
        let (view, cx) = open(false, cx);
        view.update(cx, |host, cx| {
            host.spinner.icon(cx);
            let first = host.spinner.shown.get();
            cx.background_executor().advance_clock(STEP_INTERVAL * 3);
            host.spinner.icon(cx);
            assert!(first.is_some());
            assert_eq!(host.spinner.shown.get(), first);
        });
    }
}
