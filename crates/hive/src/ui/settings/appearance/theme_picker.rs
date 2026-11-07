//! Palette swatches share the terminal's resolved colors, so previews cannot drift.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use hive_core::settings::{DarkTheme, LightTheme};

use crate::state::store::Store;
use crate::ui::hover::{Hover, SmoothHoverBg as _};
use crate::ui::theme::catalog::{self, TerminalPalette};

const CARD_WIDTH: Rems = rems(8.);
const PREVIEW_HEIGHT: Rems = rems(2.75);
const CARD_RADIUS: Pixels = px(10.);
const PREVIEW_RADIUS: Pixels = px(6.);
const TEXT_SAMPLE_WIDTH: Rems = rems(3.);
const TEXT_SAMPLE_HEIGHT: Rems = rems(0.3);
const SELECTED_OPACITY: f32 = 0.035;
const HOVER_OPACITY: f32 = 0.065;
const FOCUS_OPACITY: f32 = 0.12;
const PREVIEW_BORDER_OPACITY: f32 = 0.12;

pub(super) fn render(store: &Entity<Store>, window: &mut Window, cx: &mut App) -> Div {
    let settings = &store.read(cx).state.settings;
    let (dark, light) = (settings.dark_theme(), settings.light_theme());
    let dark_cards = DarkTheme::ALL.map(|choice| {
        let store = store.clone();
        swatch(
            choice.label(),
            choice == dark,
            catalog::dark(choice).terminal(),
            move |cx| {
                store.update(cx, |store, cx| store.set_dark_theme(choice, cx));
            },
            window,
            cx,
        )
    });
    let light_cards = LightTheme::ALL.map(|choice| {
        let store = store.clone();
        swatch(
            choice.label(),
            choice == light,
            catalog::light(choice).terminal(),
            move |cx| {
                store.update(cx, |store, cx| store.set_light_theme(choice, cx));
            },
            window,
            cx,
        )
    });
    v_flex()
        .gap_5()
        .child(group("Dark", dark_cards, cx))
        .child(group("Light", light_cards, cx))
}

fn group(title: &'static str, cards: [Stateful<Div>; 3], cx: &App) -> Div {
    v_flex()
        .gap_2()
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(title),
        )
        .child(h_flex().flex_wrap().gap_3().children(cards))
}

fn swatch(
    name: &'static str,
    selected: bool,
    palette: TerminalPalette,
    select: impl Fn(&mut App) + Clone + 'static,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let hover = Hover::new(name, window, cx);
    let theme = cx.theme();
    let (fg, accent) = (theme.foreground, theme.blue);
    let keyboard_select = select.clone();
    v_flex()
        .id(name)
        .role(Role::Button)
        .aria_label(name)
        .aria_toggled(if selected {
            Toggled::True
        } else {
            Toggled::False
        })
        .tab_index(0)
        .w(CARD_WIDTH)
        .max_w_full()
        .flex_none()
        .p_1p5()
        .gap_1p5()
        .border_2()
        .border_color(if selected { accent } else { theme.transparent })
        .rounded(CARD_RADIUS)
        .cursor_pointer()
        .focus_visible(move |style| style.border_color(accent).bg(accent.opacity(FOCUS_OPACITY)))
        .child(
            v_flex()
                .w_full()
                .h(PREVIEW_HEIGHT)
                .flex_none()
                .p_2p5()
                .gap_1p5()
                .rounded(PREVIEW_RADIUS)
                .border_1()
                .border_color(palette.foreground.opacity(PREVIEW_BORDER_OPACITY))
                .bg(palette.background)
                .child(
                    div()
                        .w(TEXT_SAMPLE_WIDTH)
                        .h(TEXT_SAMPLE_HEIGHT)
                        .rounded_full()
                        .bg(palette.foreground),
                )
                .child(h_flex().gap_1p5().children(
                    [4, 1, 2, 3, 5].map(|ix| div().size_2().rounded_full().bg(palette.ansi[ix])),
                )),
        )
        .child(
            h_flex()
                .gap_1p5()
                .text_xs()
                .text_color(if selected { fg } else { theme.muted_foreground })
                .font_weight(if selected {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .child(div().flex_1().min_w_0().child(name))
                .child(div().size_3p5().flex_none().when(selected, |el| {
                    el.child(Icon::new(IconName::Check).size_3p5().text_color(accent))
                })),
        )
        .on_click(move |_, _, cx| select(cx))
        .on_key_down(move |event, _, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                cx.stop_propagation();
                keyboard_select(cx);
            }
        })
        .hover_bg(
            &hover,
            fg.opacity(if selected { SELECTED_OPACITY } else { 0. }),
            fg.opacity(HOVER_OPACITY),
        )
}
