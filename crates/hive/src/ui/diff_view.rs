//! Shared viewport for wrapped Git diffs. The widget still owns vertical scrolling.
use gpui_kit::component::{
    ActiveTheme as _,
    diff::{Diff, DiffMode, DiffState},
    scroll::{ScrollableMask, Scrollbar},
};
use gpui_kit::*;

const MIN_CODE_COLUMNS: f32 = 80.;
const LINE_NUMBER_DIGITS: f32 = 4.;

actions!(diff_view, [PanLeft, PanRight]);

pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("left", PanLeft, Some("DiffViewport > Diff")),
        KeyBinding::new("right", PanRight, Some("DiffViewport > Diff")),
    ]);
}

pub(super) fn render(
    state: &Entity<DiffState>,
    diff: Diff,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let rem = window.rem_size();
    let theme = cx.theme();
    let size = rem * (f32::from(theme.mono_font_size) / 16.);
    let font = cx
        .text_system()
        .resolve_font(&font(theme.mono_font_family.clone()));
    let cell = window.text_system().layout_width(font, size, '0');
    // Reserve space for line numbers, change signs and padding outside the code columns.
    let gutter = rem + size * 0.7 * LINE_NUMBER_DIGITS;
    let code = cell * MIN_CODE_COLUMNS + rem * 2.;
    let width = if state.read(cx).mode() == DiffMode::Split {
        (code + gutter) * 2.
    } else {
        code + gutter * 2.
    };
    let scroll = window
        .use_keyed_state(("diff-pan", state.entity_id()), cx, |_, _| {
            ScrollHandle::new()
        })
        .read(cx)
        .clone();
    let (left, right) = (scroll.clone(), scroll.clone());
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .key_context("DiffViewport")
        .on_action(move |_: &PanLeft, window, _| pan(&left, rem * 3., window))
        .on_action(move |_: &PanRight, window, _| pan(&right, -rem * 3., window))
        .child(
            div()
                .id("diff-pan-area")
                .size_full()
                .overflow_hidden()
                .track_scroll(&scroll)
                .child(diff.soft_wrap(true).min_w(width)),
        )
        .child(ScrollableMask::new(Axis::Horizontal, &scroll))
        .child(Scrollbar::horizontal(&scroll))
}

fn pan(scroll: &ScrollHandle, distance: Pixels, window: &mut Window) {
    let x = (scroll.offset().x + distance).clamp(-scroll.max_offset().x, px(0.));
    scroll.set_offset(point(x, px(0.)));
    window.refresh();
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit as gpui;
    use gpui_kit::component::{diff::DiffFile, v_flex};

    struct Host {
        width: Pixels,
        state: Entity<DiffState>,
    }

    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let diff = Diff::new(&self.state).render_header(|_, _, _| {
                div()
                    .w_full()
                    .h_4()
                    .debug_selector(|| "patch-header".into())
            });
            v_flex()
                .w(self.width)
                .h(px(300.))
                .child(super::render(&self.state, diff, window, cx))
        }
    }

    fn header(cx: &mut VisualTestContext) -> Bounds<Pixels> {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.debug_bounds("patch-header").unwrap()
    }

    #[gpui::test]
    fn narrow_diffs_pan_to_a_bounded_wrap_width_and_wide_diffs_fill_the_pane(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            bind_keys(cx);
        });
        let (host, cx) = cx.add_window_view(|window, cx| {
            let patch = format!(
                "--- a/file\n+++ b/file\n@@ -1 +1 @@\n-old\n+{}\n",
                "x".repeat(2000)
            );
            let files = DiffFile::parse(&patch).unwrap();
            let state = cx.new(|cx| DiffState::new(files, cx).with_mode(DiffMode::Split));
            state.read(cx).focus_handle(cx).focus(window, cx);
            Host {
                width: px(450.),
                state,
            }
        });
        let split = header(cx);
        assert!(split.size.width > px(450.) && split.size.width < px(2200.));
        cx.simulate_keystrokes("right");
        let panned = header(cx);
        assert!(panned.left() < split.left());
        cx.simulate_event(ScrollEvent {
            position: point(px(200.), px(120.)),
            delta: ScrollDelta::Pixels(point(px(-60.), px(0.))),
            ..Default::default()
        });
        assert!(header(cx).left() < panned.left());
        host.update(cx, |host, cx| {
            host.state
                .update(cx, |diff, cx| diff.set_mode(DiffMode::Unified, cx));
            cx.notify();
        });
        let unified = header(cx);
        assert!(unified.size.width > px(450.) && unified.size.width < split.size.width);
        host.update(cx, |host, cx| {
            host.width = px(2200.);
            cx.notify();
        });
        let wide = header(cx);
        assert!(wide.size.width > split.size.width);
        assert_eq!(wide.left(), split.left());
    }
}
