//! Automation playback (WP-2.12): an envelope drives a parameter on its
//! exact frame without a recompile, a hand move takes the parameter back,
//! and Re-Enable Automation hands it over again. Every block runs as a
//! real-time section under the allocation trap.

use crate::common;
use bl_core::time::Ticks;
use bl_engine::TransportCommand;
use bl_model::{AutomationTarget, Command, MixerParam, Output, PanMode, TrackKind};
use common::{Rig, dc, track};

/// The volume control of a track, as an automation target.
fn volume() -> AutomationTarget {
    AutomationTarget::Mixer {
        param: MixerParam::Volume,
    }
}

/// At 120 BPM and 48 kHz a quarter note is 24,000 frames.
const QUARTER: usize = 24_000;

// Spec: automation R2
#[test]
fn a_ramp_reaches_its_target_on_the_exact_sample() {
    let mut rig = Rig::new(64);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    dc(&mut rig.editor, a, 1.0);
    // The fader rests silent, so only the envelope brings the signal up: a
    // ramp from 0 at 1.1.1 to 1 at beat 2.
    rig.apply(Command::SetVolume {
        track: a,
        volume: 0.0,
    });
    rig.apply(Command::SetAutomationBreakpoint {
        track: a,
        target: volume(),
        at: Ticks::ZERO,
        value: 0.0,
    });
    let synced = rig.apply(Command::SetAutomationBreakpoint {
        track: a,
        target: volume(),
        at: Ticks::QUARTER,
        value: 1.0,
    });
    assert!(!synced.program, "automation needs no new program");
    rig.sender.transport(TransportCommand::Play);
    let total = QUARTER + 256;
    let out = rig.render(total, 64);
    assert_eq!(out.len(), total);
    // Every frame is the envelope's own value, to the bit: the ramp's line,
    // and the target exactly on the frame it falls on.
    for (frame, &sample) in out.iter().enumerate() {
        let want = (frame as f64 / QUARTER as f64).min(1.0) as f32;
        assert_eq!(sample, want, "frame {frame} is the envelope's value");
    }
    assert_eq!(out[QUARTER], 1.0, "the target on its exact sample");
    assert!(out[QUARTER..].iter().all(|&s| s == 1.0), "and it holds on");

    // The value depends on the frame alone, whatever the blocks: a run in
    // one-frame blocks gives the same samples.
    let mut fine = Rig::new(1);
    let b = track(&mut fine.editor, TrackKind::Midi, None);
    dc(&mut fine.editor, b, 1.0);
    fine.apply(Command::SetVolume {
        track: b,
        volume: 0.0,
    });
    fine.apply(Command::SetAutomationBreakpoint {
        track: b,
        target: volume(),
        at: Ticks::ZERO,
        value: 0.0,
    });
    fine.apply(Command::SetAutomationBreakpoint {
        track: b,
        target: volume(),
        at: Ticks::QUARTER,
        value: 1.0,
    });
    fine.sender.transport(TransportCommand::Play);
    let small = fine.render(2_000, 1);
    assert_eq!(small, out[..2_000], "one-frame blocks give the same values");
}

// Spec: automation R12
#[test]
fn a_curved_segment_follows_its_shape() {
    let mut rig = Rig::new(64);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    dc(&mut rig.editor, a, 1.0);
    // The fader rests silent, so only the envelope brings the signal up: a
    // segment from 0 at 1.1.1 to 1 at beat 2, curved by a bend of -1.
    rig.apply(Command::SetVolume {
        track: a,
        volume: 0.0,
    });
    rig.apply(Command::SetAutomationBreakpoint {
        track: a,
        target: volume(),
        at: Ticks::ZERO,
        value: 0.0,
    });
    rig.apply(Command::SetAutomationBreakpoint {
        track: a,
        target: volume(),
        at: Ticks::QUARTER,
        value: 1.0,
    });
    let synced = rig.apply(Command::SetAutomationCurve {
        track: a,
        target: volume(),
        at: Ticks::ZERO,
        bend: -1.0,
    });
    assert!(!synced.program, "a curve needs no new program");
    rig.sender.transport(TransportCommand::Play);
    let total = QUARTER + 256;
    let out = rig.render(total, 64);
    assert_eq!(out.len(), total);
    // Every frame is the bend's own power curve, to the bit: at a bend of
    // -1 it's √t, which leads the straight line, and it reaches the target
    // on the frame its breakpoint falls on.
    for (frame, &sample) in out.iter().enumerate() {
        let along = (frame as f64 / QUARTER as f64).min(1.0);
        let want = along.powf(2f64.powf(-1.0)) as f32;
        assert_eq!(sample, want, "frame {frame} follows the bend");
    }
    assert!(out[QUARTER / 2] > 0.5, "the curve bows away from the line");
    assert_eq!(out[QUARTER], 1.0, "the target on its exact sample");

    // The value depends on the frame alone, whatever the blocks: a run in
    // one-frame blocks gives the same samples. A bend of 0 straightens the
    // segment again, as the double-click does, and the value is the line
    // again.
    let mut fine = Rig::new(1);
    let b = track(&mut fine.editor, TrackKind::Midi, None);
    dc(&mut fine.editor, b, 1.0);
    fine.apply(Command::SetVolume {
        track: b,
        volume: 0.0,
    });
    fine.apply(Command::SetAutomationBreakpoint {
        track: b,
        target: volume(),
        at: Ticks::ZERO,
        value: 0.0,
    });
    fine.apply(Command::SetAutomationBreakpoint {
        track: b,
        target: volume(),
        at: Ticks::QUARTER,
        value: 1.0,
    });
    fine.apply(Command::SetAutomationCurve {
        track: b,
        target: volume(),
        at: Ticks::ZERO,
        bend: -1.0,
    });
    fine.sender.transport(TransportCommand::Play);
    let small = fine.render(2_000, 1);
    assert_eq!(small, out[..2_000], "one-frame blocks give the same values");
    fine.apply(Command::SetAutomationCurve {
        track: b,
        target: volume(),
        at: Ticks::ZERO,
        bend: 0.0,
    });
    let straight = fine.render(2_000, 1);
    for (frame, &sample) in straight.iter().enumerate() {
        let along = (2_000 + frame) as f64 / QUARTER as f64;
        assert_eq!(
            sample,
            along as f32,
            "frame {} is the line again",
            2_000 + frame
        );
    }
}

