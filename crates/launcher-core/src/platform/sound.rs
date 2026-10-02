//! Click sounds, played by the launcher itself through the system's audio output. The web view
//! plays no sound: on Linux it would need GStreamer's plugins, and a missing one hangs the window.

use std::collections::HashMap;
use std::num::NonZero;
use std::sync::Mutex;
use std::sync::mpsc::{self, Sender};

use launcher_shared::ClickSound;

/// The built-in WAV file of `sound`.
pub fn click_wav(sound: ClickSound) -> &'static [u8] {
    match sound {
        ClickSound::TypewriterSoftClick => {
            include_bytes!("../../../../assets/sounds/typewriter-soft-click.wav")
        }
        ClickSound::GateLatchClick => include_bytes!("../../../../assets/sounds/gate-latch-click.wav"),
        ClickSound::PlasticBubbleClick => {
            include_bytes!("../../../../assets/sounds/plastic-bubble-click.wav")
        }
        ClickSound::SoftPop => include_bytes!("../../../../assets/sounds/soft-pop-click.wav"),
        ClickSound::GlassTap => include_bytes!("../../../../assets/sounds/glass-tap-click.wav"),
        ClickSound::WaterDrop => include_bytes!("../../../../assets/sounds/water-drop-click.wav"),
        ClickSound::MechanicalKey => include_bytes!("../../../../assets/sounds/mechanical-key-click.wav"),
        ClickSound::DigitalBlip => include_bytes!("../../../../assets/sounds/digital-blip-click.wav"),
        ClickSound::WoodTap => include_bytes!("../../../../assets/sounds/wood-tap-click.wav"),
        ClickSound::CosmicZap => include_bytes!("../../../../assets/sounds/cosmic-zap-click.wav"),
    }
}

/// A decoded sound: interleaved samples in -1..=1.
#[derive(Debug, Clone, PartialEq)]
pub struct Pcm {
    pub channels: u16,
    pub rate: u32,
    pub samples: Vec<f32>,
}

/// `wav` decoded (integer or float PCM); `None` when it is no WAV this can read.
pub fn decode(wav: &[u8]) -> Option<Pcm> {
    let reader = hound::WavReader::new(std::io::Cursor::new(wav)).ok()?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.into_samples::<f32>().collect::<Result<_, _>>().ok()?,
        hound::SampleFormat::Int => {
            let full = (1u64 << spec.bits_per_sample.clamp(1, 32).saturating_sub(1)) as f32;
            reader.into_samples::<i32>().map(|s| s.map(|v| v as f32 / full)).collect::<Result<_, _>>().ok()?
        }
    };
    (spec.channels > 0 && spec.sample_rate > 0 && !samples.is_empty()).then_some(Pcm {
        channels: spec.channels,
        rate: spec.sample_rate,
        samples,
    })
}

/// A click's volume.
const VOLUME: f32 = 0.55;

/// Plays clicks on the system's default output from a thread of its own, which opens the device
/// at the first click. Without an output device the clicks are silent.
#[derive(Default)]
pub struct ClickPlayer {
    clicks: Mutex<Option<Sender<ClickSound>>>,
}

impl ClickPlayer {
    pub fn new() -> ClickPlayer {
        ClickPlayer::default()
    }

    /// Plays `sound` (returns at once).
    pub fn play(&self, sound: ClickSound) {
        let mut clicks = self.clicks.lock().unwrap_or_else(|e| e.into_inner());
        let sender = clicks.get_or_insert_with(spawn_player);
        if sender.send(sound).is_err() {
            // The thread is gone: the next click starts a new one.
            *clicks = None;
        }
    }
}

fn spawn_player() -> Sender<ClickSound> {
    let (tx, rx) = mpsc::channel::<ClickSound>();
    let started = std::thread::Builder::new().name("click-sounds".into()).spawn(move || {
        let mut device = match rodio::DeviceSinkBuilder::open_default_sink() {
            Ok(device) => device,
            Err(e) => {
                tracing::warn!("Click sounds are off: no audio output ({e})");
                // Clicks keep coming; they are dropped.
                for _ in rx {}
                return;
            }
        };
        device.log_on_drop(false);
        let mut decoded: HashMap<&'static str, Option<Pcm>> = HashMap::new();
        for sound in rx {
            let pcm = decoded.entry(sound.as_config_str()).or_insert_with(|| decode(click_wav(sound)));
            let Some(pcm) = pcm else { continue };
            let (Some(channels), Some(rate)) = (NonZero::new(pcm.channels), NonZero::new(pcm.rate)) else {
                continue;
            };
            use rodio::Source;
            let click =
                rodio::buffer::SamplesBuffer::new(channels, rate, pcm.samples.clone()).amplify(VOLUME);
            device.mixer().add(click);
        }
    });
    if let Err(e) = started {
        tracing::warn!("Click sounds are off: {e}");
    }
    tx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_click_sound_is_built_in_and_decodes() {
        for sound in ClickSound::ALL {
            let pcm = decode(click_wav(sound)).unwrap_or_else(|| panic!("{sound:?} decodes"));
            assert!(pcm.channels > 0 && pcm.rate >= 8_000, "{sound:?}: {} ch, {} Hz", pcm.channels, pcm.rate);
            assert!(pcm.samples.len() > 1_000, "{sound:?} is a sound, not silence");
            assert!(pcm.samples.iter().all(|s| s.abs() <= 1.0), "{sound:?} stays in range");
            assert!(pcm.samples.iter().any(|s| s.abs() > 0.01), "{sound:?} is audible");
        }
        assert_eq!(decode(b"not a wav"), None);
    }
}
