//! Decoding previews in the background (spec: browser R11, R12).
//!
//! A preview is decoded and converted to the device's sample rate on a
//! thread of its own, so selecting file after file never stalls the window.
//! Only the newest request is decoded; older ones still waiting are skipped.
//! The result goes to the audio thread's preview voice
//! (`bl_engine::preview`) from the main thread.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::SystemTime;

use bl_core::time::{Bpm, TempoMap, Ticks};
use bl_engine::preview::PreviewSamples;
use bl_media::{Decoder, SampleData, convert_sample_rate};
use bl_model::{ClipData, Command, ContentHash, DeviceRole, Editor, WarpMarker};
use bl_plugin_host::Hosts;
use bl_plugin_host::au::AuHost;
use bl_plugin_host::clap::ClapHost;
use bl_plugin_host::vst3::Vst3Host;
use bl_project::clip_file::ClipFile;

use super::Waker;

/// The longest preview decoded into memory, in seconds: longer files preview
/// their start, until previews stream from disk with WP-1.3's streamer.
pub const MAX_PREVIEW_SECONDS: u64 = 600;

/// Decoded audio as the preview voice reads it.
#[derive(Debug)]
pub struct MediaSamples(pub SampleData);

impl PreviewSamples for MediaSamples {
    fn channel_count(&self) -> usize {
        self.0.channel_count()
    }

    fn frames(&self) -> usize {
        self.0.frames()
    }

    fn channel(&self, index: usize) -> &[f32] {
        self.0.channel(index).unwrap_or_default()
    }
}

/// A decoded preview, or why it couldn't be.
#[derive(Debug)]
pub struct Loaded {
    /// The request it answers.
    pub id: u64,
    /// The file.
    pub path: PathBuf,
    /// The audio at the requested rate, or the error.
    pub result: Result<SampleData, String>,
    /// A waveform overview for the preview strip: the lowest and highest
    /// sample of each of [`OVERVIEW_POINTS`] stretches, every channel
    /// together.
    pub overview: Vec<[f32; 2]>,
    /// For a clip file, the audio clip it holds: the preview plays through
    /// its warp settings, markers, loop and start and end (clip files R13).
    /// For a MIDI clip file, the warp that lays its render on the set's beats
    /// (clip files R16).
    pub clip: Option<Box<bl_model::AudioData>>,
}

/// How many stretches a preview's waveform overview has.
pub const OVERVIEW_POINTS: usize = 256;

/// The lowest and highest sample of each of `points` equal stretches of
/// `data`, every channel together.
#[must_use]
pub fn overview(data: &SampleData, points: usize) -> Vec<[f32; 2]> {
    let frames = data.frames();
    if frames == 0 || points == 0 {
        return Vec::new();
    }
    (0..points)
        .map(|point| {
            let start = point * frames / points;
            let end = ((point + 1) * frames / points).max(start + 1).min(frames);
            let mut range = [0.0f32, 0.0f32];
            for channel in data.channels() {
                for sample in &channel[start..end] {
                    range[0] = range[0].min(*sample);
                    range[1] = range[1].max(*sample);
                }
            }
            range
        })
        .collect()
}

struct Request {
    id: u64,
    path: PathBuf,
    rate: u32,
    /// Set for a clip file: the folders to search for its audio if it isn't
    /// where the file says.
    clip_search: Option<Vec<PathBuf>>,
    /// The set's tempo, which a MIDI clip file is rendered at.
    bpm: f64,
    /// Set when a newer request, or a stop, makes this one's render pointless.
    cancel: Arc<AtomicBool>,
}

/// The preview decoder: a thread that decodes the newest request.
pub struct Loader {
    requests: Option<Sender<Request>>,
    results: Option<Receiver<Loaded>>,
    waker: Waker,
    next: u64,
    cancel: Arc<AtomicBool>,
}

impl std::fmt::Debug for Loader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loader")
            .field("next", &self.next)
            .finish_non_exhaustive()
    }
}

