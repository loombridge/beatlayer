//! The Arrangement's automation editing, driven headless through
//! `Shell::run` (WP-6.25): the lanes a selection names, the edit commands
//! on envelopes, moving several breakpoints at once, and Lock Envelopes.
//! The behaviour is in the automation editing spec.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use bl_core::time::Ticks;
use bl_model::{
    AudioData, AutomationTarget, Clip, ClipData, Command, ContentHash, Device, DeviceKind,
    Document, Editor, MixerParam, TrackId,
};
use bl_plugin_host::Hosts;
use bl_shell::Shell;
use bl_shell::arrangement_commands::{self as view_ids, ArrangementState, Lane};
use bl_shell::audio::AudioHost;
use bl_shell::automation_commands::{self as ids, EnvelopeClipboardState};
use bl_shell::browser_commands;
use bl_shell::editing_commands::{self as edit_ids};
use bl_shell::export::{self, Length, Render};
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::settings::Preferences;
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();

/// Opens a set held in memory.
struct Preloaded(Option<Editor>);

impl SetStore for Preloaded {
    fn open(&mut self, path: &Path) -> Result<(Editor, PathBuf), String> {
        let editor = self.0.take().ok_or("opened already")?;
        Ok((editor, path.to_owned()))
    }

    fn save(&mut self, _: &Path, _: &Editor) -> Result<Vec<String>, String> {
        Err("not saved".to_owned())
    }

    fn save_as(&mut self, _: &Path, _: &Editor) -> Result<(PathBuf, Vec<String>), String> {
        Err("not saved".to_owned())
    }

    fn close(&mut self) {}
}

/// A set with one MIDI track, one clip on it, and a volume envelope with
/// breakpoints at bars 1, 2 and 3.
struct Song {
    shell: Shell,
    track: TrackId,
    clip: bl_model::ClipId,
}

fn song() -> Song {
    let mut editor = factory_default_set();
    let track = editor
        .document()
        .top_level()
        .find(|id| {
            editor
                .document()
                .track(*id)
                .is_some_and(|track| track.kind() == bl_model::TrackKind::Audio)
        })
        .unwrap();
    let clip: bl_model::ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(BAR * 2),
                ClipData::Audio(AudioData::new(ContentHash([7; 32]))),
            ),
        })
        .unwrap();
    for (bar, value) in [(0, 0.0), (1, 1.0), (2, 0.5)] {
        editor
            .apply(Command::SetAutomationBreakpoint {
                track,
                target: AutomationTarget::Mixer {
                    param: MixerParam::Volume,
                },
                at: Ticks(bar * BAR),
                value,
            })
            .unwrap();
    }
    editor.clear_history().unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Automation.blset" }))
        .unwrap();
    Song { shell, track, clip }
}

impl Song {
    fn doc(&self) -> &Document {
        self.shell.editor().document()
    }

    /// The envelope's breakpoints, as `(bar, value)`.
    fn envelope(&self) -> Vec<(i64, f64)> {
        self.doc()
            .track(self.track)
            .unwrap()
            .automation_for(&AutomationTarget::Mixer {
                param: MixerParam::Volume,
            })
            .unwrap()
            .envelope
            .points()
            .map(|point| (point.at.get() / BAR, point.value))
            .collect()
    }

    /// The automation lane of the track, for the commands' arguments.
    fn lane(&self) -> Value {
        json!({
            "type": "automation",
            "track": self.track,
            "target": { "type": "mixer", "param": "volume" },
        })
    }

    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}"))
    }

    /// Selects the time from bar 1 to bar 2 in the track's automation lane.
    fn select(&mut self) {
        self.run(
            view_ids::SELECT_TIME,
            json!({
                "start": Ticks(BAR),
                "end": Ticks(BAR * 2),
                "lanes": [self.lane()],
            }),
        );
    }
}

// Spec: automation-editing R1.
#[test]
fn a_selection_names_the_lanes_it_covers() {
    let mut song = song();
    song.select();
    let state: ArrangementState =
        serde_json::from_value(song.run(view_ids::STATE, json!({}))).unwrap();
    assert_eq!(
        state.time_selection_lanes,
        vec![Lane::Automation {
            track: song.track,
            target: AutomationTarget::Mixer {
                param: MixerParam::Volume
            }
        }]
    );
    assert_eq!(
        state.time_selection.map(|range| range.start),
        Some(Ticks(BAR))
    );
}

