//! Grooves through the shell (WP-6.19; the grooves spec R5, R16 to R19 and
//! R23): loading and saving groove files, making a groove of a clip, writing
//! a clip's groove into it, making a clip of a groove, and the pool query.
//!
//! The laws are `bl-model`'s ([`GroovePlay`] and [`AudioPass`]), the same
//! arithmetic the engine plays through, so what a Commit writes is what was
//! heard. What the shell adds is what the document doesn't hold: the User
//! Library, the files, a clip's transients and the audio under them.

use std::collections::BTreeSet;
use std::path::PathBuf;

use bl_core::time::Ticks;
use bl_media::{ContentHash as MediaHash, decode_file};
use bl_model::{
    AudioData, AudioPass, AutomationTarget, Clip, ClipData, ClipId, Command, Groove, GrooveId,
    GrooveNote, GroovePlay, MidiData, MixerParam, MovedTransients, Note, WarpMarker, fader,
};
use bl_project::groove_file::{GROOVE_EXTENSION, GrooveFile, GrooveFileError};
use bl_shell_spec::groove::{
    self as ids, COMMIT_STEP, CommitGrooveArgs, EXTRACT_STEP, ExtractGrooveArgs, GrooveClip,
    GrooveCommitted, GrooveExtracted, GrooveHotSwap, GrooveLoaded, GroovePoolEntry,
    GroovePoolState, GroovePoolView, GrooveSaved, GrooveToClipArgs, HOT_SWAP_STEP,
    HotSwapGrooveArgs, LOAD_STEP, LoadGrooveArgs, SaveGrooveArgs, TO_CLIP_STEP,
    ToggleGroovePoolArgs,
};
use serde_json::Value;

use super::*;

/// The velocity of an extracted audio groove note at the loudest peak, and
/// how far below it, in dB, a note has velocity 1 (the grooves spec, R17).
const LOUDEST: f64 = 127.0;
const RANGE_DB: f64 = 48.0;
/// How long after a transient the audio's peak level is read (R17).
const PEAK_SECONDS: f64 = 0.020;

/// A "not now" error naming the command.
fn unavailable(id: &str, reason: impl ToString) -> ShellError {
    ShellError::Unavailable {
        command: id.to_owned(),
        reason: reason.to_string(),
    }
}

/// A failure naming the command.
fn failed(id: &str, message: impl ToString) -> ShellError {
    ShellError::Failed {
        command: id.to_owned(),
        message: message.to_string(),
    }
}

fn file_error(id: &str, error: &GrooveFileError) -> ShellError {
    match error {
        GrooveFileError::Io { .. } => failed(id, error),
        _ => ShellError::InvalidArgs {
            command: id.to_owned(),
            message: error.to_string(),
        },
    }
}

