//! Use Current Scale across the engine and the scale-aware MIDI effects
//! (WP-6.8; the scales spec, R15 to R18): MIDI clips play through a device
//! into the note-recording test instrument, and the device's scale switches on
//! the frame a Session clip launches and on the frame the playhead enters an
//! Arrangement clip, at every block size. Every block is a real-time section
//! under the allocation trap.

use crate::common;
use bl_devices::arpeggiator::{ARPEGGIATOR, DISTANCE, STEPS};
use bl_devices::dice::{CHANCE, CHOICES, DICE, INTERVAL, SIGN};
use bl_devices::lift::{LIFT, PITCH};
use bl_devices::remap::REMAP;
use bl_devices::stack::{STACK, shift};
use bl_engine::NoteKind;
use bl_model::{Scale, ScaleName};
use common::chain::{BEAT_FRAMES, Chain, note, nothing_hangs};

const SIZES: [usize; 4] = [1, 64, 512, 4096];
/// How many frames before a clip's end its last note starts.
const BEFORE: i64 = 10;

fn c_major() -> Scale {
    Scale::new(0, ScaleName::Major)
}

fn e_minor() -> Scale {
    Scale::new(4, ScaleName::Minor)
}

/// A note of `key` at a tick position, a tenth of a beat long.
fn note_at_tick(key: u8, tick: i64) -> bl_model::Note {
    let tenth = bl_core::time::TICKS_PER_QUARTER / 10;
    bl_model::Note::new(
        key,
        bl_core::time::Ticks(tick),
        bl_core::time::Ticks(tenth),
        100,
    )
}

/// The ticks in `BEFORE` frames.
fn before() -> i64 {
    BEFORE * bl_core::time::TICKS_PER_QUARTER / BEAT_FRAMES
}

fn ons(chain: &Chain) -> Vec<(i64, u8)> {
    chain
        .heard()
        .iter()
        .filter(|h| h.1 == NoteKind::On)
        .map(|h| (h.0, h.2))
        .collect()
}

// Spec: scales R15, R16. Pitch counts degrees of the scale of the Arrangement
// clip under the playhead, changing on the frame the playhead enters the next
// clip, at any block size; a clip with Scale Mode off gives semitones.
#[test]
fn a_device_s_scale_switches_on_the_frame_the_playhead_enters_a_clip() {
    for size in SIZES {
        let mut chain = Chain::new(4096);
        chain.effect(LIFT, &[(PITCH, 1.0), (bl_devices::lift::USE_SCALE, 1.0)]);
        let tick = bl_core::time::TICKS_PER_QUARTER;
        // C Major from beat 0 to 2 with C3 on beat 1 and a few frames before
        // beat 2; Scale Mode off from beat 2 to 4 with C3 on its first frame
        // and a beat later. The notes sit in their clips.
        chain.clip_at(
            0,
            2,
            c_major(),
            [
                note_at_tick(60, tick),
                note_at_tick(60, 2 * tick - before()),
            ],
        );
        chain.clip_at(
            2,
            2,
            Scale::default(),
            [note_at_tick(60, 0), note_at_tick(60, tick)],
        );
        chain.sync();
        chain.play();
        chain.run(4 * BEAT_FRAMES as usize, size);
        assert_eq!(
            ons(&chain),
            [
                (BEAT_FRAMES, 62),
                (2 * BEAT_FRAMES - BEFORE, 62),
                (2 * BEAT_FRAMES, 61),
                (3 * BEAT_FRAMES, 61),
            ],
            "block size {size}"
        );
    }
}