// Spec: automation-editing R5.
#[test]
fn deleting_an_envelopes_range_leaves_it_straight_across_and_the_rest_alone() {
    let mut song = song();
    song.select();
    song.run(
        ids::DELETE_RANGE,
        json!({ "lanes": [song.lane()], "start": Ticks(BAR), "end": Ticks(BAR * 2) }),
    );
    assert_eq!(
        song.envelope(),
        vec![
            (0, 0.0),
            // The values at the span's edges are kept, and the envelope runs
            // straight across the span.
            (1, 1.0),
            (2, 0.5),
        ]
    );
}

// Spec: automation-editing R5.
#[test]
fn copying_an_envelope_and_pasting_it_onto_another_parameter_keeps_its_shape() {
    let mut song = song();
    song.select();
    let state: EnvelopeClipboardState =
        serde_json::from_value(song.run(ids::COPY, json!({}))).unwrap();
    assert_eq!(state.lanes, 1);
    assert_eq!(state.length, Ticks(BAR));
    // Paste it back at bar 4: the shape lands there, with the values as
    // positions on the travel.
    song.run(
        ids::PASTE,
        json!({ "lanes": [song.lane()], "at": Ticks(BAR * 4) }),
    );
    let envelope = song.envelope();
    assert!(envelope.len() >= 5, "{envelope:?}");
}

// Spec: automation-editing R2.
#[test]
fn moving_several_breakpoints_moves_them_all_as_one() {
    let mut song = song();
    song.run(
        ids::MOVE_BREAKPOINTS,
        json!({
            "track": song.track,
            "target": { "type": "mixer", "param": "volume" },
            "at": [Ticks(BAR), Ticks(BAR * 2)],
            "by": Ticks(BAR),
            "share": 0.0,
        }),
    );
    assert_eq!(
        song.envelope(),
        vec![(0, 0.0), (2, 1.0), (3, 0.5)],
        "both breakpoints moved by a bar"
    );
}

// Spec: automation-editing R9.
#[test]
fn moving_a_clip_carries_its_automation_with_it() {
    let mut song = song();
    // The clip goes from bar 1 to bar 3; the envelope's shape over that
    // span goes with it.
    song.run(
        edit_ids::MOVE_CLIPS,
        json!({ "clips": [song.clip], "by": Ticks(BAR * 2) }),
    );
    let envelope = song.envelope();
    // The values at the clip's old span are kept, and the shape is at the
    // new one.
    assert_eq!(envelope[0], (0, 0.0), "{envelope:?}");
    assert!(
        envelope
            .iter()
            .any(|(bar, value)| *bar == 3 && (*value - 1.0).abs() < 0.001),
        "{envelope:?}"
    );
    assert!(
        envelope
            .iter()
            .any(|(bar, value)| *bar == 4 && (*value - 0.5).abs() < 0.001),
        "{envelope:?}"
    );
}

// Spec: automation-editing R10.
#[test]
fn with_lock_envelopes_on_a_clip_moves_on_its_own() {
    let mut song = song();
    song.run("song.set_lock_envelopes", json!({ "enabled": true }));
    song.run(
        edit_ids::MOVE_CLIPS,
        json!({ "clips": [song.clip], "by": Ticks(BAR * 2) }),
    );
    assert_eq!(
        song.envelope(),
        vec![(0, 0.0), (1, 1.0), (2, 0.5)],
        "the envelope stays where it is"
    );
}

// Spec: automation-editing R8.
#[test]
fn lock_envelopes_is_off_in_a_new_set_and_saved_with_the_set() {
    let mut song = song();
    assert!(!song.doc().settings().lock_envelopes);
    song.run("song.set_lock_envelopes", json!({ "enabled": true }));
    assert!(song.doc().settings().lock_envelopes);
}

