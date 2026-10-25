//! Slice to New MIDI Track (WP-4.16; the slice-to-midi spec, R1 to R14):
//! cutting an audio clip into slices on a new MIDI track, and the slicing
//! dialog's commands. The clip's warped audio, when the slices keep its
//! warped timing, goes through WP-4.20's processed-file writer, as
//! Consolidate's render does.

use std::cell::RefCell;
use std::fs;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use bl_core::time::{TempoMap, Ticks, TimeSignature};
use bl_devices::sampler as sampler_ids;
use bl_model::{
    AudioData, Chain, ChainId, Clip, ClipData, ClipId, Command, ContentHash, Device, DeviceId,
    DeviceKind, DeviceRole, Document, MacroTarget, MidiData, Note, RackKind, SceneId, TrackId,
    TrackKind,
};
use bl_project::rack_preset::{RackPreset, Samples, Saved};
use bl_shell_spec::slice::{
    self as ids, DRUM_RACK_PRESET, FACTORY_PRESETS, ONE_SAMPLER_PRESET, STEP, SliceArgs, SliceBy,
    SliceDialogStatus, SliceResult,
};
use serde_json::Value;

use super::*;
use crate::processed::{Kind, Request};

/// The most slices a slicing makes: a rack holds at most 128 chains (R5).
const MAX_SLICES: usize = 128;
/// The most the one-sampler preset takes: the sampler's own limit (R5).
const MAX_ONE_SAMPLER: usize = 64;
/// The velocity every note gets (R9).
const VELOCITY: u8 = 127;
/// The note the first slice's chain receives: C1, the first pad a new drum
/// rack shows (R8).
const FIRST_NOTE: u8 = 36;
/// The note a drum chain plays (drum racks R3): C3, the sampler's reference
/// pitch, so a slice sounds at its own pitch.
const PLAY_NOTE: u8 = 60;
/// The sampler device type's ID.
const SAMPLER: &str = "beatlayer.snapshot";
/// The Slice By choice that places no automatic slices and plays the
/// device's own slice list (sampler R14).
const SLICE_BY_MANUAL: f64 = 3.0;
/// The Mode choice that plays one slice per note (sampler R13).
const MODE_SLICE: f64 = 2.0;
/// The Mode choice that plays the region straight through (sampler R10).
const MODE_ONE_SHOT: f64 = 1.0;
/// Where a user slicing preset's samples go when the set's media store
/// lacks them, under the User Library (R11).
const PRESET_SAMPLES_FOLDER: &str = "Samples/Preset Samples";
/// The User Library folder user slicing presets live in (R11).
const SLICING_FOLDER: &str = "Defaults/Slicing";
/// The longest a slicing's render may run: a day, as an export's is.
const LONGEST: f64 = 86_400.0;

/// How a slicing preset shapes the new track (R7, R9, R10, R11).
enum Preset {
    /// The factory Drum Rack preset: one chain per slice, each a sampler in
    /// One-Shot mode playing its slice (R7). The default.
    DrumRack,
    /// The factory one-sampler preset: one sampler in Slice mode whose
    /// slices are the slice points (R10).
    OneSampler,
    /// A user's Drum Rack preset, whose first chain is the pattern every
    /// slice's chain copies (R11).
    User(Box<RackPreset>),
}

/// Everything a slicing needs to build the new track, worked out before the
/// warped audio is rendered (if it is).
struct Plan {
    /// Each slice's first frame in the file the samplers play, at the
    /// engine's rate, in order (R4).
    frames: Vec<u64>,
    /// Where each slice ends: the next slice's start, the last the region's
    /// end. The last is the rendered file's length, filled in once it's
    /// whole.
    ends: Vec<u64>,
    /// The file's length, in frames at the engine's rate.
    file_frames: u64,
    /// Each note's start and end, in ticks from the clip's contents (R9).
    notes: Vec<(Ticks, Ticks)>,
    /// How the preset shapes the track.
    preset: Preset,
    /// The new track's name (the clip's), place, and scene.
    name: String,
    parent: Option<TrackId>,
    index: usize,
    scene: Option<SceneId>,
    /// The audio clip's place and length, and whether it loops (R9).
    start: Ticks,
    length: Ticks,
    looping: bool,
}