impl Loader {
    /// A decoder that wakes `waker` when a preview is ready. Its thread
    /// starts with the first request.
    #[must_use]
    pub fn new(waker: Waker) -> Self {
        Self {
            requests: None,
            results: None,
            waker,
            next: 0,
            cancel: Arc::default(),
        }
    }

    /// Drops the render under way, if any.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Asks for `path` decoded at `rate`; returns the request's ID.
    ///
    /// # Errors
    ///
    /// If the decoding thread can't start.
    pub fn request(&mut self, path: &Path, rate: u32) -> Result<u64, String> {
        self.send(path, rate, None, 0.0)
    }

    /// Asks for the audio clip in the clip file `path` and the audio it
    /// plays, decoded at `rate`, searching `search` for the audio if it moved
    /// (clip files R13, R14); returns the request's ID. A MIDI clip file is
    /// rendered at `bpm` instead (clip files R16).
    ///
    /// # Errors
    ///
    /// If the decoding thread can't start.
    pub fn request_clip(
        &mut self,
        path: &Path,
        rate: u32,
        search: Vec<PathBuf>,
        bpm: f64,
    ) -> Result<u64, String> {
        self.send(path, rate, Some(search), bpm)
    }

    fn send(
        &mut self,
        path: &Path,
        rate: u32,
        clip_search: Option<Vec<PathBuf>>,
        bpm: f64,
    ) -> Result<u64, String> {
        if self.requests.is_none() {
            let (sender, receiver) = mpsc::channel::<Request>();
            let (results, results_receiver) = mpsc::channel();
            let waker = self.waker.clone();
            std::thread::Builder::new()
                .name("bl-browser-preview".to_owned())
                .spawn(move || {
                    let mut renders = Renders::default();
                    while let Ok(mut request) = receiver.recv() {
                        while let Ok(newer) = receiver.try_recv() {
                            request = newer;
                        }
                        let (result, clip) = match &request.clip_search {
                            Some(search) => match decode_clip(&request, search, &mut renders) {
                                Ok((data, clip)) => (Ok(data), Some(clip)),
                                Err(error) => (Err(error), None),
                            },
                            None => (decode(&request.path, request.rate), None),
                        };
                        let overview = result
                            .as_ref()
                            .map(|data| overview(data, OVERVIEW_POINTS))
                            .unwrap_or_default();
                        let loaded = Loaded {
                            id: request.id,
                            path: request.path,
                            result,
                            overview,
                            clip,
                        };
                        if results.send(loaded).is_err() {
                            return;
                        }
                        waker.wake();
                    }
                })
                .map_err(|e| format!("couldn't start the preview decoder: {e}"))?;
            self.requests = Some(sender);
            self.results = Some(results_receiver);
        }
        self.next += 1;
        self.cancel();
        self.cancel = Arc::default();
        let request = Request {
            id: self.next,
            path: path.to_path_buf(),
            rate,
            clip_search,
            bpm,
            cancel: Arc::clone(&self.cancel),
        };
        self.requests
            .as_ref()
            .ok_or("the preview decoder stopped")?
            .send(request)
            .map_err(|_| "the preview decoder stopped".to_owned())?;
        Ok(self.next)
    }

    /// Previews decoded since the last call.
    pub fn poll(&mut self) -> Vec<Loaded> {
        self.results
            .as_ref()
            .map(|results| results.try_iter().collect())
            .unwrap_or_default()
    }
}

/// The part of `data` from `start` seconds to `end` seconds (the end of the
/// audio when `None`): an unwarped audio clip file's preview plays this once
/// (clip files R13). The audio itself if the part would be empty.
#[must_use]
pub fn trim(data: &SampleData, start: f64, end: Option<f64>) -> SampleData {
    let rate = f64::from(data.sample_rate());
    let frames = data.frames();
    let from = ((start.max(0.0) * rate) as usize).min(frames);
    let to = end.map_or(frames, |end| {
        ((end.max(0.0) * rate) as usize).clamp(from, frames)
    });
    let planes: Vec<Vec<f32>> = data.channels().map(|c| c[from..to].to_vec()).collect();
    SampleData::new(data.sample_rate(), planes).unwrap_or_else(|_| data.clone())
}