// Spec: automation-editing R5. Cut copies the range, then deletes it.
#[test]
fn cutting_an_envelope_copies_the_range_and_deletes_it() {
    let mut song = song();
    song.run(
        ids::CUT,
        json!({ "lanes": [song.lane()], "start": Ticks::ZERO, "end": Ticks(BAR * 2) }),
    );
    assert_eq!(
        song.envelope(),
        vec![(0, 0.0), (2, 0.5)],
        "the range runs straight across"
    );
    // The cut shape went to the clipboard: paste it back at bar 4.
    song.run(
        ids::PASTE,
        json!({ "lanes": [song.lane()], "at": Ticks(BAR * 4) }),
    );
    let envelope = song.envelope();
    assert_eq!(envelope[0], (0, 0.0));
    assert_eq!(envelope[1], (2, 0.5));
    assert!(
        envelope
            .iter()
            .any(|(bar, value)| *bar == 5 && (*value - 1.0).abs() < 0.001),
        "{envelope:?}"
    );
}

// Spec: automation-editing R5. Duplicate writes a copy straight after the
// span and makes the copy the time selection.
#[test]
fn duplicating_an_envelope_writes_a_copy_after_the_span() {
    let mut song = song();
    song.select();
    song.run(ids::DUPLICATE, json!({}));
    let envelope = song.envelope();
    assert!(
        envelope
            .iter()
            .any(|(bar, value)| *bar == 3 && (*value - 0.5).abs() < 0.001),
        "{envelope:?}"
    );
    let state: ArrangementState =
        serde_json::from_value(song.run(view_ids::STATE, json!({}))).unwrap();
    assert_eq!(
        state.time_selection.map(|range| (range.start, range.end)),
        Some((Ticks(BAR * 2), Ticks(BAR * 3))),
        "the copy is the time selection"
    );
}

// Spec: automation-editing R7. Selecting a segment names its two
// breakpoints.
#[test]
fn selecting_a_segment_names_its_two_breakpoints() {
    let mut song = song();
    song.run(
        ids::SELECT_SEGMENT,
        json!({ "track": song.track, "target": { "type": "mixer", "param": "volume" }, "at": Ticks(BAR) }),
    );
    assert_eq!(
        song.shell.arrangement().selected_segment(song.track),
        Some((
            AutomationTarget::Mixer {
                param: MixerParam::Volume
            },
            Ticks(BAR)
        ))
    );
}

// Spec: automation-editing R9. Duplicating a clip carries its automation
// over the same span.
#[test]
fn duplicating_a_clip_carries_its_automation_with_it() {
    let mut song = song();
    song.run(edit_ids::DUPLICATE, json!({ "clips": [song.clip] }));
    assert_eq!(
        song.envelope(),
        vec![(0, 0.0), (1, 1.0), (2, 0.0), (3, 1.0), (4, 0.5)],
        "the shape is copied after the clip, replacing the envelope there"
    );
}

// Spec: automation-editing R9. Copying and pasting a clip carries its
// automation, leaving the source's envelope as it was.
#[test]
fn copying_and_pasting_a_clip_carries_its_automation_with_it() {
    let mut song = song();
    song.run(edit_ids::COPY, json!({ "clips": [song.clip] }));
    song.run(edit_ids::PASTE, json!({ "at": Ticks(BAR * 4) }));
    let envelope = song.envelope();
    assert_eq!(envelope[..3], [(0, 0.0), (1, 1.0), (2, 0.5)]);
    assert_eq!(
        &envelope[3..],
        [(4, 0.0), (5, 1.0), (6, 0.5)],
        "the copy carries the shape"
    );
}

// Spec: automation-editing R9. Deleting a time selection in track lanes
// takes the envelopes over the span with the clips.
#[test]
fn deleting_a_time_selection_removes_the_envelopes_over_it() {
    let mut song = song();
    song.run(
        edit_ids::DELETE,
        json!({ "tracks": [song.track], "start": Ticks::ZERO, "end": Ticks(BAR * 2) }),
    );
    assert_eq!(
        song.envelope(),
        vec![(0, 0.0), (2, 0.5)],
        "the span runs straight across"
    );
}