impl Shell {
    /// Runs one of the shell's groove commands.
    pub(super) fn run_grooves(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::LOAD => self.load_groove(id, args),
            ids::SAVE => self.save_groove(id, args),
            ids::EXTRACT => self.extract_grooves(id, args),
            ids::COMMIT => self.commit_grooves(id, args),
            ids::TO_CLIP => self.groove_to_clip(id, args),
            ids::POOL => {
                Self::args::<NoArgs>(id, args)?;
                Ok(to_value(&self.groove_pool()))
            }
            ids::VIEW_TOGGLE_GROOVE_POOL => {
                // Spec: grooves R24. View state: no undo step.
                let ToggleGroovePoolArgs { shown } = Self::args(id, args)?;
                self.groove_pool.shown = shown.unwrap_or(!self.groove_pool.shown);
                if self.groove_pool.shown {
                    self.show_browser_in(self.acting_window());
                }
                self.touch();
                Ok(to_value(&self.groove_pool_view()))
            }
            ids::FOCUS_GROOVE_POOL => {
                // Spec: grooves R24. The pool shows, and takes the keyboard.
                Self::args::<NoArgs>(id, args)?;
                self.groove_pool.shown = true;
                self.run(
                    crate::browser_commands::FOCUS_BROWSER,
                    json!({ "part": "groove_pool" }),
                )?;
                Ok(to_value(&self.groove_pool_view()))
            }
            ids::HOT_SWAP => self.hot_swap_groove(id, args),
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// Whether the Control Bar shows the Global Amount field: while the pool
    /// holds a groove (R30, L-CTRL-008).
    #[must_use]
    pub fn global_amount_shown(&self) -> bool {
        !self.editor.document().settings().grooves.pool.is_empty()
    }

    /// The Groove Pool's view: whether it shows and what the browser's
    /// hot-swap replaces (R24, R32). A hot-swap aimed at a groove that has
    /// left the pool is over.
    #[must_use]
    pub fn groove_pool_view(&self) -> GroovePoolView {
        let pool = &self.editor.document().settings().grooves;
        let hot_swap = match self.groove_pool.hot_swap {
            Some(GrooveHotSwap::Groove(groove)) if pool.get(groove).is_none() => None,
            other => other,
        };
        GroovePoolView {
            shown: self.groove_pool.shown,
            hot_swap,
        }
    }

    /// `groove.hot_swap` (R32): makes a groove, or the auto-load groove, the
    /// browser's target, and opens its Grooves label; or ends it.
    fn hot_swap_groove(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let HotSwapGrooveArgs {
            groove,
            auto_load,
            on,
        } = Self::args(id, args)?;
        let target = match (groove, auto_load) {
            (Some(_), true) => {
                return Err(ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: "name `groove` or `auto_load`, not both".to_owned(),
                });
            }
            (Some(groove), false) => {
                self.pool_groove(id, groove)?;
                Some(GrooveHotSwap::Groove(groove))
            }
            (None, true) => Some(GrooveHotSwap::AutoLoad),
            (None, false) => None,
        };
        let armed = self.groove_pool_view().hot_swap;
        match target {
            Some(target) if on.unwrap_or(armed != Some(target)) => {
                self.groove_pool.hot_swap = Some(target);
                // The Grooves label opens, with the search cleared as a new
                // step, as a device's hot-swap does.
                self.browser_view.hot_swap_started();
                self.show_browser_in(self.acting_window());
                self.run(
                    crate::browser_commands::OPEN_PLACE,
                    json!({ "label": "grooves" }),
                )?;
            }
            None if on == Some(true) => {
                return Err(ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: "name `groove` or `auto_load` to hot-swap".to_owned(),
                });
            }
            _ => self.groove_pool.hot_swap = None,
        }
        self.touch();
        Ok(to_value(&self.groove_pool_view()))
    }

    /// The set's grooves as `groove.pool` reports them (R23).
    #[must_use]
    pub fn groove_pool(&self) -> GroovePoolState {
        let doc = self.editor.document();
        let pool = &doc.settings().grooves;
        GroovePoolState {
            view: self.groove_pool_view(),
            grooves: pool
                .pool
                .iter()
                .map(|groove| {
                    let clips = doc.clips_using_groove(groove.id);
                    GroovePoolEntry {
                        groove: groove.clone(),
                        active: !clips.is_empty(),
                        clips,
                    }
                })
                .collect(),
            global_amount: pool.global_amount,
            auto_load: pool.auto_load,
            auto_load_groove: pool.auto_load_groove,
        }
    }

    /// Spec: grooves R3. `clip.set_groove`, `groove.extract` and
    /// `groove.commit` name the clips they act on; left out, they act on the
    /// selected ones.
    pub(super) fn prepare_groove_clips(
        &self,
        id: &str,
        mut args: Value,
    ) -> Result<Value, ShellError> {
        if id != ids::SET_CLIP_GROOVE {
            return Ok(args);
        }
        let Some(object) = args.as_object_mut() else {
            return Ok(args);
        };
        if object.contains_key("clips") {
            return Ok(args);
        }
        let clips = self.scale_targets();
        if clips.is_empty() {
            return Err(unavailable(id, "no clip is selected"));
        }
        object.insert("clips".to_owned(), serde_json::json!(clips));
        Ok(args)
    }

    /// The clips a groove command acts on (R16, R17): the ones it names; or,
    /// for Commit given a groove, the ones that use it; or the selected ones.
    fn groove_clips(
        &self,
        id: &str,
        named: Option<Vec<ClipId>>,
        groove: Option<GrooveId>,
    ) -> Result<Vec<ClipId>, ShellError> {
        let doc = self.editor.document();
        let clips = match (named, groove) {
            (Some(clips), _) => clips,
            (None, Some(groove)) => doc.clips_using_groove(groove),
            (None, None) => self.scale_targets(),
        };
        if clips.is_empty() {
            return Err(unavailable(id, "no clip is selected"));
        }
        if let Some(missing) = clips.iter().find(|clip| doc.clip(**clip).is_none()) {
            return Err(ShellError::InvalidArgs {
                command: id.to_owned(),
                message: format!("clip {} does not exist", missing.0),
            });
        }
        Ok(clips)
    }

    /// The groove of the pool with this ID.
    fn pool_groove(&self, id: &str, groove: GrooveId) -> Result<Groove, ShellError> {
        self.editor
            .document()
            .settings()
            .grooves
            .get(groove)
            .cloned()
            .ok_or_else(|| ShellError::InvalidArgs {
                command: id.to_owned(),
                message: format!("the Groove Pool holds no groove {}", groove.0),
            })
    }

    /// The groove file at `path`, named after the file when it holds no name.
    fn read_groove(&self, id: &str, path: &str) -> Result<GrooveFile, ShellError> {
        let path = PathBuf::from(path);
        let mut file = GrooveFile::load(&path).map_err(|error| file_error(id, &error))?;
        if file.name.is_empty() {
            file.name = bl_project::preset::preset_name(&path);
        }
        Ok(file)
    }

    /// `groove.load` (R5).
    fn load_groove(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let LoadGrooveArgs {
            path,
            clips,
            replace,
        } = Self::args(id, args)?;
        let file = self.read_groove(id, &path)?;
        // Spec: grooves R32. What the file's groove takes the place of: the
        // old groove, the clips that used it, and the auto-load setting.
        let doc = self.editor.document();
        let pool = &doc.settings().grooves;
        let old = match replace {
            Some(GrooveHotSwap::Groove(old)) => Some(self.pool_groove(id, old)?.id),
            Some(GrooveHotSwap::AutoLoad) => pool.auto_load_groove,
            None => None,
        };
        let swapped = old.map(|old| (old, doc.clips_using_groove(old), pool.auto_load_groove));
        let groove: GrooveId = self.editor.new_id();
        let name = file.name.clone();
        let step = if replace.is_some() {
            HOT_SWAP_STEP
        } else {
            LOAD_STEP
        };
        self.groove_edit(id, step, |editor| {
            editor.apply(Command::AddGroove {
                groove: file.into_groove(groove),
            })?;
            let auto_load = replace == Some(GrooveHotSwap::AutoLoad);
            if let Some((old, users, auto)) = swapped {
                if !users.is_empty() {
                    editor.apply(Command::SetClipGroove {
                        clips: users,
                        groove: Some(groove),
                    })?;
                }
                if auto == Some(old) {
                    editor.apply(Command::SetAutoLoadGroove {
                        enabled: None,
                        groove: Some(groove),
                    })?;
                }
                editor.apply(Command::DeleteGroove { groove: old })?;
            } else if auto_load {
                editor.apply(Command::SetAutoLoadGroove {
                    enabled: None,
                    groove: Some(groove),
                })?;
            }
            if !clips.is_empty() {
                editor.apply(Command::SetClipGroove {
                    clips,
                    groove: Some(groove),
                })?;
            }
            Ok(())
        })?;
        // A hot-swap aimed at the groove that left stays on, aimed at its
        // replacement.
        if let (Some(old), Some(GrooveHotSwap::Groove(armed))) = (old, self.groove_pool.hot_swap)
            && old == armed
        {
            self.groove_pool.hot_swap = Some(GrooveHotSwap::Groove(groove));
        }
        Ok(to_value(&GrooveLoaded { groove, name }))
    }

    /// `groove.save` (R19).
    fn save_groove(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let SaveGrooveArgs {
            groove,
            path,
            replace,
        } = Self::args(id, args)?;
        let groove = self.pool_groove(id, groove)?;
        let path = match path {
            Some(path) if !path.is_empty() => PathBuf::from(path),
            Some(_) => {
                return Err(ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: "`path` is empty".to_owned(),
                });
            }
            None => {
                let library = self.browser_view.user_library().ok_or_else(|| {
                    unavailable(id, "there's no User Library to save a groove in")
                })?;
                let name: String = groove
                    .name
                    .chars()
                    .map(|c| {
                        if matches!(c, '/' | '\\' | ':') {
                            '-'
                        } else {
                            c
                        }
                    })
                    .collect();
                library
                    .join("Grooves")
                    .join(format!("{name}.{GROOVE_EXTENSION}"))
            }
        };
        if path.exists() && !replace {
            return Err(failed(
                id,
                format!(
                    "{} is already there; pass `replace` to write over it",
                    path.display()
                ),
            ));
        }
        GrooveFile::of(&groove)
            .save(&path)
            .map_err(|error| file_error(id, &error))?;
        // The Grooves label lists it.
        self.browser_view.files_written();
        self.touch();
        Ok(to_value(&GrooveSaved {
            path: path.display().to_string(),
        }))
    }

    /// `groove.to_clip` (R18).
    fn groove_to_clip(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let GrooveToClipArgs {
            groove,
            path,
            track,
            at,
        } = Self::args(id, args)?;
        if at < Ticks::ZERO {
            return Err(ShellError::InvalidArgs {
                command: id.to_owned(),
                message: "`at` can't be before the song's start".to_owned(),
            });
        }
        // A groove of the pool, or a file's, which joins the pool.
        let (groove, joins) = match (groove, path) {
            (Some(groove), None) => (self.pool_groove(id, groove)?, false),
            (None, Some(path)) => {
                let file = self.read_groove(id, &path)?;
                (file.into_groove(self.editor.new_id()), true)
            }
            _ => {
                return Err(ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: "give `groove` or `path`, not both".to_owned(),
                });
            }
        };
        if let Some(track) = track {
            match self
                .editor
                .document()
                .track(track)
                .map(bl_model::Track::kind)
            {
                Some(TrackKind::Midi) => {}
                Some(_) => {
                    return Err(unavailable(
                        id,
                        format!("track {} isn't a MIDI track", track.0),
                    ));
                }
                None => {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: format!("track {} does not exist", track.0),
                    });
                }
            }
        }
        let clip: ClipId = self.editor.new_id();
        let new_track: TrackId = track.unwrap_or_else(|| self.editor.new_id());
        let mut data = MidiData::new(groove_notes(&groove));
        data.looping = true;
        data.loop_end = Some(groove.length);
        let mut made = Clip::new(clip, at, groove.length, ClipData::Midi(data));
        made.name.clone_from(&groove.name);
        let (groove_id, length, name) = (groove.id, groove.length, groove.name.clone());
        let look = self.prefs.look.clone();
        self.groove_edit(id, TO_CLIP_STEP, |editor| {
            if joins {
                editor.apply(Command::AddGroove { groove })?;
            }
            if track.is_none() {
                let top = editor.document().top_level().len();
                editor.apply(Command::CreateTrack {
                    id: new_track,
                    kind: TrackKind::Midi,
                    name,
                    parent: None,
                    index: top,
                })?;
                crate::colours::colour_new(editor, &look, &[new_track], &[])?;
            }
            crate::arrangement::placing::clear(
                editor,
                new_track,
                at,
                at.saturating_add(length),
                &BTreeSet::new(),
            )?;
            editor.apply(Command::CreateClip {
                track: new_track,
                clip: made,
            })?;
            Ok(())
        })?;
        Ok(to_value(&GrooveClip {
            clip,
            track: new_track,
            groove: groove_id,
        }))
    }

    /// Runs `edit` as one undo step named `label`, and marks the set
    /// changed.
    fn groove_edit(
        &mut self,
        id: &str,
        label: &'static str,
        edit: impl FnOnce(&mut Editor) -> Result<(), bl_model::EditError>,
    ) -> Result<(), ShellError> {
        let before = self.editor.document().clone();
        self.editor
            .transact(label, edit)
            .map_err(|error| failed(id, error))?;
        if *self.editor.document() != before {
            self.changed();
        }
        Ok(())
    }

    /// `groove.extract` (R17).
    fn extract_grooves(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let ExtractGrooveArgs { clips } = Self::args(id, args)?;
        let clips = self.groove_clips(id, clips, None)?;
        let mut grooves = Vec::with_capacity(clips.len());
        for clip in clips {
            let made: GrooveId = self.editor.new_id();
            grooves.push(self.extract_groove(id, clip, made)?);
        }
        let ids_made: Vec<GrooveId> = grooves.iter().map(|groove| groove.id).collect();
        self.groove_edit(id, EXTRACT_STEP, |editor| {
            for groove in grooves {
                editor.apply(Command::AddGroove { groove })?;
            }
            Ok(())
        })?;
        Ok(to_value(&GrooveExtracted { grooves: ids_made }))
    }

    /// The groove `clip` gives, as the grooves spec's R17 says.
    fn extract_groove(
        &mut self,
        id: &str,
        clip: ClipId,
        made: GrooveId,
    ) -> Result<Groove, ShellError> {
        let value = self
            .editor
            .document()
            .clip(clip)
            .cloned()
            .ok_or_else(|| failed(id, format!("clip {} does not exist", clip.0)))?;
        let (length, notes) = match &value.data {
            ClipData::Midi(midi) => midi_groove(&value, midi),
            ClipData::Audio(audio) => self.audio_groove(id, clip, &value, audio)?,
        };
        if length <= Ticks::ZERO {
            return Err(unavailable(id, "the part of the clip that plays is empty"));
        }
        let name = if value.name.is_empty() {
            "Groove".to_owned()
        } else {
            value.name.clone()
        };
        Ok(Groove::new(made, name, length, notes))
    }

    /// The groove notes of an audio clip's transients and the length of the
    /// part of the clip that plays (R17).
    fn audio_groove(
        &mut self,
        id: &str,
        clip: ClipId,
        value: &Clip,
        audio: &AudioData,
    ) -> Result<(Ticks, Vec<GrooveNote>), ShellError> {
        let transients = self.ready_transients(id, clip)?;
        let file_end = self.file_seconds(audio);
        let tempo = self.editor.document().settings().tempo.clone();
        // The part that plays, in seconds into the file, and where each
        // second falls in ticks from the part's start.
        let (from, to) = if audio.looping {
            (audio.loop_start, audio.loop_end)
        } else {
            (audio.start, audio.end)
        };
        let (from_seconds, to_seconds, ticks_of): (f64, f64, Box<dyn Fn(f64) -> f64>) = if audio
            .warp
        {
            let map = audio.warp_map().map_err(|error| failed(id, error))?;
            let to = match (to, file_end) {
                (Some(to), _) => to,
                (None, Some(end)) => map.time_to_beat(end),
                (None, None) => from + value.length.get() as f64,
            };
            (
                map.beat_to_time(from),
                map.beat_to_time(to),
                Box::new(move |time| map.time_to_beat(time) - from),
            )
        } else {
            let to = to
                .or(file_end)
                .ok_or_else(|| failed(id, "the media store doesn't have the clip's file"))?;
            // An unwarped clip's seconds fall at the set's tempo where the
            // clip starts.
            let start = tempo.ticks_to_seconds(value.start);
            let origin = value.start.get() as f64;
            (
                from,
                to,
                Box::new(move |time| tempo.seconds_to_ticks_exact(start + (time - from)) - origin),
            )
        };
        let length = Ticks(ticks_of(to_seconds).round() as i64);
        let hits: Vec<f64> = transients
            .into_iter()
            .filter(|t| *t >= from_seconds && *t < to_seconds)
            .collect();
        let peaks = self.peaks_after(id, audio, &hits)?;
        let loudest = peaks.iter().copied().fold(0.0_f64, f64::max);
        let mut notes: Vec<GrooveNote> = Vec::new();
        for (time, peak) in hits.iter().zip(peaks) {
            let position = Ticks(ticks_of(*time).round() as i64);
            if position < Ticks::ZERO || position >= length {
                continue;
            }
            let velocity = peak_velocity(peak, loudest);
            match notes.last_mut() {
                // The loudest of notes that start together.
                Some(last) if last.position == position => {
                    last.velocity = last.velocity.max(velocity);
                }
                _ => notes.push(GrooveNote { position, velocity }),
            }
        }
        Ok((length, notes))
    }

    /// The audio's peak level in the 20 ms after each of `times`, in seconds
    /// into the clip's file.
    fn peaks_after(
        &self,
        id: &str,
        audio: &AudioData,
        times: &[f64],
    ) -> Result<Vec<f64>, ShellError> {
        let entry = self
            .media
            .entry(MediaHash::from_bytes(audio.media.0))
            .ok_or_else(|| failed(id, "the media store doesn't have the clip's file"))?;
        let data = decode_file(&entry.path).map_err(|error| failed(id, error))?;
        let rate = f64::from(data.sample_rate());
        Ok(times
            .iter()
            .map(|time| {
                let from = (time * rate).round().max(0.0) as usize;
                let to = (from + (PEAK_SECONDS * rate) as usize).min(data.frames());
                data.channels()
                    .flat_map(|channel| channel.get(from..to).unwrap_or_default())
                    .fold(0.0_f64, |peak, sample| peak.max(f64::from(sample.abs())))
            })
            .collect())
    }

    /// `groove.commit` (R16).
    fn commit_grooves(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let CommitGrooveArgs {
            clips,
            groove,
            seed,
        } = Self::args(id, args)?;
        let clips = self.groove_clips(id, clips, groove)?;
        let seed = seed.unwrap_or_else(new_seed);
        let mut steps: Vec<Vec<Command>> = Vec::new();
        let mut done = Vec::new();
        for clip in clips {
            let doc = self.editor.document();
            let Some(value) = doc.clip(clip).cloned() else {
                continue;
            };
            let Some(play) = value.groove.and_then(|g| doc.settings().grooves.play(g)) else {
                continue;
            };
            let mut commands = match &value.data {
                ClipData::Midi(midi) => vec![Command::SetClipData {
                    clip,
                    data: ClipData::Midi(committed_midi(&value, midi, &play, seed)),
                }],
                ClipData::Audio(audio) if audio.warp => {
                    self.committed_audio(id, clip, &value, audio, &play, seed)?
                }
                // An unwarped clip plays its groove unchanged, so there is
                // nothing to write.
                ClipData::Audio(_) => Vec::new(),
            };
            commands.push(Command::SetClipGroove {
                clips: vec![clip],
                groove: None,
            });
            steps.push(commands);
            done.push(clip);
        }
        self.groove_edit(id, COMMIT_STEP, |editor| {
            for command in steps.into_iter().flatten() {
                editor.apply(command)?;
            }
            Ok(())
        })?;
        Ok(to_value(&GrooveCommitted { clips: done }))
    }

    /// The commands that write a warped audio clip's groove into it (R16):
    /// its warp markers where the groove puts each grid line, transient and
    /// marker, and, for a positive Velocity, the Volume clip envelope of its
    /// gains.
    fn committed_audio(
        &mut self,
        id: &str,
        clip: ClipId,
        value: &Clip,
        audio: &AudioData,
        play: &GroovePlay,
        seed: u64,
    ) -> Result<Vec<Command>, ShellError> {
        let transients = self.ready_transients(id, clip)?;
        let map = audio.warp_map().map_err(|error| failed(id, error))?;
        // The part of the contents that plays: its first pass.
        let lo = audio.start;
        let hi = if audio.looping {
            audio.loop_end
        } else {
            audio.end
        }
        .or_else(|| self.file_seconds(audio).map(|end| map.time_to_beat(end)))
        .unwrap_or(lo + value.length.get() as f64);
        let moved = MovedTransients::new(
            play,
            transients.iter().map(|t| map.time_to_beat(*t)).collect(),
        );
        let pass = AudioPass::new(play, &moved, (lo, hi), seed, 0);
        // Where each point of the old map plays now: the markers the clip
        // had, each transient the groove moved, and each grid line.
        let mut pairs: Vec<(f64, f64)> = audio
            .warp_markers
            .iter()
            .map(|marker| {
                let beat = marker.beat.get() as f64;
                let played = if beat > lo && beat < hi {
                    pass.map(beat)
                } else {
                    beat
                };
                (marker.time, played)
            })
            .collect();
        let mut points = Vec::new();
        pass.moved(&mut points);
        pass.lines(hi, &mut points);
        pairs.extend(
            points
                .into_iter()
                .map(|(source, played)| (map.beat_to_time(source), played)),
        );
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut markers: Vec<WarpMarker> = Vec::new();
        for (time, played) in pairs {
            let beat = Ticks(played.round() as i64);
            if markers
                .last()
                .is_none_or(|last| time > last.time + 1e-6 && beat > last.beat)
            {
                markers.push(WarpMarker { time, beat });
            }
        }
        let tempo = audio.tempo.unwrap_or(120.0);
        let mut next = audio
            .with_map(markers, tempo)
            .map_err(|error| failed(id, error))?;
        let mut commands = Vec::new();
        let volume = AutomationTarget::Mixer {
            param: MixerParam::Volume,
        };
        let mut steps = Vec::new();
        if play.has_velocity() {
            pass.gain_steps(hi, &mut steps);
        }
        let loudest = steps.iter().map(|step| step.1).fold(1.0_f64, f64::max);
        if !steps.is_empty() {
            // A Volume envelope only lowers the level, so its steps are
            // shares of the loudest, and the clip's gain rises by it.
            next.gain *= loudest;
        }
        commands.push(Command::SetClipData {
            clip,
            data: ClipData::Audio(next),
        });
        if !steps.is_empty() {
            commands.push(Command::DeleteClipEnvelope {
                clip,
                target: volume,
                kind: bl_model::EnvelopeKind::Modulation,
            });
            let share = |gain: f64| {
                (fader::fader_position(gain / loudest) / fader::fader_position(1.0)).clamp(0.0, 1.0)
            };
            let mut points: Vec<(Ticks, f64)> = Vec::new();
            for (at, gain) in &steps {
                let at = Ticks(at.round() as i64).max(Ticks::ZERO);
                if let Some(&(last_at, last_value)) = points.last() {
                    // A step: hold the old value to the tick before it.
                    if at - Ticks(1) > last_at {
                        points.push((at - Ticks(1), last_value));
                    }
                    if at <= last_at {
                        continue;
                    }
                }
                points.push((at, share(*gain)));
            }
            for (at, value) in points {
                commands.push(Command::SetClipEnvelopeBreakpoint {
                    clip,
                    target: volume,
                    kind: bl_model::EnvelopeKind::Modulation,
                    at,
                    value,
                });
            }
        }
        Ok(commands)
    }
}

