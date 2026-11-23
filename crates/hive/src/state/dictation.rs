//! Dictating a task: one recording, then one request that turns it into text. Each form owns
//! its own, so closing the form ends whatever was under way.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::*;
use hive_core::dictation::{MAX_RECORDING, Recipe};
use hive_core::llm::Provider;

use super::ai_providers::KeyStatus;
use crate::app::AppState;
use crate::microphone::{self, Permission, Recording};

/// How often the level and the clock of a recording are read. Twenty times a second moves
/// the waveform with the voice without redrawing on every frame.
const LEVEL_TICK: Duration = Duration::from_millis(50);
/// How many readings the waveform shows: the last two and a half seconds.
pub(crate) const LEVELS: usize = 50;

pub(crate) enum Phase {
    Idle,
    /// macOS is being asked for the microphone, or the microphone is opening.
    Starting,
    Recording {
        recording: Recording,
        /// Loudness at each of the latest ticks, from 0 to 1, the newest last.
        levels: VecDeque<f32>,
        elapsed: Duration,
    },
    Transcribing,
    /// macOS does not let Hive use the microphone.
    Blocked,
    Failed {
        message: SharedString,
        /// The recording, kept when it was the request that failed, so it can be sent again.
        audio: Option<Arc<[u8]>>,
    },
}

/// The text of a finished dictation.
pub(crate) struct Transcribed(pub(crate) String);

/// Why the microphone button cannot start a dictation, in the order the user fixes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Availability {
    Ready,
    /// No provider or model is chosen in Settings.
    Unconfigured,
    /// The chosen provider has no API key.
    MissingKey(Provider),
    /// Not running from an app bundle, where macOS would end a process that records.
    Unbundled,
}

/// Whether a dictation can start. A key not checked yet counts as present: the request
/// reports a missing one itself.
pub(crate) fn availability(recipe: &Recipe, key: KeyStatus, bundled: bool) -> Availability {
    let Some(provider) = recipe.provider().filter(|_| recipe.is_configured()) else {
        return Availability::Unconfigured;
    };
    if key == KeyStatus::Missing {
        return Availability::MissingKey(provider);
    }
    if !bundled {
        return Availability::Unbundled;
    }
    Availability::Ready
}

/// Adds the newest reading, dropping the oldest once the waveform is full.
fn remember(levels: &mut VecDeque<f32>, level: f32) {
    if levels.len() == LEVELS {
        levels.pop_front();
    }
    levels.push_back(level);
}

pub(crate) struct Dictation {
    phase: Phase,
    /// Whether Hive runs from an app bundle; it cannot change while Hive runs.
    bundled: bool,
    /// Opens the microphone, then reads its level; dropping it stops both.
    recording: Option<Task<()>>,
    /// Reads the recording and waits for the text; dropping it discards a late reply.
    request: Option<Task<()>>,
}

impl EventEmitter<Transcribed> for Dictation {}

impl Dictation {
    pub(crate) fn new() -> Self {
        Self {
            phase: Phase::Idle,
            bundled: microphone::is_bundled(),
            recording: None,
            request: None,
        }
    }

    pub(crate) fn phase(&self) -> &Phase {
        &self.phase
    }

    /// Whether a dictation can start with what Settings and the Keychain hold now.
    pub(crate) fn availability(&self, cx: &App) -> Availability {
        let app = AppState::get(cx);
        let recipe = &app.store.read(cx).state.settings.dictation;
        let key = recipe.provider().map_or(KeyStatus::Unknown, |provider| {
            app.ai_providers.read(cx).provider(provider).status
        });
        availability(recipe, key, self.bundled)
    }

    /// Whether the panel of a dictation is on screen: anything but rest.
    pub(crate) fn is_active(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    /// Asks macOS for the microphone when it has not been asked yet, then records.
    pub(crate) fn start(&mut self, cx: &mut Context<Self>) {
        if !matches!(
            self.phase,
            Phase::Idle | Phase::Blocked | Phase::Failed { .. }
        ) {
            return;
        }
        self.phase = Phase::Starting;
        self.request = None;
        self.recording = Some(cx.spawn(async move |this, cx| {
            let allowed = match microphone::permission() {
                Permission::Allowed => true,
                Permission::NotAsked => microphone::request_access().await,
                Permission::Blocked | Permission::Unavailable => false,
            };
            if !allowed {
                let _ = this.update(cx, |this, cx| this.enter(Phase::Blocked, cx));
                return;
            }
            // Opening the input device takes long enough to stall a frame.
            let opened = cx
                .background_executor()
                .spawn(async { Recording::start(MAX_RECORDING) })
                .await;
            let started = this.update(cx, |this, cx| match opened {
                Ok(recording) => {
                    let phase = Phase::Recording {
                        recording,
                        levels: VecDeque::with_capacity(LEVELS),
                        elapsed: Duration::ZERO,
                    };
                    this.enter(phase, cx);
                    true
                }
                Err(message) => {
                    this.fail(message, None, cx);
                    false
                }
            });
            if !matches!(started, Ok(true)) {
                return;
            }
            loop {
                cx.background_executor().timer(LEVEL_TICK).await;
                if !matches!(this.update(cx, |this, cx| this.tick(cx)), Ok(true)) {
                    return;
                }
            }
        }));
        cx.notify();
    }

    /// Reads the meter; `false` once there is nothing left to read. The recorder closes the
    /// microphone itself at the limit, which ends the dictation as Done would.
    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        let Phase::Recording {
            recording,
            levels,
            elapsed,
        } = &mut self.phase
        else {
            return false;
        };
        if !recording.is_recording() {
            self.finish(cx);
            return false;
        }
        remember(levels, recording.level());
        *elapsed = recording.elapsed();
        cx.notify();
        true
    }

