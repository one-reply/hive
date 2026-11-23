//! The microphone for dictating a task: macOS permission and one WAV recording at a time.
//! `AVAudioRecorder` encodes, meters and stops on its own, so no audio thread runs here.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use block2::RcBlock;
use objc2::AnyThread as _;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_avf_audio::{
    AVAudioRecorder, AVFormatIDKey, AVLinearPCMBitDepthKey, AVLinearPCMIsBigEndianKey,
    AVLinearPCMIsFloatKey, AVNumberOfChannelsKey, AVSampleRateKey,
};
use objc2_foundation::{NSBundle, NSDictionary, NSNumber, NSString, NSURL};

/// Microphone list in the Privacy pane of System Settings.
const SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";
/// `kAudioFormatLinearPCM`, the four-character code `lpcm`.
const LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
/// Speech models work at 16 kHz mono; more only makes the upload larger.
const SAMPLE_RATE: f64 = 16_000.;
const BIT_DEPTH: u32 = 16;
/// The span of the meter that speech moves in, in decibels below full scale: room noise sits
/// under the floor and a raised voice reaches the ceiling, so a waveform uses its whole height.
const QUIET_DECIBELS: f32 = -50.;
const LOUD_DECIBELS: f32 = -5.;

static NEXT_RECORDING: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Permission {
    Allowed,
    Blocked,
    /// macOS asks the first time Hive requests access.
    NotAsked,
    /// Not running from an app bundle (`cargo run`): macOS ends a process that touches the
    /// microphone without a usage description of its own.
    Unavailable,
}

impl From<AVAuthorizationStatus> for Permission {
    fn from(status: AVAuthorizationStatus) -> Self {
        match status {
            AVAuthorizationStatus::Authorized => Self::Allowed,
            AVAuthorizationStatus::NotDetermined => Self::NotAsked,
            // Denied by the user or restricted by a profile; a status this build does not
            // know must not be read as consent.
            _ => Self::Blocked,
        }
    }
}

/// Whether Hive runs from an app bundle, which carries the microphone usage description.
pub(crate) fn is_bundled() -> bool {
    NSBundle::mainBundle().bundleIdentifier().is_some()
}

/// What macOS currently lets Hive do with the microphone, without asking the user.
pub(crate) fn permission() -> Permission {
    if !is_bundled() {
        return Permission::Unavailable;
    }
    // SAFETY: the constant is a static string of AVFoundation, read-only for the process.
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        return Permission::Unavailable;
    };
    // SAFETY: audio is one of the two media types the method accepts.
    unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) }.into()
}

/// Asks macOS for the microphone, which shows its dialog the first time. The answer arrives
/// on a queue of its own, hence the channel.
pub(crate) async fn request_access() -> bool {
    if !is_bundled() {
        return false;
    }
    // SAFETY: as in `permission`.
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        return false;
    };
    let (sender, receiver) = async_channel::bounded(1);
    let completion = RcBlock::new(move |granted: Bool| {
        let _ = sender.try_send(granted.as_bool());
    });
    // SAFETY: audio is an accepted media type and the block outlives the call, which copies it.
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &completion) };
    drop(completion);
    receiver.recv().await.unwrap_or(false)
}

/// The System Settings page where the user allows Hive to use the microphone.
pub(crate) fn settings_url() -> &'static str {
    SETTINGS_URL
}

/// A recording in progress: 16 kHz mono 16-bit WAV in a temporary file. Dropping it stops the
/// microphone and removes the file.
pub(crate) struct Recording {
    recorder: Retained<AVAudioRecorder>,
    path: PathBuf,
}