impl Shell {
    /// Runs one of the slice-to-midi commands (R1, R13).
    pub(super) fn run_slice(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::OPEN => self.open_slice_dialog(id, args),
            ids::CLOSE => {
                self.slice_dialog = None;
                Ok(to_value(&self.slice_status()))
            }
            ids::SLICE => {
                // Spec: R1. The dialog's OK runs the command and closes the
                // dialog; a refusal leaves it open on the slice to fix.
                let result = self.slice_to_midi(id, args)?;
                self.slice_dialog = None;
                Ok(result)
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// Whether one of the slice-to-midi commands can run with `args`: the
    /// menu items' enabled states read this (R1).
    pub(super) fn can_slice(&self, id: &str, args: &Value) -> bool {
        match id {
            ids::SLICE | ids::OPEN => self.slice_clip(args).is_ok(),
            ids::CLOSE => true,
            _ => false,
        }
    }

    /// The slicing dialog's state, for the window to draw (R1).
    #[must_use]
    pub fn slice_dialog(&self) -> Option<SliceDialogStatus> {
        self.slice_dialog.clone()
    }

    fn slice_status(&self) -> SliceDialogStatus {
        self.slice_dialog.clone().unwrap_or_default()
    }

    /// `clip.open_slice_dialog` (R1): opens the dialog on the clip it names
    /// or the selected audio clip.
    fn open_slice_dialog(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let args: ids::OpenArgs = Self::args(id, args)?;
        let clip = self.slice_clip(&json!({ "clip": args.clip }))?;
        let status = SliceDialogStatus {
            open: true,
            clip: Some(clip),
            presets: self.slicing_presets(),
        };
        self.slice_dialog = Some(status.clone());
        Ok(to_value(&status))
    }

    /// The audio clip a slicing acts on (R1): the one the call names, or the
    /// one audio clip selected in either view.
    fn slice_clip(&self, args: &Value) -> Result<ClipId, ShellError> {
        let doc = self.editor.document();
        let named = args
            .get("clip")
            .filter(|value| !value.is_null())
            .and_then(|value| serde_json::from_value::<ClipId>(value.clone()).ok());
        if let Some(clip) = named {
            return match doc.clip(clip).map(|clip| &clip.data) {
                Some(ClipData::Audio(_)) => Ok(clip),
                Some(_) => Err(unavailable(ids::SLICE, "the clip isn't an audio clip")),
                None => Err(invalid(ids::SLICE, "no clip has that ID")),
            };
        }
        let audio = |id: &ClipId| {
            doc.clip(*id)
                .is_some_and(|clip| matches!(clip.data, ClipData::Audio(_)))
        };
        let selected = self.arrangement.selected_clips();
        if selected.len() == 1 && audio(&selected[0]) {
            return Ok(selected[0]);
        }
        self.clip_view
            .shown_audio_clip(doc, &self.arrangement)
            .ok_or_else(|| unavailable(ids::SLICE, "select one audio clip first"))
    }

    /// `clip.slice_to_midi_track` (R3 to R13).
    fn slice_to_midi(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let args: SliceArgs = Self::args(id, args)?;
        let by = args.by.unwrap_or(SliceBy::Sixteenth);
        let preserve = args.preserve_warped_timing.unwrap_or(true);
        let clip_id = self.slice_clip(&json!({ "clip": args.clip }))?;
        let (clip, audio, tempo, parent, index, scene) = {
            let doc = self.editor.document();
            let clip = doc
                .clip(clip_id)
                .cloned()
                .ok_or_else(|| invalid(id, "no clip has that ID"))?;
            let ClipData::Audio(audio) = clip.data.clone() else {
                return Err(unavailable(id, "the clip isn't an audio clip"));
            };
            let tempo = doc.settings().tempo.clone();
            let track = doc
                .clip_track(clip_id)
                .ok_or_else(|| invalid(id, "no clip has that ID"))?;
            let parent = doc.parent(track);
            let siblings: Vec<TrackId> = match parent {
                Some(group) => doc
                    .track(group)
                    .map(|group| group.children().collect())
                    .unwrap_or_default(),
                None => doc.top_level().collect(),
            };
            let index = siblings
                .iter()
                .position(|&other| other == track)
                .map_or(0, |at| at + 1);
            let scene = doc.track(track).and_then(|track| {
                track
                    .slots()
                    .find(|(_, slot)| slot.id == clip_id)
                    .map(|(scene, _)| scene)
            });
            (clip, audio, tempo, parent, index, scene)
        };
        if by == SliceBy::WarpMarkers && !audio.warp {
            return Err(unavailable(
                id,
                "the clip isn't warped: Warp Markers needs a warped clip",
            ));
        }
        let rate = self.engine_rate();
        // The region the clip plays, in the form the clip shows (R3).
        let file_seconds = self
            .file_seconds(&audio)
            .ok_or_else(|| failed(id, "the clip's file isn't in the media store"))?;
        let (region_start, region_end) = audio.crop_region(file_seconds);
        // The slice positions, in the clip's own form (R4).
        let points = self.slice_points(
            id,
            clip_id,
            &audio,
            by,
            region_start,
            region_end,
            &tempo,
            &clip,
        )?;
        if points.is_empty() {
            return Err(unavailable(id, "the region holds no slice"));
        }
        // The preset (R7, R10, R11), and its limit (R5).
        let preset = self.slicing_preset(id, args.preset.as_deref())?;
        let limit = match preset {
            Preset::OneSampler => MAX_ONE_SAMPLER,
            _ => MAX_SLICES,
        };
        if points.len() > limit {
            return Err(unavailable(
                id,
                format!(
                    "{} slices: a slicing makes at most {limit}; choose a coarser division or \
                     slice a shorter clip",
                    points.len()
                ),
            ));
        }
        // With Preserve warped timing on, a warped clip's slices come from
        // its warped audio, rendered as Consolidate renders it (R6).
        let warped = audio.warp && preserve;
        let song_region = region_song(&clip, &audio, &tempo, region_start, region_end);
        let (frames, ends, file_frames) = if warped {
            let frames = rendered_frames(&clip, &audio, &tempo, rate, song_region.0, &points);
            let ends = next_ends(&frames, 0);
            (frames, ends, 0)
        } else {
            let file_frames = self
                .sample_frames(audio.media, rate)
                .ok_or_else(|| failed(id, "the clip's file isn't in the media store"))?;
            let frames = file_frames_of(&audio, rate, &points, file_frames);
            let ends = next_ends(
                &frames,
                region_end_frame(&audio, rate, region_end, file_frames),
            );
            (frames, ends, file_frames)
        };
        let notes = slice_notes(
            &clip,
            &audio,
            &tempo,
            &points,
            region_start,
            region_end,
            warped,
        );
        let plan = Plan {
            frames,
            ends,
            file_frames,
            notes,
            preset,
            name: clip.name.clone(),
            parent,
            index,
            scene,
            start: clip.start,
            length: clip.length,
            looping: audio.looping,
        };
        if warped {
            return self.render_slices(id, clip_id, song_region.0, song_region.1, plan);
        }
        let media = audio.media;
        let result = self.build_sliced_track(id, &plan, media)?;
        Ok(to_value(&result))
    }

    /// The clip's file, decoded at the engine's rate, in frames: what the
    /// sampler's slice frames index.
    fn sample_frames(&self, media: ContentHash, rate: u32) -> Option<u64> {
        self.media
            .load_at_rate(bl_media::ContentHash::from_bytes(media.0), rate)
            .ok()
            .map(|sample| sample.frames() as u64)
    }

    /// The slice positions over the region, in the form the clip shows, in
    /// order (R4): the region's start, then a point for each transient, warp
    /// marker, or beat division inside it.
    #[expect(
        clippy::too_many_arguments,
        reason = "the region, the clip and the tempo it is counted in, named where they're used"
    )]
    fn slice_points(
        &mut self,
        id: &str,
        clip_id: ClipId,
        audio: &AudioData,
        by: SliceBy,
        region_start: f64,
        region_end: f64,
        tempo: &TempoMap,
        clip: &Clip,
    ) -> Result<Vec<f64>, ShellError> {
        let mut points = vec![region_start];
        match by {
            SliceBy::Transients => {
                let transients = self.ready_transients(id, clip_id)?;
                for time in transients {
                    let at = audio.in_clip_form(time);
                    if at > region_start && at < region_end {
                        points.push(at);
                    }
                }
            }
            SliceBy::WarpMarkers => {
                for marker in &audio.warp_markers {
                    let at = marker.beat.get() as f64;
                    if at > region_start && at < region_end {
                        points.push(at);
                    }
                }
            }
            division => {
                let signature = audio
                    .signature
                    .unwrap_or_else(|| self.signature_at(clip.start));
                let step = division.ticks(signature).expect("a division has a length");
                let mut k = 1_i64;
                loop {
                    let at = if audio.warp {
                        region_start + (step.get() * k) as f64
                    } else {
                        region_start + self.seconds_of_ticks(clip.start, step.get() * k, tempo)
                    };
                    if at >= region_end {
                        break;
                    }
                    points.push(at);
                    k += 1;
                }
            }
        }
        points.sort_by(|a, b| a.partial_cmp(b).expect("finite positions"));
        points.dedup_by(|a, b| (*a - *b).abs() < f64::EPSILON);
        Ok(points)
    }

