//! The set file format: a zstd frame holding one JSON object.
//!
//! ```text
//! zstd frame (with its content checksum)
//! └─ {
//!      "format": "beatlayer-set",
//!      "schema": 1,              the schema version
//!      "revision": 7,            the number of saves so far
//!      "saved_by": "Beatlayer 0.1.0",
//!      "ids": 412,               the next object ID to hand out
//!      "media": [ { "hash": "…", "path": "Samples/Imported/Kick.wav", "size": 88244 } ],
//!      "windows": { "12": { "x": 80, "y": 120, "width": 320, "height": 174 } },
//!      "folds": [ 12, 15 ],     the clips whose note editors are folded
//!      "document": { … }         the song, as bl-model serialises it
//!    }
//! ```
//!
//! `windows` holds where each floating device's window sits (WP-2.5b), and
//! `folds` the clips whose MIDI Note Editors are folded (WP-2.3). Both are
//! left out when empty. They sit beside the document rather than in it, so
//! moving a window or folding an editor is never an undo step.
//!
//! `format` and `schema` come first and keep their names in every version,
//! so a reader can tell what a file is and which migrations it needs before
//! reading the rest. Anything else may change from one schema version to
//! the next (see [`crate::migrate`]).
//!
//! A frame is self-contained, so the journal (IMP-05, WP-1.11) appends its
//! own frames to a separate file beside the set, each naming the `revision`
//! it follows, without changing what a normal save writes (see
//! [`crate::journal`]).

use std::io::Read;

use bl_core::id::IdAllocator;
use bl_model::{ClipFolds, DeviceWindows, Document, Editor, LaunchSettings};
use ruzstd::decoding::StreamingDecoder;
use ruzstd::encoding::{CompressionLevel, compress_to_vec};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{LoadError, SaveError};
use crate::media::Media;
use crate::migrate::{self, MIGRATIONS, Migration, SCHEMA_VERSION};

/// The `format` field of every set file.
pub const FORMAT: &str = "beatlayer-set";

/// What a set file says saved it.
pub const SAVED_BY: &str = concat!("Beatlayer ", env!("CARGO_PKG_VERSION"));

/// The first bytes of a zstd frame.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

/// Everything a set file holds about the song: what a save writes and a load
/// gives back.
#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    /// The song.
    pub document: Document,
    /// The project's ID allocator, so IDs stay unique after a reload.
    pub ids: IdAllocator,
    /// Where the media files the clips use are.
    pub media: Media,
    /// Where each floating device's window sits (WP-2.5b): saved with the
    /// set, but not part of the document, so it's no undo step.
    pub windows: DeviceWindows,
    /// Which clips' MIDI Note Editors are folded to the rows that hold notes
    /// (WP-2.3): saved with the set, but not part of the document, so
    /// folding is no undo step.
    pub folds: ClipFolds,
    /// The global launch quantisation and its enabled switch (WP-3.2),
    /// saved with the set but not part of the document, so changing them is
    /// no undo step (the launch quantisation spec, R1).
    pub launch: LaunchSettings,
}

impl Project {
    /// A new, empty song with no media.
    #[must_use]
    pub fn new() -> Self {
        Self::from_editor(&Editor::new(), Media::new())
    }

    /// The editor's current document and ID allocator, with this media
    /// table.
    #[must_use]
    pub fn from_editor(editor: &Editor, media: Media) -> Self {
        Self {
            document: editor.document().clone(),
            ids: editor.ids(),
            media,
            windows: DeviceWindows::new(),
            folds: ClipFolds::new(),
            launch: LaunchSettings::default(),
        }
    }

    /// The same project with these window frames, keeping only those of
    /// devices its document has.
    #[must_use]
    pub fn with_windows(mut self, windows: &DeviceWindows) -> Self {
        self.windows = windows
            .iter()
            .filter(|(device, frame)| self.document.device(**device).is_some() && frame.is_valid())
            .map(|(device, frame)| (*device, *frame))
            .collect();
        self
    }

    /// The same project with these folded clips, keeping only those its
    /// document has.
    #[must_use]
    pub fn with_folds(mut self, folds: &ClipFolds) -> Self {
        self.folds = folds
            .iter()
            .filter(|clip| self.document.clip(**clip).is_some())
            .copied()
            .collect();
        self
    }

    /// An editor for the song, with an empty undo history.
    #[must_use]
    pub fn editor(&self) -> Editor {
        Editor::from_document(self.document.clone(), self.ids)
    }

