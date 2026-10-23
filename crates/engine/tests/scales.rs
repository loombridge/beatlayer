//! A device's current scale (WP-6.8; the scales spec, R15): the scale of the
//! clip playing on its track, or the set's current scale, or the chromatic one
//! where Scale Mode is off, changing on the frame a clip starts or stops at
//! any block size, and reaching the devices in racks. The scale probe adds a
//! number naming the scale its block carries to every frame, so the tests read
//! where the scale changed off the output. Every block runs as a real-time
//! section under the allocation trap.

use crate::common;
use std::sync::Arc;

use bl_core::time::Ticks;
use bl_engine::testing::{self, scale_code};
use bl_engine::{MediaMap, SessionCommand, TransportCommand};
use bl_media::SampleData;
use bl_model::{
    AudioData, Clip, ClipData, Command, ContentHash, LaunchQuantization, LaunchSettings, MidiData,
    RackKind, Scale, ScaleName, TrackId, TrackKind,
};
use common::{Rig, device, rack, track};

const BEAT_FRAMES: usize = 24_000;
const BEAT: Ticks = Ticks::QUARTER;
const SIZES: [usize; 4] = [1, 64, 512, 4096];

fn c_major() -> Scale {
    Scale::new(0, ScaleName::Major)
}

fn d_dorian() -> Scale {
    Scale::new(2, ScaleName::Dorian)
}

fn e_minor() -> Scale {
    Scale::new(4, ScaleName::Minor)
}

/// A MIDI track holding the scale probe.
fn probed(rig: &mut Rig) -> TrackId {
    let id = track(&mut rig.editor, TrackKind::Midi, None);
    device(&mut rig.editor, id, testing::SCALE, &[]);
    id
}

/// A MIDI clip `beats` long from beat `at`, with `scale`.
fn clip(rig: &mut Rig, at: i64, beats: i64, scale: Scale) -> Clip {
    let id = rig.editor.new_id();
    let mut clip = Clip::new(
        id,
        BEAT * at,
        BEAT * beats,
        ClipData::Midi(MidiData::new([])),
    );
    clip.scale = scale;
    clip
}

fn arrange(rig: &mut Rig, track: TrackId, clip: Clip) {
    rig.editor
        .apply(Command::CreateClip { track, clip })
        .unwrap();
}

/// What the probe heard over `frames` frames, in blocks of `size`.
fn heard(rig: &mut Rig, frames: usize, size: usize) -> Vec<f32> {
    rig.render(frames, size)
}

/// Where in `heard` the code first becomes `code`, from frame `from`.
fn first(heard: &[f32], code: f32, from: usize) -> Option<usize> {
    heard[from..]
        .iter()
        .position(|sample| *sample == code)
        .map(|at| at + from)
}

#[test]
fn the_scale_changes_where_the_playhead_enters_and_leaves_an_arrangement_clip() {
    // Spec: scales R15. Two clips back to back, then nothing: the set's
    // current scale, F# Lydian, fills the gaps.
    let set = Scale::new(6, ScaleName::Lydian);
    for size in SIZES {
        let mut rig = Rig::new(4096);
        let id = probed(&mut rig);
        let (first_clip, second) = (
            clip(&mut rig, 1, 2, c_major()),
            clip(&mut rig, 3, 2, e_minor()),
        );
        arrange(&mut rig, id, first_clip);
        arrange(&mut rig, id, second);
        rig.editor
            .apply(Command::SetScale {
                enabled: Some(true),
                root: Some(6),
                name: Some("lydian".to_owned()),
            })
            .unwrap();
        rig.sync();
        rig.sender.transport(TransportCommand::Play);
        let out = heard(&mut rig, 6 * BEAT_FRAMES, size);
        let (major, minor, lydian) = (
            scale_code(&c_major()),
            scale_code(&e_minor()),
            scale_code(&set),
        );
        for (frame, want) in [
            (0, lydian),
            (BEAT_FRAMES - 1, lydian),
            (BEAT_FRAMES, major),
            (3 * BEAT_FRAMES - 1, major),
            (3 * BEAT_FRAMES, minor),
            (5 * BEAT_FRAMES - 1, minor),
            (5 * BEAT_FRAMES, lydian),
        ] {
            assert_eq!(out[frame], want, "block size {size}, frame {frame}");
        }
    }
}

