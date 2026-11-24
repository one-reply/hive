//! Dictating the task: the microphone button of the Task toolbar, holding Space in the field,
//! and the panel that slides up from the bottom of the field while a dictation is under way.
use super::*;
use crate::state::dictation::{Availability, LEVELS, Phase};
use gpui_kit::base::motion::{Transition, transition};
use gpui_kit::component::hover_card::HoverCard;
use gpui_kit::component::spinner::Spinner;
use hive_core::dictation::MAX_RECORDING;
use std::time::Duration;

/// How long the panel takes to slide over the bottom of the field.
const PANEL_SLIDE: Duration = Duration::from_millis(180);
const PANEL_HEIGHT: Pixels = px(52.);
/// The waveform of a recording: one bar per reading, as tall as the voice was loud.
const WAVE_HEIGHT: Pixels = px(28.);
const BAR_WIDTH: Pixels = px(3.);
const BAR_GAP: Pixels = px(2.);
/// A bar of silence is a dot, so the waveform keeps its shape between words.
const BAR_MIN_HEIGHT: Pixels = px(3.);
/// How much of its color an old bar keeps, and over how many bars the newest fades to it:
/// about the number a panel of the form's width shows.
const OLDEST_BAR_OPACITY: f32 = 0.3;
const FADE_BARS: usize = 100;
/// Wide enough for the reason on two lines above its button.
const SETUP_CARD_WIDTH: Pixels = px(240.);

/// Holding Space in the Task field. A tap types a space as usual; once the key repeats, the
/// hold records, and letting go sends the recording.
#[derive(Default)]
pub(super) enum SpaceHold {
    #[default]
    Up,
    /// Space went down and typed a space over `replaced`, the selection there was.
    Pressed { replaced: String },
    /// The hold started a dictation.
    Recording,
}

/// What a Space key-down in the Task field does.
#[derive(Debug, PartialEq, Eq)]
enum SpaceDown {
    Type,
    Record,
    /// The key repeats while recording: no spaces are typed under the dictation.
    Swallow,
}

fn space_down(hold: &SpaceHold, is_held: bool, ready: bool) -> SpaceDown {
    match hold {
        SpaceHold::Recording => SpaceDown::Swallow,
        SpaceHold::Pressed { .. } if is_held && ready => SpaceDown::Record,
        _ => SpaceDown::Type,
    }
}

fn is_plain_space(keystroke: &Keystroke) -> bool {
    keystroke.key == "space" && keystroke.modifiers.number_of_modifiers() == 0
}

/// The space the press that turned into a hold typed just before the caret.
fn typed_space(value: &str, caret: usize) -> Option<std::ops::Range<usize>> {
    let start = caret.checked_sub(1)?;
    (value.get(start..caret) == Some(" ")).then_some(start..caret)
}

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

/// Height of the bar for a loudness between 0 and 1.
fn bar_height(level: f32) -> Pixels {
    BAR_MIN_HEIGHT + (WAVE_HEIGHT - BAR_MIN_HEIGHT) * level.clamp(0., 1.)
}

/// Opacity of a bar `age` readings old: the newest is solid and the bars fade as they near
/// the far end of the panel.
fn bar_opacity(age: usize) -> f32 {
    let faded = (age as f32 / FADE_BARS as f32).min(1.);
    1. - (1. - OLDEST_BAR_OPACITY) * faded
}

/// The recent loudness as bars centered on one line, filling the width it is given: the
/// newest bar is at the right edge and older ones run off the left. Readings not taken yet
/// are bars of silence, so the waveform is full from the start.
fn waveform(levels: &std::collections::VecDeque<f32>, color: Hsla) -> Div {
    let silent = LEVELS.saturating_sub(levels.len());
    let newest_first = levels
        .iter()
        .rev()
        .copied()
        .chain(std::iter::repeat_n(0., silent));
    h_flex()
        // Laid out from the right, so the bars that do not fit are the oldest ones.
        .flex_row_reverse()
        .flex_1()
        .min_w_0()
        .h(WAVE_HEIGHT)
        .gap(BAR_GAP)
        .overflow_hidden()
        .children(newest_first.enumerate().map(|(age, level)| {
            div()
                .flex_none()
                .w(BAR_WIDTH)
                .h(bar_height(level))
                .rounded_full()
                .bg(color.opacity(bar_opacity(age)))
        }))
}