// Spec: automation R4, R5
#[test]
fn a_hand_move_takes_the_parameter_back_and_re_enable_returns_it() {
    let mut rig = Rig::new(64);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    dc(&mut rig.editor, a, 1.0);
    rig.apply(Command::SetAutomationBreakpoint {
        track: a,
        target: volume(),
        at: Ticks::ZERO,
        value: 0.5,
    });
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(256, 64);
    assert!(out.iter().all(|&s| s == 0.5), "the envelope drives it");

    // A hand move overrides the envelope, with no new program; the control's
    // value takes over, gliding to it as any control move does.
    let synced = rig.apply(Command::SetVolume {
        track: a,
        volume: 0.25,
    });
    assert!(!synced.program, "overriding needs no new program");
    assert_eq!(synced.params, 1);
    let out = rig.render(2_048, 64);
    assert!(out[..960].windows(2).all(|w| w[1] < w[0]), "it glides down");
    assert!(
        out[960..].iter().all(|&s| s == 0.25),
        "and rests at the control"
    );

    // Re-Enable Automation hands the parameter back to its envelope.
    let synced = rig.apply(Command::ReEnableAutomation {});
    assert!(!synced.program, "re-enabling needs no new program");
    let out = rig.render(256, 64);
    assert!(
        out.iter().all(|&s| s == 0.5),
        "the envelope drives it again"
    );
    assert_eq!(rig.monitor.state().swaps, 1, "no recompile at all");
}

// Spec: automation R2
#[test]
fn a_sends_level_follows_its_envelope() {
    let mut rig = Rig::new(64);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    let effects = track(&mut rig.editor, TrackKind::Return, None);
    dc(&mut rig.editor, a, 1.0);
    // The send carries the track into the return, and the track's own output
    // is silent, so only the send is heard: its envelope is the whole
    // output.
    rig.apply(Command::SetOutput {
        track: a,
        output: Output::SendsOnly,
    });
    let synced = rig.apply(Command::SetAutomationBreakpoint {
        track: a,
        target: AutomationTarget::Send { to: effects },
        at: Ticks::ZERO,
        value: 0.0,
    });
    rig.apply(Command::SetAutomationBreakpoint {
        track: a,
        target: AutomationTarget::Send { to: effects },
        at: Ticks::QUARTER,
        value: 1.0,
    });
    assert!(!synced.program, "automation needs no new program");
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(QUARTER + 128, 64);
    for (frame, &sample) in out.iter().enumerate() {
        let want = (frame as f64 / QUARTER as f64).min(1.0) as f32;
        assert_eq!(sample, want, "frame {frame}");
    }
    assert_eq!(rig.monitor.state().swaps, 1, "no recompile");
}

// Spec: automation R6
#[test]
fn deleting_the_automation_hands_the_parameter_back() {
    let mut rig = Rig::new(64);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    dc(&mut rig.editor, a, 1.0);
    rig.apply(Command::SetAutomationBreakpoint {
        track: a,
        target: volume(),
        at: Ticks::ZERO,
        value: 0.25,
    });
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(128, 64);
    assert!(out.iter().all(|&s| s == 0.25));
    rig.apply(Command::DeleteAutomation {
        track: a,
        target: volume(),
    });
    let out = rig.render(1_024, 64);
    assert!(
        out[960..].iter().all(|&s| s == 1.0),
        "the control takes over"
    );
}

/// A mixer control as an automation target.
fn mixer(param: MixerParam) -> AutomationTarget {
    AutomationTarget::Mixer { param }
}