    /// Closes the microphone and sends the recording to the model chosen in Settings.
    pub(crate) fn finish(&mut self, cx: &mut Context<Self>) {
        let previous = std::mem::replace(&mut self.phase, Phase::Transcribing);
        let Phase::Recording { recording, .. } = previous else {
            self.phase = previous;
            return;
        };
        self.request = Some(cx.spawn(async move |this, cx| {
            let audio = cx
                .background_executor()
                .spawn(async move { recording.finish() })
                .await;
            let _ = this.update(cx, |this, cx| match audio {
                Ok(audio) => this.transcribe(audio.into(), cx),
                Err(error) => {
                    tracing::warn!(%error, "could not read the dictation recording");
                    this.fail("Could not read the recording.", None, cx);
                }
            });
        }));
        cx.notify();
    }

    /// Sends the recording of a failed request again.
    pub(crate) fn retry(&mut self, cx: &mut Context<Self>) {
        if let Phase::Failed {
            audio: Some(audio), ..
        } = &self.phase
        {
            let audio = audio.clone();
            self.transcribe(audio, cx);
        }
    }

    /// Back to rest: the microphone closes, the recording is deleted and a reply still on
    /// its way is dropped.
    pub(crate) fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.is_active() {
            return;
        }
        self.recording = None;
        self.request = None;
        self.enter(Phase::Idle, cx);
    }

    fn transcribe(&mut self, audio: Arc<[u8]>, cx: &mut Context<Self>) {
        let recipe = AppState::get(cx)
            .store
            .read(cx)
            .state
            .settings
            .dictation
            .clone();
        self.transcribe_with(audio, cx, move |wav| {
            recipe.transcribe(wav).map_err(|error| {
                tracing::warn!(%error, "dictation was not transcribed");
                error.to_string()
            })
        });
    }

    /// `request` is the blocking call that turns the recording into text.
    fn transcribe_with(
        &mut self,
        audio: Arc<[u8]>,
        cx: &mut Context<Self>,
        request: impl FnOnce(&[u8]) -> Result<String, String> + Send + 'static,
    ) {
        self.phase = Phase::Transcribing;
        self.request = Some(cx.spawn(async move |this, cx| {
            let sent = audio.clone();
            let reply = cx
                .background_executor()
                .spawn(async move { request(&sent) })
                .await;
            let _ = this.update(cx, |this, cx| match reply {
                Ok(text) => {
                    this.enter(Phase::Idle, cx);
                    cx.emit(Transcribed(text));
                }
                Err(message) => this.fail(message, Some(audio), cx),
            });
        }));
        cx.notify();
    }

    fn fail(
        &mut self,
        message: impl Into<SharedString>,
        audio: Option<Arc<[u8]>>,
        cx: &mut Context<Self>,
    ) {
        let message = message.into();
        self.enter(Phase::Failed { message, audio }, cx);
    }

    fn enter(&mut self, phase: Phase, cx: &mut Context<Self>) {
        self.phase = phase;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use gpui_kit as gpui;
    use gpui_kit::{AppContext as _, Entity, TestAppContext};
    use hive_core::dictation::Recipe;
    use hive_core::llm::Provider;

    use super::{
        Availability, Dictation, KeyStatus, LEVELS, Phase, Transcribed, availability, remember,
    };

    fn configured() -> Recipe {
        let mut recipe = Recipe::default();
        recipe.set_provider(Some(Provider::GoogleAi)).unwrap();
        recipe.set_model(Some("gemini-flash".into()));
        recipe
    }

    /// A dictation and the texts it has emitted.
    fn dictation(
        cx: &mut TestAppContext,
    ) -> (Entity<Dictation>, Arc<std::sync::Mutex<Vec<String>>>) {
        let dictation = cx.new(|_| Dictation::new());
        let texts = Arc::new(std::sync::Mutex::new(Vec::new()));
        let heard = texts.clone();
        cx.update(|cx| {
            cx.subscribe(&dictation, move |_, event: &Transcribed, _| {
                heard.lock().unwrap().push(event.0.clone());
            })
            .detach();
        });
        (dictation, texts)
    }

    fn audio() -> Arc<[u8]> {
        Arc::from(&b"RIFF-recording"[..])
    }

    #[test]
    fn availability_names_the_first_thing_to_fix() {
        let unconfigured = Recipe::default();
        assert_eq!(
            availability(&unconfigured, KeyStatus::Missing, false),
            Availability::Unconfigured
        );
        let mut without_model = Recipe::default();
        without_model.set_provider(Some(Provider::OpenAi)).unwrap();
        assert_eq!(
            availability(&without_model, KeyStatus::Configured, true),
            Availability::Unconfigured
        );
        let recipe = configured();
        assert_eq!(
            availability(&recipe, KeyStatus::Missing, false),
            Availability::MissingKey(Provider::GoogleAi)
        );
        assert_eq!(
            availability(&recipe, KeyStatus::Configured, false),
            Availability::Unbundled
        );
        assert_eq!(
            availability(&recipe, KeyStatus::Configured, true),
            Availability::Ready
        );
    }

    #[test]
    fn the_waveform_keeps_only_its_newest_readings() {
        let mut levels = std::collections::VecDeque::new();
        for reading in 0..LEVELS + 3 {
            remember(&mut levels, reading as f32);
        }
        assert_eq!(levels.len(), LEVELS);
        assert_eq!(levels.front(), Some(&3.));
        assert_eq!(levels.back(), Some(&((LEVELS + 2) as f32)));
    }

    #[test]
    fn a_key_not_checked_yet_does_not_block_dictation() {
        for status in [KeyStatus::Unknown, KeyStatus::Checking] {
            assert_eq!(
                availability(&configured(), status, true),
                Availability::Ready
            );
        }
    }

    #[gpui::test]
    fn a_transcription_emits_its_text_and_returns_to_rest(cx: &mut TestAppContext) {
        let (dictation, texts) = dictation(cx);
        dictation.update(cx, |dictation, cx| {
            dictation.transcribe_with(audio(), cx, |wav| {
                assert_eq!(wav, b"RIFF-recording");
                Ok("Fix the login".into())
            });
            assert!(matches!(dictation.phase(), Phase::Transcribing));
        });
        cx.run_until_parked();
        assert_eq!(*texts.lock().unwrap(), ["Fix the login"]);
        dictation.read_with(cx, |dictation, _| assert!(!dictation.is_active()));
    }

    #[gpui::test]
    fn cancelling_while_transcribing_drops_the_reply_that_arrives_later(cx: &mut TestAppContext) {
        let (dictation, texts) = dictation(cx);
        dictation.update(cx, |dictation, cx| {
            dictation.transcribe_with(audio(), cx, |_| Ok("late".into()));
            dictation.cancel(cx);
        });
        cx.run_until_parked();
        assert!(texts.lock().unwrap().is_empty());
        dictation.read_with(cx, |dictation, _| assert!(!dictation.is_active()));
    }

    #[gpui::test]
    fn a_failed_request_keeps_the_recording_for_another_try(cx: &mut TestAppContext) {
        let (dictation, texts) = dictation(cx);
        let sent = Arc::new(AtomicUsize::new(0));
        let counted = sent.clone();
        dictation.update(cx, |dictation, cx| {
            dictation.transcribe_with(audio(), cx, move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
                Err("Google AI: Request timed out. Try again.".into())
            });
        });
        cx.run_until_parked();
        dictation.read_with(cx, |dictation, _| match dictation.phase() {
            Phase::Failed { message, audio } => {
                assert_eq!(message, "Google AI: Request timed out. Try again.");
                assert_eq!(audio.as_deref(), Some(&b"RIFF-recording"[..]));
            }
            _ => panic!("the failure is shown"),
        });
        assert!(texts.lock().unwrap().is_empty());
        assert_eq!(sent.load(Ordering::Relaxed), 1);
    }

    #[gpui::test]
    fn a_dictation_at_rest_ignores_done_retry_and_cancel(cx: &mut TestAppContext) {
        let (dictation, texts) = dictation(cx);
        dictation.update(cx, |dictation, cx| {
            dictation.finish(cx);
            dictation.retry(cx);
            dictation.cancel(cx);
        });
        cx.run_until_parked();
        assert!(texts.lock().unwrap().is_empty());
        dictation.read_with(cx, |dictation, _| assert!(!dictation.is_active()));
    }
}
