//! Automation playback (WP-2.12): an envelope drives a parameter on its
//! exact frame without a recompile, a hand move takes the parameter back,
//! and Re-Enable Automation hands it over again. Every block runs as a
//! real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::Ticks;
use bl_engine::TransportCommand;
use bl_model::{AutomationTarget, Command, MixerParam, Output, TrackKind};
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
