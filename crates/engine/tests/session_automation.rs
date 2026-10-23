//! A Session clip's automation envelopes (WP-3.12): while the clip plays on
//! a taken-over track they set the parameter over the track's own
//! automation, on the clip's own timeline and the exact frame; when the clip
//! stops the parameter holds its last value until Back to Arrangement; a
//! hand move turns the envelope off. Every block runs as a real-time section
//! under the allocation trap.

use crate::common;
use bl_core::time::Ticks;
use bl_engine::{SessionCommand, TransportCommand};
use bl_model::{
    AutomationTarget, Clip, ClipData, ClipId, Command, EnvelopeKind, MidiData, MixerParam, SceneId,
    TrackId, TrackKind,
};
use common::{Rig, dc, track};

/// A 4/4 bar is a whole note of ticks and 96,000 frames at 120 BPM and
/// 48 kHz.
const BAR: usize = 96_000;

/// The volume control of a track, as an envelope target.
fn volume() -> AutomationTarget {
    AutomationTarget::Mixer {
        param: MixerParam::Volume,
    }
}

/// The value a rising ramp over one bar holds at `frame` of the bar.
fn ramp(frame: usize) -> f64 {
    (frame % BAR) as f64 / BAR as f64
}

/// A MIDI track whose DC device plays 1.0, so the output is the track's
/// volume, with the fader at `control` and a one-bar looping Session clip in
/// the first scene whose automation envelope ramps the volume from 0 to 1
/// over the bar. `block` is the largest block.
fn rig_with_clip(block: usize, control: f64) -> (Rig, TrackId, ClipId, SceneId) {
    let mut rig = Rig::new(block);
    let track = track(&mut rig.editor, TrackKind::Midi, None);
    dc(&mut rig.editor, track, 1.0);
    rig.apply(Command::SetVolume {
        track,
        volume: control,
    });
    let scene = rig.editor.document().scenes().next().unwrap().id;
    let clip: ClipId = rig.editor.new_id();
    let data = MidiData {
        looping: true,
        loop_end: Some(Ticks::WHOLE),
        ..MidiData::new([])
    };
    rig.apply(Command::CreateSlotClip {
        track,
        scene,
        clip: Clip::new(clip, Ticks::ZERO, Ticks::WHOLE, ClipData::Midi(data)),
    });
    for (at, value) in [(Ticks::ZERO, 0.0), (Ticks::WHOLE, 1.0)] {
        rig.apply(Command::SetClipEnvelopeBreakpoint {
            clip,
            target: volume(),
            kind: EnvelopeKind::Automation,
            at,
            value,
        });
    }
    (rig, track, clip, scene)
}

/// Gives the track's volume the Arrangement automation `value` throughout.
fn arrangement_volume(rig: &mut Rig, track: TrackId, value: f64) {
    rig.apply(Command::SetAutomationBreakpoint {
        track,
        target: volume(),
        at: Ticks::ZERO,
        value,
    });
}

fn assert_near(out: &[f32], from: usize, expected: impl Fn(usize) -> f64, what: &str) {
    for (i, &sample) in out.iter().enumerate() {
        let frame = from + i;
        let want = expected(frame);
        assert!(
            (f64::from(sample) - want).abs() < 1e-5,
            "{what}: frame {frame} is {sample}, not {want}"
        );
    }
}

/// Plays the launch of the clip at the second bar line, its stop at the
/// fifth and Back to Arrangement half a bar after that, in blocks of
/// `block`, and returns everything the track played.
fn performance(block: usize) -> Vec<f32> {
    let (mut rig, track, _, scene) = rig_with_clip(block, 0.5);
    arrangement_volume(&mut rig, track, 0.8);
    rig.sender.transport(TransportCommand::Play);
    let mut out = rig.render(BAR / 2, block);
    // Asked for in the first bar, the launch lands on the next bar line.
    rig.sender.session(SessionCommand::Launch { track, scene });
    out.extend(rig.render(BAR * 3, block));
    // Asked for half way through the fourth bar, the stop lands on the line
    // after it, where the clip has played its loop three times.
    rig.sender.session(SessionCommand::StopTrack(track));
    out.extend(rig.render(BAR, block));
    rig.sender.session(SessionCommand::BackToArrangement);
    out.extend(rig.render(BAR / 2, block));
    out
}

// Spec: session-automation R6, R7
#[test]
fn a_playing_clip_sets_the_parameter_over_the_arrangement_and_a_stop_holds_it() {
    let out = performance(64);
    assert_eq!(out.len(), BAR * 5);
    // Before the launch's line the Arrangement's automation plays.
    assert_near(&out[..BAR], 0, |_| 0.8, "the Arrangement before the launch");
    // From it the clip's envelope sets the volume, repeating with the loop.
    assert_near(
        &out[BAR..BAR * 3],
        BAR,
        ramp,
        "the clip's envelope over the Arrangement's",
    );
    // The stop waits for the line after the request: the clip plays the bar
    // before it.
    assert_near(
        &out[BAR * 3..BAR * 4],
        BAR * 3,
        ramp,
        "the loop's third pass",
    );
    // After the stop the parameter holds the last value the clip gave it,
    // not the Arrangement's and not the control's.
    let held = out[BAR * 4 - 1];
    assert!(held > 0.99, "the last frame of the ramp, {held}");
    assert_near(
        &out[BAR * 4..BAR * 5 - BAR / 2],
        BAR * 4,
        |_| f64::from(held),
        "held after the stop",
    );
    // Back to Arrangement gives it back.
    assert_near(
        &out[BAR * 5 - BAR / 2..],
        BAR * 5 - BAR / 2,
        |_| 0.8,
        "the Arrangement again",
    );
}

// Spec: session-automation R8
#[test]
fn the_values_are_the_same_at_any_block_size() {
    let reference = performance(64);
    for block in [1, 7, 480, 4096] {
        assert_eq!(performance(block), reference, "blocks of {block}");
    }
}

// Spec: session-automation R9
#[test]
fn a_hand_move_takes_the_parameter_back_until_the_clip_is_re_enabled() {
    let (mut rig, track, _, scene) = rig_with_clip(64, 0.5);
    arrangement_volume(&mut rig, track, 0.8);
    rig.sender.transport(TransportCommand::Play);
    let mut out = rig.render(1_024, 64);
    rig.sender.session(SessionCommand::Launch { track, scene });
    out.extend(rig.render(BAR, 64));
    assert_near(&out[BAR..], BAR, ramp, "the clip plays");
    // Moving the fader by hand turns the clip's envelope off: the control's
    // value plays from the next frame.
    rig.apply(Command::SetVolume { track, volume: 0.3 });
    // The fader glides from where the clip left it to the hand's value,
    // which then plays over both the clip's envelope and the Arrangement's.
    let out = rig.render(2_048, 64);
    assert!(out[0] < 0.1, "it starts from the clip's value, {}", out[0]);
    assert!(
        out[1_024..].iter().all(|&s| (s - 0.3).abs() < 1e-5),
        "the hand's value plays"
    );
    // Re-enabling the parameter gives it back to the clip.
    rig.apply(Command::ReEnableAutomationFor {
        track,
        target: volume(),
    });
    let from = BAR + 1_024 + 2_048;
    let out = rig.render(1_024, 64);
    assert_near(&out, from, ramp, "the clip plays again");
}
