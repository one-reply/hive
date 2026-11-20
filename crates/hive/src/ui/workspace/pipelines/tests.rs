use super::{PanelSlot, PipelinePanel, Screen, TogglePipelines};
use crate::ui::keymap::SHORTCUTS;
use gpui_kit as gpui;
use gpui_kit::prelude::*;
use gpui_kit::{Context, FocusHandle, KeyBinding, TestAppContext, Window, div};

/// Uses the production drawer focus path without starting agents, shells, or disk writers.
struct Harness {
    slots: [PanelSlot; 2],
    documents: [FocusHandle; 2],
    project: usize,
    checkout: std::path::PathBuf,
    terminal_switches: usize,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let slot = &self.slots[self.project];
        let visible = if slot.open {
            slot.view.read(cx).focus.clone()
        } else {
            self.documents[self.project].clone()
        };
        div()
            .size_full()
            .key_context("Workspace")
            .on_action(cx.listener(|this, _: &TogglePipelines, window, cx| {
                if this.slots[this.project].toggle(
                    &this.checkout,
                    &this.documents[this.project],
                    window,
                    cx,
                ) {
                    this.terminal_switches += 1;
                }
                cx.notify();
            }))
            .child(
                div()
                    .size_full()
                    .key_context(if slot.open { "Pipelines" } else { "Terminal" })
                    .track_focus(&visible),
            )
    }
}
#[gpui::test]
fn shortcut_and_project_switch_preserve_independent_drawer_and_document_focus(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let shortcut = SHORTCUTS
            .iter()
            .find(|s| s.label == "Toggle pipelines")
            .unwrap();
        cx.bind_keys([KeyBinding::new(
            shortcut.default,
            TogglePipelines,
            Some(shortcut.context),
        )]);
    });
    let (view, cx) = cx.add_window_view(|w, cx| {
        let slots = ["/api", "/web"].map(|p| PanelSlot {
            view: cx.new(|cx| PipelinePanel {
                project: p.into(),
                focus: cx.focus_handle(),
                screen: Screen::Library,
                draft: None,
                selected_template: None,
                _subscriptions: vec![],
            }),
            open: false,
            previous: None,
        });
        let documents = [cx.focus_handle(), cx.focus_handle()];
        documents[0].focus(w, cx);
        Harness {
            slots,
            documents,
            project: 0,
            checkout: "/a".into(),
            terminal_switches: 0,
        }
    });
    cx.update(|w, cx| w.draw(cx).clear(cx));
    cx.simulate_keystrokes("ctrl-`");
    cx.update(|w, cx| {
        assert!(view.read(cx).slots[0].open);
        assert!(view.read(cx).slots[0].view.read(cx).focus.is_focused(w));
        view.update(cx, |this, cx| {
            this.project = 1;
            this.documents[1].focus(w, cx);
            cx.notify();
        });
        w.draw(cx).clear(cx);
        assert!(!view.read(cx).slots[1].open);
    });
    cx.simulate_keystrokes("ctrl-`");
    cx.update(|w, cx| {
        assert!(view.read(cx).slots[1].open);
        view.update(cx, |this, cx| {
            this.project = 0;
            this.slots[0].view.read(cx).focus.clone().focus(w, cx);
            cx.notify();
        });
        w.draw(cx).clear(cx);
    });
    cx.simulate_keystrokes("ctrl-`");
    cx.update(|w, cx| {
        assert!(!view.read(cx).slots[0].open);
        assert!(view.read(cx).slots[1].open);
        assert!(view.read(cx).documents[0].is_focused(w));
    });
    cx.simulate_keystrokes("ctrl-`");
    view.update(cx, |this, _| this.checkout = "/b".into());
    cx.simulate_keystrokes("ctrl-`");
    view.read_with(cx, |v, _| assert_eq!(v.terminal_switches, 1));
}