/// A seed no earlier commit had: the clock's nanoseconds.
fn new_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |since| since.as_nanos() as u64)
}

/// The velocity of a groove note made from audio whose peak was `peak`, the
/// loudest of its clip's being `loudest` (R17): 127 at the loudest and 1 at
/// 48 dB below it.
fn peak_velocity(peak: f64, loudest: f64) -> u8 {
    if peak <= 0.0 || loudest <= 0.0 {
        return 1;
    }
    let db = 20.0 * (peak / loudest).log10();
    (1.0 + (LOUDEST - 1.0) * (1.0 + db / RANGE_DB).clamp(0.0, 1.0)).round() as u8
}

/// The length and the groove notes of a MIDI clip's part that plays (R17):
/// its loop brace while the loop is on, otherwise its Start to its End
/// marker.
fn midi_groove(clip: &Clip, midi: &MidiData) -> (Ticks, Vec<GrooveNote>) {
    let (from, to) = if midi.looping {
        let end = midi.loop_end.unwrap_or_else(|| {
            (midi.loop_start + clip.length).max(midi.loop_start + Ticks::QUARTER)
        });
        (midi.loop_start, end)
    } else {
        (midi.start, midi.end.unwrap_or(midi.start + clip.length))
    };
    let mut notes: Vec<GrooveNote> = Vec::new();
    let mut held: Vec<&Note> = midi
        .notes
        .iter()
        .filter(|note| !note.mute && note.start >= from && note.start < to)
        .collect();
    held.sort_by_key(|note| note.start);
    for note in held {
        let position = note.start - from;
        match notes.last_mut() {
            // The loudest of notes that start together.
            Some(last) if last.position == position => {
                last.velocity = last.velocity.max(note.velocity);
            }
            _ => notes.push(GrooveNote {
                position,
                velocity: note.velocity,
            }),
        }
    }
    (to - from, notes)
}

