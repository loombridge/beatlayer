//! WP-6.36's acceptance in a render: the set a DAWproject file opens as plays
//! the same samples as the same set built by commands. The file is
//! `bitwig-shaped`, which holds a nested warped audio clip that loops, an
//! unwarped loop, a group, a return fed by a send and a track routed into
//! another. The expected set is written here from the DAWproject spec, apart
//! from the importer's code: each clip is worked out from the file's times.

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

use bl_cli::golden::SAMPLE_RATE;
use bl_cli::render::{self, Cue, Script, Settings};
use bl_core::time::Ticks;
use bl_engine::MediaMap;
use bl_interop::dawproject::{self, Context, Imported};
use bl_model::{
    AudioData, Clip, ClipData, Command, ContentHash, Document, Editor, Input, Monitor, Output,
    TrackId, TrackKind, WarpMarker, WarpMode,
};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/dawproject")
        .join(name)
}

/// The file, imported with Re-Pitch as the Default Warp Mode.
fn import() -> Imported {
    import_of("bitwig-shaped")
}

/// A fixture, imported with Re-Pitch as the Default Warp Mode.
fn import_of(name: &str) -> Imported {
    let bytes = std::fs::read(fixture(&format!("{name}.dawproject"))).unwrap();
    let container = dawproject::read_container(&bytes).unwrap();
    let folder = fixture("");
    let mut context = Context::new(&folder, &folder);
    context.default_warp_mode = WarpMode::RePitch;
    dawproject::import(&container, &context).unwrap()
}

/// The media the container holds, decoded, and the hash of each by its path.
fn media(imported: &Imported) -> (MediaMap, impl Fn(&str) -> ContentHash + '_) {
    let mut map = MediaMap::new();
    for file in &imported.files {
        let bytes = file.bytes.clone().expect("the container holds it");
        let decoded = bl_media::Decoder::from_reader(Cursor::new(bytes), Some("wav"))
            .and_then(bl_media::Decoder::decode_to_end)
            .expect("the file decodes");
        map.insert(file.hash, Arc::new(decoded));
    }
    let hash = |path: &str| {
        imported
            .files
            .iter()
            .find(|file| file.path == path)
            .unwrap_or_else(|| panic!("no file at {path}"))
            .hash
    };
    (map, hash)
}

struct Built {
    editor: Editor,
}

impl Built {
    fn apply(&mut self, command: Command) {
        self.editor.apply(command).expect("the commands are valid");
    }

    fn track(&mut self, kind: TrackKind, name: &str, parent: Option<TrackId>) -> TrackId {
        let document = self.editor.document();
        let index = match (kind, parent) {
            (TrackKind::Return, _) => document.returns().len(),
            (_, Some(group)) => document.track(group).unwrap().children().len(),
            (_, None) => document.top_level().len(),
        };
        let id = self.editor.new_id();
        self.apply(Command::CreateTrack {
            id,
            kind,
            name: name.to_owned(),
            parent,
            index,
        });
        id
    }

    fn clip(&mut self, track: TrackId, start: Ticks, length: Ticks, name: &str, audio: AudioData) {
        let id = self.editor.new_id();
        let mut clip = Clip::new(id, start, length, ClipData::Audio(audio));
        clip.name = name.to_owned();
        self.apply(Command::CreateClip { track, clip });
    }
}

