//! Shortcuts page: Hive's shortcuts grouped by category, with their keys, and recording new
//! keys for one.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{setting_row, setting_title};
use crate::app::AppState;
use crate::ui::keymap::{self, Assignment, Category, Rejection, SHORTCUTS};

/// Whether a row is waiting for keys.
enum Recording {
    Idle,
    /// Every keystroke in this window goes to `record` until it is accepted or cancelled.
    Listening {
        ix: usize,
        rejection: Option<Rejection>,
        _intercept: Subscription,
    },
}

pub(super) struct ShortcutsPage {
    recording: Recording,
    _subscriptions: Vec<Subscription>,
}

impl ShortcutsPage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let store = AppState::get(cx).store.clone();
        Self {
            recording: Recording::Idle,
            _subscriptions: vec![cx.observe(&store, |_, _, cx| cx.notify())],
        }
    }

    /// Key bindings match before any element sees the key, so recording intercepts keystrokes
    /// instead of listening for key-down: otherwise pressing a bound combination would run it.
    fn start(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let page = cx.weak_entity();
        let settings_window = window.window_handle();
        let intercept = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle() != settings_window {
                return;
            }
            cx.stop_propagation();
            let keystroke = event.keystroke.clone();
            if let Err(err) = page.update(cx, |page, cx| page.record(ix, &keystroke, cx)) {
                tracing::warn!(%err, "shortcut recorder is gone");
            }
        });
        self.recording = Recording::Listening {
            ix,
            rejection: None,
            _intercept: intercept,
        };
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.recording = Recording::Idle;
        cx.notify();
    }

    fn record(&mut self, ix: usize, keystroke: &Keystroke, cx: &mut Context<Self>) {
        if keystroke.key == "escape" && keystroke.modifiers == Modifiers::none() {
            return self.cancel(cx);
        }
        let store = AppState::get(cx).store.clone();
        let os = cx.platform_info().os;
        let assignment = keymap::assign(
            ix,
            keystroke,
            store.read(cx).state.settings.keybindings(),
            os,
        );
        let assignment = match assignment {
            Ok(assignment) => assignment,
            Err(err) => {
                if let Recording::Listening { rejection, .. } = &mut self.recording {
                    *rejection = Some(err);
                }
                return cx.notify();
            }
        };
        let name = SHORTCUTS[ix].name();
        store.update(cx, |store, cx| match assignment {
            Assignment::Default => store.reset_keybinding(name, cx),
            Assignment::Custom(keys) => store.set_keybinding(name, &keys, cx),
        });
        self.cancel(cx);
    }

    /// A category header above its shortcuts, in registry order.
    fn render_category(&self, category: Category, cx: &mut Context<Self>) -> Div {
        let muted = cx.theme().muted_foreground;
        let rows = SHORTCUTS
            .iter()
            .enumerate()
            .filter(|(_, shortcut)| shortcut.category == category)
            .map(|(ix, _)| self.render_row(ix, cx))
            .collect::<Vec<_>>();
        v_flex()
            .child(
                div()
                    .pb_1()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(muted)
                    .child(category.label().to_uppercase()),
            )
            .children(rows)
    }

    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> Div {
        let shortcut = &SHORTCUTS[ix];
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        let store = AppState::get(cx).store.clone();
        let settings = &store.read(cx).state.settings;
        let is_custom = settings.keybinding(shortcut.name()).is_some();
        let keys = shortcut.keystrokes(settings).to_owned();
        let os = cx.platform_info().os;

        let (is_listening, rejection) = match &self.recording {
            Recording::Listening {
                ix: active,
                rejection,
                ..
            } if *active == ix => (true, rejection.as_ref().map(Rejection::message)),
            _ => (false, None),
        };
        let description: SharedString = if is_listening {
            "Press the new keys. Esc cancels.".into()
        } else {
            match Keystroke::parse_with_os(shortcut.default, os) {
                Ok(default) => format!("Default: {}", default.display(os)).into(),
                Err(_) => SharedString::default(),
            }
        };
        let title = setting_title(shortcut.label, description, cx)
            .when_some(rejection, |el, message| {
                el.child(div().text_xs().text_color(danger).child(message))
            });

        let current: AnyElement = match Keystroke::parse_with_os(&keys, os) {
            Ok(keystroke) => Kbd::new(keystroke).into_any_element(),
            Err(_) => div()
                .text_xs()
                .text_color(muted)
                .child(keys)
                .into_any_element(),
        };
        let edit = Button::new(("edit-shortcut", ix))
            .cursor_pointer()
            .ghost()
            .small()
            .label(if is_listening { "Cancel" } else { "Edit" })
            .on_click(cx.listener(move |this, _, window, cx| {
                if is_listening {
                    this.cancel(cx);
                } else {
                    this.start(ix, window, cx);
                }
            }));
        let controls = h_flex()
            .gap_2()
            .items_center()
            .when(!is_listening, |el| el.child(current))
            .child(edit)
            .when(is_custom && !is_listening, |el| {
                el.child(
                    Button::new(("reset-shortcut", ix))
                        .cursor_pointer()
                        .ghost()
                        .small()
                        .icon(Lucide::RotateCcw)
                        .tooltip("Reset to default")
                        .on_click(move |_, _, cx| {
                            AppState::get(cx)
                                .store
                                .clone()
                                .update(cx, |store, cx| store.reset_keybinding(shortcut.name(), cx))
                        }),
                )
            });
        setting_row(title, controls, cx)
    }
}

impl Render for ShortcutsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_6()
            .children(Category::ALL.map(|category| self.render_category(category, cx)))
    }
}
