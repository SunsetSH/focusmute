//! Sound playback helpers for mute/unmute feedback.
//!
//! Sounds are pre-decoded at load time into raw samples, so playback only
//! needs to clone the sample buffer (no re-parsing on every mute toggle).
//! Built-in presets other than `classic` are synthesized in memory from short
//! square/pulse/triangle tones, so they need no asset files.

use std::io::Cursor;
use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::time::Duration;
#[cfg(not(windows))]
use std::time::Instant;

#[cfg(not(windows))]
use rodio::Player;
#[cfg(not(windows))]
use rodio::buffer::SamplesBuffer;
use rodio::{Decoder, Source};

/// Extra wait allowed beyond a sound's nominal duration before concluding
/// the output stream died mid-play (covers device-open jitter, WASAPI
/// engine latency, and thread scheduling).
#[cfg(not(windows))]
const DRAIN_TIMEOUT_MARGIN: Duration = Duration::from_millis(500);

/// Poll interval for [`wait_for_drain`].
#[cfg(not(windows))]
const DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(10);

// Embedded mute/unmute notification sounds for the `classic` preset.
pub(crate) const SOUND_MUTED: &[u8] = include_bytes!("../assets/muted.wav");
pub(crate) const SOUND_UNMUTED: &[u8] = include_bytes!("../assets/unmuted.wav");

/// Which transition a sound announces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SoundEvent {
    Mute,
    Unmute,
}

/// Preset used when the configured name is unknown.
const DEFAULT_PRESET: &str = "chip";

/// Sample rate of synthesized presets.
const SYNTH_RATE: u32 = 44_100;

/// Attack/release ramp on every tone — long enough to avoid clicks, short
/// enough to keep the electronic edge.
const SYNTH_FADE_MS: u32 = 3;

#[derive(Debug, Clone, Copy)]
enum Wave {
    /// 50% duty — the classic 8-bit beep.
    Square,
    /// 25% duty — thinner, more nasal NES-style pulse.
    Pulse25,
    /// Softer, flute-like electronic tone.
    Triangle,
}

/// One synthesized segment. `from_hz == 0.0` is silence; `from_hz != to_hz`
/// glides linearly between the two frequencies.
#[derive(Debug, Clone, Copy)]
struct Tone {
    from_hz: f32,
    to_hz: f32,
    ms: u32,
    wave: Wave,
}

const fn beep(hz: f32, ms: u32, wave: Wave) -> Tone {
    Tone {
        from_hz: hz,
        to_hz: hz,
        ms,
        wave,
    }
}

const fn glide(from_hz: f32, to_hz: f32, ms: u32, wave: Wave) -> Tone {
    Tone {
        from_hz,
        to_hz,
        ms,
        wave,
    }
}

const fn rest(ms: u32) -> Tone {
    beep(0.0, ms, Wave::Square)
}

// Mute steps down in pitch and unmute steps up, so the direction is audible
// without looking at the panel.

/// Two-note triangle chime (the "lock/unlock" pair from Hranilka).
const CHIP_MUTE: &[Tone] = &[
    beep(990.0, 70, Wave::Triangle),
    beep(660.0, 110, Wave::Triangle),
];
const CHIP_UNMUTE: &[Tone] = &[
    beep(660.0, 70, Wave::Triangle),
    beep(990.0, 110, Wave::Triangle),
];

/// Single short square tick — the least intrusive option.
const BLIP_MUTE: &[Tone] = &[beep(520.0, 60, Wave::Square)];
const BLIP_UNMUTE: &[Tone] = &[beep(1040.0, 60, Wave::Square)];

/// Fast square arpeggio over a C major chord.
const ARCADE_MUTE: &[Tone] = &[
    beep(1046.5, 45, Wave::Square),
    beep(784.0, 45, Wave::Square),
    beep(523.3, 80, Wave::Square),
];
const ARCADE_UNMUTE: &[Tone] = &[
    beep(523.3, 45, Wave::Square),
    beep(659.3, 45, Wave::Square),
    beep(784.0, 45, Wave::Square),
    beep(1046.5, 80, Wave::Square),
];

/// Triangle pitch glide — a "power down / power up" swoop.
const SWEEP_MUTE: &[Tone] = &[glide(1400.0, 350.0, 150, Wave::Triangle)];
const SWEEP_UNMUTE: &[Tone] = &[glide(350.0, 1400.0, 150, Wave::Triangle)];