// Spec: scales R15, R16. A Session clip's scale is the device's on the frame it
// launches, at any block size.
#[test]
fn a_device_s_scale_switches_on_the_frame_a_session_clip_launches() {
    for size in SIZES {
        let mut chain = Chain::new(4096);
        chain.effect(LIFT, &[(PITCH, 2.0), (bl_devices::lift::USE_SCALE, 1.0)]);
        // The set's scale is off. The slot clip is E Minor, with its first
        // note on its first frame.
        let scene = chain.slot_clip(4, e_minor(), [note(61, 0, 1)]);
        chain.sync();
        chain.play();
        chain.run(100, size);
        chain.launch(scene);
        chain.run(3 * BEAT_FRAMES as usize, size);
        // C#3 is outside E Minor; the scale note below is C3, two degrees up
        // is E3, four semitones, so C#3 goes to 65. The chromatic scale of
        // the set would have made it 63.
        assert_eq!(ons(&chain), [(BEAT_FRAMES, 65)], "block size {size}");
    }
}

// Spec: scales R15, R16. With no clip playing, the set's current scale is the
// device's; a clip playing on the track overrides it.
#[test]
fn the_set_s_scale_is_the_device_s_where_no_clip_plays() {
    let mut chain = Chain::new(512);
    chain.effect(LIFT, &[(PITCH, 1.0), (bl_devices::lift::USE_SCALE, 1.0)]);
    // The clip plays from beat 1 on; before it nothing plays, so there is
    // nothing to hear: the set's scale shows after the clip.
    chain.clip_at(1, 1, c_major(), [note_at_tick(60, 0)]);
    chain.scale_set(e_minor());
    chain.sync();
    chain.play();
    chain.run(3 * BEAT_FRAMES as usize, 512);
    // C3 under C Major (the clip's, not E Minor): D3.
    assert_eq!(ons(&chain), [(BEAT_FRAMES, 62)]);
}

// Spec: scales R17. A note held across a change of the device's scale ends on
// the key it started on, at any block size, and nothing hangs.
#[test]
fn a_note_held_across_a_scale_change_ends_on_its_own_key() {
    for size in SIZES {
        let mut chain = Chain::new(4096);
        chain.effect(LIFT, &[(PITCH, 2.0), (bl_devices::lift::USE_SCALE, 1.0)]);
        chain.clip(4, [note(60, 0, 2)]);
        chain.scale_clips(c_major());
        chain.sync();
        chain.play();
        chain.run(BEAT_FRAMES as usize, size);
        chain.scale_clips(e_minor());
        chain.run(2 * BEAT_FRAMES as usize, size);
        let heard = chain.heard();
        let keys: Vec<(NoteKind, u8)> = heard.iter().map(|h| (h.1, h.2)).collect();
        assert_eq!(
            keys,
            [(NoteKind::On, 64), (NoteKind::Off, 64)],
            "block size {size}"
        );
        assert!(nothing_hangs(&heard));
    }
}

// Spec: scales R16. Switching Use Current Scale while a note is held doesn't
// move it either.
#[test]
fn switching_use_current_scale_while_a_note_is_held_does_not_move_it() {
    let mut chain = Chain::new(512);
    let pitch = chain.effect(LIFT, &[(PITCH, 2.0)]);
    chain.clip(4, [note(60, 0, 2)]);
    chain.scale_clips(c_major());
    chain.sync();
    chain.play();
    chain.run(BEAT_FRAMES as usize, 512);
    chain.set(pitch, bl_devices::lift::USE_SCALE, 1.0);
    chain.run(2 * BEAT_FRAMES as usize, 512);
    let keys: Vec<(NoteKind, u8)> = chain.heard().iter().map(|h| (h.1, h.2)).collect();
    assert_eq!(keys, [(NoteKind::On, 62), (NoteKind::Off, 62)]);
}

// Spec: scales R16. Each of the five devices has Use Current Scale as a
// parameter, off in a new device, drawn in the title bar.
#[test]
fn the_five_scale_aware_devices_have_the_switch_in_the_title_bar() {
    for (description, switch) in [
        (
            bl_devices::arpeggiator_type().description,
            bl_devices::arpeggiator::USE_SCALE,
        ),
        (
            bl_devices::stack_type().description,
            bl_devices::stack::USE_SCALE,
        ),
        (
            bl_devices::lift_type().description,
            bl_devices::lift::USE_SCALE,
        ),
        (
            bl_devices::dice_type().description,
            bl_devices::dice::USE_SCALE,
        ),
        (
            bl_devices::remap_type().description,
            bl_devices::remap::USE_SCALE,
        ),
    ] {
        let param = description.param(switch).expect("the device has it");
        assert_eq!(param.name, "Use Current Scale");
        assert_eq!(param.default, 0.0, "off in a new device");
        assert_eq!(description.title_switch, Some(switch));
        assert!(param.automatable);
        description.check().unwrap();
    }
    // Note Echo has none.
    let echo = bl_devices::note_echo_type().description;
    assert_eq!(echo.title_switch, None);
}

