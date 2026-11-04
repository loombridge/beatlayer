//! Impulse-response slots (WP-8.30; the impulse responses spec, R1 to R7):
//! turning a file into a response a [`Convolver`] plays, and the data and
//! node half of a device that convolves.
//!
//! [`load`] decodes the file, refuses what R3 refuses, resamples it to the
//! engine's rate, trims its silent tail (R6), normalises it (R7) and prepares
//! it. It allocates and takes as long as the file is long, so it runs on a
//! loader thread ([`load_cached`] keeps the last few results by content hash
//! for the compiler to find), never on the main or the audio thread.

use std::path::Path;
use std::sync::{Arc, Mutex};

use bl_dsp::convolution::{self, Convolver, Layout, PreparedIr};
use bl_engine::{DataFn, DeviceData, FileOf, NodeConfig, NodeData};
use bl_media::{SampleData, convert_sample_rate};
use bl_model::Device;

/// The level, in dB below a response's peak, under which its tail is cut
/// (R6).
const TAIL_DB: f64 = -90.0;

/// A response ready to play, with what `device.load_sample` reports of it
/// (R12).
pub struct Loaded {
    /// The prepared response, at the engine's rate.
    pub ir: Arc<PreparedIr>,
    /// Its channel layout.
    pub layout: Layout,
    /// The file's length in frames at its own rate.
    pub file_frames: usize,
    /// The response's length in frames at the engine's rate, after trimming.
    pub frames: usize,
    /// The gain normalising applied, in dB.
    pub gain_db: f32,
}

impl Loaded {
    /// The layout's name as `device.impulse_responses` reports it.
    #[must_use]
    pub fn layout_name(&self) -> &'static str {
        match self.layout {
            Layout::Mono => "mono",
            Layout::Stereo => "stereo",
            Layout::TrueStereo => "true_stereo",
        }
    }
}

/// Why a file can't be a response.
#[derive(Debug, PartialEq, Eq)]
pub enum LoadError {
    /// The file has a channel count other than 1, 2 or 4 (R3).
    Channels(usize),
    /// The file is longer than 30 seconds at its own rate (R3).
    TooLong,
    /// The file is silent, so there is nothing to play.
    Silent,
    /// The file can't be read.
    Media(String),
    /// The engine can't prepare it.
    Prepare(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Channels(n) => write!(
                f,
                "an impulse response has 1, 2 or 4 channels; this file has {n}"
            ),
            Self::TooLong => write!(
                f,
                "an impulse response is at most {} seconds long",
                convolution::MAX_SECONDS
            ),
            Self::Silent => write!(f, "the file is silent"),
            Self::Media(why) | Self::Prepare(why) => f.write_str(why),
        }
    }
}

/// Reads the file at `path` as a response at engine rate `rate` (R3 to R7).
/// Not for the main or the audio thread.
///
/// # Errors
///
/// If the file can't be read or is refused by R3, or is silent.
pub fn load(path: &Path, rate: u32) -> Result<Loaded, LoadError> {
    let info = bl_media::probe(path).map_err(|e| LoadError::Media(e.to_string()))?;
    check_header(usize::from(info.channels), info.frames, info.sample_rate)?;
    let data = bl_media::decode_file(path).map_err(|e| LoadError::Media(e.to_string()))?;
    from_samples(&data, rate)
}

fn check_header(channels: usize, frames: Option<u64>, rate: u32) -> Result<(), LoadError> {
    if !matches!(channels, 1 | 2 | 4) {
        return Err(LoadError::Channels(channels));
    }
    if frames.is_some_and(|f| f > u64::from(convolution::MAX_SECONDS) * u64::from(rate)) {
        return Err(LoadError::TooLong);
    }
    Ok(())
}

/// [`load`] for audio already decoded.
///
/// # Errors
///
/// As [`load`].
pub fn from_samples(data: &SampleData, rate: u32) -> Result<Loaded, LoadError> {
    check_header(
        data.channel_count(),
        Some(data.frames() as u64),
        data.sample_rate(),
    )?;
    let file_frames = data.frames();
    let data = convert_sample_rate(data, rate).map_err(|e| LoadError::Media(e.to_string()))?;
    let mut channels: Vec<Vec<f32>> = data.channels().map(<[f32]>::to_vec).collect();
    // R6: keep up to the last frame any channel reaches the tail level.
    let peak = channels
        .iter()
        .flatten()
        .fold(0.0_f32, |peak, s| peak.max(s.abs()));
    if peak == 0.0 {
        return Err(LoadError::Silent);
    }
    let floor = peak * 10.0_f32.powf((TAIL_DB / 20.0) as f32);
    let frames = channels
        .iter()
        .filter_map(|c| c.iter().rposition(|s| s.abs() >= floor))
        .max()
        .map_or(1, |last| last + 1);
    // R7: energy 1 per output channel.
    let target = if channels.len() == 1 { 1.0 } else { 2.0 };
    let energy: f64 = channels
        .iter()
        .flat_map(|c| &c[..frames])
        .map(|&s| f64::from(s) * f64::from(s))
        .sum();
    let gain = (target / energy).sqrt() as f32;
    for channel in &mut channels {
        channel.truncate(frames);
        channel.iter_mut().for_each(|s| *s *= gain);
    }
    let refs: Vec<&[f32]> = channels.iter().map(Vec::as_slice).collect();
    let ir = convolution::prepare(&refs, rate).map_err(|e| LoadError::Prepare(e.to_string()))?;
    Ok(Loaded {
        layout: ir.layout(),
        ir,
        file_frames,
        frames,
        gain_db: 20.0 * gain.log10(),
    })
}

