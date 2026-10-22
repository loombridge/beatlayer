//! Decoding previews in the background (spec: browser R11, R12).
//!
//! A preview is decoded and converted to the device's sample rate on a
//! thread of its own, so selecting file after file never stalls the window.
//! Only the newest request is decoded; older ones still waiting are skipped.
//! The result goes to the audio thread's preview voice
//! (`bl_engine::preview`) from the main thread.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use bl_engine::preview::PreviewSamples;
use bl_media::{Decoder, SampleData, convert_sample_rate};

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
}

/// The preview decoder: a thread that decodes the newest request.
pub struct Loader {
    requests: Option<Sender<Request>>,
    results: Option<Receiver<Loaded>>,
    waker: Waker,
    next: u64,
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
        }
    }

    /// Asks for `path` decoded at `rate`; returns the request's ID.
    ///
    /// # Errors
    ///
    /// If the decoding thread can't start.
    pub fn request(&mut self, path: &Path, rate: u32) -> Result<u64, String> {
        self.send(path, rate, None)
    }

    /// Asks for the audio clip in the clip file `path` and the audio it
    /// plays, decoded at `rate`, searching `search` for the audio if it moved
    /// (clip files R13, R14); returns the request's ID.
    ///
    /// # Errors
    ///
    /// If the decoding thread can't start.
    pub fn request_clip(
        &mut self,
        path: &Path,
        rate: u32,
        search: Vec<PathBuf>,
    ) -> Result<u64, String> {
        self.send(path, rate, Some(search))
    }

    fn send(
        &mut self,
        path: &Path,
        rate: u32,
        clip_search: Option<Vec<PathBuf>>,
    ) -> Result<u64, String> {
        if self.requests.is_none() {
            let (sender, receiver) = mpsc::channel::<Request>();
            let (results, results_receiver) = mpsc::channel();
            let waker = self.waker.clone();
            std::thread::Builder::new()
                .name("bl-browser-preview".to_owned())
                .spawn(move || {
                    while let Ok(mut request) = receiver.recv() {
                        while let Ok(newer) = receiver.try_recv() {
                            request = newer;
                        }
                        let (result, clip) = match &request.clip_search {
                            Some(search) => {
                                match decode_clip(&request.path, request.rate, search) {
                                    Ok((data, clip)) => (Ok(data), Some(clip)),
                                    Err(error) => (Err(error), None),
                                }
                            }
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
        let request = Request {
            id: self.next,
            path: path.to_path_buf(),
            rate,
            clip_search,
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

/// Reads the clip file at `path` and decodes the audio its audio clip plays at
/// `rate` (clip files R13).
///
/// # Errors
///
/// A message if the file can't be read, holds a MIDI clip, or its audio is
/// missing or can't be decoded.
pub fn decode_clip(
    path: &Path,
    rate: u32,
    search: &[PathBuf],
) -> Result<(SampleData, Box<bl_model::AudioData>), String> {
    let file = bl_project::clip_file::ClipFile::load(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let bl_model::ClipData::Audio(audio) = file.clip.data else {
        return Err(
            "this clip file holds a MIDI clip, and MIDI clips don't preview yet".to_owned(),
        );
    };
    let folder = path.parent().unwrap_or_else(|| Path::new("."));
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
    Ok((decode(&source, rate)?, Box::new(audio)))
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
