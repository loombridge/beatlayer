//! What the Sample Editor's keys and menu items mean (WP-4.10, the sample
//! editor spec R10, R13, R19): Insert Warp Marker, Delete Warp Marker,
//! Insert Transient and Delete Transient name no clip, markers, or times, so
//! the shell fills them in from what the editor has selected, and the
//! editor's gestures ask which clips a marker edit reaches.

use std::sync::Arc;

use bl_analysis::cache::Analysis;
use bl_model::ClipId;

use super::*;

/// The commands a key or menu item runs on the Sample Editor's selection.
const KEYED: [&str; 5] = [
    "clip.insert_warp_marker",
    "clip.delete_warp_markers",
    "clip.insert_transient",
    "clip.delete_transients",
    "clip.quantize_audio",
];

/// The commands whose gesture frames the shell checks against the files and
/// fills the transients into before the document runs them.
const GESTURE_WARP: [&str; 4] = [
    "clip.insert_warp_marker",
    "clip.move_warp_markers",
    "clip.shift_warp_marker",
    "clip.move_transient",
];

/// An audio clip's transients, in order of their times in the file.
#[derive(Clone, Debug, Default)]
pub struct TransientList {
    shared: Option<Arc<Analysis>>,
    edited: Vec<f64>,
}

/// What the media store knows of an audio clip's file: its name, its sample
/// rate, its bit depth and its channel count (the sample editor spec, R2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipMediaInfo {
    /// The file's name.
    pub name: String,
    /// The file's sample rate, in hertz.
    pub sample_rate: u32,
    /// The file's bit depth, for the codecs that have one.
    pub bits_per_sample: Option<u32>,
    /// The file's channel count.
    pub channels: u16,
}

impl std::ops::Deref for TransientList {
    type Target = [f64];

    fn deref(&self) -> &[f64] {
        match &self.shared {
            Some(analysis) => &analysis.transients,
            None => &self.edited,
        }
    }
}

impl Shell {
    /// Whether `id` with `args` is a key or menu item for the Sample
    /// Editor's selection: it names no clip.
    pub(super) fn is_sample_key(&self, id: &str, args: &Value) -> bool {
        KEYED.contains(&id) && args.get("clips").is_none() && args.get("clip").is_none()
    }

    /// The audio clips a warp marker edit made on `shown`, with the markers
    /// at `places`, reaches: the shown clip, and the other selected clips
    /// that are warped, have a file as long as its file, and have those
    /// places (R19). Clips of another length are left alone.
    #[must_use]
    pub fn sample_edit_clips(&self, shown: ClipId, places: &[u32]) -> Vec<ClipId> {
        let doc = self.editor.document();
        let audio = |id: ClipId| match doc.clip(id).map(|c| &c.data) {
            Some(ClipData::Audio(audio)) => Some(audio),
            _ => None,
        };
        let mut clips = vec![shown];
        let selected = self.arrangement.selected_clips();
        let Some(length) = audio(shown).and_then(|a| self.file_seconds(a)) else {
            return clips;
        };
        if !selected.contains(&shown) {
            return clips;
        }
        for &other in selected {
            let Some(data) = audio(other).filter(|_| other != shown) else {
                continue;
            };
            if data.warp
                && self
                    .file_seconds(data)
                    .is_some_and(|len| (len - length).abs() <= 1e-9)
                && places
                    .iter()
                    .all(|&place| (place as usize) < data.warp_markers.len())
            {
                clips.push(other);
            }
        }
        clips
    }

    /// The length of an audio clip's file in seconds, where the media store
    /// knows it.
    #[must_use]
    pub fn clip_file_seconds(&self, clip: ClipId) -> Option<f64> {
        match self.editor.document().clip(clip).map(|c| &c.data) {
            Some(ClipData::Audio(audio)) => self.file_seconds(audio),
            _ => None,
        }
    }

    /// Where the media store last found an audio clip's file.
    #[must_use]
    pub fn clip_file_path(&self, clip: ClipId) -> Option<PathBuf> {
        let Some(ClipData::Audio(audio)) = self.editor.document().clip(clip).map(|c| &c.data)
        else {
            return None;
        };
        self.media
            .entry(bl_media::ContentHash::from_bytes(audio.media.0))
            .map(|entry| entry.path)
    }

    /// What the media store knows of an audio clip's file: its name, its
    /// sample rate, its bit depth and its channel count (the sample editor
    /// spec, R2).
    #[must_use]
    pub fn clip_media_info(&self, clip: ClipId) -> Option<ClipMediaInfo> {
        let Some(ClipData::Audio(audio)) = self.editor.document().clip(clip).map(|c| &c.data)
        else {
            return None;
        };
        let entry = self
            .media
            .entry(bl_media::ContentHash::from_bytes(audio.media.0))?;
        let info = entry.info?;
        Some(ClipMediaInfo {
            name: entry
                .path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
            sample_rate: info.sample_rate,
            bits_per_sample: info.bits_per_sample,
            channels: info.channels,
        })
    }

    /// The analysis of an audio clip's file, if it is ready (WP-4.3).
    fn clip_analysis(&self, clip: ClipId) -> Option<Arc<Analysis>> {
        let Some(ClipData::Audio(audio)) = self.editor.document().clip(clip).map(|c| &c.data)
        else {
            return None;
        };
        self.analysis
            .as_ref()?
            .get(bl_media::ContentHash::from_bytes(audio.media.0))
    }