/// The longest MIDI clip file previewed, in seconds of its render.
const LONGEST_RENDER_SECONDS: f64 = 60.0;

/// How many MIDI clip files' renders the decoder keeps.
// shortcut: a count, not a size: eight renders of the longest loop are about
// 180 MB; keep by size if previews of long loops become common.
const KEPT_RENDERS: usize = 8;

/// What a clip file looked like when it was rendered: it changed if this did.
type Stamp = (u64, Option<SystemTime>);

type Preview = (SampleData, Box<bl_model::AudioData>);

/// A MIDI clip file's render and what it was made from.
struct Render {
    path: PathBuf,
    stamp: Stamp,
    bpm: f64,
    rate: u32,
    preview: Preview,
}

/// MIDI clip files' renders, kept until the file or the tempo changes (clip
/// files R16), so selecting a file again doesn't render it again.
#[derive(Default)]
struct Renders(Vec<Render>);

impl Renders {
    fn find(&self, path: &Path, stamp: Stamp, bpm: f64, rate: u32) -> Option<Preview> {
        self.0
            .iter()
            .find(|r| r.path == path && r.stamp == stamp && r.bpm == bpm && r.rate == rate)
            .map(|r| r.preview.clone())
    }

    /// Keeps `render`, in place of the file's earlier one; the oldest render
    /// goes when there are [`KEPT_RENDERS`].
    fn keep(&mut self, render: Render) {
        self.0.retain(|r| r.path != render.path);
        if self.0.len() >= KEPT_RENDERS {
            self.0.remove(0);
        }
        self.0.push(render);
    }
}

/// Reads the clip file `request` names and gives what its preview plays: the
/// audio its audio clip plays, decoded at the request's rate, or the render
/// of its MIDI clip (clip files R13, R16). `renders` keeps the last renders.
///
/// # Errors
///
/// A message if the file can't be read, its audio is missing or can't be
/// decoded, or its MIDI clip can't be rendered.
fn decode_clip(
    request: &Request,
    search: &[PathBuf],
    renders: &mut Renders,
) -> Result<Preview, String> {
    let (path, rate, bpm) = (&request.path, request.rate, request.bpm);
    // A Session clip of a set has no file to stamp, so its render isn't kept.
    let stamp = std::fs::metadata(path)
        .ok()
        .map(|m| (m.len(), m.modified().ok()));
    if let Some(preview) = stamp.and_then(|stamp| renders.find(path, stamp, bpm, rate)) {
        return Ok(preview);
    }
    let file = crate::set_exchange::read_clip_file(path)?;
    let folder = path.parent().unwrap_or_else(|| Path::new("."));
    let audio = match &file.clip.data {
        ClipData::Audio(audio) => audio,
        ClipData::Midi(_) => {
            let preview = render_midi(path, file, rate, bpm, search, &request.cancel)?;
            if let Some(stamp) = stamp {
                renders.keep(Render {
                    path: path.clone(),
                    stamp,
                    bpm,
                    rate,
                    preview: preview.clone(),
                });
            }
            return Ok(preview);
        }
    };
    let reference = file.media.get(&audio.media);
    let source = reference
        .and_then(|r| {
            r.path
                .resolve(folder)
                .filter(|p| crate::clip_files::holds(p, r.size, audio.media))
        })
        .or_else(|| {
            let size = reference?.size;
            let mut folders = vec![folder.to_owned()];
            folders.extend(search.iter().cloned());
            bl_media::MediaStore::new()
                .find_by_content(
                    &[(bl_media::ContentHash::from_bytes(audio.media.0), size)],
                    &folders,
                )
                .into_iter()
                .next()
                .map(|(_, found)| found)
        })
        .ok_or_else(|| "the audio this clip plays is missing".to_owned())?;
    Ok((decode(&source, rate)?, Box::new(audio.clone())))
}

