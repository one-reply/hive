//! Dictating the task: the microphone button of the Task toolbar and the panel that slides up
//! from the bottom of the field while a dictation is under way.
use super::*;
use crate::state::dictation::{Availability, Phase};
use gpui_kit::base::motion::{Transition, transition};
use gpui_kit::component::hover_card::HoverCard;
use gpui_kit::component::spinner::Spinner;
use hive_core::dictation::MAX_RECORDING;
use std::time::Duration;

/// How long the panel takes to slide over the bottom of the field.
const PANEL_SLIDE: Duration = Duration::from_millis(180);
const PANEL_HEIGHT: Pixels = px(44.);
const METER_WIDTH: Pixels = px(96.);
const METER_HEIGHT: Pixels = px(4.);
/// Wide enough for the reason on two lines above its button.
const SETUP_CARD_WIDTH: Pixels = px(240.);

/// Minutes and seconds of a recording, as a clock shows them.
fn clock(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// What is put into the task for a dictated `text`: set apart from a word it would
/// otherwise run into.
fn dictated(text: &str, before: Option<char>) -> String {
    let text = text.trim();
    match before {
        Some(previous) if !previous.is_whitespace() => format!(" {text}"),
        _ => text.to_owned(),
    }
}

impl LaunchForm {
    /// Ends a dictation under way: the form is being left or its run started.
    pub(in crate::ui::workspace::pipelines) fn stop_dictation(&mut self, cx: &mut Context<Self>) {
        self.dictation
            .update(cx, |dictation, cx| dictation.cancel(cx));
    }

    /// Puts a finished dictation where the caret is.
    pub(super) fn insert_dictated(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let task = self.task.read(cx);
        let before = task.value()[..task.selected_range().start]
            .chars()
            .next_back();
        self.insert_into_task(&dictated(text, before), window, cx);
    }

    /// The microphone of the toolbar. What keeps it from recording is explained on hover,
    /// with the way to fix it where there is one.
    pub(super) fn dictation_button(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dictation = self.dictation.read(cx);
        let availability = dictation.availability(cx);
        let ready =
            availability == Availability::Ready && !dictation.is_active() && !self.is_starting();
        let hover = Hover::new("task-dictate", window, cx).enabled(ready);
        let button = Button::new("task-dictate")
            .cursor_pointer()
            .small()
            .icon(Lucide::Mic)
            .accessibility_label("Dictate the task")
            .disabled(!ready);
        let (reason, action, open): (SharedString, _, fn(&mut App)) = match availability {
            Availability::Ready => {
                return button
                    .tooltip("Dictate the task")
                    .on_click(cx.listener(|form, _, _, cx| {
                        form.dictation
                            .update(cx, |dictation, cx| dictation.start(cx))
                    }))
                    .smooth_hover(ButtonLook::Ghost, &hover, cx)
                    .into_any_element();
            }
            Availability::Unbundled => {
                return button
                    .tooltip("Dictation records only in the Hive app, not under cargo run.")
                    .smooth_hover(ButtonLook::Ghost, &hover, cx)
                    .into_any_element();
            }
            Availability::Unconfigured => (
                "Choose a provider and a model to dictate the task.".into(),
                "Configure in Settings",
                crate::ui::settings::open_dictation,
            ),
            Availability::MissingKey(provider) => (
                format!("{provider} has no API key, so it cannot transcribe.").into(),
                "Add API key",
                crate::ui::settings::open_ai,
            ),
        };
        // A tooltip cannot hold a button; a hover card stays open while the pointer is on it.
        HoverCard::new("task-dictate-setup")
            .trigger(button.smooth_hover(ButtonLook::Ghost, &hover, cx))
            .content(move |_, window, cx| {
                let hover = Hover::new("task-dictate-configure", window, cx);
                v_flex()
                    .w(SETUP_CARD_WIDTH)
                    .gap_2()
                    .child(div().text_sm().child(reason.clone()))
                    .child(
                        h_flex().child(
                            Button::new("task-dictate-configure")
                                .cursor_pointer()
                                .small()
                                .icon(Lucide::Settings)
                                .label(action)
                                .on_click(move |_, _, cx| open(cx))
                                .smooth_hover(ButtonLook::Primary, &hover, cx),
                        ),
                    )
            })
            .into_any_element()
    }

    /// The Task field with the dictation panel over its bottom edge while one is under way.
    pub(super) fn with_dictation(
        &self,
        field: impl IntoElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let active = self.dictation.read(cx).is_active();
        let shown = transition(
            "task-dictation-panel",
            if active { 1.0_f32 } else { 0. },
            Transition::new(PANEL_SLIDE).ease(ease_in_out),
            window,
            cx,
        );
        let frame = div().relative().child(field);
        if !active && shown <= 0. {
            return frame;
        }
        let radius = cx.theme().radius;
        let panel = self
            .dictation_panel(window, cx)
            .absolute()
            .left_0()
            .right_0()
            .bottom(-PANEL_HEIGHT * (1. - shown));
        // Clips the panel to the field while it slides; the field's focus ring, drawn
        // outside the frame, stays whole.
        frame.child(
            div()
                .absolute()
                .inset_0()
                .rounded(radius)
                .overflow_hidden()
                .child(panel),
        )
    }

    fn dictation_panel(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let cancel =
            |id: &'static str, label: &'static str, w: &mut Window, cx: &mut Context<Self>| {
                button(
                    id,
                    label,
                    ButtonLook::Ghost,
                    w,
                    cx,
                    |form: &mut Self, _, cx| {
                        form.dictation
                            .update(cx, |dictation, cx| dictation.cancel(cx))
                    },
                )
            };
        let theme = cx.theme();
        let (muted, danger, track) = (theme.muted_foreground, theme.danger, theme.muted);
        let bar = h_flex()
            .h(PANEL_HEIGHT)
            .px_3()
            .gap_3()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_sm();
        let status = |text: SharedString| div().flex_1().min_w_0().truncate().child(text);
        match self.dictation.read(cx).phase() {
            // Drawn only while the panel slides away.
            Phase::Idle => bar,
            Phase::Starting => bar
                .child(Spinner::new().small())
                .child(status("Opening the microphone…".into()).text_color(muted))
                .child(cancel("dictation-cancel", "Cancel", window, cx)),
            Phase::Recording { level, elapsed, .. } => {
                let time = format!("{} / {}", clock(*elapsed), clock(MAX_RECORDING));
                let meter = div()
                    .w(METER_WIDTH)
                    .h(METER_HEIGHT)
                    .rounded_full()
                    .bg(track)
                    .child(div().h_full().w(relative(*level)).rounded_full().bg(danger));
                bar.child(Icon::new(Lucide::Mic).small().text_color(danger))
                    .child(meter)
                    .child(status(time.into()).text_color(muted))
                    .child(cancel("dictation-cancel", "Cancel", window, cx))
                    .child(button(
                        "dictation-done",
                        "Done",
                        ButtonLook::Primary,
                        window,
                        cx,
                        |form: &mut Self, _, cx| {
                            form.dictation
                                .update(cx, |dictation, cx| dictation.finish(cx))
                        },
                    ))
            }
            Phase::Transcribing => bar
                .child(Spinner::new().small())
                .child(status("Turning the recording into text…".into()).text_color(muted))
                .child(cancel("dictation-cancel", "Cancel", window, cx)),
            Phase::Blocked => bar
                .child(status("macOS does not let Hive use the microphone.".into()))
                .child(button(
                    "dictation-permission",
                    "Open System Settings",
                    ButtonLook::Outline,
                    window,
                    cx,
                    |_: &mut Self, _, cx| cx.open_url(crate::microphone::settings_url()),
                ))
                .child(cancel("dictation-cancel", "Close", window, cx)),
            Phase::Failed { message, audio } => {
                let (message, can_retry) = (message.clone(), audio.is_some());
                bar.child(status(message).text_color(danger))
                    .when(can_retry, |bar| {
                        bar.child(button(
                            "dictation-retry",
                            "Retry",
                            ButtonLook::Outline,
                            window,
                            cx,
                            |form: &mut Self, _, cx| {
                                form.dictation
                                    .update(cx, |dictation, cx| dictation.retry(cx))
                            },
                        ))
                    })
                    .child(cancel("dictation-cancel", "Discard", window, cx))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{clock, dictated};
    use std::time::Duration;

    #[test]
    fn a_recording_is_timed_in_minutes_and_two_digit_seconds() {
        assert_eq!(clock(Duration::ZERO), "0:00");
        assert_eq!(clock(Duration::from_millis(9_900)), "0:09");
        assert_eq!(clock(Duration::from_secs(75)), "1:15");
        assert_eq!(clock(Duration::from_secs(300)), "5:00");
    }

    #[test]
    fn dictated_text_is_set_apart_from_the_word_before_the_caret() {
        assert_eq!(dictated(" Fix the login.\n", Some('.')), " Fix the login.");
        assert_eq!(dictated("Fix the login.", Some('\n')), "Fix the login.");
        assert_eq!(dictated("Fix the login.", Some(' ')), "Fix the login.");
        assert_eq!(dictated("Fix the login.", None), "Fix the login.");
    }
}