    /// The set's time signature where the clip starts.
    fn signature_at(&self, at: Ticks) -> TimeSignature {
        self.editor
            .document()
            .settings()
            .time_signatures
            .signature_at(at)
    }

    /// `ticks` at the set's tempo, in seconds.
    fn seconds_of_ticks(&self, clip_start: Ticks, ticks: i64, tempo: &TempoMap) -> f64 {
        let base = tempo.ticks_to_seconds(clip_start);
        tempo.ticks_to_seconds(clip_start + Ticks(ticks)) - base
    }

    /// The slicing preset `name` names (R11): one of Beatlayer's factory
    /// presets, or a user preset in the User Library's `Defaults/Slicing/`
    /// folder. `None` is the default.
    fn slicing_preset(&mut self, id: &str, name: Option<&str>) -> Result<Preset, ShellError> {
        let name = name.unwrap_or(DRUM_RACK_PRESET);
        match name {
            DRUM_RACK_PRESET => return Ok(Preset::DrumRack),
            ONE_SAMPLER_PRESET => return Ok(Preset::OneSampler),
            _ => {}
        }
        let path = self
            .slicing_preset_path(name)
            .ok_or_else(|| unavailable(id, format!("no slicing preset is called `{name}`")))?;
        let saved = Saved::load(&path).map_err(|error| failed(id, format!("{name}: {error}")))?;
        let Saved::Rack(preset) = saved else {
            return Err(unavailable(
                id,
                format!("`{name}` isn't a rack preset, so it can't slice"),
            ));
        };
        if preset.rack.rack_kind() != Some(RackKind::Drum) {
            return Err(unavailable(
                id,
                format!("`{name}` isn't a Drum Rack preset, so it can't slice"),
            ));
        }
        let pattern = preset.rack.chains.first();
        let holds_sampler = pattern.is_some_and(|chain| chain.devices.iter().any(is_sampler));
        if !holds_sampler {
            return Err(unavailable(
                id,
                format!("`{name}` has no sampler in its first chain, so it can't slice"),
            ));
        }
        self.bring_in_samples(id, &preset.samples)?;
        Ok(Preset::User(preset))
    }