    /// The same project with this launch quantisation.
    #[must_use]
    pub fn with_launch(mut self, launch: LaunchSettings) -> Self {
        self.launch = launch;
        self
    }
}

impl Default for Project {
    fn default() -> Self {
        Self::new()
    }
}

/// What a set file says about itself, apart from the song.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetInfo {
    /// How many times the set has been saved: 1 after the first save, and up
    /// by one with each save after that.
    pub revision: u64,
    /// The program and version that saved it.
    pub saved_by: String,
    /// The schema version the file was written in, or the Save Continuously
    /// save point it opened at (the song's own form). If it's older than
    /// [`SCHEMA_VERSION`], the file was migrated as it loaded, and saving it
    /// writes the current version.
    pub schema: u32,
}

impl SetInfo {
    /// Whether the set was saved before relative modulation, so its clip
    /// envelopes hold parameter units that
    /// [`Document::relative_clip_envelopes`](bl_model::Document::relative_clip_envelopes)
    /// turns into modulation (the modulation spec, R5).
    #[must_use]
    pub fn before_relative_modulation(&self) -> bool {
        self.schema < 5
    }
}

/// Limits and settings for loading a set.
#[derive(Clone, Copy, Debug)]
pub struct LoadOptions {
    /// The most JSON a set may hold, in bytes. Loading stops with
    /// [`LoadError::TooLarge`] past it, so a small damaged or hostile file
    /// can't make the loader decompress without end.
    pub max_json_bytes: usize,
    /// The migrations to run on files from older schema versions:
    /// [`MIGRATIONS`] unless a test or tool says otherwise.
    pub migrations: &'static [Migration],
}

impl LoadOptions {
    /// The default limit on a set's JSON: 512 MiB, far beyond any real song.
    pub const DEFAULT_MAX_JSON_BYTES: usize = 512 << 20;
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            max_json_bytes: Self::DEFAULT_MAX_JSON_BYTES,
            migrations: MIGRATIONS,
        }
    }
}

/// The saved form, borrowed for writing.
#[derive(Serialize)]
struct Saving<'a> {
    format: &'static str,
    schema: u32,
    revision: u64,
    saved_by: &'static str,
    ids: IdAllocator,
    media: &'a Media,
    #[serde(skip_serializing_if = "DeviceWindows::is_empty")]
    windows: &'a DeviceWindows,
    #[serde(skip_serializing_if = "ClipFolds::is_empty")]
    folds: &'a ClipFolds,
    /// The global launch quantisation and its switch (WP-3.2).
    launch: LaunchSettings,
    document: &'a Document,
}

impl<'a> Saving<'a> {
    fn of(project: &'a Project, revision: u64) -> Self {
        Self {
            format: FORMAT,
            schema: SCHEMA_VERSION,
            revision,
            saved_by: SAVED_BY,
            ids: project.ids,
            media: &project.media,
            windows: &project.windows,
            folds: &project.folds,
            launch: project.launch,
            document: &project.document,
        }
    }
}

/// The saved form, owned for reading. Field for field the same as
/// [`Saving`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    #[expect(dead_code, reason = "checked by the probe before this is read")]
    format: String,
    #[expect(dead_code, reason = "checked by the probe before this is read")]
    schema: u32,
    revision: u64,
    saved_by: String,
    ids: IdAllocator,
    media: Media,
    #[serde(default)]
    windows: DeviceWindows,
    #[serde(default)]
    folds: ClipFolds,
    /// The global launch quantisation and its switch (WP-3.2). Sets saved
    /// before WP-3.2 load with a new set's.
    #[serde(default)]
    launch: LaunchSettings,
    document: Document,
}

/// The two fields every version keeps, read before anything else.
#[derive(Deserialize)]
struct Probe {
    format: Option<Value>,
    schema: Option<Value>,
}

/// The JSON a save writes, before compression.
///
/// # Errors
///
/// [`SaveError::Encode`] if the project can't be written as JSON, which
/// shouldn't happen for any project the model accepts.
pub fn encode_json(project: &Project, revision: u64) -> Result<Vec<u8>, SaveError> {
    serde_json::to_vec(&Saving::of(project, revision)).map_err(|error| SaveError::Encode {
        reason: error.to_string(),
    })
}

/// The bytes of a set file for `project`, saved as revision `revision`.
///
/// The same project and revision always give the same bytes.
///
/// # Errors
///
/// As [`encode_json`].
pub fn encode(project: &Project, revision: u64) -> Result<Vec<u8>, SaveError> {
    Ok(compress(&encode_json(project, revision)?))
}

