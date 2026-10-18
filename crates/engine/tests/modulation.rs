//! Relative modulation (WP-5.9; the modulation spec, R1 to R6): a clip's
//! modulation envelope acts on top of a parameter's base, which a control or
//! the track's automation sets, on the mixer's volume, pan and sends. Every
//! block runs as a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::Ticks;
use bl_engine::TransportCommand;
use bl_engine::mixer::pan_gains;
use bl_model::fader::{fader_gain, fader_position};
use bl_model::{
    AutomationTarget, Clip, ClipData, ClipId, Command, MidiData, MixerParam, TrackId, TrackKind,
};
use common::{Rig, dc, track};

/// At 120 BPM and 48 kHz a 4/4 bar is 96,000 frames; the same bar is a whole
/// note of ticks.
const BAR: usize = 96_000;

fn mixer(param: MixerParam) -> AutomationTarget {
    AutomationTarget::Mixer { param }
}

/// A rig whose MIDI track carries a DC device (so its output is 1.0 before
/// the mixer) with a one-bar clip at the start of the song.
fn rig_with_clip(max_frames: usize) -> (Rig, TrackId, ClipId) {
    let mut rig = Rig::new(max_frames);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    dc(&mut rig.editor, a, 1.0);
    let clip = rig.editor.new_id();
    rig.apply(Command::CreateClip {
        track: a,
        clip: Clip::new(
            clip,
            Ticks::ZERO,
            Ticks::WHOLE,
            ClipData::Midi(MidiData::default()),
        ),
    });
    (rig, a, clip)
}

fn envelope(rig: &mut Rig, clip: ClipId, target: AutomationTarget, points: &[(Ticks, f64)]) {
    for &(at, value) in points {
        rig.apply(Command::SetClipEnvelopeBreakpoint {
            kind: bl_model::EnvelopeKind::Modulation,
            clip,
            target,
            at,
            value,
        });
    }
}

// Spec: modulation R1, R4
#[test]
fn mixer_controls_and_sends_take_modulation() {
    // Volume and pan, centred: a volume share of 0.5 brings the fader's
    // position to half; a pan share of −1 sweeps hard left.
    let (mut rig, a, clip) = rig_with_clip(64);
    rig.apply(Command::SetVolume {
        track: a,
        volume: 0.5,
    });
    envelope(
        &mut rig,
        clip,
        mixer(MixerParam::Volume),
        &[(Ticks::ZERO, 0.5)],
    );
    envelope(
        &mut rig,
        clip,
        mixer(MixerParam::Pan),
        &[(Ticks::ZERO, -1.0)],
    );
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(2_048, 64);
    let want = fader_gain(fader_position(0.5) * 0.5) * pan_gains(-1.0)[0];
    assert!(want > 0.0 && want < 0.5, "{want}");
    assert!(
        out[1_024..]
            .iter()
            .all(|&s| (f64::from(s) - want).abs() < 1e-6),
        "{} isn't {want}",
        out[2_000]
    );

    // A send's level: the knob's position brought down to a share.
    let (mut rig, a, clip) = rig_with_clip(64);
    let ret = track(&mut rig.editor, TrackKind::Return, None);
    rig.apply(Command::SetSend {
        track: a,
        to: ret,
        level: 1.0,
    });
    let send = AutomationTarget::Send { to: ret };
    let direct = pan_gains(0.0)[0];
    rig.sender.transport(TransportCommand::Play);
    let alone = rig.render(2_048, 64)[2_000];
    assert!(
        (f64::from(alone) - (direct + direct * direct)).abs() < 1e-6,
        "the send at 0 dB adds the signal again: {alone}"
    );
    envelope(&mut rig, clip, send, &[(Ticks::ZERO, 0.0)]);
    rig.sender.transport(TransportCommand::Stop);
    rig.sender.transport(TransportCommand::Play);
    let silent = rig.render(2_048, 64)[2_000];
    assert!(
        (f64::from(silent) - direct).abs() < 1e-6,
        "a send at 0 % reaches −∞ dB: {silent}"
    );
}

// Spec: modulation R2
#[test]
fn modulation_rides_on_automation_and_follows_it_down() {
    // The manual's example: automation lowers the volume across the bar
    // while the clip's modulation holds a share of it, so the result
    // follows the automation, which carries the modulation's reach down.
    let (mut rig, a, clip) = rig_with_clip(64);
    rig.apply(Command::SetAutomationBreakpoint {
        track: a,
        target: mixer(MixerParam::Volume),
        at: Ticks::ZERO,
        value: 1.0,
    });
    rig.apply(Command::SetAutomationBreakpoint {
        track: a,
        target: mixer(MixerParam::Volume),
        at: Ticks::WHOLE,
        value: 0.25,
    });
    envelope(
        &mut rig,
        clip,
        mixer(MixerParam::Volume),
        &[(Ticks::ZERO, 0.5)],
    );
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(BAR, 64);
    let centre = pan_gains(0.0)[0];
    for frame in [0, 1, 24_000, 48_000, 72_000, BAR - 1] {
        let base = 1.0 + (0.25 - 1.0) * frame as f64 / BAR as f64;
        let want = fader_gain(fader_position(base) * 0.5) * centre;
        assert!(
            (f64::from(out[frame]) - want).abs() < 1e-6,
            "frame {frame}: {} isn't {want}",
            out[frame]
        );
    }
    assert!(
        out[1_000] > out[BAR - 1_000],
        "the modulated level falls with the automation"
    );
}

