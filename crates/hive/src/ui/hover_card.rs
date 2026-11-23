//! A card that shows while the pointer is on its trigger or on the card itself.
//!
//! gpui-component's `HoverCard` opens and closes on a delay and lets the pointer move onto
//! its content, but it drops the content the moment it closes and cannot be closed from the
//! inside. [`hover_card`] adds the two missing parts: the card fades and rises in, plays the
//! same motion backwards before the library removes it, and a click inside can put it away
//! through [`Dismiss`].
use gpui_kit::base::motion::{Presence, Transition};
use gpui_kit::component::ThemeStyled as _;
use gpui_kit::component::hover_card::HoverCard;
use gpui_kit::*;
use std::time::Duration;

/// The card should feel like a tooltip, not a deliberate hover.
const OPEN_DELAY: Duration = Duration::from_millis(150);
/// How long the card takes to fade and rise in, and to sink and fade out.
const TRANSITION: Duration = Duration::from_millis(180);
/// Outlasts `TRANSITION`, so the card is removed only once it has faded out.
const CLOSE_DELAY: Duration = Duration::from_millis(200);
/// How far below its place the card starts rising from.
const RISE: Pixels = px(6.);

/// Where the pointer is, and whether a click inside the card has put it away.
#[derive(Clone, Copy, Default, PartialEq)]
struct CardHover {
    trigger: bool,
    card: bool,
    dismissed: bool,
}

impl CardHover {
    fn is_shown(self) -> bool {
        !self.dismissed && (self.trigger || self.card)
    }

    fn pointer_on_trigger(&mut self, hovered: bool) {
        self.trigger = hovered;
        // Coming back to the trigger asks for the card again.
        if hovered {
            self.dismissed = false;
        }
    }

    fn dismiss(&mut self) {
        self.dismissed = true;
        // The body is gone from the next frame on, so it never reports the pointer leaving.
        self.card = false;
    }
}

fn update(hover: &Entity<CardHover>, cx: &mut App, change: impl FnOnce(&mut CardHover)) {
    hover.update(cx, |hover, cx| {
        let before = *hover;
        change(hover);
        if *hover != before {
            cx.notify();
        }
    });
}

/// Puts the card away from inside its content, for a click that takes the user elsewhere.
#[derive(Clone)]
pub(crate) struct Dismiss(Entity<CardHover>);

impl Dismiss {
    pub(crate) fn dismiss(&self, cx: &mut App) {
        update(&self.0, cx, CardHover::dismiss);
    }
}