/// A rig playing a track of DC 1.0 whose only automated control is `param`,
/// resting at `value`, in blocks of 64.
fn automated_only(param: MixerParam, value: f64, split: bool) -> (Rig, bl_model::TrackId) {
    let mut rig = Rig::new(64);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    dc(&mut rig.editor, a, 1.0);
    if split {
        rig.apply(Command::SetPanMode {
            track: a,
            mode: PanMode::Split,
        });
    }
    rig.apply(Command::SetAutomationBreakpoint {
        track: a,
        target: mixer(param),
        at: Ticks::ZERO,
        value,
    });
    rig.sender.transport(TransportCommand::Play);
    rig.render(2_048, 64);
    (rig, a)
}

// Spec: mixer R18. A control the envelopes don't drive glides as it does with
// none on the track, in both pan modes.
#[test]
fn a_fader_and_an_activator_glide_while_only_pan_is_automated() {
    for (param, split) in [(MixerParam::Pan, false), (MixerParam::SplitLeft, true)] {
        let (mut rig, a) = automated_only(param, 0.0, split);
        let rest = rig.last()[0];
        assert!(rest > 0.0, "{param:?} plays at rest");
        rig.apply(Command::SetVolume {
            track: a,
            volume: 0.0,
        });
        let out = rig.render(2_048, 64);
        assert!(
            out[0] > rest * 0.99,
            "{param:?}: the fader doesn't step: {}",
            out[0]
        );
        assert!(
            out[..960].windows(2).all(|w| w[1] < w[0]),
            "{param:?}: the fader glides down"
        );
        assert!(out[960..].iter().all(|&s| s == 0.0), "{param:?}: and rests");

        rig.apply(Command::SetVolume {
            track: a,
            volume: 1.0,
        });
        rig.render(2_048, 64);
        rig.apply(Command::SetMute {
            track: a,
            mute: true,
        });
        let out = rig.render(2_048, 64);
        assert!(
            out[0] > rest * 0.99,
            "{param:?}: the activator doesn't step"
        );
        assert!(
            out[..960].windows(2).all(|w| w[1] < w[0]),
            "{param:?}: the activator glides down"
        );
        assert!(out[960..].iter().all(|&s| s == 0.0));
        rig.apply(Command::SetMute {
            track: a,
            mute: false,
        });
        let out = rig.render(2_048, 64);
        assert!(
            out[..960].windows(2).all(|w| w[1] > w[0]),
            "{param:?}: and glides back up"
        );
    }
}

// Spec: mixer R18. A pan move glides while the volume is automated.
#[test]
fn a_pan_glides_while_only_the_volume_is_automated() {
    let (mut rig, a) = automated_only(MixerParam::Volume, 1.0, false);
    assert_eq!(rig.last()[0], 1.0, "centred");
    rig.apply(Command::SetPan { track: a, pan: 1.0 });
    let out = rig.render(2_048, 64);
    assert!(out[0] > 0.9, "the pan doesn't step: {}", out[0]);
    assert!(out[960..].iter().all(|&s| s < 1e-9), "and rests hard right");
    assert!(out[..960].windows(2).all(|w| w[1] < w[0]), "gliding");
}

// Spec: mixer R18. Solo glides a send closed and open while the send's level
// follows an envelope.
#[test]
fn a_solo_glides_an_automated_send_closed_and_open() {
    let mut rig = Rig::new(64);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    let b = track(&mut rig.editor, TrackKind::Audio, None);
    let effects = track(&mut rig.editor, TrackKind::Return, None);
    dc(&mut rig.editor, a, 1.0);
    rig.apply(Command::SetOutput {
        track: a,
        output: Output::SendsOnly,
    });
    rig.apply(Command::SetAutomationBreakpoint {
        track: a,
        target: AutomationTarget::Send { to: effects },
        at: Ticks::ZERO,
        value: 1.0,
    });
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(2_048, 64);
    assert!(out[1_024..].iter().all(|&s| s == 1.0), "the send settles");

    // The widest step between two frames, and the first frame after the move.
    let steps = |out: &[f32]| {
        out.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max)
    };
    rig.apply(Command::SetSolo {
        track: b,
        solo: true,
    });
    let out = rig.render(2_048, 64);
    assert!(out[0] > 0.9, "the send doesn't step shut: {}", out[0]);
    assert!(steps(&out) < 0.01, "and glides: {}", steps(&out));
    assert!(out[960 + 64..].iter().all(|&s| s == 0.0), "to rest shut");

    rig.apply(Command::SetSolo {
        track: b,
        solo: false,
    });
    let out = rig.render(2_048, 64);
    assert!(out[0] < 0.1, "the send doesn't step open: {}", out[0]);
    assert!(steps(&out) < 0.01, "and glides: {}", steps(&out));
    assert!(out[960 + 64..].iter().all(|&s| s == 1.0), "to rest open");
}