    /// Makes sure the set can find the samples a user preset carries (R11).
    fn bring_in_samples(&mut self, id: &str, samples: &Samples) -> Result<(), ShellError> {
        if samples.is_empty() {
            return Ok(());
        }
        let folder = crate::browser::places::default_user_library()
            .unwrap_or_else(|| self.temporary_folder())
            .join(PRESET_SAMPLES_FOLDER);
        for (hash, file) in samples {
            let wanted = bl_media::ContentHash::from_bytes(hash.0);
            if self
                .media
                .entry(wanted)
                .is_some_and(|entry| entry.path.is_file())
            {
                continue;
            }
            fs::create_dir_all(&folder)
                .map_err(|error| failed(id, format!("{}: {error}", folder.display())))?;
            let name = bl_project::preset::file_safe(&file.name);
            let path = folder.join(format!("{}-{name}", &hash.to_string()[..16]));
            if !path.is_file() {
                fs::write(&path, &file.data)
                    .map_err(|error| failed(id, format!("{}: {error}", path.display())))?;
            }
            let size = file.data.len() as u64;
            self.media
                .import(&path)
                .map_err(|error| failed(id, error.to_string()))?;
            self.store.add_media(*hash, &path, size);
        }
        Ok(())
    }

    /// The path of the user slicing preset `name`, if the User Library has
    /// one (R11).
    fn slicing_preset_path(&self, name: &str) -> Option<std::path::PathBuf> {
        let library = crate::browser::places::default_user_library()?;
        let path = library
            .join(SLICING_FOLDER)
            .join(format!("{name}.blpreset"));
        path.is_file().then_some(path)
    }