/// `trigger` with a card that `anchor` places against it. `content` builds the card's body
/// each frame the card is open; the popover look and the motion are added here. `id` must be
/// unique among the hover cards of the view.
pub(crate) fn hover_card(
    id: impl Into<ElementId>,
    anchor: Anchor,
    trigger: impl IntoElement + 'static,
    content: impl Fn(&Dismiss, &mut Window, &mut App) -> Div + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id: ElementId = id.into();
    let hover = window.use_keyed_state((id.clone(), "hover"), cx, |_, _| CardHover::default());
    let trigger = div().id((id.clone(), "trigger")).child(trigger).on_hover({
        let hover = hover.clone();
        move |hovered, _, cx| update(&hover, cx, |hover| hover.pointer_on_trigger(*hovered))
    });
    // `HoverCard` drops its content the moment it closes, so the card plays its own exit while
    // the pointer is away and the close delay outlasts it. Its presence state lives only while
    // the card is rendered, so every open starts the entrance from the beginning.
    let mut card = HoverCard::new(id.clone());
    // The library clips its content box, which would cut the popover ring and shadow drawn just
    // outside our card down to its rounded corners.
    card.style().overflow.x = Some(Overflow::Visible);
    card.style().overflow.y = Some(Overflow::Visible);
    card.anchor(anchor)
        .open_delay(OPEN_DELAY)
        .close_delay(CLOSE_DELAY)
        .appearance(false)
        .trigger(trigger)
        .content(move |_, window, cx| {
            let body = (id.clone(), "body");
            let state = *hover.read(cx);
            // The library closes a card only once the pointer has left it, and an empty body
            // is under no pointer.
            if state.dismissed {
                return div().id(body);
            }
            let shown = Presence::new(ElementId::from((id.clone(), "presence")), state.is_shown())
                .transition(Transition::new(TRANSITION).ease(ease_in_out))
                .sample(window, cx)
                .progress;
            let dismiss = Dismiss(hover.clone());
            let hover = hover.clone();
            content(&dismiss, window, cx)
                .popover_style(cx)
                .relative()
                .opacity(shown)
                .top(RISE * (1. - shown))
                .id(body)
                .on_hover(move |hovered, _, cx| update(&hover, cx, |hover| hover.card = *hovered))
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    // Named imports: a glob of `super` would bring in GPUI's `test` attribute and shadow the
    // standard one.
    use super::{CLOSE_DELAY, CardHover, OPEN_DELAY, hover_card};
    use gpui_kit as gpui;
    use gpui_kit::prelude::*;
    use gpui_kit::{
        Anchor, Context, IntoElement, Modifiers, Render, TestAppContext, VisualTestContext, Window,
        div, point, px,
    };

    /// A trigger with room above it for the card, whose body holds one element that puts
    /// the card away.
    struct Harness;

    impl Render for Harness {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let card = hover_card(
                "card",
                Anchor::BottomLeft,
                div().debug_selector(|| "trigger".into()).size(px(20.)),
                |dismiss, _, _| {
                    let dismiss = dismiss.clone();
                    div().size(px(60.)).child(
                        div()
                            .id("put-away")
                            .debug_selector(|| "put-away".into())
                            .size(px(20.))
                            .on_click(move |_, _, cx| dismiss.dismiss(cx)),
                    )
                },
                window,
                cx,
            );
            div()
                .size_full()
                .child(div().absolute().left(px(100.)).top(px(300.)).child(card))
        }
    }

    /// The card is a deferred popup: its bounds are known from the second frame on.
    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });
    }

    fn pass(delay: std::time::Duration, cx: &mut VisualTestContext) {
        cx.executor().advance_clock(delay);
        cx.run_until_parked();
        draw(cx);
    }

    #[gpui::test]
    fn a_click_inside_puts_the_card_away_and_the_trigger_brings_it_back(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|_, _| Harness);
        draw(cx);
        let trigger = cx.debug_bounds("trigger").unwrap().center();
        cx.simulate_mouse_move(trigger, None, Modifiers::default());
        pass(OPEN_DELAY, cx);
        let put_away = cx
            .debug_bounds("put-away")
            .expect("the card opens after its delay")
            .center();

        cx.simulate_mouse_move(put_away, None, Modifiers::default());
        cx.simulate_click(put_away, Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("put-away").is_none());

        // The library closes a card once the pointer moves and finds nothing under it.
        let beside = put_away + point(px(1.), px(0.));
        cx.simulate_mouse_move(beside, None, Modifiers::default());
        pass(CLOSE_DELAY, cx);
        // A card the library still held open would show the moment the trigger is reached.
        cx.simulate_mouse_move(trigger, None, Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("put-away").is_none());
        pass(OPEN_DELAY, cx);
        assert!(cx.debug_bounds("put-away").is_some());
    }

    #[test]
    fn the_card_shows_while_the_pointer_is_on_the_trigger_or_on_the_card() {
        let mut hover = CardHover::default();
        assert!(!hover.is_shown());
        hover.pointer_on_trigger(true);
        assert!(hover.is_shown());
        // On its way from the trigger to the card the pointer is on both for a moment.
        hover.card = true;
        hover.pointer_on_trigger(false);
        assert!(hover.is_shown());
        hover.card = false;
        assert!(!hover.is_shown());
    }

    #[test]
    fn a_dismissed_card_stays_away_until_the_pointer_returns_to_the_trigger() {
        let mut hover = CardHover {
            card: true,
            ..Default::default()
        };
        hover.dismiss();
        assert!(!hover.is_shown());
        // The pointer still rests where the card was.
        assert!(!hover.card);
        hover.pointer_on_trigger(false);
        assert!(!hover.is_shown());
        hover.pointer_on_trigger(true);
        assert!(hover.is_shown());
    }
}