/// Two 25%-pulse blips an octave apart, like a radio key.
const PULSE_MUTE: &[Tone] = &[
    beep(784.0, 50, Wave::Pulse25),
    rest(35),
    beep(392.0, 80, Wave::Pulse25),
];
const PULSE_UNMUTE: &[Tone] = &[
    beep(392.0, 50, Wave::Pulse25),
    rest(35),
    beep(784.0, 80, Wave::Pulse25),
];

/// Tones for a synthesized preset, or `None` for `classic` and unknown names.
fn preset_tones(preset: &str, event: SoundEvent) -> Option<&'static [Tone]> {
    let (mute, unmute) = match preset {
        "chip" => (CHIP_MUTE, CHIP_UNMUTE),
        "blip" => (BLIP_MUTE, BLIP_UNMUTE),
        "arcade" => (ARCADE_MUTE, ARCADE_UNMUTE),
        "sweep" => (SWEEP_MUTE, SWEEP_UNMUTE),
        "pulse" => (PULSE_MUTE, PULSE_UNMUTE),
        _ => return None,
    };
    Some(match event {
        SoundEvent::Mute => mute,
        SoundEvent::Unmute => unmute,
    })
}

/// Render tones into mono f32 samples at [`SYNTH_RATE`].
fn synthesize(tones: &[Tone]) -> DecodedSound {
    let rate = SYNTH_RATE as f32;
    let fade = (SYNTH_RATE * SYNTH_FADE_MS / 1000).max(1) as f32;
    let mut samples = Vec::new();
    for tone in tones {
        let n = (SYNTH_RATE * tone.ms / 1000) as usize;
        if tone.from_hz <= 0.0 {
            samples.resize(samples.len() + n, 0.0);
            continue;
        }
        // Square-type waves carry far more energy than a triangle at the same
        // peak, so they get a lower peak to land at a similar loudness.
        let peak = match tone.wave {
            Wave::Square | Wave::Pulse25 => 0.32,
            Wave::Triangle => 0.6,
        };
        let mut phase = 0.0_f32;
        for i in 0..n {
            let t = i as f32 / n as f32;
            let hz = tone.from_hz + (tone.to_hz - tone.from_hz) * t;
            let v = match tone.wave {
                Wave::Square if phase < 0.5 => 1.0,
                Wave::Pulse25 if phase < 0.25 => 1.0,
                Wave::Square | Wave::Pulse25 => -1.0,
                Wave::Triangle if phase < 0.5 => 4.0 * phase - 1.0,
                Wave::Triangle => 3.0 - 4.0 * phase,
            };
            let env = (i as f32 / fade).min((n - 1 - i) as f32 / fade).min(1.0);
            samples.push(v * peak * env);
            phase = (phase + hz / rate).fract();
        }
    }
    DecodedSound {
        channels: NonZeroU16::new(1).expect("1 is non-zero"),
        sample_rate: NonZeroU32::new(SYNTH_RATE).expect("rate is non-zero"),
        samples: Arc::new(samples),
    }
}

/// The built-in sound for `preset`. Unknown names use [`DEFAULT_PRESET`].
pub(crate) fn builtin_sound(preset: &str, event: SoundEvent) -> DecodedSound {
    if preset == "classic" {
        let wav = match event {
            SoundEvent::Mute => SOUND_MUTED,
            SoundEvent::Unmute => SOUND_UNMUTED,
        };
        return decode_wav(wav).expect("embedded WAV must be valid");
    }
    let tones = preset_tones(preset, event).unwrap_or_else(|| {
        log::warn!("[sound] unknown sound preset \"{preset}\", using \"{DEFAULT_PRESET}\"");
        preset_tones(DEFAULT_PRESET, event).expect("default preset exists")
    });
    synthesize(tones)
}

/// Pre-decoded sound ready for playback via `SamplesBuffer`.
///
/// Samples are wrapped in `Arc` so cloning a `DecodedSound` (e.g. on every
/// mute toggle) is a cheap ref-count bump instead of copying the sample buffer.
#[derive(Clone)]
pub(crate) struct DecodedSound {
    channels: NonZeroU16,
    sample_rate: NonZeroU32,
    samples: Arc<Vec<f32>>,
}

impl DecodedSound {
    /// Nominal playback duration of the decoded samples.
    #[cfg_attr(windows, allow(dead_code))]
    fn duration(&self) -> Duration {
        let frames = self.samples.len() as f64 / f64::from(self.channels.get());
        Duration::from_secs_f64(frames / f64::from(self.sample_rate.get()))
    }
}

