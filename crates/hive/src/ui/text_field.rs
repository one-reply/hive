//! Text fields with an animated focus frame. gpui-component's `Input` snaps its border and
//! ring on and off; here the field renders without its own chrome and the frame around it
//! fades between the resting and the focused look. The resting border is pulled towards the
//! foreground so it reads on both themes (lighter on dark, darker on light).
//!
//! A multi-line field's scrollbar is drawn by the frame at its right edge rather than by the
//! editor, which clips it to its own box: a caller that reserves room for a trailing overlay
//! (the commit message's AI button) would otherwise push the scrollbar in by that much.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::input::{EditorScrollbarContext, InputBaseState, InputModeKind};
use gpui_kit::base::motion::{Transition, transition};
use gpui_kit::base::{Scrollbar, ScrollbarHandle};
use gpui_kit::component::input::{Editor, EditorState, Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Colorize as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

const FOCUS_DURATION: Duration = Duration::from_millis(160);
/// How far the resting border moves from `theme.input` towards the foreground.
const BORDER_CONTRAST: f32 = 0.12;
/// Same geometry and opacity as gpui-component's own focus ring, so fields match other controls.
const RING_WIDTH: Pixels = px(3.);
const RING_OPACITY: f32 = 0.5;

/// Builds the frame's scrollbar for the field keyed by the given id.
type FrameScrollbar = Box<dyn FnOnce(EntityId, &mut Window, &mut App) -> Scrollbar>;

/// A single- or multi-line text field inside the animated focus frame.
#[derive(IntoElement)]
pub(crate) struct TextField {
    field: AnyElement,
    /// Set for a multi-line field, whose scrollbar the frame draws.
    scrollbar: Option<FrameScrollbar>,
    focus: FocusHandle,
    /// Keys the focus transition; the state entity outlives re-renders, the element does not.
    key: EntityId,
    /// Draws the frame in the danger color, for a value the caller rejects.
    invalid: bool,
}

impl TextField {
    pub(crate) fn input(input: Input, state: &Entity<InputState>, cx: &App) -> Self {
        Self {
            field: input.appearance(false).into_any_element(),
            scrollbar: None,
            focus: state.focus_handle(cx),
            key: state.entity_id(),
            invalid: false,
        }
    }

    pub(crate) fn textarea(textarea: Textarea, state: &Entity<TextareaState>, cx: &App) -> Self {
        Self::multi_line(textarea.appearance(false).into_any_element(), state, cx)
    }

    /// A highlighted editor that looks like the other fields of its form.
    pub(crate) fn editor(editor: Editor, state: &Entity<EditorState>, cx: &App) -> Self {
        Self::multi_line(editor.appearance(false).into_any_element(), state, cx)
    }

    fn multi_line<M: InputModeKind>(
        field: AnyElement,
        state: &Entity<InputBaseState<M>>,
        cx: &App,
    ) -> Self {
        let scrolled = state.clone();
        Self {
            field,
            scrollbar: Some(Box::new(move |key, window, cx| {
                frame_scrollbar(&scrolled, key, window, cx)
            })),
            focus: state.focus_handle(cx),
            key: state.entity_id(),
            invalid: false,
        }
    }
}

impl TextField {
    /// Marks the value as rejected: the frame turns to the danger color, focused or not.
    pub(crate) fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
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
        let scrollbar = self.scrollbar.map(|build| build(self.key, window, cx));
        let theme = cx.theme();
        let resting = theme.input.mix_oklab(theme.foreground, BORDER_CONTRAST);
        let accent = if self.invalid {
            theme.danger
        } else {
            theme.ring
        };
        let border = if self.invalid {
            theme.danger
        } else {
            resting.mix_oklab(accent, t)
        };
        let ring = accent.alpha(RING_OPACITY * t);
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
            .children(scrollbar)
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

/// The editor's scroll geometry from its latest prepaint. The editor reports it through its
/// scrollbar renderer, which paints nothing; the frame's own scrollbar reads it later in the
/// same frame, since the frame lays out its scrollbar after the field.
#[derive(Clone, Default)]
struct EditorScroll(Rc<RefCell<Option<EditorScrollbarContext>>>);

impl ScrollbarHandle for EditorScroll {
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        self.0
            .borrow()
            .as_ref()
            .map(ScrollbarHandle::viewport_bounds)
            .unwrap_or_default()
    }
    fn offset(&self) -> Point<Pixels> {
        self.0
            .borrow()
            .as_ref()
            .map(ScrollbarHandle::offset)
            .unwrap_or_default()
    }
    fn set_offset(&self, offset: Point<Pixels>) {
        if let Some(context) = self.0.borrow().as_ref() {
            context.set_offset(offset);
        }
    }
    fn content_size(&self) -> Size<Pixels> {
        self.0
            .borrow()
            .as_ref()
            .map(ScrollbarHandle::content_size)
            .unwrap_or_default()
    }
}

/// A vertical scrollbar over the whole frame, driven by the field's own scroll state. The
/// frame is exactly as tall as the editor (its border is an overlay), so their scroll ranges
/// match.
fn frame_scrollbar<M: InputModeKind>(
    state: &Entity<InputBaseState<M>>,
    key: EntityId,
    window: &mut Window,
    cx: &mut App,
) -> Scrollbar {
    let scroll = window.use_keyed_state(("text-field-scroll", key), cx, |_, cx| {
        let scroll = EditorScroll::default();
        let reported = scroll.clone();
        state.update(cx, |state, _| {
            state.set_scrollbar_renderer(Some(Rc::new(move |context, _, _| {
                reported.0.replace(Some(context));
                Empty.into_any_element()
            })))
        });
        scroll
    });
    let scroll = scroll.read(cx).clone();
    Scrollbar::vertical(&scroll)
        .id(("text-field-scrollbar", key))
        .viewport_from_layout()
}