/// The clip's first pass through its contents, in ticks: the stretch the
/// engine plays its notes from, which is where a grooved note is held
/// inside (R11).
fn first_pass(clip: &Clip, midi: &MidiData) -> (Ticks, Ticks) {
    let lo = midi.start;
    let room = if midi.looping {
        let end = midi.loop_end.unwrap_or_else(|| {
            (midi.loop_start + clip.length).max(midi.loop_start + Ticks::QUARTER)
        });
        end - lo
    } else {
        midi.end.unwrap_or(lo + clip.length) - lo
    };
    (lo, lo + room.min(clip.length))
}

/// A MIDI clip's notes where they play with `play` in the clip's first pass,
/// at the velocities they play at (R16), Random drawn from `seed`.
fn committed_midi(clip: &Clip, midi: &MidiData, play: &GroovePlay, seed: u64) -> MidiData {
    let (lo, hi) = first_pass(clip, midi);
    let mut next = midi.clone();
    next.notes = midi
        .notes
        .iter()
        .map(|note| {
            let mut moved = note.clone();
            if note.start >= lo && note.start < hi {
                let at = play.note_start(note.start, note.key, seed, 0);
                moved.start = Ticks((at.round() as i64).clamp(lo.get(), hi.get() - 1));
                moved.velocity = play.note_velocity(note.start, note.velocity);
            }
            moved
        })
        .collect();
    next
}

/// A groove made into a clip's notes (R18): a note on C3 at each groove
/// note with its velocity, a sixteenth long or as long as the gap to the next
/// note, whichever is shorter.
fn groove_notes(groove: &Groove) -> Vec<Note> {
    groove
        .notes
        .iter()
        .enumerate()
        .map(|(i, note)| {
            let next = groove
                .notes
                .get(i + 1)
                .map_or(groove.length, |next| next.position);
            let length = (next - note.position).min(Ticks::SIXTEENTH).max(Ticks(1));
            Note::new(60, note.position, length, note.velocity)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: grooves R17
    #[test]
    fn an_audio_peak_is_a_velocity_over_48_db() {
        assert_eq!(peak_velocity(1.0, 1.0), 127);
        assert_eq!(peak_velocity(10f64.powf(-48.0 / 20.0), 1.0), 1);
        assert_eq!(peak_velocity(10f64.powf(-60.0 / 20.0), 1.0), 1);
        assert_eq!(peak_velocity(0.5, 1.0), 111);
        assert_eq!(peak_velocity(0.0, 1.0), 1);
    }
}