    /// The Slicing Preset chooser's entries (R11): Beatlayer's factory
    /// presets first, then the user's, by name.
    fn slicing_presets(&self) -> Vec<String> {
        let mut names: Vec<String> = FACTORY_PRESETS
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        if let Some(library) = crate::browser::places::default_user_library()
            && let Ok(entries) = fs::read_dir(library.join(SLICING_FOLDER))
        {
            let mut user: Vec<String> = entries
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    (path.extension().is_some_and(|ext| ext == "blpreset"))
                        .then(|| bl_project::preset::preset_name(&path))
                })
                .collect();
            user.sort();
            names.extend(user);
        }
        names
    }

    /// Renders the clip's warped audio over the region and builds the track
    /// when the file is whole (R6, R14).
    fn render_slices(
        &mut self,
        id: &str,
        clip: ClipId,
        start: Ticks,
        end: Ticks,
        mut plan: Plan,
    ) -> Result<Value, ShellError> {
        let document = self.editor.document();
        let track = document
            .clip_track(clip)
            .ok_or_else(|| invalid(id, "no clip has that ID"))?;
        let prepared = slice_document(document, track, clip).map_err(|why| failed(id, why))?;
        let rate = self.engine_rate();
        let media: Arc<dyn bl_engine::MediaLibrary> = self.media.clone();
        let spec = crate::export::Render {
            start,
            length: crate::export::Length::To(end),
            sample_rate: rate,
            engine_rate: rate,
            block: crate::export::BLOCK,
        };
        let cancel = AtomicBool::new(false);
        let mut rendered: Vec<f32> = Vec::new();
        {
            let mut sink = |samples: &[f32]| {
                rendered.extend_from_slice(samples);
                Ok(())
            };
            crate::export::render(
                &prepared,
                media,
                self.plugin_hosts(),
                &spec,
                LONGEST,
                &cancel,
                &mut |_| {},
                &mut sink,
                None,
            )
            .map_err(|error| failed(id, error))?;
        }
        if rendered.is_empty() {
            return Err(unavailable(id, "the region holds no audio"));
        }
        let total = rendered.len() as u64 / 2;
        // The last slice runs to the rendered file's end (R4).
        if let Some(last) = plan.ends.last_mut() {
            *last = total;
        }
        plan.file_frames = total;
        let name = format!("{} Slice [{}].wav", plan.name, crate::recording::stamp());
        let request = Request {
            kind: Kind::Slice,
            name,
            spec: bl_media::EncodeSpec {
                file_type: bl_media::FileType::Wav,
                bit_depth: bl_media::BitDepth::Float32,
                dither: bl_media::Dither::None,
                sample_rate: rate,
                channels: 2,
            },
            frames: total,
            produce: Box::new(move |sink| sink.write(&rendered)),
        };
        let landed: Rc<RefCell<Option<SliceResult>>> = Rc::new(RefCell::new(None));
        let broke: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let done = Rc::clone(&landed);
        let bad = Rc::clone(&broke);
        let command = id.to_owned();
        self.start_processed(request, move |shell, written| {
            let media = ContentHash(*written.hash.as_bytes());
            match shell.build_sliced_track(&command, &plan, media) {
                Ok(result) => {
                    *done.borrow_mut() = Some(result);
                    Ok(())
                }
                Err(error) => {
                    *bad.borrow_mut() = Some(error.to_string());
                    Err(error.to_string())
                }
            }
        })
        .map_err(|error| failed(id, error))?;
        self.wait_processed();
        if let Some(error) = broke.borrow().clone() {
            return Err(failed(id, error));
        }
        let result = landed
            .borrow()
            .clone()
            .ok_or_else(|| failed(id, "the slices weren't written"))?;
        Ok(to_value(&result))
    }

    /// Builds the new MIDI track, its devices and its clip, as one undo step
    /// (R7 to R9, R12, R13).
    fn build_sliced_track(
        &mut self,
        id: &str,
        plan: &Plan,
        media: ContentHash,
    ) -> Result<SliceResult, ShellError> {
        let track: TrackId = self.editor.new_id();
        let device: DeviceId = self.editor.new_id();
        let clip: ClipId = self.editor.new_id();
        let device = self.slice_devices(device, plan, media);
        let base = match plan.preset {
            Preset::OneSampler => PLAY_NOTE,
            _ => FIRST_NOTE,
        };
        let notes = plan
            .notes
            .iter()
            .enumerate()
            .map(|(at, (start, end))| {
                // Spec: R8. The notes rise chromatically from C1. A slicing
                // of more than 92 slices would pass G8, the highest MIDI
                // note, so the staircase wraps to the lowest note and
                // carries on (Beatlayer's own; the manual says only that the
                // order is chromatic).
                let key = (usize::from(base) + at) % 128;
                let key = u8::try_from(key).expect("below 128");
                Note::new(key, *start, *end - *start, VELOCITY)
            })
            .collect::<Vec<_>>();
        let mut data = MidiData::new(notes);
        data.looping = plan.looping;
        let midi_clip = Clip {
            id: clip,
            name: plan.name.clone(),
            start: if plan.scene.is_some() {
                Ticks::ZERO
            } else {
                plan.start
            },
            length: plan.length,
            offset: Ticks::ZERO,
            mute: false,
            data: ClipData::Midi(data),
            envelopes: Vec::new(),
            launch: bl_model::ClipLaunch::default(),
            scale: bl_model::Scale::default(),
            groove: None,
            colour: None,
            info_text: String::new(),
        };
        self.editor.begin(STEP);
        let outcome = (|| -> Result<(), ShellError> {
            self.editor
                .apply(Command::CreateTrack {
                    id: track,
                    kind: TrackKind::Midi,
                    name: plan.name.clone(),
                    parent: plan.parent,
                    index: plan.index,
                })
                .map_err(|error| failed(id, error))?;
            self.editor
                .apply(Command::InsertDevice {
                    track,
                    index: 0,
                    device,
                })
                .map_err(|error| failed(id, error))?;
            match plan.scene {
                Some(scene) => self.editor.apply(Command::CreateSlotClip {
                    track,
                    scene,
                    clip: midi_clip,
                }),
                None => self.editor.apply(Command::CreateClip {
                    track,
                    clip: midi_clip,
                }),
            }
            .map_err(|error| failed(id, error))?;
            crate::colours::colour_new(&mut self.editor, &self.prefs.look, &[track], &[clip])
                .map_err(|error| failed(id, error))
        })();
        match outcome {
            Ok(()) => {
                if self.editor.commit().unwrap_or(false) {
                    self.dirty = true;
                }
                self.changed();
                let _ = self.arrangement.run(
                    view_ids::SELECT_TRACKS,
                    json!({ "tracks": [track] }),
                    self.editor.document(),
                );
                Ok(SliceResult {
                    track,
                    clip,
                    slices: plan.frames.len(),
                })
            }
            Err(error) => {
                self.editor.cancel().ok();
                Err(error)
            }
        }
    }

    /// The device the new track holds (R7, R10, R11).
    fn slice_devices(&mut self, id: DeviceId, plan: &Plan, media: ContentHash) -> Device {
        match &plan.preset {
            Preset::OneSampler => {
                let last = plan.ends.last().copied().unwrap_or(plan.file_frames);
                sampler_device(
                    id,
                    media,
                    MODE_SLICE,
                    Some((plan.frames[0], last)),
                    &plan.frames,
                    plan.file_frames,
                )
            }
            Preset::DrumRack => self.drum_rack(id, plan, media, None),
            Preset::User(preset) => {
                let preset = (**preset).clone();
                self.drum_rack(id, plan, media, Some(preset))
            }
        }
    }

    /// A Drum Rack with one chain per slice (R7), shaped by the user preset
    /// `pattern` when there is one (R11).
    fn drum_rack(
        &mut self,
        id: DeviceId,
        plan: &Plan,
        media: ContentHash,
        pattern: Option<RackPreset>,
    ) -> Device {
        let mut rack = Device::rack(id, RackKind::Drum);
        let template = pattern
            .as_ref()
            .and_then(|preset| preset.rack.chains.first().cloned());
        for (at, (&start, &end)) in plan.frames.iter().zip(&plan.ends).enumerate() {
            let chain_id: ChainId = self.editor.new_id();
            let devices = match &template {
                Some(template) => {
                    let mut chain = template.clone();
                    chain
                        .devices
                        .iter_mut()
                        .for_each(|device| device.reassign_ids(&mut || self.editor.new_id()));
                    let first = chain
                        .devices
                        .iter_mut()
                        .find(|device| is_sampler(device))
                        .expect("the preset's first chain holds a sampler");
                    *first = sampler_device(
                        first.id,
                        media,
                        MODE_ONE_SHOT,
                        Some((start, end)),
                        &[],
                        plan.file_frames,
                    );
                    chain.devices
                }
                None => {
                    let sampler_id: DeviceId = self.editor.new_id();
                    vec![sampler_device(
                        sampler_id,
                        media,
                        MODE_ONE_SHOT,
                        Some((start, end)),
                        &[],
                        plan.file_frames,
                    )]
                }
            };
            let note = (usize::from(FIRST_NOTE) + at) % 128;
            let note = u8::try_from(note).expect("below 128");
            let mut chain =
                Chain::for_pad(chain_id, format!("Slice {}", at + 1), note).with_devices(devices);
            chain.play = PLAY_NOTE;
            rack.chains.push(chain);
        }
        if let Some(preset) = pattern {
            rack.macros = preset.rack.macros.clone();
            remap_macros(&mut rack, &preset.rack);
        }
        rack
    }
}