/// Compresses bytes into one zstd frame with a content checksum, as a set
/// file holds its JSON.
#[must_use]
pub fn compress(bytes: &[u8]) -> Vec<u8> {
    compress_to_vec(bytes, CompressionLevel::Fastest)
}

/// Reads a set file's bytes.
///
/// # Errors
///
/// - [`LoadError::NotASet`] if the bytes aren't a zstd frame, or the JSON
///   inside doesn't say it's a set.
/// - [`LoadError::Corrupt`] if the frame or the JSON is broken.
/// - [`LoadError::TooLarge`] if the JSON is over the limit.
/// - [`LoadError::TooNew`], [`LoadError::TooOld`], or [`LoadError::Migration`]
///   if the schema version can't be brought up to date.
/// - [`LoadError::Invalid`] if the JSON breaks a rule.
pub fn decode(bytes: &[u8], options: &LoadOptions) -> Result<(Project, SetInfo), LoadError> {
    let json = decompress(bytes, options.max_json_bytes)?;
    decode_json(&json, options)
}

/// Reads a set file's JSON, as [`decode`] does after decompressing it.
///
/// # Errors
///
/// As [`decode`], apart from the errors about compression.
pub fn decode_json(json: &[u8], options: &LoadOptions) -> Result<(Project, SetInfo), LoadError> {
    if json.len() > options.max_json_bytes {
        return Err(LoadError::TooLarge {
            limit: options.max_json_bytes,
        });
    }
    // A JSON object, not an array that serde would also read as a struct.
    if json.trim_ascii_start().first() != Some(&b'{') {
        return Err(LoadError::NotASet);
    }
    let probe: Probe = serde_json::from_slice(json).map_err(|e| LoadError::from_json(&e))?;
    if probe.format.as_ref().and_then(Value::as_str) != Some(FORMAT) {
        return Err(LoadError::NotASet);
    }
    let schema = probe
        .schema
        .as_ref()
        .and_then(Value::as_u64)
        .and_then(|schema| u32::try_from(schema).ok())
        .ok_or_else(|| LoadError::invalid("the schema version isn't a whole number"))?;
    if schema > SCHEMA_VERSION {
        return Err(LoadError::TooNew {
            found: schema,
            newest: SCHEMA_VERSION,
        });
    }
    let saved: Saved = if schema == SCHEMA_VERSION {
        serde_json::from_slice(json).map_err(|e| LoadError::from_json(&e))?
    } else {
        let value: Value = serde_json::from_slice(json).map_err(|e| LoadError::from_json(&e))?;
        let value = migrate::run(value, schema, options.migrations)?;
        Saved::deserialize(value).map_err(|e| LoadError::from_json(&e))?
    };
    let Saved {
        revision,
        saved_by,
        mut ids,
        media,
        windows,
        folds,
        launch,
        document,
        ..
    } = saved;
    // The allocator must be able to hand out at least one more ID, and
    // never one the document already uses.
    let max_id = document.max_id();
    if max_id.get() == u64::MAX {
        return Err(LoadError::invalid(
            "the document's object IDs are exhausted",
        ));
    }
    ids.skip_past(max_id);
    if ids.peek().get() == u64::MAX {
        return Err(LoadError::invalid("the set's object IDs are exhausted"));
    }
    // A frame that's damaged, or for a device that isn't there, is
    // dropped: the window opens where the app puts a new one. So is a
    // fold for a clip that isn't there.
    let project = Project {
        document,
        ids,
        media,
        windows: DeviceWindows::new(),
        folds: ClipFolds::new(),
        launch,
    }
    .with_windows(&windows)
    .with_folds(&folds);
    let info = SetInfo {
        revision,
        saved_by,
        schema,
    };
    Ok((project, info))
}

/// Decompresses a set file's one zstd frame, checking its checksum and that
/// nothing follows it.
fn decompress(bytes: &[u8], limit: usize) -> Result<Vec<u8>, LoadError> {
    let mut rest = bytes;
    let json = decompress_frame(&mut rest, limit)?;
    if !rest.is_empty() {
        return Err(LoadError::corrupt(format_args!(
            "{} bytes follow the compressed data",
            rest.len()
        )));
    }
    Ok(json)
}

