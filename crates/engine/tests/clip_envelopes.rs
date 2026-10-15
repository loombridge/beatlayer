//! Clip envelopes (WP-3.15): an envelope a clip carries modulates the
//! parameter it names while the clip plays, on the frames the automation
//! player's read path gives, following the clip's own timeline or its own.
//! Every block runs as a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::Ticks;
use bl_engine::TransportCommand;
use bl_model::{
    AutomationTarget, Clip, ClipData, ClipId, Command, MidiData, MixerParam, TrackId, TrackKind,
    Vector,
};
use common::{Rig, dc, track};

/// At 120 BPM and 48 kHz a quarter note is 24,000 frames and a 4/4 bar
/// 96,000; the same bar is a whole note of ticks.
const BAR: usize = 96_000;

/// The volume control of a track, as an envelope target.
fn volume() -> AutomationTarget {
    AutomationTarget::Mixer {
        param: MixerParam::Volume,
    }
}

/// A rig whose track carries a DC device (so its output is 1.0 whenever it
/// isn't scaled), with blocks of up to `max_frames` and the fader at
/// `volume`.
fn rig_at(max_frames: usize, volume: f64) -> (Rig, TrackId) {
    let mut rig = Rig::new(max_frames);
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    dc(&mut rig.editor, a, 1.0);
    rig.apply(Command::SetVolume { track: a, volume });
    (rig, a)
}

/// A MIDI clip on `track` at `start`, lasting `length` ticks, whose loop
/// brace is its first bar while it runs `bars` bars long: its contents play
/// once from the start marker, then repeat the brace, as its notes do.
fn looping_clip(rig: &mut Rig, track: TrackId, start: Ticks, bars: i64) -> ClipId {
    let id = rig.editor.new_id();
    let data = MidiData {
        notes: Vector::new(),
        start: Ticks::ZERO,
        end: None,
        looping: true,
        loop_start: Ticks::ZERO,
        loop_end: Some(Ticks::WHOLE),
        signature: None,
    };
    rig.apply(Command::CreateClip {
        track,
        clip: Clip::new(id, start, Ticks::WHOLE * bars, ClipData::Midi(data)),
    });
    id
}

/// The value a clip envelope segment from `from` to `to` frame holds at song
/// frame `at`, as the engine works it out: content position in ticks, then
/// the envelope's own value there.
fn ramp_value(at: usize, from: usize, to: usize) -> f64 {
    let whole = Ticks::WHOLE.get() as f64;
    let per_frame = whole / (to - from) as f64;
    (at.saturating_sub(from) as f64) * per_frame / whole
}

/// The sample the DC device's 1.0 becomes through a fader at 0.5 with
/// `modulation` added.
fn want(modulation: f64) -> f32 {
    (0.5 + modulation) as f32
}

// Spec: clip-envelopes R3, R4
#[test]
fn a_linked_clip_envelope_drives_the_parameter_while_the_clip_plays() {
    let (mut rig, a) = rig_at(64, 0.5);
    let clip = looping_clip(&mut rig, a, Ticks::ZERO, 2);
    rig.apply(Command::SetClipEnvelopeBreakpoint {
        clip,
        target: volume(),
        at: Ticks::ZERO,
        value: 0.0,
    });
    let synced = rig.apply(Command::SetClipEnvelopeBreakpoint {
        clip,
        target: volume(),
        at: Ticks::WHOLE,
        value: 1.0,
    });
    assert!(!synced.program, "a clip envelope needs no new program");
    rig.sender.transport(TransportCommand::Play);
    let total = BAR * 2 + 256;
    let out = rig.render(total, 64);
    assert_eq!(out.len(), total);
    // The envelope follows the clip's one-bar loop: each bar ramps from the
    // fader's 0.5 to 1.5, and after the clip the fader plays alone.
    for (frame, &sample) in out[..BAR * 2].iter().enumerate() {
        let expected = want(ramp_value(frame % BAR, 0, BAR));
        assert!(
            (sample - expected).abs() < 1e-6,
            "frame {frame}: {sample} isn't {expected}"
        );
    }
    assert!(
        out[BAR * 2..].iter().all(|&s| s == want(0.0)),
        "after the clip the fader plays alone"
    );
    assert!(
        (out[BAR - 1] - want(ramp_value(BAR - 1, 0, BAR))).abs() < 1e-6,
        "the ramp reaches its value on the frame before the wrap"
    );
    assert!(
        (out[BAR] - want(ramp_value(0, 0, BAR))).abs() < 1e-6,
        "and restarts on the loop's first frame"
    );

    // The value depends on the frame alone, whatever the blocks: a run in
    // one-frame blocks gives the same samples.
    let (mut fine, b) = rig_at(1, 0.5);
    let clip = looping_clip(&mut fine, b, Ticks::ZERO, 2);
    fine.apply(Command::SetClipEnvelopeBreakpoint {
        clip,
        target: volume(),
        at: Ticks::ZERO,
        value: 0.0,
    });
    fine.apply(Command::SetClipEnvelopeBreakpoint {
        clip,
        target: volume(),
        at: Ticks::WHOLE,
        value: 1.0,
    });
    fine.sender.transport(TransportCommand::Play);
    let small = fine.render(4_000, 1);
    assert!(
        small
            .iter()
            .zip(&out[..4_000])
            .all(|(a, b)| (a - b).abs() < 1e-9),
        "one-frame blocks give the same values"
    );
}