/// An "invalid arguments" error naming the command.
fn invalid(id: &str, message: impl ToString) -> ShellError {
    ShellError::InvalidArgs {
        command: id.to_owned(),
        message: message.to_string(),
    }
}

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

/// Whether `device` is Beatlayer's sampler.
fn is_sampler(device: &Device) -> bool {
    matches!(&device.kind, DeviceKind::Native { device_type } if device_type == SAMPLER)
}

/// A native sampler device in `mode` playing `media`'s region (sampler R1,
/// R3, R13). `region` is the played region's frames and `slices` its manual
/// slice points, both at the engine's rate.
fn sampler_device(
    id: DeviceId,
    media: ContentHash,
    mode: f64,
    region: Option<(u64, u64)>,
    slices: &[u64],
    file_frames: u64,
) -> Device {
    let mut device = Device::new(
        id,
        "Simpler",
        DeviceKind::Native {
            device_type: SAMPLER.to_owned(),
        },
    )
    .with_role(DeviceRole::Instrument);
    device.set_media(0, Some(media));
    device.params.insert(sampler_ids::MODE, mode);
    if let Some((start, end)) = region {
        // Spec: sampler R3. The region is Start and Length as shares of the
        // sample, and the sampler rounds each down to a frame, so half a
        // frame is added: the shares then land on the slice's own frames.
        let total = file_frames.max(1) as f64;
        device
            .params
            .insert(sampler_ids::START, (start as f64 + 0.5) * 100.0 / total);
        device.params.insert(
            sampler_ids::LENGTH,
            end.saturating_sub(start) as f64 * 100.0 / total,
        );
    }
    if !slices.is_empty() {
        device.slices = slices.to_vec();
        device.params.insert(sampler_ids::SLICE_BY, SLICE_BY_MANUAL);
    }
    device
}