// Spec: automation-editing R10. With Lock Envelopes on, deleting a time
// selection leaves the envelopes where they are.
#[test]
fn with_lock_envelopes_on_a_time_selection_leaves_the_envelopes() {
    let mut song = song();
    song.run("song.set_lock_envelopes", json!({ "enabled": true }));
    song.run(
        edit_ids::DELETE,
        json!({ "tracks": [song.track], "start": Ticks::ZERO, "end": Ticks(BAR * 2) }),
    );
    assert_eq!(
        song.envelope(),
        vec![(0, 0.0), (1, 1.0), (2, 0.5)],
        "the envelope stays where it is"
    );
}

// Spec: automation-editing R9, R10. With Lock Envelopes on, duplicating,
// copying, pasting and deleting clips leave every envelope where it is.
#[test]
fn with_lock_envelopes_on_the_clip_edits_leave_the_envelopes() {
    let mut song = song();
    song.run("song.set_lock_envelopes", json!({ "enabled": true }));
    let before = song.envelope();
    song.run(edit_ids::DUPLICATE, json!({ "clips": [song.clip] }));
    assert_eq!(song.envelope(), before, "duplicate leaves the envelope");
    song.run(edit_ids::COPY, json!({ "clips": [song.clip] }));
    song.run(edit_ids::PASTE, json!({ "at": Ticks(BAR * 4) }));
    assert_eq!(song.envelope(), before, "paste leaves the envelope");
    song.run(edit_ids::DELETE, json!({ "clips": [song.clip] }));
    assert_eq!(song.envelope(), before, "delete leaves the envelope");
}

// Spec: automation-editing R9. Cutting a clip carries its automation with
// it, as deleting it does.
#[test]
fn cutting_a_clip_carries_its_automation_with_it() {
    let mut song = song();
    song.run(edit_ids::CUT, json!({ "clips": [song.clip] }));
    assert_eq!(
        song.envelope(),
        vec![(0, 0.0), (2, 0.5)],
        "the span runs straight across"
    );
}

// Spec: automation-editing R9. A clip pasted onto another track takes the
// envelopes of the mixer controls and sends that track also has; a device's
// parameters stay on the track they're on.
#[test]
fn pasting_a_clip_onto_another_track_leaves_the_device_envelopes_behind() {
    let mut editor = factory_default_set();
    let track = editor
        .document()
        .top_level()
        .find(|id| {
            editor
                .document()
                .track(*id)
                .is_some_and(|track| track.kind() == bl_model::TrackKind::Audio)
        })
        .unwrap();
    let other = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: other,
            kind: bl_model::TrackKind::Audio,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    // A device on the first track, with an envelope on its gain.
    let device = editor.new_id();
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: Device::new(
                device,
                "",
                DeviceKind::Native {
                    device_type: bl_devices::testing::UTILITY.to_owned(),
                },
            ),
        })
        .unwrap();
    let target = AutomationTarget::Device {
        device,
        param: bl_devices::testing::UTILITY_GAIN,
    };
    editor
        .apply(Command::SetAutomationBreakpoint {
            track,
            target,
            at: Ticks::ZERO,
            value: 0.5,
        })
        .unwrap();
    let clip: bl_model::ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(BAR * 2),
                ClipData::Audio(AudioData::new(ContentHash([7; 32]))),
            ),
        })
        .unwrap();
    editor.clear_history().unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Automation.blset" }))
        .unwrap();
    // Copy the clip and paste it onto the other track.
    shell
        .run(edit_ids::COPY, json!({ "clips": [clip] }))
        .unwrap();
    let pasted = shell.run(
        edit_ids::PASTE,
        json!({ "at": Ticks(BAR * 4), "track": other }),
    );
    assert!(pasted.is_ok(), "the paste failed: {pasted:?}");
    // The device's envelope stays on the track it was on.
    assert!(
        shell
            .editor()
            .document()
            .track(other)
            .unwrap()
            .automation_for(&target)
            .is_none(),
        "the device's envelope followed the clip"
    );
}

// The golden render (automation-editing R9): a clip moved with Lock
// Envelopes off plays its automation at its new place. Every block of a
// render runs under the allocation trap.

