//! Transients through the shell (WP-4.3, the warping spec R20 to R27): the
//! analysis cache a set's files are analysed through, `clip.transients`, and
//! the checks and fill-ins the document can't make, since it holds no media:
//! a transient edit names transients that exist, the transients a ranged
//! warp-marker command looks among, and what a replaced file keeps.

use std::sync::Arc;

use bl_analysis::cache::{Analysis, AnalysisCache, AnalysisOptions, AnalysisStatus};
use bl_core::time::Ticks;
use bl_media::{ContentHash, MediaError};
use bl_model::{AudioData, ClipData, ClipId, Command, FileBeats, TRANSIENT_MATCH, WarpMarker};
use bl_shell_spec::warping::{
    self as spec, ClipDefaultsArgs, ClipTransient, ClipTransients, ClipTransientsArgs,
    QuantizeAudioArgs, SavedDefaults,
};
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

    pub(super) fn analysis_cache(&mut self) -> Option<&AnalysisCache> {
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

    /// Runs `clip.save_default` and `clip.defaults` (R42).
    fn run_defaults(&self, id: &str, args: Value) -> Result<Value, ShellError> {
        let ClipDefaultsArgs { clip } = Self::args(id, args)?;
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        let Some(ClipData::Audio(audio)) = self.editor.document().clip(clip).map(|c| &c.data)
        else {
            return Err(failed("the clip isn't an audio clip".to_owned()));
        };
        let Some(entry) = self.media.entry(ContentHash::from_bytes(audio.media.0)) else {
            return Err(failed(
                "the media store doesn't have the clip's file".to_owned(),
            ));
        };
        let file = if id == spec::SAVE_DEFAULT {
            crate::clip_defaults::save(&entry.path, audio.media, audio.defaults())
                .map_err(failed)?
        } else {
            crate::clip_defaults::path_for(&entry.path)
        };
        Ok(to_value(&SavedDefaults {
            clip,
            file: file.display().to_string(),
            defaults: crate::clip_defaults::load(&entry.path, audio.media),
        }))
    }

    /// Runs `clip.transients` (R24), `clip.quantize_audio` (R49), `clip.save_default` or `clip.defaults`.
    pub(super) fn run_warping(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        if id == spec::QUANTIZE_AUDIO {
            return self.run_quantize_audio(id, args);
        }
        if id != spec::TRANSIENTS {
            return self.run_defaults(id, args);
        }
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

    /// Runs `clip.quantize_audio` (WP-4.12, R49).
    pub(super) fn run_quantize_audio(
        &mut self,
        id: &str,
        args: Value,
    ) -> Result<Value, ShellError> {
        let QuantizeAudioArgs {
            clip,
            from,
            to,
            grid,
            amount,
            transients,
        } = Self::args(id, args)?;
        let (clip, audio, clip_start, clip_length) = {
            let doc = self.editor.document();
            let clip = match clip {
                Some(clip) => clip,
                None => self
                    .clip_view
                    .shown_audio_clip(doc, &self.arrangement)
                    .ok_or_else(|| ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "no audio clip is shown in the Clip View".to_owned(),
                    })?,
            };
            let Some(clip_entry) = doc.clip(clip) else {
                return Err(ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: format!("clip {clip} does not exist"),
                });
            };
            let ClipData::Audio(ref audio) = clip_entry.data else {
                return Err(ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: format!("clip {clip} is not an audio clip"),
                });
            };
            if !audio.warp {
                return Err(ShellError::Unavailable {
                    command: id.to_owned(),
                    reason: "the clip isn't warped: switch Warp on first".to_owned(),
                });
            }
            (clip, audio.clone(), clip_entry.start, clip_entry.length)
        };
        let settings = self.clip_view.quantize();
        let grid = grid
            .or(settings.grid)
            .unwrap_or_else(|| self.arrangement.grid_ticks(self.editor.document()));
        if grid <= Ticks::ZERO {
            return Err(ShellError::InvalidArgs {
                command: id.to_owned(),
                message: "`grid` must be positive".to_owned(),
            });
        }
        let amount = amount.unwrap_or(settings.amount);
        if amount > 100 {
            return Err(ShellError::InvalidArgs {
                command: id.to_owned(),
                message: "`amount` must be from 0 to 100 percent".to_owned(),
            });
        }
        if amount == 0 {
            // Spec: warping R49. 0 percent changes nothing and is no undo step.
            return Ok(Value::Null);
        }
        let transients = match transients {
            Some(ts) => ts,
            None => self.ready_transients(id, clip)?,
        };
        let map = audio.warp_map().map_err(|err| ShellError::InvalidArgs {
            command: id.to_owned(),
            message: err.to_string(),
        })?;
        let from_beat = from.unwrap_or_else(|| Ticks(audio.start.round() as i64));
        let to_beat = to.unwrap_or_else(|| {
            if let Some(end) = audio.end {
                Ticks(end.round() as i64)
            } else if let Some(file_sec) = self.file_seconds(&audio) {
                Ticks(map.time_to_beat(file_sec).round() as i64)
            } else {
                from_beat.saturating_add(clip_length)
            }
        });
        let (from_beat, to_beat) = if from_beat <= to_beat {
            (from_beat, to_beat)
        } else {
            (to_beat, from_beat)
        };

        let doc = self.editor.document();
        let signatures = &doc.settings().time_signatures;
        let step = crate::clip_view::quantize_step(grid, signatures.signature_at(clip_start));
        let mut grid_lines = Vec::new();
        crate::arrangement::grid::lines(
            signatures,
            step,
            from_beat,
            to_beat,
            usize::MAX,
            |line, _| {
                grid_lines.push(line);
            },
        );

        // For each grid line, find the nearest transient within half a grid step.
        let half_step = step.ticks.get() as f64 / 2.0;
        let mut line_matches: Vec<(Ticks, f64, f64)> = Vec::new(); // (line, transient_time, dist)
        for &line in &grid_lines {
            let line_f64 = line.get() as f64;
            let mut nearest: Option<(f64, f64)> = None;
            for &t in &transients {
                let beat_f64 = map.time_to_beat(t);
                let dist = (beat_f64 - line_f64).abs();
                if dist <= half_step + 1e-6 {
                    match nearest {
                        Some((_, best_dist)) if dist < best_dist - 1e-9 => {
                            nearest = Some((t, dist));
                        }
                        None => {
                            nearest = Some((t, dist));
                        }
                        _ => {}
                    }
                }
            }
            if let Some((t, dist)) = nearest {
                line_matches.push((line, t, dist));
            }
        }

        // Each transient matches at most one line.
        let mut chosen_matches: Vec<(Ticks, f64)> = Vec::new(); // (line, transient_time)
        for (line, t, dist) in line_matches {
            if let Some(pos) = chosen_matches
                .iter()
                .position(|(_, existing_t)| (existing_t - t).abs() <= TRANSIENT_MATCH)
            {
                let (prev_line, _) = chosen_matches[pos];
                let prev_dist = (map.time_to_beat(t) - prev_line.get() as f64).abs();
                if dist < prev_dist - 1e-9 {
                    chosen_matches[pos] = (line, t);
                }
            } else {
                chosen_matches.push((line, t));
            }
        }

        let mut new_markers: Vec<WarpMarker> = audio.warp_markers.clone();
        for (line, t) in chosen_matches {
            let mut next = new_markers.clone();
            if let Some(m) = next
                .iter_mut()
                .find(|m| (m.time - t).abs() <= TRANSIENT_MATCH)
            {
                m.beat = crate::clip_view::toward(m.beat, line, amount);
            } else {
                let beat = Ticks(map.time_to_beat(t).round() as i64);
                next.push(WarpMarker {
                    time: t,
                    beat: crate::clip_view::toward(beat, line, amount),
                });
                next.sort_by(|a, b| a.time.total_cmp(&b.time));
            }
            // A marker that would reach or pass a neighbour, such as a late
            // first hit landing on the marker at 1.1.1, stays where it is.
            if next
                .windows(2)
                .all(|pair| pair[0].time < pair[1].time && pair[0].beat < pair[1].beat)
            {
                new_markers = next;
            }
        }

        if new_markers == audio.warp_markers {
            return Ok(Value::Null);
        }

        let tempo = audio.tempo.unwrap_or(120.0);
        let updated_audio =
            audio
                .with_map(new_markers, tempo)
                .map_err(|err| ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: err.to_string(),
                })?;

        crate::arrangement::editing::transaction(&mut self.editor, "Quantize", |editor| {
            editor.apply(Command::SetClipData {
                clip,
                data: ClipData::Audio(updated_audio),
            })?;
            Ok(())
        })
        .map_err(|err| match err {
            crate::arrangement::editing::EditFailure::Refused(reason)
            | crate::arrangement::editing::EditFailure::Unavailable(reason) => {
                ShellError::Unavailable {
                    command: id.to_owned(),
                    reason,
                }
            }
            crate::arrangement::editing::EditFailure::InvalidArgs(message) => {
                ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message,
                }
            }
        })?;

        Ok(Value::Null)
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
                | "clip.warp_from_here"
                | "clip.warp_as_loop"
                | "clip.warp_selection_as_loop"
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
            // R41: the warp commands need the file's analysis, and give it
            // to the document, which holds no media.
            Command::WarpFromHere {
                clip,
                analysis: None,
                ..
            }
            | Command::WarpAsLoop {
                clip,
                analysis: None,
            } => args["analysis"] = to_value(&self.file_beats(id, clip)?),
            Command::WarpSelectionAsLoop { clip, to, .. } => {
                self.file_beats(id, clip)?;
                self.check_in_file(id, clip, to)?;
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

    /// The clip's file's tempo and beats, once its analysis is ready.
    fn file_beats(&mut self, id: &str, clip: ClipId) -> Result<FileBeats, ShellError> {
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        let Some(ClipData::Audio(audio)) = self.editor.document().clip(clip).map(|c| &c.data)
        else {
            return Err(failed("the clip isn't an audio clip".to_owned()));
        };
        let audio = audio.clone();
        let hash = ContentHash::from_bytes(audio.media.0);
        let status = if self.media.contains(hash) {
            self.analysis_cache().map(|cache| cache.request(hash))
        } else {
            None
        };
        match (status, self.file_seconds(&audio)) {
            (Some(AnalysisStatus::Ready(analysis)), Some(seconds)) => Ok(FileBeats {
                seconds,
                bpm: analysis.tempo.map(|tempo| tempo.bpm),
                beats: analysis.beats.clone(),
            }),
            (Some(AnalysisStatus::Failed(error)), _) => Err(failed(error.to_string())),
            _ => Err(failed(
                "the file's analysis isn't ready yet: try again in a moment".to_owned(),
            )),
        }
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
