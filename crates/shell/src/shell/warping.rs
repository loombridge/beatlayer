//! Transients through the shell (WP-4.3, the warping spec R20 to R27): the
//! analysis cache a set's files are analysed through, `clip.transients`, and
//! the checks and fill-ins the document can't make, since it holds no media:
//! a transient edit names transients that exist, the transients a ranged
//! warp-marker command looks among, and what a replaced file keeps.

use std::sync::Arc;

use bl_analysis::cache::{Analysis, AnalysisCache, AnalysisOptions, AnalysisStatus};
use bl_core::time::Ticks;
use bl_media::{ContentHash, MediaError};
use bl_model::{AudioData, ClipData, ClipId, Command, TRANSIENT_MATCH};
use bl_shell_spec::warping::{ClipTransient, ClipTransients, ClipTransientsArgs};
use serde_json::{Value, json};

use super::*;

/// What the analysis of a clip's file has given so far.
struct Shown {
    /// The clip's transients, edits applied.
    transients: Vec<f64>,
    ready: bool,
    error: Option<String>,
}

impl Shell {
    /// How the files' analyses are kept: in memory only until the app gives
    /// the platform's cache folder, so a headless shell writes no files
    /// (R21). The analyses of the set's files start again under the new
    /// options.
    pub fn use_analysis_options(&mut self, options: AnalysisOptions) {
        self.analysis_options = options;
        self.analysis = None;
        self.analysed.clear();
        self.start_analyses();
    }

    /// Keeps the files' analyses in the platform's cache folder, beside the
    /// peaks (R21), as the app does.
    pub fn use_platform_analysis_cache(&mut self) {
        self.use_analysis_options(AnalysisOptions::platform_default());
    }

    /// The file's analysis, waiting until it is ready. For tools and tests;
    /// the app never waits.
    ///
    /// # Errors
    ///
    /// The analysis's error, or that the media store doesn't know the file.
    pub fn wait_analysis(
        &mut self,
        media: bl_model::ContentHash,
    ) -> Result<Arc<Analysis>, Arc<MediaError>> {
        let hash = ContentHash::from_bytes(media.0);
        match self.analysis_cache() {
            Some(cache) => cache.wait(hash),
            None => Err(Arc::new(MediaError::UnknownMedia(hash))),
        }
    }

    fn analysis_cache(&mut self) -> Option<&AnalysisCache> {
        if self.analysis.is_none() {
            self.analysis =
                AnalysisCache::new(Arc::clone(&self.media), self.analysis_options.clone()).ok();
        }
        self.analysis.as_ref()
    }

    /// Starts the analysis of each audio file the set uses that hasn't been
    /// started, in the background (R20). A file the media store doesn't know
    /// yet waits for a later change.
    pub(super) fn start_analyses(&mut self) {
        let doc = self.editor.document();
        let fresh: Vec<ContentHash> = doc
            .tracks()
            .flat_map(|track| track.clips().chain(track.slots().map(|(_, clip)| clip)))
            .filter_map(|clip| match &clip.data {
                ClipData::Audio(audio) => Some(ContentHash::from_bytes(audio.media.0)),
                ClipData::Midi(_) => None,
            })
            .filter(|hash| !self.analysed.contains(hash) && self.media.contains(*hash))
            .collect();
        for hash in fresh {
            if let Some(cache) = self.analysis_cache() {
                cache.request(hash);
            }
            self.analysed.insert(hash);
        }
    }

    /// The audio clip, its file's analysis so far (started if need be), and
    /// its transients with its edits applied.
    fn shown_transients(
        &mut self,
        id: &str,
        clip: ClipId,
    ) -> Result<(AudioData, Shown), ShellError> {
        let failed = |message: &str| ShellError::Failed {
            command: id.to_owned(),
            message: message.to_owned(),
        };
        let Some(ClipData::Audio(audio)) = self.editor.document().clip(clip).map(|c| &c.data)
        else {
            return Err(failed("the clip isn't an audio clip"));
        };
        let audio = audio.clone();
        let hash = ContentHash::from_bytes(audio.media.0);
        let status = if self.media.contains(hash) {
            self.analysis_cache().map(|cache| cache.request(hash))
        } else {
            None
        };
        let (detected, ready, error) = match status {
            Some(AnalysisStatus::Ready(analysis)) => (analysis.transients.clone(), true, None),
            Some(AnalysisStatus::Pending) => (Vec::new(), false, None),
            Some(AnalysisStatus::Failed(error)) => (Vec::new(), false, Some(error.to_string())),
            None => (
                Vec::new(),
                false,
                Some("the media store doesn't have the clip's file".to_owned()),
            ),
        };
        let transients = audio.transient_edits.apply(&detected);
        Ok((
            audio,
            Shown {
                transients,
                ready,
                error,
            },
        ))
    }

