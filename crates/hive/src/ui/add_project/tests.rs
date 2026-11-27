use super::{Chooser, show_chooser};
use crate::ui::destructive::testing::{is_open, press, settle, window};
use gpui_kit as gpui;
use gpui_kit::{AppContext as _, Focusable as _, Task, TestAppContext};
use hive_core::host::ssh_config::SshHost;

#[gpui::test]
fn tab_and_enter_open_the_other_host_field_without_choosing_local(cx: &mut TestAppContext) {
    let cx = window(cx);
    let chooser = cx.update(|window, cx| {
        let chooser = cx.new(|cx| Chooser::new(Task::ready(vec![]), window, cx));
        show_chooser(chooser.clone(), window, cx);
        chooser
    });
    settle(cx);
    cx.update(|window, cx| assert!(chooser.read(cx).local.is_focused(window)));
    press("tab", cx);
    press("enter", cx);
    cx.update(|window, cx| {
        let chooser = chooser.read(cx);
        assert!(chooser.adding_other);
        assert!(chooser.other.focus_handle(cx).is_focused(window));
    });
    press("enter", cx);
    assert!(is_open(cx));
    cx.simulate_input("test-host");
    assert_eq!(
        chooser
            .read_with(cx, |chooser, cx| chooser.other_target(cx))
            .as_deref(),
        Some("test-host")
    );
    press("escape", cx);
    assert!(!is_open(cx));
}

#[gpui::test]
fn tab_reveals_hosts_below_the_scroll_view(cx: &mut TestAppContext) {
    let cx = window(cx);
    cx.update(|window, _| window.activate_window());
    let hosts = (0..12)
        .map(|ix| SshHost {
            alias: format!("host-{ix}"),
            hostname: None,
            user: None,
        })
        .collect();
    let chooser = cx.update(|window, cx| {
        let chooser = cx.new(|cx| Chooser::new(Task::ready(hosts), window, cx));
        show_chooser(chooser.clone(), window, cx);
        chooser
    });
    settle(cx);
    for _ in 0..12 {
        press("tab", cx);
    }
    cx.update(|_, cx| {
        let scroll = &chooser.read(cx).host_scroll;
        assert!(
            scroll.offset().y < gpui::px(0.),
            "viewport {:?}, last row {:?}, bottom item {}",
            scroll.bounds(),
            scroll.bounds_for_item(11),
            scroll.bottom_item()
        );
        assert_eq!(scroll.bottom_item(), 11);
    });
    press("tab", cx);
    press("enter", cx);
    assert!(chooser.read_with(cx, |chooser, _| chooser.adding_other));
}