// Spec: clip-envelopes R2, R3
#[test]
fn an_unlinked_clip_envelope_runs_once_from_the_clip_start() {
    let (mut rig, a) = rig_at(64, 0.5);
    let clip = looping_clip(&mut rig, a, Ticks::ZERO, 2);
    // The envelope is two bars long while the clip loops every bar: unlinked
    // it doesn't repeat, so the fader climbs to 1.5 across both bars.
    rig.apply(Command::SetClipEnvelopeLinked {
        clip,
        target: volume(),
        linked: false,
    });
    rig.apply(Command::SetClipEnvelopeBreakpoint {
        clip,
        target: volume(),
        at: Ticks::ZERO,
        value: 0.0,
    });
    rig.apply(Command::SetClipEnvelopeBreakpoint {
        clip,
        target: volume(),
        at: Ticks::WHOLE * 2,
        value: 1.0,
    });
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(BAR * 2 + 64, 64);
    for (frame, &sample) in out[..BAR * 2].iter().enumerate() {
        let expected = want(ramp_value(frame, 0, BAR * 2));
        assert!(
            (sample - expected).abs() < 1e-6,
            "frame {frame}: {sample} isn't {expected}"
        );
    }
    assert!(
        out[BAR * 2..].iter().all(|&s| s == want(0.0)),
        "after the clip the fader plays alone"
    );
}

// Spec: clip-envelopes R3
#[test]
fn an_envelope_plays_only_where_its_clip_does() {
    let (mut rig, a) = rig_at(64, 0.5);
    // The clip sits at bar 2, so the fader plays alone before it and after
    // it.
    let clip = rig.editor.new_id();
    let data = MidiData {
        notes: Vector::new(),
        start: Ticks::ZERO,
        end: None,
        looping: false,
        loop_start: Ticks::ZERO,
        loop_end: None,
        signature: None,
    };
    rig.apply(Command::CreateClip {
        track: a,
        clip: Clip::new(clip, Ticks::WHOLE, Ticks::WHOLE, ClipData::Midi(data)),
    });
    rig.apply(Command::SetClipEnvelopeBreakpoint {
        clip,
        target: volume(),
        at: Ticks::ZERO,
        value: 0.0,
    });
    rig.apply(Command::SetClipEnvelopeBreakpoint {
        clip,
        target: volume(),
        at: Ticks::WHOLE,
        value: 1.0,
    });
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(BAR * 3, 64);
    assert!(
        out[..BAR].iter().all(|&s| s == want(0.0)),
        "before the clip the fader plays alone"
    );
    for (frame, &sample) in out[BAR..BAR * 2].iter().enumerate() {
        let expected = want(ramp_value(frame, 0, BAR));
        assert!(
            (sample - expected).abs() < 1e-6,
            "frame {}: {sample} isn't {expected}",
            frame + BAR
        );
    }
    assert!(
        out[BAR * 2..].iter().all(|&s| s == want(0.0)),
        "after it, again"
    );
}

// Spec: clip-envelopes R5
#[test]
fn clearing_an_envelope_hands_the_parameter_back() {
    let (mut rig, a) = rig_at(64, 0.5);
    let clip = looping_clip(&mut rig, a, Ticks::ZERO, 1);
    rig.apply(Command::SetClipEnvelopeBreakpoint {
        clip,
        target: volume(),
        at: Ticks::ZERO,
        value: 0.25,
    });
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(256, 64);
    assert!(
        out.iter().all(|&s| (s - want(0.25)).abs() < 1e-6),
        "the envelope drives it"
    );
    let synced = rig.apply(Command::DeleteClipEnvelope {
        clip,
        target: volume(),
    });
    assert!(!synced.program, "clearing an envelope needs no new program");
    let out = rig.render(2_048, 64);
    assert!(
        out[..960].windows(2).all(|w| w[1] < w[0]),
        "the fader glides back down to its control"
    );
    assert!(
        out[960..].iter().all(|&s| s == want(0.0)),
        "and rests at the control again"
    );
}