/// Decompresses the zstd frame at the start of `rest`, checking its
/// checksum, and moves `rest` past it. The continuous-save journal is a run
/// of these frames.
pub(crate) fn decompress_frame(rest: &mut &[u8], limit: usize) -> Result<Vec<u8>, LoadError> {
    if !rest.starts_with(&ZSTD_MAGIC) {
        return Err(LoadError::NotASet);
    }
    let mut decoder = StreamingDecoder::new(&mut *rest).map_err(LoadError::corrupt)?;
    let mut json = Vec::new();
    let most = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    (&mut decoder)
        .take(most)
        .read_to_end(&mut json)
        .map_err(LoadError::corrupt)?;
    if json.len() > limit {
        return Err(LoadError::TooLarge { limit });
    }
    let frame = &decoder.decoder;
    if !frame.is_finished() {
        return Err(LoadError::corrupt("the compressed data ends early"));
    }
    if let Some(stored) = frame.get_checksum_from_data()
        && frame.get_calculated_checksum() != Some(stored)
    {
        return Err(LoadError::corrupt("the checksum doesn't match"));
    }
    Ok(json)
}

/// The saved form of a project as a JSON value: what [`encode_json`] writes,
/// before it's turned into text. The journal diffs these.
pub(crate) fn to_value(project: &Project, revision: u64) -> Result<Value, SaveError> {
    serde_json::to_value(Saving::of(project, revision)).map_err(|error| SaveError::Encode {
        reason: error.to_string(),
    })
}

/// Reads a saved form held as a JSON value, with every check
/// [`decode_json`] makes.
pub(crate) fn decode_value(
    value: &Value,
    options: &LoadOptions,
) -> Result<(Project, SetInfo), LoadError> {
    let json = serde_json::to_vec(value).map_err(LoadError::corrupt)?;
    decode_json(&json, options)
}

#[cfg(test)]
mod tests {
    use bl_core::id::Id;
    use bl_model::{Command, TrackKind};
    use serde_json::json;

    use super::*;