/// The set the file describes, built by commands from its times: 120 BPM, so
/// a beat is half a second.
fn built(hash: impl Fn(&str) -> ContentHash) -> Document {
    let q = Ticks::QUARTER;
    let mut set = Built {
        editor: Editor::new(),
    };
    let group = set.track(TrackKind::Group, "Group", None);
    set.track(TrackKind::Midi, "Kick", Some(group));
    set.track(TrackKind::Midi, "Snare", Some(group));
    set.track(TrackKind::Midi, "Hybrid", None);
    let hybrid = set.track(TrackKind::Audio, "Hybrid Audio", None);
    let drums = set.track(TrackKind::Audio, "Drums", None);
    let source = set.track(TrackKind::Audio, "Source", None);
    let bus = set.track(TrackKind::Audio, "Bus", None);
    let reverb = set.track(TrackKind::Return, "Reverb", None);

    // The outer clip plays eight beats of a four-beat loop, so its two
    // warped events play twice: events A and B, each a file second warped
    // over two and four beats, cut to the two beats the clips last.
    let warped = |end_beat: i64| AudioData {
        warp: true,
        warp_markers: vec![
            WarpMarker {
                time: 0.0,
                beat: Ticks::ZERO,
            },
            WarpMarker {
                time: 1.0,
                beat: q * end_beat,
            },
        ],
        tempo: Some(end_beat as f64 * 60.0),
        warp_mode: WarpMode::RePitch,
        ..AudioData::new(hash("audio/loop.wav"))
    };
    // The outer clip fades in over half a beat at its start and out over
    // half a beat at its end: a quarter of a second each, at 120 BPM, on the
    // first and the last of its clips (R33).
    for pass in 0..2 {
        let at = q * 4 * pass;
        let first = AudioData {
            fade_in: if pass == 0 { 0.25 } else { 0.0 },
            ..warped(2)
        };
        let last = AudioData {
            fade_out: if pass == 1 { 0.25 } else { 0.0 },
            ..warped(4)
        };
        set.clip(hybrid, at, q * 2, "", first);
        set.clip(hybrid, at + q * 2, q * 2, "", last);
    }

    // An unwarped clip two seconds long that loops the first half second of
    // its file plays that half second four times in a row.
    for pass in 0..4 {
        set.clip(
            drums,
            q * pass,
            q,
            "Beat",
            AudioData {
                end: Some(0.5),
                ..AudioData::new(hash("audio/drums.wav"))
            },
        );
    }
    set.clip(
        source,
        Ticks::ZERO,
        q * 4,
        "Tone",
        AudioData::new(hash("audio/source.wav")),
    );

    set.apply(Command::SetSend {
        track: drums,
        to: reverb,
        level: 0.5,
    });
    set.apply(Command::SetOutput {
        track: source,
        output: Output::Track { track: bus },
    });
    set.apply(Command::SetMonitor {
        track: bus,
        monitor: Monitor::In,
    });
    set.apply(Command::SetInput {
        track: bus,
        input: Input::None,
        channel: None,
    });
    set.editor.document().clone()
}

fn render(document: &Document, media: &MediaMap, block: usize) -> Vec<f32> {
    let settings = Settings {
        sample_rate: SAMPLE_RATE,
        block,
        frames: SAMPLE_RATE.seconds_to_frames(5.0).get() as u64,
    };
    let script = Script {
        schedule: &[],
        cues: &[Cue::PLAY],
        launches: &[],
    };
    let mut samples = Vec::new();
    render::render_launching_with_workers(
        document,
        Arc::new(media.clone()),
        &script,
        &settings,
        0,
        |output| render::interleave(output, &mut samples),
    );
    samples
}

/// Spec: dawproject R19, R22, R29 to R34. The imported set, beat fades included, renders to
/// the same samples as the set the spec describes, at two block sizes.
#[test]
fn the_imported_set_renders_the_same_samples_as_the_set_built_by_commands() {
    let imported = import();
    let (media, hash) = media(&imported);
    let expected = built(hash);
    for block in [64, 512] {
        let from_file = render(&imported.project.document, &media, block);
        let from_commands = render(&expected, &media, block);
        assert!(
            from_file.iter().any(|sample| sample.abs() > 0.01),
            "silence"
        );
        assert_eq!(
            from_file.len(),
            from_commands.len(),
            "block size {block}: the renders differ in length"
        );
        let worst = from_file
            .iter()
            .zip(&from_commands)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert_eq!(
            worst, 0.0,
            "block size {block}: the renders differ by {worst}"
        );
    }
}