/// Renders the MIDI clip in `file`, read from `path`, for a preview (clip
/// files R16): a set of one MIDI track holding the file's device chain and
/// clip, at `bpm`, for one pass of the clip's loop (its length when it
/// doesn't loop), through plug-ins made from their saved state in hosts of
/// this thread's own, so the set's instances are never borrowed (offline
/// plug-in renders R3, R11). Returns the audio and the warp that lays it on
/// the set's beats. Stops early, with an error, when `cancel` is set.
///
/// # Errors
///
/// A message if the chain has no instrument, or the render fails or is
/// cancelled.
pub fn render_midi(
    path: &Path,
    file: ClipFile,
    rate: u32,
    bpm: f64,
    search: &[PathBuf],
    cancel: &AtomicBool,
) -> Result<Preview, String> {
    if !file
        .devices
        .iter()
        .any(|device| device.walk().any(|d| d.role == DeviceRole::Instrument))
    {
        return Err(
            "this MIDI clip file's device chain has no instrument, so there is nothing to hear"
                .to_owned(),
        );
    }
    let store = bl_media::MediaStore::new();
    let mut loaded = crate::clip_files::register_media(&store, path, file, search);
    // One pass of the loop, played from the loop's start.
    let clip = &mut loaded.file.clip;
    let length = match &mut clip.data {
        ClipData::Midi(midi) if midi.looping => {
            midi.start = midi.loop_start;
            midi.loop_end.unwrap_or(clip.length) - midi.loop_start
        }
        _ => clip.length,
    };
    clip.length = length;
    let mut editor = Editor::new();
    let tempo = TempoMap::constant(Bpm::new(bpm).map_err(|e| e.to_string())?);
    editor
        .apply(Command::SetTempo { tempo })
        .map_err(|e| e.to_string())?;
    crate::clip_files::place(
        &mut editor,
        &loaded,
        crate::clip_files::Lane::New {
            parent: None,
            index: 0,
        },
        crate::clip_files::Spot::Arrangement(Ticks::ZERO),
        &BTreeSet::new(),
    )
    .map_err(|e| e.to_string())?;
    // Hosts made on this thread are this thread's main thread: the render
    // makes its plug-ins here and they end with it.
    let (clap, vst3, au) = (ClapHost::new(), Vst3Host::new(), AuHost::new());
    let hosts = Hosts {
        clap: Some(&clap),
        vst3: Some(&vst3),
        au: Some(&au),
    };
    let spec = crate::export::Render {
        start: Ticks::ZERO,
        length: crate::export::Length::To(length),
        sample_rate: rate,
        engine_rate: rate,
        block: crate::export::BLOCK,
    };
    let mut samples = Vec::new();
    let rendered = crate::export::render(
        editor.document(),
        Arc::new(store),
        hosts,
        &spec,
        LONGEST_RENDER_SECONDS,
        cancel,
        &mut |_| {},
        &mut |block| {
            samples.extend_from_slice(block);
            Ok(())
        },
        None,
    )
    .map_err(|e| e.to_string())?;
    let channels = usize::from(rendered.channels).max(1);
    let planes: Vec<Vec<f32>> = (0..channels)
        .map(|c| samples.iter().skip(c).step_by(channels).copied().collect())
        .collect();
    let data = SampleData::new(rate, planes).map_err(|e| e.to_string())?;
    // The render is at the set's tempo already: one marker at its start, and
    // the tempo it was made at, lay it on the set's beats.
    let mut clip = bl_model::AudioData::new(ContentHash([0; 32]));
    clip.warp = true;
    clip.looping = true;
    clip.tempo = Some(bpm);
    clip.warp_markers = vec![WarpMarker {
        time: 0.0,
        beat: Ticks::ZERO,
    }];
    Ok((data, Box::new(clip)))
}