/// Gives `rack`'s macros the mappings its user preset's `pattern` holds,
/// each pointing at the copy of the device it named in the pattern chain, on
/// every slice's chain (R11).
fn remap_macros(rack: &mut Device, pattern: &Device) {
    let Some(first) = pattern.chains.first() else {
        return;
    };
    let old: Vec<DeviceId> = first
        .devices
        .iter()
        .flat_map(|device| device.walk().map(|device| device.id))
        .collect();
    let mut mappings = Vec::new();
    for chain in &rack.chains {
        let ids: Vec<DeviceId> = chain
            .devices
            .iter()
            .flat_map(|device| device.walk().map(|device| device.id))
            .collect();
        for mapping in &pattern.macros.mappings {
            let MacroTarget::Device { device, param } = mapping.target else {
                continue;
            };
            let Some(at) = old.iter().position(|id| *id == device) else {
                continue;
            };
            let Some(&device) = ids.get(at) else {
                continue;
            };
            let mut copy = mapping.clone();
            copy.target = MacroTarget::Device { device, param };
            mappings.push(copy);
        }
    }
    rack.macros.mappings = mappings;
}

/// Where each slice's frame in the clip's own file starts, unwarped (R6).
fn file_frames_of(audio: &AudioData, rate: u32, points: &[f64], file_frames: u64) -> Vec<u64> {
    let mut frames: Vec<u64> = points
        .iter()
        .map(|point| {
            ((audio.in_file_seconds(*point) * f64::from(rate)).round() as u64).min(file_frames)
        })
        .collect();
    frames.dedup();
    frames
}

/// Where the region ends in the clip's own file (R4).
fn region_end_frame(audio: &AudioData, rate: u32, region_end: f64, file_frames: u64) -> u64 {
    ((audio.in_file_seconds(region_end) * f64::from(rate)).round() as u64).min(file_frames)
}

