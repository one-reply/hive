//! The inline blame label fades in where it appears and fades out where it was.
//!
//! The canvas paints every frame from scratch, so the label that just went away is kept here
//! with its last position until its fade-out ends.
use gpui_kit::ease_out_quint;
use std::time::{Duration, Instant};

const FADE_IN: Duration = Duration::from_millis(180);
const FADE_OUT: Duration = Duration::from_millis(140);

/// The labels to paint this frame, each with its opacity.
pub(crate) struct Frame<K, L> {
    pub current: Option<(K, L, f32)>,
    pub leaving: Option<(L, f32)>,
}

impl<K, L> Frame<K, L> {
    /// Another frame is needed to finish a fade.
    pub fn is_animating(&self) -> bool {
        self.current
            .as_ref()
            .is_some_and(|(_, _, opacity)| *opacity < 1.)
            || self.leaving.is_some()
    }
}

/// `K` identifies a label (a line and its origin); `L` is where it is painted, which may move
/// (scrolling) without restarting the fade.
pub(crate) struct Fade<K, L> {
    current: Option<(K, L, Instant)>,
    /// The previous label, its opacity when it left, and when it left.
    leaving: Option<(L, f32, Instant)>,
}

impl<K, L> Default for Fade<K, L> {
    fn default() -> Self {
        Self {
            current: None,
            leaving: None,
        }
    }
}

impl<K: PartialEq + Clone, L: Clone> Fade<K, L> {
    /// Record what should show at `now` and return what to paint.
    pub fn advance(&mut self, next: Option<(K, L)>, now: Instant) -> Frame<K, L> {
        match (&mut self.current, next) {
            (Some((key, place, _)), Some((next_key, next_place))) if *key == next_key => {
                *place = next_place;
            }
            (_, next) => {
                if let Some((_, place, since)) = self.current.take() {
                    self.leaving = Some((place, fade_in(since, now), now));
                }
                self.current = next.map(|(key, place)| (key, place, now));
            }
        }
        let leaving = self.leaving.take().and_then(|(place, from, since)| {
            let opacity = from * (1. - eased(since, now, FADE_OUT));
            (opacity > 0.).then(|| {
                self.leaving = Some((place.clone(), from, since));
                (place, opacity)
            })
        });
        Frame {
            current: self
                .current
                .as_ref()
                .map(|(key, place, since)| (key.clone(), place.clone(), fade_in(*since, now))),
            leaving,
        }
    }
}

fn fade_in(since: Instant, now: Instant) -> f32 {
    eased(since, now, FADE_IN)
}

/// Eased progress of a transition of `duration` that started `since`, in 0..=1.
fn eased(since: Instant, now: Instant, duration: Duration) -> f32 {
    let progress = now.saturating_duration_since(since).as_secs_f32() / duration.as_secs_f32();
    ease_out_quint()(progress.min(1.))
}

#[cfg(test)]
mod tests {
    use super::{FADE_IN, FADE_OUT, Fade};
    use std::time::{Duration, Instant};

    fn opacity_of_current(fade: &mut Fade<u32, i32>, next: Option<(u32, i32)>, at: Instant) -> f32 {
        fade.advance(next, at).current.map_or(0., |(_, _, o)| o)
    }

    #[test]
    fn a_new_label_fades_in_and_then_settles() {
        let start = Instant::now();
        let mut fade = Fade::default();
        let frame = fade.advance(Some((1, 10)), start);
        assert_eq!(frame.current.map(|(_, _, o)| o), Some(0.));
        assert!(fade.advance(Some((1, 10)), start).is_animating());
        let half = opacity_of_current(&mut fade, Some((1, 10)), start + FADE_IN / 2);
        assert!(half > 0.5 && half < 1., "{half}");
        let frame = fade.advance(Some((1, 10)), start + FADE_IN);
        assert_eq!(frame.current.as_ref().map(|(_, _, o)| *o), Some(1.));
        assert!(!frame.is_animating());
    }

    #[test]
    fn moving_the_same_label_keeps_its_fade() {
        let start = Instant::now();
        let mut fade = Fade::default();
        fade.advance(Some((1, 10)), start);
        let frame = fade.advance(Some((1, 20)), start + FADE_IN);
        assert_eq!(
            frame.current.map(|(_, place, o)| (place, o)),
            Some((20, 1.))
        );
        assert!(frame.leaving.is_none());
    }

    #[test]
    fn a_replaced_label_fades_out_where_it_was() {
        let start = Instant::now();
        let mut fade = Fade::default();
        fade.advance(Some((1, 10)), start);
        let left = start + FADE_IN;
        let frame = fade.advance(Some((2, 30)), left);
        assert_eq!(frame.leaving, Some((10, 1.)));
        assert_eq!(frame.current.map(|(key, _, o)| (key, o)), Some((2, 0.)));
        let frame = fade.advance(None, left + FADE_OUT / 2);
        let (place, opacity) = frame.leaving.expect("still fading out");
        assert_eq!(place, 30, "the newer label is the one leaving now");
        assert!(opacity < 1.);
        let frame = fade.advance(None, left + FADE_OUT + Duration::from_millis(1) + FADE_OUT);
        assert!(frame.leaving.is_none() && frame.current.is_none());
        assert!(!frame.is_animating());
    }

    #[test]
    fn a_label_leaving_mid_fade_starts_from_its_current_opacity() {
        let start = Instant::now();
        let mut fade = Fade::default();
        fade.advance(Some((1, 10)), start);
        let frame = fade.advance(None, start);
        assert!(
            frame.leaving.is_none(),
            "an invisible label has nothing to fade"
        );
        assert!(!frame.is_animating());
    }
}