// Spec: scales R16, resonators R1. Resonators, an audio effect, has Use Current
// Scale as a parameter too, off in a new device, drawn in the title bar.
#[test]
fn resonators_has_the_switch_in_the_title_bar() {
    let description = bl_devices::resonators_type().description;
    let param = description
        .param(bl_devices::resonators::USE_SCALE)
        .expect("the device has it");
    assert_eq!(param.name, "Use Current Scale");
    assert_eq!(param.default, 0.0, "off in a new device");
    assert_eq!(
        description.title_switch,
        Some(bl_devices::resonators::USE_SCALE)
    );
    assert!(param.automatable);
    description.check().unwrap();
}

// Spec: scales R16, R29; spectral-resonator R1. Spectral Resonator, an audio
// effect, has Use Current Scale as a parameter too, off in a new device,
// drawn in the title bar.
#[test]
fn spectral_resonator_has_the_switch_in_the_title_bar() {
    let description = bl_devices::spectral_resonator_type().description;
    let param = description
        .param(bl_devices::spectral_resonator::USE_SCALE)
        .expect("the device has it");
    assert_eq!(param.name, "Use Current Scale");
    assert_eq!(param.default, 0.0, "off in a new device");
    assert_eq!(
        description.title_switch,
        Some(bl_devices::spectral_resonator::USE_SCALE)
    );
    assert!(param.automatable);
    description.check().unwrap();
}

// Spec: scales R15. A chain of all five, counting degrees, plays through a
// change of the clip's scale at every block size, and its renders are the
// same: two renders are bit for bit equal, and nothing hangs.
#[test]
fn a_chain_of_scale_aware_devices_plays_through_a_scale_change() {
    let render = |size: usize| {
        let mut chain = Chain::new(4096);
        let on = bl_devices::lift::USE_SCALE;
        chain.effect(bl_devices::lift::LIFT, &[(PITCH, 1.0), (on, 1.0)]);
        chain.effect(
            DICE,
            &[
                (bl_devices::dice::USE_SCALE, 1.0),
                (CHANCE, 60.0),
                (CHOICES, 3.0),
                (INTERVAL, 1.0),
                (SIGN, 2.0),
            ],
        );
        chain.effect(
            STACK,
            &[(bl_devices::stack::USE_SCALE, 1.0), (shift(0), 2.0)],
        );
        chain.effect(
            REMAP,
            &[
                (bl_devices::remap::USE_SCALE, 1.0),
                (bl_devices::remap::BASE, 7.0),
            ],
        );
        chain.effect(
            ARPEGGIATOR,
            &[
                (bl_devices::arpeggiator::USE_SCALE, 1.0),
                (STEPS, 2.0),
                (DISTANCE, 1.0),
            ],
        );
        chain.clip(8, (0..8).map(|b| note(60 + (b as u8 % 5) * 2, b, 1)));
        chain.scale_clips(c_major());
        chain.sync();
        chain.play();
        chain.run(3 * BEAT_FRAMES as usize, size);
        chain.scale_clips(e_minor());
        chain.run(3 * BEAT_FRAMES as usize, size);
        chain.stop();
        chain.run(BEAT_FRAMES as usize, size);
        chain.heard()
    };
    for size in SIZES {
        let heard = render(size);
        assert!(!heard.is_empty());
        assert!(nothing_hangs(&heard), "block size {size}");
        assert_eq!(heard, render(size), "block size {size}: a render repeats");
    }
}
