//! Clip envelopes (WP-3.15): an envelope a clip carries modulates the
//! parameter it names while the clip plays, on the frames the automation
//! player's read path gives, following the clip's own timeline or its own.
//! Every block runs as a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::{Bpm, TempoMap, Ticks};
use bl_engine::TransportCommand;
use bl_model::fader::{fader_gain, fader_position};
use bl_model::{
    AutomationTarget, Clip, ClipData, ClipId, Command, MidiData, MixerParam, TrackId, TrackKind,
    Vector,
};
use common::{RATE, Rig, dc, track};

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

/// The sample the DC device's 1.0 becomes through a fader at 0.5 under a
/// volume envelope at `share` (modulation spec, R4): the fader's position
/// brought down to that share of its own, so 1.0 leaves the fader as it is
/// and 0.0 is silence.
fn want(share: f64) -> f32 {
    fader_gain(fader_position(0.5) * share) as f32
}

/// What the fader plays alone: 0.5.
fn alone() -> f32 {
    want(1.0)
}

// Spec: clip-envelopes R3, R4
#[test]
fn a_linked_clip_envelope_drives_the_parameter_while_the_clip_plays() {
    let (mut rig, a) = rig_at(64, 0.5);
    let clip = looping_clip(&mut rig, a, Ticks::ZERO, 2);
    rig.apply(Command::SetClipEnvelopeBreakpoint {
        kind: bl_model::EnvelopeKind::Modulation,
        clip,
        target: volume(),
        at: Ticks::ZERO,
        value: 0.0,
    });
    let synced = rig.apply(Command::SetClipEnvelopeBreakpoint {
        kind: bl_model::EnvelopeKind::Modulation,
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
    // The envelope follows the clip's one-bar loop: each bar ramps the
    // fader's share from silence to its own 0.5, and after the clip the
    // fader plays alone.
    for (frame, &sample) in out[..BAR * 2].iter().enumerate() {
        let expected = want(ramp_value(frame % BAR, 0, BAR));
        assert!(
            (sample - expected).abs() < 1e-6,
            "frame {frame}: {sample} isn't {expected}"
        );
    }
    assert!(
        out[BAR * 2..].iter().all(|&s| s == alone()),
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
        kind: bl_model::EnvelopeKind::Modulation,
        clip,
        target: volume(),
        at: Ticks::ZERO,
        value: 0.0,
    });
    fine.apply(Command::SetClipEnvelopeBreakpoint {
        kind: bl_model::EnvelopeKind::Modulation,
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
    // it doesn't repeat, so the fader climbs from silence to its own level
    // across both bars.
    rig.apply(Command::SetClipEnvelopeLinked {
        kind: bl_model::EnvelopeKind::Modulation,
        clip,
        target: volume(),
        linked: false,
    });
    rig.apply(Command::SetClipEnvelopeBreakpoint {
        kind: bl_model::EnvelopeKind::Modulation,
        clip,
        target: volume(),
        at: Ticks::ZERO,
        value: 0.0,
    });
    rig.apply(Command::SetClipEnvelopeBreakpoint {
        kind: bl_model::EnvelopeKind::Modulation,
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
        out[BAR * 2..].iter().all(|&s| s == alone()),
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
        kind: bl_model::EnvelopeKind::Modulation,
        clip,
        target: volume(),
        at: Ticks::ZERO,
        value: 0.0,
    });
    rig.apply(Command::SetClipEnvelopeBreakpoint {
        kind: bl_model::EnvelopeKind::Modulation,
        clip,
        target: volume(),
        at: Ticks::WHOLE,
        value: 1.0,
    });
    rig.sender.transport(TransportCommand::Play);
    let out = rig.render(BAR * 3, 64);
    assert!(
        out[..BAR].iter().all(|&s| s == alone()),
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
        out[BAR * 2..].iter().all(|&s| s == alone()),
        "after it, again"
    );
}

// Spec: clip-envelopes R5
#[test]
fn clearing_an_envelope_hands_the_parameter_back() {
    let (mut rig, a) = rig_at(64, 0.5);
    let clip = looping_clip(&mut rig, a, Ticks::ZERO, 1);
    rig.apply(Command::SetClipEnvelopeBreakpoint {
        kind: bl_model::EnvelopeKind::Modulation,
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
        kind: bl_model::EnvelopeKind::Modulation,
        clip,
        target: volume(),
    });
    assert!(!synced.program, "clearing an envelope needs no new program");
    let out = rig.render(2_048, 64);
    assert!(
        out[..960].windows(2).all(|w| w[1] > w[0]),
        "the fader glides back up to its control"
    );
    assert!(
        out[960..].iter().all(|&s| s == alone()),
        "and rests at the control again"
    );
}

// Spec: clip-envelopes R2
#[test]
fn a_linked_clip_envelope_lands_where_the_tempo_map_places_it() {
    let (mut rig, a) = rig_at(64, 0.5);
    // The tempo ramps from 120 to 240 BPM over the bar's first quarter, so
    // the envelope's middle breakpoint — the bar's half — doesn't sit at the
    // segment's frame midpoint: each breakpoint lands on the frame the tempo
    // map gives its position, the frame a note at the same position plays
    // on.
    let tempo = TempoMap::from_breakpoints([
        (Ticks::ZERO, Bpm::new(120.0).unwrap()),
        (Ticks::WHOLE / 4, Bpm::new(240.0).unwrap()),
    ])
    .unwrap();
    rig.apply(Command::SetTempo {
        tempo: tempo.clone(),
    });
    let clip = looping_clip(&mut rig, a, Ticks::ZERO, 1);
    for (at, value) in [
        (Ticks::ZERO, 0.0),
        (Ticks::WHOLE / 2, 0.5),
        (Ticks::WHOLE, 1.0),
    ] {
        rig.apply(Command::SetClipEnvelopeBreakpoint {
            kind: bl_model::EnvelopeKind::Modulation,
            clip,
            target: volume(),
            at,
            value,
        });
    }
    rig.sender.transport(TransportCommand::Play);
    let end = tempo.ticks_to_frames(Ticks::WHOLE, RATE).get() as usize;
    let out = rig.render(end + 64, 64);
    assert!(
        out[..end].windows(2).all(|w| w[1] >= w[0]),
        "the envelope climbs from the clip's start to its end"
    );
    let middle = tempo.ticks_to_frames(Ticks::WHOLE / 2, RATE).get() as usize;
    assert!(
        (out[middle] - want(0.5)).abs() < 1e-6,
        "the breakpoint lands on the frame the tempo map gives it: {} isn't {}",
        out[middle],
        want(0.5)
    );
    // Between the breakpoints the line runs between the frames they were
    // placed on, the way a track envelope's does.
    let quarter = tempo.ticks_to_frames(Ticks::WHOLE / 4, RATE).get() as usize;
    let line = 0.5 * quarter as f64 / middle as f64;
    assert!(
        (out[quarter] - want(line)).abs() < 1e-6,
        "frame {quarter}: {} isn't {}",
        out[quarter],
        want(line)
    );
    assert!(
        out[end..].iter().all(|&s| s == alone()),
        "after the clip the fader plays alone"
    );
}
