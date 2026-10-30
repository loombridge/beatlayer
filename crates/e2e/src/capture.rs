//! What the null device played: a [`Recorder`] is patched to the device as
//! its cable (`bl_audio_io::Cable`), sees every output block the stream
//! ran, and keeps a copy of the blocks played while it records.
//!
//! The cable also decides what the device's inputs carry, in place of the
//! null device's own test tones: silence, or a sine the recorder makes on
//! one input channel ([`Recorder::set_input_tone`]), so a recording test
//! has something to record.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use bl_audio_io::Cable;

use crate::audio::Audio;

/// What a live capture gives: the device's output channels at its rate.
pub type Capture = Audio;

/// A sine on one input channel.
#[derive(Clone, Copy, Debug)]
struct Tone {
    channel: usize,
    hz: f64,
    amplitude: f32,
    phase: f64,
}

/// What the cable and the test share.
#[derive(Debug, Default)]
struct Shared {
    /// What was played while recording, one `Vec` per output channel.
    played: Vec<Vec<f32>>,
    /// The inputs' tone, if any.
    tone: Option<Tone>,
    /// The device's rate, for the tone.
    rate: u32,
}

/// Copies the null device's output while it records. Clone it freely:
/// every clone is the same recorder.
#[derive(Clone, Debug)]
pub struct Recorder {
    recording: Arc<AtomicBool>,
    shared: Arc<Mutex<Shared>>,
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new()
    }
}

impl Recorder {
    /// A recorder that isn't recording, with silent inputs.
    #[must_use]
    pub fn new() -> Self {
        Self {
            recording: Arc::new(AtomicBool::new(false)),
            shared: Arc::new(Mutex::new(Shared {
                rate: 48_000,
                ..Shared::default()
            })),
        }
    }

    /// The cable to give the null backend (`NullBackend::with_cable`).
    #[must_use]
    pub fn cable(&self) -> Cable {
        let recording = Arc::clone(&self.recording);
        let shared = Arc::clone(&self.shared);
        Cable::new(move |played, next| {
            let mut shared = shared.lock().unwrap_or_else(PoisonError::into_inner);
            if recording.load(Ordering::Acquire) {
                if shared.played.len() < played.len() {
                    shared.played.resize_with(played.len(), Vec::new);
                }
                for (keep, block) in shared.played.iter_mut().zip(played) {
                    keep.extend_from_slice(block);
                }
            }
            let rate = f64::from(shared.rate.max(1));
            let mut tone = shared.tone;
            for (channel, input) in next.iter_mut().enumerate() {
                match &mut tone {
                    Some(tone) if tone.channel == channel => {
                        for sample in input.iter_mut() {
                            tone.phase = (tone.phase + tone.hz / rate).fract();
                            *sample =
                                (tone.phase * std::f64::consts::TAU).sin() as f32 * tone.amplitude;
                        }
                    }
                    _ => input.fill(0.0),
                }
            }
            shared.tone = tone;
        })
    }

    /// Tells the recorder the device's rate, which its input tone follows.
    pub fn set_rate(&self, rate: u32) {
        self.lock().rate = rate;
    }

    /// Puts a sine of `hz` at `amplitude` peak on input channel `channel`
    /// (counting from 0), or, with `None`, silences the inputs.
    pub fn set_input_tone(&self, tone: Option<(usize, f64, f32)>) {
        self.lock().tone = tone.map(|(channel, hz, amplitude)| Tone {
            channel,
            hz,
            amplitude,
            phase: 0.0,
        });
    }

    /// Starts keeping what the device plays, from the next block, after
    /// forgetting what was kept before.
    pub fn start(&self) {
        self.lock().played.clear();
        self.recording.store(true, Ordering::Release);
    }

    /// Stops keeping what the device plays, and gives what was kept.
    pub fn stop(&self) -> Capture {
        self.recording.store(false, Ordering::Release);
        let mut shared = self.lock();
        let channels = std::mem::take(&mut shared.played);
        Audio::new(shared.rate, channels)
    }

    /// Whether it's recording.
    #[must_use]
    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Acquire)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