    /// Runs `clip.transients` (R24).
    pub(super) fn run_warping(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let ClipTransientsArgs { clip } = Self::args(id, args)?;
        let (audio, shown) = self.shown_transients(id, clip)?;
        let map = audio.warp.then(|| audio.warp_map().ok()).flatten();
        let transients = shown
            .transients
            .iter()
            .map(|&time| ClipTransient {
                time,
                beat: map
                    .as_ref()
                    .map(|map| Ticks(map.time_to_beat(time).round() as i64)),
                by_hand: audio
                    .transient_edits
                    .added
                    .iter()
                    .any(|&added| (added - time).abs() <= TRANSIENT_MATCH),
            })
            .collect();
        Ok(to_value(&ClipTransients {
            clip,
            analysis_ready: shown.ready,
            analysis_error: shown.error,
            transients,
        }))
    }

    /// The length of the clip's file in seconds, where the store knows it.
    pub(super) fn file_seconds(&self, audio: &AudioData) -> Option<f64> {
        let info = self
            .media
            .info(ContentHash::from_bytes(audio.media.0))
            .ok()?;
        Some(info.frames? as f64 / f64::from(info.sample_rate))
    }

    /// Checks a document command that names transients or replaces a file
    /// against the media, and fills in what only the shell knows, before the
    /// command runs (R23, R25 to R27).
    pub(super) fn prepare_warping(
        &mut self,
        id: &str,
        mut args: Value,
    ) -> Result<Value, ShellError> {
        if !matches!(
            id,
            "clip.insert_warp_marker"
                | "clip.insert_warp_markers"
                | "clip.insert_transient"
                | "clip.delete_transients"
                | "clip.move_transient"
                | "clip.set_data"
        ) {
            return Ok(args);
        }
        let Ok(command) = model::command_from_args(id, args.clone()) else {
            return Ok(args);
        };
        match command {
            Command::InsertWarpMarker {
                clips,
                pin_neighbours: true,
                transients,
                ..
            } => {
                // The model refuses several clips, whose transients differ.
                if let [clip] = clips[..] {
                    self.fill_transients(id, clip, transients, &mut args)?;
                }
            }
            Command::InsertWarpMarkers {
                clip,
                to,
                transients,
                ..
            } => {
                self.fill_transients(id, clip, transients, &mut args)?;
                // A range with no transients gets markers at its ends, so its
                // end must lie in the file.
                let end = match self.editor.document().clip(clip).map(|c| &c.data) {
                    Some(ClipData::Audio(audio)) => audio
                        .warp_map()
                        .ok()
                        .map(|map| map.beat_to_time(to.get() as f64)),
                    _ => None,
                };
                if let Some(end) = end {
                    self.check_in_file(id, clip, end)?;
                }
            }
            Command::InsertTransient { clip, time } => {
                self.check_in_file(id, clip, time)?;
                self.check_no_transient_at(id, clip, time, None)?;
            }
            Command::MoveTransient { clip, from, to } => {
                self.check_in_file(id, clip, to)?;
                self.check_transients_exist(id, clip, &[from])?;
                self.check_no_transient_at(id, clip, to, Some(from))?;
            }
            Command::DeleteTransients { clip, times } => {
                self.check_transients_exist(id, clip, &times)?;
            }
            Command::SetClipData {
                clip,
                data: ClipData::Audio(mut next),
            } => {
                // R27: a replaced file keeps the warp markers only if the new
                // file is exactly as long; otherwise the clip takes the
                // settings a new clip of the file gets, here unwarped.
                let Some(ClipData::Audio(now)) = self.editor.document().clip(clip).map(|c| &c.data)
                else {
                    return Ok(args);
                };
                if next.media == now.media {
                    return Ok(args);
                }
                let same_length = |a: ContentHash, b: ContentHash| {
                    let (a, b) = (self.media.info(a).ok()?, self.media.info(b).ok()?);
                    Some(
                        a.frames? * u64::from(b.sample_rate)
                            == b.frames? * u64::from(a.sample_rate),
                    )
                };
                let kept = same_length(
                    ContentHash::from_bytes(now.media.0),
                    ContentHash::from_bytes(next.media.0),
                ) == Some(true);
                if !kept && (next.warp || !next.warp_markers.is_empty()) {
                    next = AudioData {
                        warp: false,
                        warp_markers: Vec::new(),
                        tempo: None,
                        signature: None,
                        ..next.in_seconds().into_owned()
                    };
                    args["data"] = to_value(&ClipData::Audio(next));
                }
            }
            _ => {}
        }
        Ok(args)
    }