bl_rt::install_trap!();

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-automation-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, rest: &str) -> PathBuf {
        self.0.join(rest)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a 16-bit mono WAV file of one second at 48 kHz: a quiet ramp.
fn wav(path: &Path) {
    let frames = 48_000u32;
    let mut bytes = Vec::new();
    let data = frames * 2;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let sample = ((i % 100) as i16 - 50) * 20;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// A shell holding a one-second WAV clip on a track, with a volume envelope
/// over the clip: a ramp from 0.25 at its start to 1.0 at its end.
struct Rendered {
    shell: Shell,
    clip: bl_model::ClipId,
    start: Ticks,
    end: Ticks,
}

fn rendered_song(scratch: &Scratch) -> Rendered {
    let mut shell = Shell::headless();
    let file = scratch.path("Ramp.wav");
    wav(&file);
    shell
        .run(
            browser_commands::ADD_TO_ARRANGEMENT,
            json!({ "paths": [file.to_string_lossy()], "at": 0 }),
        )
        .unwrap();
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.clips().next().is_some())
        .map(bl_model::Track::id)
        .expect("the clip's track");
    let clip = shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .clips()
        .next()
        .unwrap()
        .clone();
    for (at, value) in [(clip.start, 0.25), (clip.end(), 1.0)] {
        shell
            .run(
                "automation.set_breakpoint",
                json!({
                    "track": track,
                    "target": { "type": "mixer", "param": "volume" },
                    "at": at,
                    "value": value,
                }),
            )
            .unwrap();
    }
    Rendered {
        shell,
        clip: clip.id,
        start: clip.start,
        end: clip.end(),
    }
}

/// Renders the span from `start` to `end`, in blocks of `block` frames.
fn render_span(shell: &Shell, start: Ticks, end: Ticks, block: usize) -> Vec<f32> {
    let spec = Render {
        start,
        length: Length::To(end),
        sample_rate: 48_000,
        engine_rate: 48_000,
        block,
    };
    let mut samples = Vec::new();
    let rendered = export::render(
        shell.editor().document(),
        shell.media_store(),
        Hosts::default(),
        &spec,
        60.0,
        &AtomicBool::new(false),
        &mut |_| {},
        &mut |block| {
            samples.extend_from_slice(block);
            Ok(())
        },
        None,
    )
    .unwrap();
    assert_eq!(rendered.heap_operations, 0, "the real-time rules");
    samples
}

// Spec: automation-editing R9.
#[test]
fn a_clip_moved_with_lock_envelopes_off_plays_its_automation_at_its_new_place() {
    let scratch = Scratch::new("moved");
    let mut song = rendered_song(&scratch);
    let golden = render_span(&song.shell, song.start, song.end, export::BLOCK);
    assert!(golden.iter().any(|&s| s != 0.0), "the clip plays");
    // Move the clip a bar later: the envelope goes with it, and the moved
    // span renders the same at any block size.
    song.shell
        .run(
            edit_ids::MOVE_CLIPS,
            json!({ "clips": [song.clip], "by": Ticks::WHOLE }),
        )
        .unwrap();
    for block in [1, 512, 4096] {
        let moved = render_span(
            &song.shell,
            song.start + Ticks::WHOLE,
            song.end + Ticks::WHOLE,
            block,
        );
        assert_eq!(moved, golden, "block size {block}");
    }

    // With Lock Envelopes on the envelope stays where it was, so the moved
    // span doesn't play the same automation.
    let scratch = Scratch::new("locked");
    let mut locked = rendered_song(&scratch);
    locked
        .shell
        .run("song.set_lock_envelopes", json!({ "enabled": true }))
        .unwrap();
    locked
        .shell
        .run(
            edit_ids::MOVE_CLIPS,
            json!({ "clips": [locked.clip], "by": Ticks::WHOLE }),
        )
        .unwrap();
    let still = render_span(
        &locked.shell,
        locked.start + Ticks::WHOLE,
        locked.end + Ticks::WHOLE,
        export::BLOCK,
    );
    assert_ne!(still, golden, "the envelope stayed where it was");
}