/// Wait until `is_empty` reports the player queue drained, or `timeout` elapses.
///
/// Returns `true` if playback drained, `false` on timeout. This replaces
/// `Player::sleep_until_end()`, whose end-signal only fires when the output
/// stream polls the queued source to exhaustion — if the WASAPI session is
/// disconnected mid-play (format change, exclusive-mode grab, device
/// removal), that signal never comes and the wait would block forever while
/// holding the dead sink. A polled deadline keeps the helper thread's
/// lifetime bounded either way.
#[cfg(not(windows))]
fn wait_for_drain(is_empty: impl Fn() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while !is_empty() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(DRAIN_POLL_INTERVAL);
    }
    true
}

/// Decode raw WAV bytes into a `DecodedSound`.
fn decode_wav(wav_bytes: &[u8]) -> Option<DecodedSound> {
    let decoder = Decoder::new(Cursor::new(wav_bytes.to_vec())).ok()?;
    let channels = decoder.channels();
    let sample_rate = decoder.sample_rate();
    let samples: Vec<f32> = decoder.collect();
    Some(DecodedSound {
        channels,
        sample_rate,
        samples: Arc::new(samples),
    })
}

/// Load and decode sound from a custom path, falling back to the built-in
/// `preset` sound on any error.
///
/// Returns `(decoded_sound, optional_warning)`. The warning is set when a custom
/// path was specified but the file could not be loaded (missing, invalid WAV, etc.).
pub(crate) fn load_sound_data(
    path: &str,
    preset: &str,
    event: SoundEvent,
) -> (DecodedSound, Option<String>) {
    let path = path.trim();
    if path.is_empty() {
        return (builtin_sound(preset, event), None);
    }
    match std::fs::read(path) {
        Ok(data) => match decode_wav(&data) {
            Some(decoded) => (decoded, None),
            None => {
                let msg = format!("{path} is not a valid WAV file, using built-in");
                log::warn!("[sound] {msg}");
                (builtin_sound(preset, event), Some(msg))
            }
        },
        Err(e) => {
            let msg = format!("could not read {path}: {e}, using built-in");
            log::warn!("[sound] {msg}");
            (builtin_sound(preset, event), Some(msg))
        }
    }
}

