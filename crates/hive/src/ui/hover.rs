//! Smooth hover: pointer state per element, eased across frames.
//!
//! GPUI's `.hover()` style and gpui-component's `Button` switch colors in a single frame.
//! [`Hover`] keeps the pointer state in keyed element state and eases a 0..1 progress with
//! `motion::transition`; callers mix their idle and hovered colors by it. Buttons get the
//! same through [`SmoothButton`], which replaces the library variant with a custom one whose
//! idle and hover colors are both the eased mix, so the library's instant swap has nothing
//! to swap. The pointer is tracked on a wrapper: a `Button` with a tooltip already owns its
//! single `on_hover` slot, and a second listener panics.
use gpui_kit::base::animation::Lerp as _;
use gpui_kit::base::motion::{Transition, transition};
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Colorize as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::time::Duration;

const HOVER_DURATION: Duration = Duration::from_millis(140);

/// The eased hover state of one element, sampled for the frame being rendered.
#[derive(Clone)]
pub(crate) struct Hover {
    id: ElementId,
    hovered: Entity<bool>,
    progress: f32,
    enabled: bool,
}

impl Hover {
    /// `id` must be unique among the hoverable elements the current view renders.
    pub(crate) fn new(id: impl Into<ElementId>, window: &mut Window, cx: &mut App) -> Self {
        let id: ElementId = id.into();
        let hovered = window.use_keyed_state((id.clone(), "hovered"), cx, |_, _| false);
        let target = if *hovered.read(cx) { 1. } else { 0. };
        let progress = transition(
            (id.clone(), "hover"),
            target,
            Transition::new(HOVER_DURATION).ease(ease_in_out),
            window,
            cx,
        );
        Self {
            id,
            hovered,
            progress,
            enabled: true,
        }
    }

    /// A disabled or loading element keeps its library look and does not light up.
    pub(crate) fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub(crate) fn is_hovered(&self, cx: &App) -> bool {
        *self.hovered.read(cx)
    }

    pub(crate) fn progress(&self) -> f32 {
        self.progress
    }

    /// `idle` at rest, `hovered` under the pointer, eased in between.
    pub(crate) fn mix(&self, idle: Hsla, hovered: Hsla) -> Hsla {
        // A transparent end fades in alpha only; lerping its black channels would dim the
        // color on the way.
        let idle = if idle.a == 0. {
            hovered.alpha(0.)
        } else {
            idle
        };
        let hovered = if hovered.a == 0. {
            idle.alpha(0.)
        } else {
            hovered
        };
        idle.lerp(&hovered, self.progress)
    }

    pub(crate) fn listener(&self) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
        let state = self.hovered.clone();
        move |hovered, _, cx| set_hovered(&state, *hovered, cx)
    }
}

fn set_hovered(state: &Entity<bool>, hovered: bool, cx: &mut App) {
    state.update(cx, |current, cx| {
        if *current != hovered {
            *current = hovered;
            cx.notify();
        }
    });
}

/// A background that eases in under the pointer, for rows and tiles.
pub(crate) trait SmoothHoverBg: StatefulInteractiveElement + Styled + Sized {
    fn hover_bg(self, hover: &Hover, idle: Hsla, hovered: Hsla) -> Self {
        self.bg(hover.mix(idle, hovered)).on_hover(hover.listener())
    }
}

impl<E: StatefulInteractiveElement + Styled> SmoothHoverBg for E {}

/// The library button looks [`SmoothButton`] reproduces.
#[derive(Clone, Copy)]
pub(crate) enum ButtonLook {
    Ghost,
    Outline,
    Primary,
}

impl ButtonLook {
    fn colors(self, cx: &App) -> ButtonColors {
        let theme = cx.theme();
        match self {
            // gpui-component's ghost hover: the accent surface, halved in dark themes.
            Self::Ghost => ButtonColors {
                idle_bg: theme.transparent,
                hover_bg: if theme.is_dark() {
                    theme.accent.opacity(0.5)
                } else {
                    theme.accent
                },
                idle_fg: theme.secondary_foreground,
                hover_fg: theme.accent_foreground,
                active_bg: theme.button_active,
                border: None,
            },
            Self::Outline => ButtonColors {
                idle_bg: theme.input_background(),
                hover_bg: theme.input.mix_oklab(theme.transparent, 0.5),
                idle_fg: theme.button_foreground,
                hover_fg: theme.button_foreground,
                active_bg: theme.input.mix_oklab(theme.transparent, 0.7),
                border: Some(theme.input),
            },
            Self::Primary => ButtonColors {
                idle_bg: theme.button_primary,
                hover_bg: theme.button_primary_hover,
                idle_fg: theme.button_primary_foreground,
                hover_fg: theme.button_primary_foreground,
                active_bg: theme.button_primary_active,
                border: None,
            },
        }
    }

    fn library(self, button: Button) -> Button {
        match self {
            Self::Ghost => button.ghost(),
            Self::Outline => button.outline(),
            Self::Primary => button.primary(),
        }
    }
}

struct ButtonColors {
    idle_bg: Hsla,
    hover_bg: Hsla,
    idle_fg: Hsla,
    hover_fg: Hsla,
    active_bg: Hsla,
    border: Option<Hsla>,
}

/// A gpui-component `Button` whose hover eases in and out, inside a wrapper that tracks the
/// pointer.
///
/// Replaces the variant call (`.ghost()`, `.outline()`, `.primary()`) and ends the chain.
/// Not for buttons with a dropdown menu: the eased background would cover the library's
/// "open" surface.
pub(crate) trait SmoothButton {
    fn smooth_hover(self, look: ButtonLook, hover: &Hover, cx: &App) -> Stateful<Div>;
}

impl SmoothButton for Button {
    fn smooth_hover(self, look: ButtonLook, hover: &Hover, cx: &App) -> Stateful<Div> {
        div()
            .id(hover.id.clone())
            .flex()
            // Flex siblings share the row evenly only when the automatic
            // content-based minimum width is lifted.
            .min_w_0()
            .on_hover(hover.listener())
            .child(styled_button(self, look, hover, cx))
    }
}

fn styled_button(button: Button, look: ButtonLook, hover: &Hover, cx: &App) -> Button {
    if !hover.enabled {
        return look.library(button);
    }
    let colors = look.colors(cx);
    let bg = hover.mix(colors.idle_bg, colors.hover_bg);
    let fg = hover.mix(colors.idle_fg, colors.hover_fg);
    // A non-outline custom variant draws no border and uses `color` for it; the outline
    // look adds the border back with the same color in both states.
    let variant = ButtonCustomVariant::new(cx)
        .color(colors.border.unwrap_or(colors.idle_bg))
        .foreground(fg)
        .hover(bg)
        .active(colors.active_bg);
    button
        .custom(variant)
        .bg(bg)
        .when_some(colors.border, |button, border| {
            button.border_1().border_color(border)
        })
}