/// Where each slice starts in the rendered warped file (R6), in frames from
/// the region's start.
fn rendered_frames(
    clip: &Clip,
    audio: &AudioData,
    tempo: &TempoMap,
    rate: u32,
    region_start: Ticks,
    points: &[f64],
) -> Vec<u64> {
    let base = tempo.ticks_to_seconds(region_start);
    let mut frames: Vec<u64> = points
        .iter()
        .map(|point| {
            let song = song_of(clip, audio, tempo, *point);
            ((tempo.ticks_to_seconds(song) - base) * f64::from(rate))
                .round()
                .max(0.0) as u64
        })
        .collect();
    frames.dedup();
    frames
}

/// Each slice's end: the next slice's start, the last the region's end.
fn next_ends(frames: &[u64], last: u64) -> Vec<u64> {
    let mut ends: Vec<u64> = frames.iter().skip(1).copied().collect();
    ends.push(last.max(frames.last().copied().unwrap_or(0)));
    ends
}

/// Where a position in the clip's own form plays in the song (R6).
fn song_of(clip: &Clip, audio: &AudioData, tempo: &TempoMap, value: f64) -> Ticks {
    if audio.warp {
        clip.start + Ticks((value - audio.start).round() as i64)
    } else {
        let base = tempo.ticks_to_seconds(clip.start);
        tempo.seconds_to_ticks(base + (value - audio.start))
    }
}

/// The region's start and end as song positions (R6).
fn region_song(
    clip: &Clip,
    audio: &AudioData,
    tempo: &TempoMap,
    start: f64,
    end: f64,
) -> (Ticks, Ticks) {
    (
        song_of(clip, audio, tempo, start),
        song_of(clip, audio, tempo, end),
    )
}

/// Each note's start and end, in ticks from the clip's contents (R9): where
/// its slice plays, and the next slice's note, the last the region's end.
fn slice_notes(
    clip: &Clip,
    audio: &AudioData,
    tempo: &TempoMap,
    points: &[f64],
    region_start: f64,
    region_end: f64,
    warped: bool,
) -> Vec<(Ticks, Ticks)> {
    let offset = |value: f64| -> Ticks {
        if warped {
            song_of(clip, audio, tempo, value) - clip.start
        } else {
            let base = tempo.ticks_to_seconds(clip.start);
            let from = base + (audio.in_file_seconds(region_start) - audio.start);
            let at = base + (audio.in_file_seconds(value) - audio.start);
            tempo.seconds_to_ticks(at) - tempo.seconds_to_ticks(from)
        }
    };
    let starts: Vec<Ticks> = points.iter().map(|point| offset(*point)).collect();
    let mut notes = Vec::with_capacity(starts.len());
    for (at, start) in starts.iter().enumerate() {
        let end = starts
            .get(at + 1)
            .copied()
            .unwrap_or_else(|| offset(region_end));
        notes.push((*start, end.max(*start)));
    }
    notes
}

/// A document holding only `clip`, for a slicing's render (R6): its track
/// alone, through Consolidate's own preparation (audio clip editing R20),
/// at unity gain and without its fades.
///
/// # Errors
///
/// A message if the document refuses one of the steps.
fn slice_document(document: &Document, track: TrackId, clip: ClipId) -> Result<Document, String> {
    // Spec: R6. The clip's audio reaches Main alone, before the track's
    // devices and mixer, as Consolidate renders it (audio clip editing
    // R20).
    let mut doc = super::audio_editing::consolidate_document(document, track, None)?;
    let extra: Vec<ClipId> = doc
        .track(track)
        .map(|track| {
            track
                .clips()
                .filter(|other| other.id != clip)
                .map(|other| other.id)
                .collect()
        })
        .unwrap_or_default();
    for other in extra {
        doc.apply(&Command::DeleteClip { clip: other })
            .map_err(|error| error.to_string())?;
    }
    if let Some(ClipData::Audio(mut audio)) = doc.clip(clip).map(|clip| clip.data.clone()) {
        audio.gain = 1.0;
        audio.fade_in = 0.0;
        audio.fade_out = 0.0;
        doc.apply(&Command::SetClipData {
            clip,
            data: ClipData::Audio(audio),
        })
        .map_err(|error| error.to_string())?;
    }
    Ok(doc)
}