impl Recording {
    /// Blocking: opens the default input and records until `limit` at the latest. The error
    /// is worded for the user.
    pub(crate) fn start(limit: Duration) -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!(
            "hive-dictation-{}-{}.wav",
            std::process::id(),
            NEXT_RECORDING.fetch_add(1, Ordering::Relaxed)
        ));
        let url = NSURL::from_file_path(&path).ok_or("Could not create the recording file.")?;
        // SAFETY: the keys are static strings of AVFAudio, read-only for the process.
        let keys = unsafe {
            [
                AVFormatIDKey,
                AVSampleRateKey,
                AVNumberOfChannelsKey,
                AVLinearPCMBitDepthKey,
                AVLinearPCMIsFloatKey,
                AVLinearPCMIsBigEndianKey,
            ]
        };
        let Some(keys) = keys.into_iter().collect::<Option<Vec<&NSString>>>() else {
            return Err("This macOS cannot record audio.".into());
        };
        let values = [
            NSNumber::new_u32(LINEAR_PCM),
            NSNumber::new_f64(SAMPLE_RATE),
            NSNumber::new_u32(1),
            NSNumber::new_u32(BIT_DEPTH),
            NSNumber::new_bool(false),
            NSNumber::new_bool(false),
        ];
        let values: Vec<&AnyObject> = values.iter().map(|value| value.as_ref()).collect();
        let settings = NSDictionary::<NSString, AnyObject>::from_slices(&keys, &values);
        // SAFETY: the URL and the settings are valid for the call, which retains what it keeps.
        let recorder = unsafe {
            AVAudioRecorder::initWithURL_settings_error(AVAudioRecorder::alloc(), &url, &settings)
        }
        .map_err(|error| {
            tracing::warn!(error = %error.localizedDescription(), "could not open the microphone");
            "Could not open the microphone.".to_owned()
        })?;
        let recording = Self { recorder, path };
        // SAFETY: plain messages to a recorder this value owns.
        let started = unsafe {
            recording.recorder.setMeteringEnabled(true);
            recording.recorder.recordForDuration(limit.as_secs_f64())
        };
        if !started {
            return Err("No microphone is available.".into());
        }
        Ok(recording)
    }

    /// Whether the microphone is still open; it closes by itself at the limit.
    pub(crate) fn is_recording(&self) -> bool {
        // SAFETY: a plain message to a recorder this value owns.
        unsafe { self.recorder.isRecording() }
    }

    /// How long the recording is so far.
    pub(crate) fn elapsed(&self) -> Duration {
        // SAFETY: a plain message to a recorder this value owns.
        let seconds = unsafe { self.recorder.currentTime() };
        Duration::try_from_secs_f64(seconds).unwrap_or_default()
    }

    /// How loud the input is right now, from 0 (silence) to 1.
    pub(crate) fn level(&self) -> f32 {
        // SAFETY: metering was enabled at the start; channel 0 is the only channel.
        level(unsafe {
            self.recorder.updateMeters();
            self.recorder.averagePowerForChannel(0)
        })
    }

    /// Blocking: closes the microphone and returns the recorded WAV.
    pub(crate) fn finish(self) -> std::io::Result<Vec<u8>> {
        // SAFETY: a plain message to a recorder this value owns; stopping flushes the file.
        unsafe { self.recorder.stop() };
        std::fs::read(&self.path)
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        // SAFETY: plain messages to a recorder this value owns; both are harmless when the
        // recording has already stopped.
        unsafe {
            self.recorder.stop();
            self.recorder.deleteRecording();
        }
    }
}

/// Maps the meter's decibels onto 0..=1. The meter reports -160 for silence and can report
/// a little above 0 for clipping.
fn level(decibels: f32) -> f32 {
    if !decibels.is_finite() {
        return 0.;
    }
    ((decibels - QUIET_DECIBELS) / (LOUD_DECIBELS - QUIET_DECIBELS)).clamp(0., 1.)
}

#[cfg(test)]
mod tests {
    use super::{Permission, level};
    use objc2_av_foundation::AVAuthorizationStatus;

    #[test]
    fn only_an_explicit_grant_allows_the_microphone() {
        assert_eq!(
            Permission::from(AVAuthorizationStatus::Authorized),
            Permission::Allowed
        );
        assert_eq!(
            Permission::from(AVAuthorizationStatus::NotDetermined),
            Permission::NotAsked
        );
        for refused in [
            AVAuthorizationStatus::Denied,
            AVAuthorizationStatus::Restricted,
            // A status a later macOS adds.
            AVAuthorizationStatus(99),
        ] {
            assert_eq!(Permission::from(refused), Permission::Blocked);
        }
    }

    #[test]
    fn decibels_map_to_a_level_between_zero_and_one() {
        assert_eq!(level(-160.), 0.);
        assert_eq!(level(-50.), 0.);
        assert_eq!(level(-27.5), 0.5);
        assert_eq!(level(-5.), 1.);
        assert_eq!(level(3.), 1.);
        assert_eq!(level(f32::NAN), 0.);
        assert_eq!(level(f32::NEG_INFINITY), 0.);
    }
}
