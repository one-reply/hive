//! What a recorded WAV holds, read before it is sent. A model given silence or a broken
//! file does not fail: it invents a transcription.

/// Shortest recording worth sending; less holds no word.
const MIN_SECONDS: f32 = 0.3;
/// Loudest sample below which a recording is room noise: -50 dBFS, the floor of the app's
/// level meter, well under the quietest speech.
const SILENCE_PEAK: f32 = 0.003;
/// `WAVE_FORMAT_PCM`: integer samples, the only format whose loudness is read here.
const PCM: u16 = 1;

/// The audio of a WAV file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Audio {
    pub(super) sample_rate: u32,
    pub(super) channels: u16,
    pub(super) bits: u16,
    pub(super) seconds: f32,
    /// Loudest sample, from 0 to 1; `None` for a sample format this reader does not decode.
    pub(super) peak: Option<f32>,
}

/// Reads the format and the samples of a WAV file, skipping the chunks around them: Apple's
/// recorders pad the header with a `FLLR` chunk. `None` for anything that is not a WAV with
/// samples in it.
pub(super) fn inspect(wav: &[u8]) -> Option<Audio> {
    let mut rest = wav.strip_prefix(b"RIFF")?.get(4..)?.strip_prefix(b"WAVE")?;
    let mut format = None;
    while let (Some(id), Some(size)) = (rest.get(..4), rest.get(4..8)) {
        let size = u32::from_le_bytes(size.try_into().ok()?) as usize;
        let content = &rest[8..];
        if id == b"data" {
            // A recorder cut short leaves a size that runs past the end of the file.
            let samples = &content[..size.min(content.len())];
            return audio(format?, samples);
        }
        let chunk = content.get(..size)?;
        if id == b"fmt " {
            format = Some(Format::read(chunk)?);
        }
        // Chunks are aligned to two bytes.
        rest = content.get(size + size % 2..).unwrap_or_default();
    }
    None
}

#[derive(Clone, Copy)]
struct Format {
    tag: u16,
    channels: u16,
    sample_rate: u32,
    bits: u16,
}

impl Format {
    fn read(chunk: &[u8]) -> Option<Self> {
        let word = |at: usize| Some(u16::from_le_bytes(chunk.get(at..at + 2)?.try_into().ok()?));
        Some(Self {
            tag: word(0)?,
            channels: word(2)?,
            sample_rate: u32::from_le_bytes(chunk.get(4..8)?.try_into().ok()?),
            bits: word(14)?,
        })
    }
}

fn audio(format: Format, samples: &[u8]) -> Option<Audio> {
    let frame = usize::from(format.channels) * usize::from(format.bits / 8);
    if frame == 0 || format.sample_rate == 0 {
        return None;
    }
    let peak = (format.tag == PCM && format.bits == 16).then(|| {
        let loudest = samples
            .as_chunks::<2>()
            .0
            .iter()
            .map(|sample| i16::from_le_bytes(*sample).unsigned_abs())
            .max()
            .unwrap_or_default();
        f32::from(loudest) / f32::from(i16::MAX)
    });
    Some(Audio {
        sample_rate: format.sample_rate,
        channels: format.channels,
        bits: format.bits,
        seconds: (samples.len() / frame) as f32 / format.sample_rate as f32,
        peak,
    })
}

/// The audio of a recording a model can transcribe, or why it must not be sent, worded for
/// the user.
pub(super) fn usable(wav: &[u8]) -> Result<Audio, &'static str> {
    let audio = inspect(wav).ok_or("The recording could not be read. Record again.")?;
    if audio.seconds < MIN_SECONDS {
        return Err("The recording is too short. Record again.");
    }
    if audio.peak.is_some_and(|peak| peak < SILENCE_PEAK) {
        return Err(
            "No speech was heard in the recording. Check the input device in System Settings → Sound.",
        );
    }
    Ok(audio)
}

#[cfg(test)]
pub(super) mod tests {
    use super::{inspect, usable};

    /// A 16 kHz mono 16-bit WAV of `samples`, with the header padding Apple's recorder writes.
    pub(in crate::dictation) fn wav(samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut wav = b"RIFF\0\0\0\0WAVE".to_vec();
        wav.extend(b"fmt \x10\0\0\0\x01\0\x01\0");
        wav.extend(16_000_u32.to_le_bytes());
        wav.extend(32_000_u32.to_le_bytes());
        wav.extend(b"\x02\0\x10\0");
        // An odd-sized chunk, so the reader must skip its padding byte too.
        wav.extend(b"FLLR\x03\0\0\0\0\0\0\0");
        wav.extend(b"data");
        wav.extend((data.len() as u32).to_le_bytes());
        wav.extend(data);
        wav
    }

    /// A second of a tone loud enough to be speech.
    pub(in crate::dictation) fn speech() -> Vec<u8> {
        wav(&[8_000; 16_000])
    }

    #[test]
    fn a_recording_is_described_past_the_filler_chunk_of_its_header() {
        let audio = inspect(&wav(&[0, -16_384, 8_192, 0].repeat(4_000))).unwrap();
        assert_eq!(
            (audio.sample_rate, audio.channels, audio.bits),
            (16_000, 1, 16)
        );
        assert_eq!(audio.seconds, 1.);
        let peak = audio.peak.unwrap();
        assert!((peak - 0.5).abs() < 0.001, "{peak}");
    }

    #[test]
    fn a_data_size_running_past_the_file_is_read_up_to_its_end() {
        let mut cut = speech();
        cut.truncate(cut.len() - 16_000);
        assert_eq!(inspect(&cut).unwrap().seconds, 0.5);
    }

    #[test]
    fn what_is_not_a_wav_with_samples_is_not_described() {
        let whole = speech();
        for broken in [
            &b""[..],
            b"RIFF",
            b"RIFF\0\0\0\0WAVEdata",
            b"not a recording at all, just bytes",
            // The header alone, cut before its data chunk.
            &whole[..40],
        ] {
            assert_eq!(inspect(broken), None, "{broken:?}");
        }
        // Samples without a format before them say nothing about their rate.
        assert_eq!(inspect(b"RIFF\0\0\0\0WAVEdata\x02\0\0\0\x01\x02"), None);
    }

    #[test]
    fn only_a_recording_with_speech_in_it_is_usable() {
        assert!(usable(&speech()).is_ok());
        assert!(
            usable(b"garbage")
                .unwrap_err()
                .contains("could not be read")
        );
        assert!(
            usable(&wav(&[8_000; 1_600]))
                .unwrap_err()
                .contains("too short")
        );
        // A dead input records exact zeros; a quiet room stays under the speech floor.
        for quiet in [0, 40] {
            let message = usable(&wav(&[quiet; 16_000])).unwrap_err();
            assert!(message.contains("No speech"), "{quiet}: {message}");
        }
    }
}