/// Encode samples as a 16-bit PCM WAV file with `volume` baked in.
#[cfg(any(windows, test))]
fn encode_wav(sound: &DecodedSound, volume: f32) -> Vec<u8> {
    let channels = sound.channels.get();
    let rate = sound.sample_rate.get();
    let data_len = (sound.samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
    wav.extend_from_slice(&(channels * 2).to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    let volume = volume.clamp(0.0, 1.0);
    for sample in sound.samples.iter() {
        let value = (sample * volume).clamp(-1.0, 1.0) * f32::from(i16::MAX);
        wav.extend_from_slice(&(value as i16).to_le_bytes());
    }
    wav
}

/// Buffers handed to `PlaySoundW`, which reads them asynchronously. A new
/// call stops the previous sound first, so keeping the last two is enough
/// to never free a buffer that is still being played.
#[cfg(windows)]
static PLAYING: std::sync::Mutex<Vec<Arc<Vec<u8>>>> = std::sync::Mutex::new(Vec::new());

/// Play a pre-decoded sound through the system mixer.
///
/// `PlaySoundW` with `SND_MEMORY | SND_ASYNC` returns immediately and needs
/// no thread or audio stream of our own: nothing is opened per beep and
/// nothing is held between beeps, so other apps can never be forced to evict
/// a FocusMute stream. Errors are ignored — a missed beep is never fatal.
#[cfg(windows)]
pub(crate) fn play_sound(sound: &DecodedSound, volume: f32) {
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
    use windows::core::PCWSTR;

    if volume <= 0.0 {
        return;
    }
    let wav = Arc::new(encode_wav(sound, volume));
    let Ok(mut playing) = PLAYING.lock() else {
        return;
    };
    playing.push(Arc::clone(&wav));
    // SAFETY: with SND_MEMORY the "name" is a pointer to a complete WAV
    // image; `PLAYING` keeps it alive until two later calls have replaced it.
    let ok = unsafe {
        PlaySoundW(
            PCWSTR(wav.as_ptr().cast()),
            None,
            SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
        )
    };
    if playing.len() > 2 {
        playing.remove(0);
    }
    if ok.as_bool() {
        // The call returned; the sound reaches the output ~20-30 ms later.
        log::debug!("[sound] playback queued (volume={:.0}%)", volume * 100.0);
    } else {
        log::warn!("[sound] PlaySound failed");
    }
}

/// Play a pre-decoded sound on a short-lived background thread.
///
/// The thread opens the default audio sink, plays the sound, waits (bounded
/// by the sound's duration plus [`DRAIN_TIMEOUT_MARGIN`]) for playback to
/// drain, then drops the sink — releasing the WASAPI endpoint immediately.
/// This avoids holding a stream between mute events, which was known to
/// wedge USB DACs (e.g. Schiit Magni Unity) when other apps (games, DAWs)
/// later tried to acquire the endpoint.
///
/// Fire-and-forget: the caller does not wait for playback to finish.
/// Errors are logged and swallowed — a missed beep is never fatal.
#[cfg(not(windows))]
pub(crate) fn play_sound(sound: &DecodedSound, volume: f32) {
    let sound = sound.clone();
    std::thread::Builder::new()
        .name("sound-play".into())
        .spawn(move || {
            let mixer = match rodio::DeviceSinkBuilder::open_default_sink() {
                Ok(m) => m,
                Err(e) => {
                    log::warn!("[sound] could not open audio output: {e}");
                    return;
                }
            };
            let player = Player::connect_new(mixer.mixer());
            player.set_volume(volume);
            let source =
                SamplesBuffer::new(sound.channels, sound.sample_rate, sound.samples.as_slice());
            player.append(source);
            log::debug!("[sound] playback started (volume={:.0}%)", volume * 100.0);
            // Wait for the beep to drain, then drop `player` and `mixer` —
            // releasing the WASAPI stream so no other app can be forced to
            // evict it later. The wait is bounded: a stream lost mid-beep
            // must not leak this thread.
            let timeout = sound.duration() + DRAIN_TIMEOUT_MARGIN;
            if !wait_for_drain(|| player.empty(), timeout) {
                log::warn!(
                    "[sound] playback did not drain within {timeout:?} — output stream likely lost"
                );
            }
            drop(player);
            drop(mixer);
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_sounds_are_valid_wav() {
        let muted = Decoder::new(Cursor::new(SOUND_MUTED));
        assert!(muted.is_ok(), "muted.wav should be a valid WAV file");

        let unmuted = Decoder::new(Cursor::new(SOUND_UNMUTED));
        assert!(unmuted.is_ok(), "unmuted.wav should be a valid WAV file");
    }

    #[test]
    fn decode_builtin_muted_has_valid_metadata() {
        let decoded = decode_wav(SOUND_MUTED).expect("should decode");
        assert!(decoded.channels.get() > 0);
        assert!(decoded.sample_rate.get() > 0);
        assert!(!decoded.samples.is_empty());
    }

    #[test]
    fn decode_builtin_unmuted_has_valid_metadata() {
        let decoded = decode_wav(SOUND_UNMUTED).expect("should decode");
        assert!(decoded.channels.get() > 0);
        assert!(decoded.sample_rate.get() > 0);
        assert!(!decoded.samples.is_empty());
    }

    #[test]
    fn decode_invalid_wav_returns_none() {
        assert!(decode_wav(b"this is not wav data").is_none());
    }

    #[test]
    fn decoded_sound_duration_from_sample_count() {
        let sound = DecodedSound {
            channels: NonZeroU16::new(2).unwrap(),
            sample_rate: NonZeroU32::new(8_000).unwrap(),
            samples: Arc::new(vec![0.0; 4_000]), // 2000 frames at 8 kHz
        };
        assert_eq!(sound.duration(), Duration::from_millis(250));
    }

    #[test]
    fn builtin_sound_durations_are_sane() {
        for bytes in [SOUND_MUTED, SOUND_UNMUTED] {
            let duration = decode_wav(bytes).unwrap().duration();
            assert!(
                duration > Duration::ZERO && duration < Duration::from_secs(5),
                "unexpected builtin sound duration: {duration:?}"
            );
        }
    }

    #[test]
    fn encoded_wav_round_trips_through_the_decoder() {
        let sound = builtin_sound("chip", SoundEvent::Mute);
        let wav = encode_wav(&sound, 0.5);
        let decoded = decode_wav(&wav).expect("encoded WAV must decode");
        assert_eq!(decoded.channels, sound.channels);
        assert_eq!(decoded.sample_rate, sound.sample_rate);
        assert_eq!(decoded.samples.len(), sound.samples.len());
        let peak_in = sound.samples.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        let peak_out = decoded.samples.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(
            (peak_out - peak_in * 0.5).abs() < 0.01,
            "volume must be baked in: {peak_in} -> {peak_out}"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn wait_for_drain_returns_immediately_when_empty() {
        let start = Instant::now();
        assert!(wait_for_drain(|| true, Duration::from_secs(5)));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[cfg(not(windows))]
    #[test]
    fn wait_for_drain_times_out_when_stream_never_drains() {
        let timeout = Duration::from_millis(50);
        let start = Instant::now();
        assert!(!wait_for_drain(|| false, timeout));
        assert!(start.elapsed() >= timeout);
    }

    #[cfg(not(windows))]
    #[test]
    fn wait_for_drain_completes_when_queue_empties_later() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let polls = AtomicUsize::new(0);
        assert!(wait_for_drain(
            || polls.fetch_add(1, Ordering::SeqCst) >= 3,
            Duration::from_secs(5),
        ));
    }

    #[test]
    fn load_sound_data_empty_path_returns_decoded_builtin() {
        let (result, warning) = load_sound_data("", "classic", SoundEvent::Mute);
        let reference = decode_wav(SOUND_MUTED).unwrap();
        assert_eq!(result.channels, reference.channels);
        assert_eq!(result.sample_rate, reference.sample_rate);
        assert_eq!(result.samples.len(), reference.samples.len());
        assert!(warning.is_none());
    }

    #[test]
    fn load_sound_data_whitespace_path_returns_builtin() {
        let (result, warning) = load_sound_data("   ", "chip", SoundEvent::Mute);
        assert!(result.channels.get() > 0);
        assert!(warning.is_none());
    }

    #[test]
    fn load_sound_data_missing_file_returns_builtin() {
        let (result, warning) =
            load_sound_data("/nonexistent/path/sound.wav", "chip", SoundEvent::Mute);
        let reference = builtin_sound("chip", SoundEvent::Mute);
        assert_eq!(result.samples.len(), reference.samples.len());
        assert!(warning.is_some(), "should warn about missing file");
    }

    #[test]
    fn load_sound_data_invalid_wav_returns_builtin() {
        let dir = std::env::temp_dir().join("focusmute_test_sound");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("not_a_wav.wav");
        std::fs::write(&path, b"this is not a wav file").unwrap();

        let (result, warning) =
            load_sound_data(path.to_str().unwrap(), "classic", SoundEvent::Mute);
        let reference = decode_wav(SOUND_MUTED).unwrap();
        assert_eq!(result.samples.len(), reference.samples.len());
        assert!(warning.is_some(), "should warn about invalid WAV");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_sound_data_valid_wav_returns_custom() {
        let dir = std::env::temp_dir().join("focusmute_test_sound_valid");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.wav");
        std::fs::write(&path, SOUND_MUTED).unwrap();

        let (result, warning) =
            load_sound_data(path.to_str().unwrap(), "classic", SoundEvent::Unmute);
        // Should decode to the muted sound data, not the unmuted fallback
        let muted_ref = decode_wav(SOUND_MUTED).unwrap();
        assert_eq!(result.samples.len(), muted_ref.samples.len());
        assert!(warning.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_listed_preset_builds_both_sounds() {
        for preset in focusmute_lib::config::VALID_SOUND_PRESETS {
            for event in [SoundEvent::Mute, SoundEvent::Unmute] {
                let sound = builtin_sound(preset, event);
                let duration = sound.duration();
                assert!(
                    duration >= Duration::from_millis(40) && duration < Duration::from_secs(1),
                    "{preset} {event:?}: unexpected duration {duration:?}"
                );
                assert!(
                    sound.samples.iter().all(|s| s.abs() <= 1.0),
                    "{preset} {event:?}: sample out of range"
                );
            }
        }
    }

    #[test]
    fn synthesized_tones_start_and_end_silent() {
        let sound = builtin_sound("arcade", SoundEvent::Unmute);
        assert_eq!(sound.samples.first().copied(), Some(0.0));
        assert!(sound.samples.last().unwrap().abs() < 0.01);
    }

    #[test]
    fn mute_and_unmute_differ_within_a_preset() {
        for preset in ["chip", "blip", "arcade", "sweep", "pulse"] {
            let mute = builtin_sound(preset, SoundEvent::Mute);
            let unmute = builtin_sound(preset, SoundEvent::Unmute);
            assert_ne!(mute.samples, unmute.samples, "{preset}");
        }
    }

    #[test]
    fn unknown_preset_falls_back_to_default() {
        let fallback = builtin_sound("no-such-preset", SoundEvent::Mute);
        let default = builtin_sound(DEFAULT_PRESET, SoundEvent::Mute);
        assert_eq!(fallback.samples, default.samples);
    }

    #[test]
    fn default_config_preset_is_synthesized() {
        let preset = focusmute_lib::config::SoundConfig::default().sound_preset;
        assert!(preset_tones(&preset, SoundEvent::Mute).is_some());
    }
}