    fn song() -> Project {
        let mut editor = Editor::new();
        let id = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind: TrackKind::Audio,
                name: "Drums".to_owned(),
                parent: None,
                index: 0,
            })
            .unwrap();
        Project::from_editor(&editor, Media::new())
    }

    fn json_of(project: &Project, revision: u64) -> Value {
        serde_json::from_slice(&encode_json(project, revision).unwrap()).unwrap()
    }

    fn load_json(value: &Value) -> Result<(Project, SetInfo), LoadError> {
        decode_json(&serde_json::to_vec(value).unwrap(), &LoadOptions::default())
    }

    #[test]
    fn encodes_and_decodes() {
        let project = song();
        let bytes = encode(&project, 3).unwrap();
        assert!(bytes.starts_with(&ZSTD_MAGIC));
        let (loaded, info) = decode(&bytes, &LoadOptions::default()).unwrap();
        assert_eq!(loaded, project);
        assert_eq!(
            info,
            SetInfo {
                revision: 3,
                saved_by: SAVED_BY.to_owned(),
                schema: SCHEMA_VERSION
            }
        );
        // The output depends only on the input.
        assert_eq!(encode(&project, 3).unwrap(), bytes);
    }

    // Spec: device-chains R29
    #[test]
    fn floating_windows_are_saved_beside_the_document() {
        let mut editor = song().editor();
        let track = editor.document().top_level().next().unwrap();
        let device: bl_model::DeviceId = editor.new_id();
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device: bl_model::Device::new(
                    device,
                    "Utility",
                    bl_model::DeviceKind::Native {
                        device_type: "beatlayer.utility".to_owned(),
                    },
                ),
            })
            .unwrap();
        editor
            .apply(Command::SetDeviceFloating {
                device,
                floating: true,
            })
            .unwrap();
        let frame = bl_model::WindowFrame {
            x: 80.0,
            y: 120.5,
            width: 320.0,
            height: 174.0,
        };
        let gone = bl_model::DeviceId(Id::new(9_999).unwrap());
        let windows = DeviceWindows::from([(device, frame), (gone, frame)]);
        let project = Project::from_editor(&editor, Media::new()).with_windows(&windows);
        // Only the frames of devices the set has are kept.
        assert_eq!(project.windows, DeviceWindows::from([(device, frame)]));
        let value = json_of(&project, 1);
        assert_eq!(
            value["windows"][device.0.get().to_string()],
            json!({ "x": 80.0, "y": 120.5, "width": 320.0, "height": 174.0 })
        );
        let (loaded, _) = load_json(&value).unwrap();
        assert_eq!(loaded, project);
        assert!(loaded.document.device(device).unwrap().floating);
        // No floating windows, no `windows`: older sets read the same.
        assert!(json_of(&song(), 1).get("windows").is_none());
        // A damaged frame, or one for a device that isn't there, is dropped.
        let mut damaged = value.clone();
        damaged["windows"][device.0.get().to_string()]["width"] = json!(-1.0);
        damaged["windows"]["9999"] = json!({ "x": 0, "y": 0, "width": 10, "height": 10 });
        assert!(load_json(&damaged).unwrap().0.windows.is_empty());
    }

    #[test]
    fn format_and_schema_come_first() {
        let json = encode_json(&song(), 1).unwrap();
        let text = String::from_utf8(json).unwrap();
        let start =
            format!(r#"{{"format":"beatlayer-set","schema":{SCHEMA_VERSION},"revision":1,"#);
        assert!(text.starts_with(&start), "{text}");
    }

    #[test]
    fn other_files_are_not_sets() {
        let options = LoadOptions::default();
        for bytes in [&b""[..], b"{}", b"PK\x03\x04", b"\x28\xb5\x2f"] {
            assert!(matches!(decode(bytes, &options), Err(LoadError::NotASet)));
        }
        // Compressed JSON that isn't a set.
        for json in [
            &b"{}"[..],
            br#"{"format":"other","schema":1}"#,
            b"[1]",
            b"7",
        ] {
            let bytes = compress_to_vec(json, CompressionLevel::Fastest);
            assert!(
                matches!(decode(&bytes, &options), Err(LoadError::NotASet)),
                "{}",
                String::from_utf8_lossy(json)
            );
        }
    }

    #[test]
    fn damage_is_detected() {
        let options = LoadOptions::default();
        let bytes = encode(&song(), 1).unwrap();

        let truncated = &bytes[..bytes.len() - 3];
        assert!(matches!(
            decode(truncated, &options),
            Err(LoadError::Corrupt { .. })
        ));

        let mut trailing = bytes.clone();
        trailing.extend_from_slice(b"extra");
        let error = decode(&trailing, &options).unwrap_err();
        assert!(error.to_string().contains("5 bytes follow"), "{error}");

        // Flip every bit of the checksum, then one bit of each byte in turn:
        // none loads as a different set.
        let mut bad_checksum = bytes.clone();
        let end = bad_checksum.len();
        for byte in &mut bad_checksum[end - 4..] {
            *byte = !*byte;
        }
        let error = decode(&bad_checksum, &options).unwrap_err();
        assert!(error.to_string().contains("checksum"), "{error}");
        let original = song();
        for at in 4..bytes.len() {
            let mut damaged = bytes.clone();
            damaged[at] ^= 0x20;
            if let Ok((project, _)) = decode(&damaged, &options) {
                assert_eq!(project, original, "damage at byte {at} loaded");
            }
        }

        let json = compress_to_vec(
            &br#"{"format":"beatlayer-set","schema":1,"#[..],
            CompressionLevel::Fastest,
        );
        assert!(matches!(
            decode(&json, &options),
            Err(LoadError::Corrupt { .. })
        ));
    }

    #[test]
    fn the_json_size_is_limited() {
        let bytes = encode(&song(), 1).unwrap();
        let json = encode_json(&song(), 1).unwrap();
        let exact = LoadOptions {
            max_json_bytes: json.len(),
            ..LoadOptions::default()
        };
        decode(&bytes, &exact).unwrap();
        decode_json(&json, &exact).unwrap();
        let short = LoadOptions {
            max_json_bytes: json.len() - 1,
            ..LoadOptions::default()
        };
        assert!(matches!(
            decode(&bytes, &short),
            Err(LoadError::TooLarge { .. })
        ));
        assert!(matches!(
            decode_json(&json, &short),
            Err(LoadError::TooLarge { .. })
        ));

        // A small file that would decompress to 16 MiB of zeros.
        let bomb = compress_to_vec(std::io::repeat(0).take(16 << 20), CompressionLevel::Fastest);
        assert!(bomb.len() < 16 << 10, "{} bytes", bomb.len());
        let small = LoadOptions {
            max_json_bytes: 1 << 20,
            ..LoadOptions::default()
        };
        assert!(matches!(
            decode(&bomb, &small),
            Err(LoadError::TooLarge { limit }) if limit == 1 << 20
        ));
    }

    #[test]
    fn schema_versions_are_checked() {
        let good = json_of(&song(), 1);

        let mut newer = good.clone();
        newer["schema"] = json!(SCHEMA_VERSION + 1);
        assert!(matches!(
            load_json(&newer),
            Err(LoadError::TooNew { found, newest }) if found == SCHEMA_VERSION + 1 && newest == SCHEMA_VERSION
        ));

        let mut older = good.clone();
        older["schema"] = json!(0);
        assert!(matches!(
            load_json(&older),
            Err(LoadError::TooOld { found: 0, .. })
        ));

        for bad in [
            json!(null),
            json!("1"),
            json!(-1),
            json!(1.5),
            json!(u64::MAX),
        ] {
            let mut value = good.clone();
            value["schema"] = bad.clone();
            assert!(
                matches!(load_json(&value), Err(LoadError::Invalid { .. })),
                "{bad}"
            );
        }
        let mut missing = good;
        missing.as_object_mut().unwrap().remove("schema");
        assert!(matches!(
            load_json(&missing),
            Err(LoadError::Invalid { .. })
        ));
    }

    #[test]
    fn rules_are_checked() {
        let good = json_of(&song(), 1);
        let broken = |edit: &dyn Fn(&mut Value)| {
            let mut value = good.clone();
            edit(&mut value);
            load_json(&value).unwrap_err()
        };
        let invalid = |error: LoadError| match error {
            LoadError::Invalid { reason } => reason,
            other => panic!("{other:?}"),
        };

        let reason = invalid(broken(&|v| {
            v.as_object_mut().unwrap().insert("extra".into(), json!(1));
        }));
        assert!(reason.contains("unknown field `extra`"), "{reason}");
        let reason = invalid(broken(&|v| {
            v.as_object_mut().unwrap().remove("ids");
        }));
        assert!(reason.contains("missing field `ids`"), "{reason}");
        let reason = invalid(broken(&|v| v["document"]["main"]["kind"] = json!("audio")));
        assert!(reason.contains("Main track"), "{reason}");
        let reason = invalid(broken(&|v| v["revision"] = json!(-1)));
        assert!(
            reason.contains("revision") || reason.contains("-1"),
            "{reason}"
        );
        let reason = invalid(broken(&|v| {
            v["media"] = json!([{"hash": "00".repeat(32), "path": "/abs.wav", "size": 1}]);
        }));
        assert!(reason.contains("relative media path"), "{reason}");

        // JSON that isn't well formed is damage, not a broken rule.
        let mut text = encode_json(&song(), 1).unwrap();
        text.pop();
        assert!(matches!(
            decode_json(&text, &LoadOptions::default()),
            Err(LoadError::Corrupt { .. })
        ));
    }

    #[test]
    fn id_allocators_are_brought_past_the_document() {
        let project = song();
        let mut value = json_of(&project, 1);
        value["ids"] = json!(1);
        let (loaded, _) = load_json(&value).unwrap();
        assert_eq!(loaded.ids.peek().get(), project.document.max_id().get() + 1);
        assert_eq!(loaded.document, project.document);

        // An allocator ahead of the document keeps its gap: IDs handed out
        // and not used stay used.
        let gap = project.document.max_id().get() + 5;
        let mut ahead = json_of(&project, 1);
        ahead["ids"] = json!(gap);
        let (loaded, _) = load_json(&ahead).unwrap();
        assert_eq!(loaded.ids.peek().get(), gap);

        // An allocator that could never hand out another ID is refused, and so
        // is a document that uses the last ID.
        value["ids"] = json!(u64::MAX);
        assert!(matches!(load_json(&value), Err(LoadError::Invalid { .. })));
        let mut last = json_of(&project, 1);
        last["document"]["main"]["id"] = json!(u64::MAX);
        let error = load_json(&last).unwrap_err();
        assert!(error.to_string().contains("exhausted"), "{error}");
        last["document"]["main"]["id"] = json!(u64::MAX - 1);
        let error = load_json(&last).unwrap_err();
        assert!(error.to_string().contains("exhausted"), "{error}");
        last["document"]["main"]["id"] = json!(u64::MAX - 2);
        let (loaded, _) = load_json(&last).unwrap();
        assert_eq!(loaded.ids.peek(), Id::new(u64::MAX - 1).unwrap());
        // The editor can still hand out that one.
        let mut editor = loaded.editor();
        let id: Id = editor.new_id();
        assert_eq!(id.get(), u64::MAX - 1);
    }
}