/// How many responses [`load_cached`] keeps.
const KEPT: usize = 8;

static CACHE: Mutex<Vec<(bl_model::ContentHash, u32, Arc<Loaded>)>> = Mutex::new(Vec::new());

/// The response for `media` at `rate` if a loader has made it and it is
/// still kept.
#[must_use]
pub fn cached(media: bl_model::ContentHash, rate: u32) -> Option<Arc<Loaded>> {
    let cache = CACHE.lock().ok()?;
    cache
        .iter()
        .find(|(m, r, _)| *m == media && *r == rate)
        .map(|(_, _, loaded)| loaded.clone())
}

/// [`load`], kept by `media` and `rate`: a response already loaded comes
/// back at once, the newest few are kept for the compiler.
///
/// # Errors
///
/// As [`load`].
pub fn load_cached(
    media: bl_model::ContentHash,
    path: &Path,
    rate: u32,
) -> Result<Arc<Loaded>, LoadError> {
    if let Some(loaded) = cached(media, rate) {
        return Ok(loaded);
    }
    let loaded = Arc::new(load(path, rate)?);
    if let Ok(mut cache) = CACHE.lock() {
        cache.retain(|(m, r, _)| !(*m == media && *r == rate));
        cache.insert(0, (media, rate, loaded.clone()));
        cache.truncate(KEPT);
    }
    Ok(loaded)
}

/// What a device's node is sent for one slot: the response to crossfade to
/// (R4). A slot with nothing in it is sent a silent one.
pub struct Response {
    /// The slot, counted from 0.
    pub slot: usize,
    /// The response.
    pub ir: Arc<PreparedIr>,
}

/// A response that plays nothing, for an emptied slot.
fn silence(rate: u32) -> Option<Arc<PreparedIr>> {
    convolution::prepare(&[&[0.0]], rate).ok()
}

/// The data of a device type with `slots` impulse-response slots: for each,
/// the response its media names, built here or found in the loader's cache,
/// or silence for an empty slot; media `file` can't name is reported
/// missing and plays silence (R9). Not for the audio thread.
#[must_use]
pub fn data(slots: usize) -> DataFn {
    Arc::new(
        move |device: &Device, config: &NodeConfig, file: FileOf<'_>| {
            let rate = config.sample_rate.hz();
            let mut data = DeviceData::default();
            for slot in 0..slots {
                let ir = match device.media_at(slot) {
                    None => silence(rate),
                    Some(media) => match file(media) {
                        Some(path) => load_cached(media, &path, rate)
                            .map(|loaded| loaded.ir.clone())
                            .ok()
                            .or_else(|| silence(rate)),
                        None => {
                            data.missing.push(media);
                            silence(rate)
                        }
                    },
                };
                if let Some(ir) = ir {
                    data.items
                        .push(Box::new(move |gc| gc.data(Response { slot, ir })));
                }
            }
            data
        },
    )
}

/// The audio-thread half of a device with one impulse-response slot: a
/// convolver, and the data it was sent while it still plays.
pub struct Convolving {
    convolver: Convolver,
    /// The responses sent and not yet retired, so the last reference to one
    /// is the data's, which the drop thread frees.
    held: Vec<NodeData>,
}

impl Convolving {
    /// A convolver at `rate`. Allocates.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        Self {
            convolver: Convolver::new(rate, convolution::MAX_SECONDS as usize * rate as usize),
            held: Vec::with_capacity(8),
        }
    }

    /// Takes a response sent for `slot`, if the data is one. Allocates
    /// nothing.
    pub fn receive(&mut self, slot: usize, data: NodeData) {
        let Some(response) = data.downcast_ref::<Response>() else {
            return;
        };
        if response.slot != slot || self.held.len() == self.held.capacity() {
            return;
        }
        let ir = response.ir.clone();
        if let Err((_, ir)) = self.convolver.swap(ir, convolution::DEFAULT_FADE) {
            drop(ir);
            return;
        }
        self.held.push(data);
    }

    /// Convolves `left` and `right` in place and lets go of what has
    /// retired.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.convolver.process(left, right);
        while let Some(old) = self.convolver.take_retired() {
            // The count first, so the data's reference is the last.
            let held = self.held.iter().position(|d| {
                d.downcast_ref::<Response>()
                    .is_some_and(|r| Arc::ptr_eq(&r.ir, &old))
            });
            drop(old);
            if let Some(held) = held {
                self.held.swap_remove(held);
            }
        }
    }
}