#[test]
fn an_audio_clip_s_scale_reaches_the_devices_on_its_track() {
    // Spec: scales R3, R4, R15. An audio clip's scale changes none of its
    // audio, here silence, and is read by the devices on its track.
    for size in SIZES {
        let mut media = MediaMap::new();
        media.insert(
            ContentHash([7; 32]),
            Arc::new(SampleData::new(48_000, vec![vec![0.0; 400_000]]).unwrap()),
        );
        let mut rig = Rig::with_registry(4096, |registry| registry.set_media(Arc::new(media)));
        let id = track(&mut rig.editor, TrackKind::Audio, None);
        device(&mut rig.editor, id, testing::SCALE, &[]);
        let mut audio = Clip::new(
            rig.editor.new_id(),
            BEAT,
            BEAT * 2,
            ClipData::Audio(AudioData::new(ContentHash([7; 32]))),
        );
        audio.scale = d_dorian();
        arrange(&mut rig, id, audio);
        rig.sync();
        rig.sender.transport(TransportCommand::Play);
        let out = heard(&mut rig, 4 * BEAT_FRAMES, size);
        assert_eq!(out[BEAT_FRAMES - 1], 0.0, "block size {size}");
        assert_eq!(out[BEAT_FRAMES], scale_code(&d_dorian()));
        assert_eq!(out[3 * BEAT_FRAMES - 1], scale_code(&d_dorian()));
        assert_eq!(out[3 * BEAT_FRAMES], 0.0);
    }
}

#[test]
fn a_clip_whose_scale_mode_is_off_gives_the_chromatic_scale_even_when_the_set_has_one() {
    // Spec: scales R15.
    let mut rig = Rig::new(512);
    let id = probed(&mut rig);
    let off = clip(&mut rig, 0, 2, Scale::default());
    arrange(&mut rig, id, off);
    rig.editor
        .apply(Command::SetScale {
            enabled: Some(true),
            root: Some(2),
            name: Some("dorian".to_owned()),
        })
        .unwrap();
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    let out = heard(&mut rig, 3 * BEAT_FRAMES, 512);
    assert_eq!(out[0], 0.0);
    assert_eq!(out[2 * BEAT_FRAMES - 1], 0.0);
    assert_eq!(out[2 * BEAT_FRAMES], scale_code(&d_dorian()));
}

#[test]
fn a_launched_session_clip_makes_its_scale_the_track_s_on_its_launch_frame() {
    // Spec: scales R15. A Quarter launch lands on beat 1, mid-block at every
    // block size but 1.
    for size in SIZES {
        let mut rig = Rig::new(4096);
        let id = probed(&mut rig);
        let scene = rig.editor.document().scenes().next().unwrap().id;
        let slot = clip(&mut rig, 0, 4, d_dorian());
        rig.editor
            .apply(Command::CreateSlotClip {
                track: id,
                scene,
                clip: slot,
            })
            .unwrap();
        rig.sync();
        rig.sender
            .session(SessionCommand::SetQuantization(LaunchSettings {
                quantization: LaunchQuantization::Quarter,
                enabled: true,
                ..LaunchSettings::default()
            }));
        rig.sender.transport(TransportCommand::Play);
        let mut out = heard(&mut rig, 100, size);
        rig.sender
            .session(SessionCommand::Launch { track: id, scene });
        out.extend(heard(&mut rig, 2 * BEAT_FRAMES - 100, size));
        assert_eq!(out[BEAT_FRAMES - 1], 0.0, "block size {size}");
        assert_eq!(
            out[BEAT_FRAMES],
            scale_code(&d_dorian()),
            "block size {size}"
        );
        assert_eq!(first(&out, scale_code(&d_dorian()), 0), Some(BEAT_FRAMES));
    }
}