// Spec: modulation R2
#[test]
fn modulation_never_changes_the_set() {
    let (mut rig, a, clip) = rig_with_clip(64);
    rig.apply(Command::SetVolume {
        track: a,
        volume: 0.5,
    });
    envelope(
        &mut rig,
        clip,
        mixer(MixerParam::Volume),
        &[(Ticks::ZERO, 0.25)],
    );
    let before = rig.editor.document().clone();
    rig.sender.transport(TransportCommand::Play);
    rig.render(BAR / 4, 64);
    assert_eq!(
        *rig.editor.document(),
        before,
        "playing modulation writes nothing into the set"
    );
    assert_eq!(rig.editor.document().track(a).unwrap().mixer().volume, 0.5);
}

// Spec: modulation R6
#[test]
fn modulation_is_the_same_at_any_block_size() {
    let play = |frames: usize| {
        let (mut rig, a, clip) = rig_with_clip(frames.max(1));
        rig.apply(Command::SetVolume {
            track: a,
            volume: 0.5,
        });
        envelope(
            &mut rig,
            clip,
            mixer(MixerParam::Volume),
            &[(Ticks::ZERO, 0.0), (Ticks::WHOLE, 1.0)],
        );
        envelope(
            &mut rig,
            clip,
            mixer(MixerParam::Pan),
            &[(Ticks::ZERO, -1.0), (Ticks::WHOLE, 1.0)],
        );
        rig.sender.transport(TransportCommand::Play);
        rig.render(3_000, frames)
    };
    let one = play(1);
    for frames in [64, 512] {
        assert_eq!(play(frames), one, "blocks of {frames} frames");
    }
}

// Spec: modulation R6
#[test]
fn a_stopped_source_hands_back_the_base_on_the_next_frame() {
    // A clip that ends: the frame after its last, the parameter plays its
    // base again.
    let (mut rig, a, clip) = rig_with_clip(64);
    rig.apply(Command::SetVolume {
        track: a,
        volume: 0.5,
    });
    rig.apply(Command::SetClipLength {
        clip,
        length: Ticks::WHOLE / 16,
    });
    envelope(
        &mut rig,
        clip,
        mixer(MixerParam::Volume),
        &[(Ticks::ZERO, 0.25)],
    );
    rig.sender.transport(TransportCommand::Play);
    let end = BAR / 16;
    let out = rig.render(end + 4_000, 64);
    let centre = pan_gains(0.0)[0];
    let modulated = fader_gain(fader_position(0.5) * 0.25) * centre;
    let base = 0.5 * centre;
    assert!((f64::from(out[end - 1]) - modulated).abs() < 1e-6);
    assert!(
        (f64::from(out[end]) - base).abs() < 1e-6,
        "the base plays from the next frame: {}",
        out[end]
    );
    assert!((f64::from(out[end + 3_999]) - base).abs() < 1e-6);
    // The engine reports the played value only while it modulates.
    let state = rig.monitor.state();
    assert!(state.modulated.is_empty(), "{:?}", state.modulated);
}

// Spec: modulation R7
#[test]
fn the_engine_publishes_what_a_modulated_parameter_played() {
    let (mut rig, a, clip) = rig_with_clip(64);
    rig.apply(Command::SetVolume {
        track: a,
        volume: 0.5,
    });
    envelope(
        &mut rig,
        clip,
        mixer(MixerParam::Volume),
        &[(Ticks::ZERO, 0.5)],
    );
    assert!(rig.monitor.state().modulated.is_empty(), "stopped");
    rig.sender.transport(TransportCommand::Play);
    rig.render(1_024, 64);
    let state = rig.monitor.state();
    let played = state
        .played(
            bl_engine::NodeKey::Mixer(a),
            bl_engine::mixer::track::VOLUME,
        )
        .expect("the volume is modulated");
    assert_eq!(played.base, 0.5);
    assert!((played.played - fader_gain(fader_position(0.5) * 0.5)).abs() < 1e-9);
    assert!(
        state
            .played(bl_engine::NodeKey::Mixer(a), bl_engine::mixer::track::PAN)
            .is_none(),
        "the pan isn't"
    );
}