/// The hash of the one audio file a small fixture holds.
fn tone(imported: &Imported) -> ContentHash {
    imported.files[0].hash
}

/// A set of one audio track `name`, built by commands.
fn one_track(name: &str) -> (Built, TrackId) {
    let mut set = Built {
        editor: Editor::new(),
    };
    let track = set.track(TrackKind::Audio, name, None);
    (set, track)
}

/// How far two renders of a fixture differ, at two block sizes.
fn assert_null(imported: &Imported, expected: &Document) {
    let (media, _) = media(imported);
    for block in [64, 512] {
        let from_file = render(&imported.project.document, &media, block);
        let from_commands = render(expected, &media, block);
        assert!(
            from_file.iter().any(|sample| sample.abs() > 0.01),
            "silence"
        );
        assert_eq!(from_file.len(), from_commands.len());
        let worst = from_file
            .iter()
            .zip(&from_commands)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert_eq!(
            worst, 0.0,
            "block size {block}: the renders differ by {worst}"
        );
    }
}

/// Spec: dawproject R56 to R58. An imported volume ramp renders the same as
/// one drawn by hand.
#[test]
fn an_imported_volume_ramp_renders_the_same_as_a_hand_drawn_one() {
    let imported = import_of("ramp");
    let (mut set, track) = one_track("Ramp");
    let q = Ticks::QUARTER;
    set.clip(
        track,
        Ticks::ZERO,
        q * 2,
        "Tone",
        AudioData::new(tone(&imported)),
    );
    set.apply(Command::SetAutomationEnvelope {
        track,
        target: bl_model::AutomationTarget::Mixer {
            param: bl_model::MixerParam::Volume,
        },
        points: vec![
            bl_model::Breakpoint {
                at: Ticks::ZERO,
                value: 1.0,
                bend: 0.0,
            },
            bl_model::Breakpoint {
                at: q * 2,
                value: 0.25,
                bend: 0.0,
            },
        ],
    });
    assert_null(&imported, set.editor.document());
}

/// Spec: dawproject R33. A crossfade imported from a negative `fadeInTime`
/// equals the one `arrangement.set_crossfade` makes between the same two
/// clips, and renders the same.
#[test]
fn an_imported_crossfade_renders_the_same_as_the_one_the_command_makes() {
    let imported = import_of("crossfade");
    let (mut set, track) = one_track("Cross");
    let q = Ticks::QUARTER;
    set.clip(
        track,
        Ticks::ZERO,
        q * 2,
        "A",
        AudioData::new(tone(&imported)),
    );
    set.clip(track, q * 2, q * 2, "B", AudioData::new(tone(&imported)));
    set.apply(Command::SetCrossfade {
        track,
        boundary: q * 2,
        before: 0.0,
        after: 0.2,
        curve: 0.0,
    });
    let expected = set.editor.document();
    let on = |document: &Document| -> Vec<Clip> {
        let track = document.tracks().find(|t| t.name() == "Cross").unwrap();
        let mut clips: Vec<Clip> = track.clips().cloned().collect();
        clips.sort_by_key(|clip| clip.start);
        clips
    };
    let (made, imported_clips) = (on(expected), on(&imported.project.document));
    for (made, imported) in made.iter().zip(&imported_clips) {
        assert_eq!(made.data, imported.data);
    }
    assert_null(&imported, expected);
}

/// Spec: dawproject R53. A Limiter imported with Threshold −6 dB, InputGain
/// +3 dB and OutputGain +2 dB peaks at −4 dBFS on a sine at full scale.
#[test]
fn an_imported_limiter_peaks_at_its_ceiling() {
    let imported = import_of("limiter");
    let (media, _) = media(&imported);
    let samples = render(&imported.project.document, &media, 512);
    // Past the limiter's lookahead and release at the start.
    let from = samples.len() / 10;
    let peak = samples[from..]
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0f32, f32::max);
    let db = 20.0 * peak.log10();
    assert!((db - -4.0).abs() < 0.1, "the peak is {db} dBFS");
}