    /// An audio clip's transients with its edits applied: those of its
    /// file's analysis when that is ready, and those put in by hand. A clip
    /// with no edits of its own shares the analysis's list, so asking every
    /// frame copies nothing.
    #[must_use]
    pub fn clip_transients(&self, clip: ClipId) -> TransientList {
        let Some(ClipData::Audio(audio)) = self.editor.document().clip(clip).map(|c| &c.data)
        else {
            return TransientList::default();
        };
        let detected = self.clip_analysis(clip);
        if audio.transient_edits.is_empty() {
            return TransientList {
                shared: detected,
                edited: Vec::new(),
            };
        }
        TransientList {
            shared: None,
            edited: audio
                .transient_edits
                .apply(detected.as_deref().map_or(&[][..], |a| &a.transients)),
        }
    }

    /// Completes a key's or menu item's command with what the Sample Editor
    /// has selected, and says which command that is: Insert Warp Marker
    /// with a time selection puts markers at its transients (R25).
    pub(super) fn complete_sample_command(
        &self,
        id: &str,
        args: Value,
    ) -> Result<(String, Value), ShellError> {
        let unavailable = |reason: &str| ShellError::Unavailable {
            command: id.to_owned(),
            reason: reason.to_owned(),
        };
        let doc = self.editor.document();
        let shown = self
            .clip_view
            .shown_audio_clip(doc, &self.arrangement)
            .ok_or_else(|| unavailable("no audio clip is shown in the Clip View"))?;
        let Some(ClipData::Audio(audio)) = doc.clip(shown).map(|c| &c.data) else {
            return Err(unavailable("no audio clip is shown in the Clip View"));
        };
        let sample = self.clip_view.sample();
        let own = sample.clip() == Some(shown);
        let selection = own.then(|| sample.time_selection()).flatten();
        let insert = own.then(|| sample.insert_marker()).flatten();
        let warped = || {
            audio
                .warp
                .then(|| audio.warp_map().ok())
                .flatten()
                .ok_or_else(|| unavailable("the clip isn't warped: switch Warp on first"))
        };
        let mut fields = match args {
            Value::Object(fields) => fields,
            _ => serde_json::Map::new(),
        };
        let command = match id {
            "clip.insert_warp_marker" => {
                let map = warped()?;
                if let Some((from, to)) = selection {
                    fields.insert("clip".to_owned(), json!(shown));
                    fields.insert(
                        "from".to_owned(),
                        json!(Ticks(map.time_to_beat(from).round() as i64)),
                    );
                    fields.insert(
                        "to".to_owned(),
                        json!(Ticks(map.time_to_beat(to).round() as i64)),
                    );
                    "clip.insert_warp_markers"
                } else if let Some(time) = insert {
                    fields.insert(
                        "clips".to_owned(),
                        json!(self.sample_edit_clips(shown, &[])),
                    );
                    fields.insert("time".to_owned(), json!(time));
                    id
                } else {
                    return Err(unavailable(
                        "click the waveform to put the insert marker there first",
                    ));
                }
            }
            "clip.delete_warp_markers" => {
                warped()?;
                let places: Vec<u32> = if own && !sample.markers().is_empty() {
                    sample.markers().to_vec()
                } else if let Some((from, to)) = selection {
                    audio
                        .warp_markers
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| m.time >= from && m.time <= to)
                        .filter_map(|(place, _)| u32::try_from(place).ok())
                        .collect()
                } else {
                    Vec::new()
                };
                if places.is_empty() {
                    return Err(unavailable(
                        "select a warp marker, or a span that holds one",
                    ));
                }
                fields.insert(
                    "clips".to_owned(),
                    json!(self.sample_edit_clips(shown, &places)),
                );
                fields.insert("markers".to_owned(), json!(places));
                id
            }
            "clip.insert_transient" => {
                let time = insert.ok_or_else(|| {
                    unavailable("click the waveform to put the insert marker there first")
                })?;
                fields.insert("clip".to_owned(), json!(shown));
                fields.insert("time".to_owned(), json!(time));
                id
            }
            "clip.quantize_audio" => {
                let map = warped()?;
                fields.insert("clip".to_owned(), json!(shown));
                if let Some((from, to)) = selection {
                    fields.insert(
                        "from".to_owned(),
                        json!(Ticks(map.time_to_beat(from).round() as i64)),
                    );
                    fields.insert(
                        "to".to_owned(),
                        json!(Ticks(map.time_to_beat(to).round() as i64)),
                    );
                }
                id
            }
            _ => {
                // A selected transient an edit has since moved or deleted is
                // no longer there to act on.
                let current = self.clip_transients(shown);
                let selected: Vec<f64> = if own {
                    sample
                        .transients()
                        .iter()
                        .copied()
                        .filter(|t| current.contains(t))
                        .collect()
                } else {
                    Vec::new()
                };
                let times: Vec<f64> = if !selected.is_empty() {
                    selected
                } else if let Some((from, to)) = selection {
                    current
                        .iter()
                        .copied()
                        .filter(|&t| t >= from && t <= to)
                        .collect()
                } else {
                    Vec::new()
                };
                if times.is_empty() {
                    return Err(unavailable("select a transient, or a span that holds one"));
                }
                fields.insert("clip".to_owned(), json!(shown));
                fields.insert("times".to_owned(), json!(times));
                id
            }
        };
        Ok((command.to_owned(), Value::Object(fields)))
    }

    /// A frame of a drag in the Sample Editor, checked against the files and
    /// with the transients it needs filled in, as `Shell::run` does for the
    /// same command (warping R11, R26).
    pub(super) fn prepare_gesture_step(
        &mut self,
        step: &Invocation,
    ) -> Result<Invocation, ShellError> {
        if !GESTURE_WARP.contains(&step.id.as_str()) {
            return Ok(step.clone());
        }
        self.check_warp_files(&step.id, &step.args)?;
        let args = self.prepare_warping(&step.id, step.args.clone())?;
        Ok(Invocation::new(&step.id, args))
    }
}