/// Decodes up to [`MAX_PREVIEW_SECONDS`] of a file and converts it to
/// `rate`.
///
/// # Errors
///
/// A message if the file can't be read or decoded.
pub fn decode(path: &Path, rate: u32) -> Result<SampleData, String> {
    let mut decoder = Decoder::open(path).map_err(|e| e.to_string())?;
    let info = *decoder.info();
    let channels = usize::from(info.channels);
    if channels == 0 {
        return Err("the file has no audio channels".to_owned());
    }
    let limit =
        usize::try_from(u64::from(info.sample_rate) * MAX_PREVIEW_SECONDS).unwrap_or(usize::MAX);
    let expected = info
        .frames
        .map_or(0, |frames| usize::try_from(frames).unwrap_or(limit))
        .min(limit);
    let mut planes: Vec<Vec<f32>> = (0..channels)
        .map(|_| Vec::with_capacity(expected))
        .collect();
    let mut block = vec![vec![0.0f32; 1 << 14]; channels];
    while planes.first().map_or(0, Vec::len) < limit {
        let mut outs: Vec<&mut [f32]> = block.iter_mut().map(|c| &mut c[..]).collect();
        let read = decoder.read(&mut outs).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        for (plane, source) in planes.iter_mut().zip(&block) {
            let room = limit - plane.len();
            plane.extend_from_slice(&source[..read.min(room)]);
        }
    }
    let data = SampleData::new(info.sample_rate, planes).map_err(|e| e.to_string())?;
    if data.sample_rate() == rate {
        Ok(data)
    } else {
        convert_sample_rate(&data, rate).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(path: &str, stamp: u64, bpm: f64) -> Render {
        Render {
            path: PathBuf::from(path),
            stamp: (stamp, None),
            bpm,
            rate: 48_000,
            preview: (
                SampleData::new(48_000, vec![vec![0.0; 4]]).unwrap(),
                Box::new(bl_model::AudioData::new(ContentHash([0; 32]))),
            ),
        }
    }

    // Spec: clip files R16. A render is kept until its file or the tempo
    // changes, and only the last few are kept.
    #[test]
    fn a_render_is_kept_until_the_file_or_the_tempo_changes() {
        let mut renders = Renders::default();
        renders.keep(render("a.blclip", 1, 120.0));
        let found = |renders: &Renders, stamp, bpm| {
            renders
                .find(Path::new("a.blclip"), (stamp, None), bpm, 48_000)
                .is_some()
        };
        assert!(
            found(&renders, 1, 120.0),
            "selected again, not rendered again"
        );
        assert!(!found(&renders, 2, 120.0), "the file changed");
        assert!(!found(&renders, 1, 90.0), "the tempo changed");
        assert!(
            renders
                .find(Path::new("a.blclip"), (1, None), 120.0, 44_100)
                .is_none(),
            "the device's rate changed"
        );

        renders.keep(render("a.blclip", 2, 120.0));
        assert!(found(&renders, 2, 120.0) && !found(&renders, 1, 120.0));
        for n in 0..KEPT_RENDERS {
            renders.keep(render(&format!("{n}.blclip"), 1, 120.0));
        }
        assert!(!found(&renders, 2, 120.0), "the oldest went");
        assert_eq!(renders.0.len(), KEPT_RENDERS);
    }

    // Spec: clip files R16. Selecting another file, or stopping, cancels the
    // render under way.
    #[test]
    fn a_newer_request_or_a_stop_cancels_the_render_before_it() {
        let mut loader = Loader::new(Waker::default());
        let file = Path::new("missing.blclip");
        loader
            .request_clip(file, 48_000, Vec::new(), 120.0)
            .unwrap();
        let first = Arc::clone(&loader.cancel);
        assert!(!first.load(Ordering::Relaxed));
        loader.request(file, 48_000).unwrap();
        let second = Arc::clone(&loader.cancel);
        assert!(first.load(Ordering::Relaxed), "a newer request cancels it");
        assert!(!second.load(Ordering::Relaxed));
        loader.cancel();
        assert!(second.load(Ordering::Relaxed), "so does a stop");
    }
}