impl LaunchForm {
    /// Ends a dictation under way: the form is being left or its run started.
    pub(in crate::ui::workspace::pipelines) fn stop_dictation(&mut self, cx: &mut Context<Self>) {
        self.dictation
            .update(cx, |dictation, cx| dictation.cancel(cx));
    }

    /// Whether the microphone can start a dictation now.
    fn can_dictate(&self, cx: &App) -> bool {
        let dictation = self.dictation.read(cx);
        dictation.availability(cx) == Availability::Ready
            && !dictation.is_active()
            && !self.is_starting()
    }

    /// Space in the Task field: types as usual until the key repeats, then records instead.
    pub(super) fn space_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !is_plain_space(&event.keystroke) {
            return;
        }
        match space_down(&self.space, event.is_held, self.can_dictate(cx)) {
            SpaceDown::Type if !event.is_held => {
                let task = self.task.read(cx);
                let replaced = task.value()[task.selected_range()].to_owned();
                self.space = SpaceHold::Pressed { replaced };
            }
            SpaceDown::Type => {}
            SpaceDown::Record => {
                if let SpaceHold::Pressed { replaced } =
                    std::mem::replace(&mut self.space, SpaceHold::Recording)
                {
                    self.take_back_space(replaced, window, cx);
                }
                self.dictation
                    .update(cx, |dictation, cx| dictation.start(cx));
                cx.stop_propagation();
            }
            SpaceDown::Swallow => cx.stop_propagation(),
        }
    }

    /// Letting go of Space sends what the hold recorded.
    pub(super) fn space_up(&mut self, event: &KeyUpEvent, cx: &mut Context<Self>) {
        if event.keystroke.key != "space" {
            return;
        }
        if matches!(std::mem::take(&mut self.space), SpaceHold::Recording) {
            self.dictation
                .update(cx, |dictation, cx| dictation.release(cx));
        }
    }

    /// Undoes the space the press typed before it turned into a hold, putting back the
    /// selection it replaced.
    fn take_back_space(&mut self, replaced: String, window: &mut Window, cx: &mut Context<Self>) {
        self.task.update(cx, |task, cx| {
            let caret = task.selected_range();
            if !caret.is_empty() {
                return;
            }
            let Some(space) = typed_space(&task.value(), caret.start) else {
                return;
            };
            task.set_selected_range(space, cx);
            task.replace(replaced, window, cx);
        });
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

    /// The microphone of the toolbar, outlined so it stands apart from the formatting icons.
    /// What keeps it from recording is explained on hover, with the way to fix it where
    /// there is one.
    pub(super) fn dictation_button(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let availability = self.dictation.read(cx).availability(cx);
        let ready = self.can_dictate(cx);
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
                    .tooltip("Dictate the task, or hold Space in it")
                    .on_click(cx.listener(|form, _, _, cx| {
                        form.dictation
                            .update(cx, |dictation, cx| dictation.start(cx))
                    }))
                    .smooth_hover(ButtonLook::Outline, &hover, cx)
                    .into_any_element();
            }
            Availability::Unbundled => {
                return button
                    .tooltip("Dictation records only in the Hive app, not under cargo run.")
                    .smooth_hover(ButtonLook::Outline, &hover, cx)
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
            .trigger(button.smooth_hover(ButtonLook::Outline, &hover, cx))
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
        let (muted, danger) = (theme.muted_foreground, theme.danger);
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
            Phase::Recording {
                levels, elapsed, ..
            } => {
                let time = format!("{} / {}", clock(*elapsed), clock(MAX_RECORDING));
                bar.child(Icon::new(Lucide::Mic).small().text_color(danger))
                    .child(waveform(levels, danger))
                    .child(div().flex_none().text_color(muted).child(time))
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
    use super::{
        BAR_MIN_HEIGHT, FADE_BARS, LEVELS, OLDEST_BAR_OPACITY, SpaceDown, SpaceHold, WAVE_HEIGHT,
        bar_height, bar_opacity, clock, dictated, space_down, typed_space,
    };
    use std::time::Duration;

    #[test]
    fn a_recording_is_timed_in_minutes_and_two_digit_seconds() {
        assert_eq!(clock(Duration::ZERO), "0:00");
        assert_eq!(clock(Duration::from_millis(9_900)), "0:09");
        assert_eq!(clock(Duration::from_secs(75)), "1:15");
        assert_eq!(clock(Duration::from_secs(300)), "5:00");
    }

    #[test]
    fn a_bar_grows_from_a_dot_to_the_full_height_with_the_voice() {
        assert_eq!(bar_height(0.), BAR_MIN_HEIGHT);
        assert_eq!(bar_height(1.), WAVE_HEIGHT);
        assert!(bar_height(0.5) > BAR_MIN_HEIGHT && bar_height(0.5) < WAVE_HEIGHT);
        // A reading outside the scale never leaves the panel.
        assert_eq!(bar_height(7.), WAVE_HEIGHT);
        assert_eq!(bar_height(-1.), BAR_MIN_HEIGHT);
    }

    #[test]
    fn bars_fade_from_the_newest_to_the_oldest() {
        assert_eq!(bar_opacity(0), 1.);
        assert!(bar_opacity(FADE_BARS / 2) < 1.);
        assert!(bar_opacity(FADE_BARS / 2) > bar_opacity(FADE_BARS));
        // Every bar past the fade keeps the same faint color.
        for age in [FADE_BARS, LEVELS - 1] {
            assert!((bar_opacity(age) - OLDEST_BAR_OPACITY).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn dictated_text_is_set_apart_from_the_word_before_the_caret() {
        assert_eq!(dictated(" Fix the login.\n", Some('.')), " Fix the login.");
        assert_eq!(dictated("Fix the login.", Some('\n')), "Fix the login.");
        assert_eq!(dictated("Fix the login.", Some(' ')), "Fix the login.");
        assert_eq!(dictated("Fix the login.", None), "Fix the login.");
    }

    #[test]
    fn a_held_space_records_only_after_its_own_press_and_when_dictation_is_ready() {
        let pressed = SpaceHold::Pressed {
            replaced: String::new(),
        };
        assert_eq!(space_down(&SpaceHold::Up, false, true), SpaceDown::Type);
        assert_eq!(space_down(&pressed, false, true), SpaceDown::Type);
        assert_eq!(space_down(&pressed, true, true), SpaceDown::Record);
        // Without a ready microphone a held key repeats spaces as it always did.
        assert_eq!(space_down(&pressed, true, false), SpaceDown::Type);
        // A repeat whose press the field never saw is not taken for a hold.
        assert_eq!(space_down(&SpaceHold::Up, true, true), SpaceDown::Type);
        assert_eq!(
            space_down(&SpaceHold::Recording, true, false),
            SpaceDown::Swallow
        );
    }

    #[test]
    fn only_a_space_right_before_the_caret_is_taken_back() {
        assert_eq!(typed_space("Fix ", 4), Some(3..4));
        assert_eq!(typed_space(" ", 1), Some(0..1));
        assert_eq!(typed_space("Fix", 3), None);
        assert_eq!(typed_space("", 0), None);
        // A caret inside a multibyte character or past the end finds nothing.
        assert_eq!(typed_space("й", 1), None);
        assert_eq!(typed_space("a ", 9), None);
    }
}