    /// The clip's transients, once its file's analysis is ready.
    fn ready_transients(&mut self, id: &str, clip: ClipId) -> Result<Vec<f64>, ShellError> {
        let (_, shown) = self.shown_transients(id, clip)?;
        if shown.ready {
            return Ok(shown.transients);
        }
        Err(ShellError::Failed {
            command: id.to_owned(),
            message: shown.error.unwrap_or_else(|| {
                "the file's analysis isn't ready yet: try again in a moment, or give the \
                 `transients` to use"
                    .to_owned()
            }),
        })
    }

    /// Fills in the clip's transients for a warp-marker command that left
    /// them out, or checks that those given lie in the file.
    fn fill_transients(
        &mut self,
        id: &str,
        clip: ClipId,
        transients: Option<Vec<f64>>,
        args: &mut Value,
    ) -> Result<(), ShellError> {
        match transients {
            None => args["transients"] = json!(self.ready_transients(id, clip)?),
            Some(given) => {
                for time in given {
                    self.check_in_file(id, clip, time)?;
                }
            }
        }
        Ok(())
    }

    /// Refuses a time where the clip already shows a transient, other than
    /// the one at `except`: the two would show as one, and deleting it would
    /// leave the detected one.
    fn check_no_transient_at(
        &mut self,
        id: &str,
        clip: ClipId,
        time: f64,
        except: Option<f64>,
    ) -> Result<(), ShellError> {
        let (_, shown) = self.shown_transients(id, clip)?;
        let near = |a: f64, b: f64| (a - b).abs() <= TRANSIENT_MATCH;
        if let Some(at) = shown
            .transients
            .iter()
            .find(|&&t| near(t, time) && !except.is_some_and(|e| near(t, e)))
        {
            return Err(ShellError::Failed {
                command: id.to_owned(),
                message: format!("the clip already has a transient at {at} s"),
            });
        }
        Ok(())
    }

    /// Refuses a time past the end of the clip's file, where the store knows
    /// its length.
    fn check_in_file(&self, id: &str, clip: ClipId, time: f64) -> Result<(), ShellError> {
        let Some(ClipData::Audio(audio)) = self.editor.document().clip(clip).map(|c| &c.data)
        else {
            return Ok(());
        };
        match self.file_seconds(audio) {
            Some(length) if time > length + 1e-9 => Err(ShellError::Failed {
                command: id.to_owned(),
                message: format!("{time} s is past the end of the file, which is {length} s long"),
            }),
            _ => Ok(()),
        }
    }

    /// Refuses a time that is none of the clip's transients. Until the
    /// file's analysis is ready, only transients put in by hand exist.
    fn check_transients_exist(
        &mut self,
        id: &str,
        clip: ClipId,
        times: &[f64],
    ) -> Result<(), ShellError> {
        let (_, shown) = self.shown_transients(id, clip)?;
        for &time in times {
            if !shown
                .transients
                .iter()
                .any(|&t| (t - time).abs() <= TRANSIENT_MATCH)
            {
                let wait = if shown.ready {
                    ""
                } else {
                    " (the file's analysis isn't ready: only transients put in by hand exist yet)"
                };
                return Err(ShellError::Failed {
                    command: id.to_owned(),
                    message: format!(
                        "the clip has no transient at {time} s; `clip.transients` lists them{wait}"
                    ),
                });
            }
        }
        Ok(())
    }
}