#[test]
fn a_stopped_session_clip_gives_the_track_back_to_the_set_s_scale() {
    // Spec: scales R15. The track stays taken over from the Arrangement when
    // the clip stops, so it reads the set's scale, not the Arrangement clip's.
    for size in SIZES {
        let mut rig = Rig::new(4096);
        let id = probed(&mut rig);
        let scene = rig.editor.document().scenes().next().unwrap().id;
        let slot = clip(&mut rig, 0, 4, d_dorian());
        rig.editor
            .apply(Command::CreateSlotClip {
                track: id,
                scene,
                clip: slot,
            })
            .unwrap();
        let arranged = clip(&mut rig, 0, 8, c_major());
        arrange(&mut rig, id, arranged);
        rig.sync();
        rig.sender
            .session(SessionCommand::SetQuantization(LaunchSettings {
                quantization: LaunchQuantization::Quarter,
                enabled: true,
                ..LaunchSettings::default()
            }));
        rig.sender.transport(TransportCommand::Play);
        let mut out = heard(&mut rig, 100, size);
        rig.sender
            .session(SessionCommand::Launch { track: id, scene });
        out.extend(heard(&mut rig, 2 * BEAT_FRAMES - 100, size));
        rig.sender.session(SessionCommand::StopTrack(id));
        out.extend(heard(&mut rig, 2 * BEAT_FRAMES, size));
        assert_eq!(out[100], scale_code(&c_major()), "the Arrangement, first");
        assert_eq!(out[BEAT_FRAMES], scale_code(&d_dorian()));
        // The stop lands on the next beat line after the command.
        let stopped = first(&out, 0.0, BEAT_FRAMES).expect("the clip stops");
        assert_eq!(stopped % BEAT_FRAMES, 0, "block size {size}");
        assert_eq!(out[stopped - 1], scale_code(&d_dorian()));
        assert_eq!(out[out.len() - 1], 0.0, "the set's scale is off");
    }
}

#[test]
fn a_scale_change_sends_data_and_no_program() {
    // Spec: scales R15. A clip's scale and the set's reach the engine as
    // data, and take effect in the next block.
    let mut rig = Rig::new(512);
    let id = probed(&mut rig);
    let arranged = clip(&mut rig, 0, 8, c_major());
    let clip_id = arranged.id;
    arrange(&mut rig, id, arranged);
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    assert_eq!(heard(&mut rig, 64, 64)[63], scale_code(&c_major()));
    let synced = rig.apply(Command::SetClipScale {
        clips: vec![clip_id],
        enabled: None,
        root: Some(4),
        name: Some("minor".to_owned()),
    });
    assert!(!synced.program, "a clip's scale makes no program");
    assert_eq!(heard(&mut rig, 64, 64)[0], scale_code(&e_minor()));
    // Off at the clip: the set's scale applies nowhere while the clip plays,
    // and changing it makes no program either.
    let synced = rig.apply(Command::SetScale {
        enabled: Some(true),
        root: Some(2),
        name: Some("dorian".to_owned()),
    });
    assert!(!synced.program, "the set's scale makes no program");
}

#[test]
fn a_device_in_a_rack_reads_its_track_s_scale() {
    // Spec: scales R15. Racks, and racks in racks, hand the scale down.
    let mut rig = Rig::new(512);
    let id = track(&mut rig.editor, TrackKind::Midi, None);
    let (_, chains) = rack(&mut rig.editor, id, RackKind::AudioEffect, 1);
    let (_, inner) = common::rack_in(&mut rig.editor, chains[0], RackKind::AudioEffect, 1);
    common::effect_in(&mut rig.editor, inner[0], testing::SCALE, &[]);
    let arranged = clip(&mut rig, 0, 4, d_dorian());
    arrange(&mut rig, id, arranged);
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    let out = heard(&mut rig, 512, 512);
    assert_eq!(out[100], scale_code(&d_dorian()));
}

#[test]
fn a_track_without_a_playing_clip_reads_the_set_s_scale_while_the_song_is_stopped() {
    // Spec: scales R15.
    let mut rig = Rig::new(512);
    let id = probed(&mut rig);
    let arranged = clip(&mut rig, 0, 4, c_major());
    arrange(&mut rig, id, arranged);
    rig.editor
        .apply(Command::SetScale {
            enabled: Some(true),
            root: Some(2),
            name: Some("dorian".to_owned()),
        })
        .unwrap();
    rig.sync();
    let out = heard(&mut rig, 512, 512);
    assert_eq!(out[10], scale_code(&d_dorian()), "stopped: the set's");
    rig.sender.transport(TransportCommand::Play);
    let out = heard(&mut rig, 512, 512);
    assert_eq!(out[10], scale_code(&c_major()), "playing: the clip's");
}
