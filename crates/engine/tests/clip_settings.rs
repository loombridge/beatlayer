//! An audio clip's sound settings as automation and clip-envelope targets
//! (WP-4.8, the warp modes spec R29 to R33): the track's automation and the
//! clip's envelopes move Gain, Transpose, Detune, Sample Offset and the
//! modes' controls while the clip plays, the same sample at any block size,
//! every block a real-time section under the allocation trap.

use crate::common;
use std::sync::Arc;

use bl_core::time::{Bpm, TempoMap, Ticks};
use bl_engine::{EngineConfig, MediaMap, Offline, Registry, TransportCommand};
use bl_media::SampleData;
use bl_model::{
    AudioData, AutomationTarget, Clip, ClipData, ClipId, ClipParam, Command, ContentHash, Editor,
    EnvelopeKind, Preserve, TrackId, TrackKind, TransientLoopMode, WarpMarker, WarpMode,
};
use common::{RATE, track};

/// A file of ones.
const DC: ContentHash = ContentHash([21; 32]);
/// A 440 Hz sine at half scale.
const SINE: ContentHash = ContentHash([22; 32]);
/// A ramp that reads its own position: frame `n` holds `n` over [`RAMP`].
const POSITION: ContentHash = ContentHash([23; 32]);
const RAMP: f32 = 480_000.0;

fn media() -> MediaMap {
    let mut map = MediaMap::new();
    let file = |samples: Vec<f32>| Arc::new(SampleData::new(48_000, vec![samples]).unwrap());
    map.insert(DC, file(vec![1.0; 96_000]));
    map.insert(
        SINE,
        file(
            (0..96_000)
                .map(|n| {
                    0.5 * (std::f64::consts::TAU * 440.0 * f64::from(n) / 48_000.0).sin() as f32
                })
                .collect(),
        ),
    );
    map.insert(
        POSITION,
        file((0..RAMP as usize).map(|n| n as f32 / RAMP).collect()),
    );
    map
}

/// An empty song at 120 BPM with an audio track.
fn song() -> (Editor, TrackId) {
    let mut editor = Editor::new();
    editor
        .apply(Command::SetTempo {
            tempo: TempoMap::constant(Bpm::new(120.0).unwrap()),
        })
        .unwrap();
    let audio = track(&mut editor, TrackKind::Audio, None);
    (editor, audio)
}

/// A warped clip of `file` from the song's start for `length`, at 120 BPM, in
/// `mode`; or an unwarped one when `warp` is false.
fn add_clip(
    editor: &mut Editor,
    track: TrackId,
    at: Ticks,
    length: Ticks,
    file: ContentHash,
    mode: Option<WarpMode>,
) -> ClipId {
    let audio = AudioData {
        warp: mode.is_some(),
        warp_markers: if mode.is_some() {
            vec![WarpMarker {
                time: 0.0,
                beat: Ticks::ZERO,
            }]
        } else {
            Vec::new()
        },
        tempo: mode.map(|_| 120.0),
        warp_mode: mode.unwrap_or_default(),
        ..AudioData::new(file)
    };
    let id = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(id, at, length, ClipData::Audio(audio)),
        })
        .unwrap();
    id
}

fn envelope(editor: &mut Editor, clip: ClipId, param: ClipParam, points: &[(Ticks, f64)]) {
    for &(at, value) in points {
        editor
            .apply(Command::SetClipEnvelopeBreakpoint {
                clip,
                target: AutomationTarget::Clip { param },
                kind: EnvelopeKind::Modulation,
                at,
                value,
            })
            .unwrap();
    }
}

fn automate(editor: &mut Editor, track: TrackId, param: ClipParam, points: &[(Ticks, f64)]) {
    for &(at, value) in points {
        editor
            .apply(Command::SetAutomationBreakpoint {
                track,
                target: AutomationTarget::Clip { param },
                at,
                value,
            })
            .unwrap();
    }
}

/// Plays `editor` for `frames` frames in blocks of `block`, under the
/// allocation trap, and returns the left channel.
fn play(editor: &Editor, frames: u64, block: usize) -> Vec<f32> {
    let registry = Registry::new().with_media(Arc::new(media()));
    let mut offline = Offline::new(EngineConfig::new(RATE, block), registry);
    offline.sync(editor.document());
    assert!(offline.missing_media().is_empty());
    offline.sender().transport(TransportCommand::Play);
    let mut out = Vec::new();
    offline.render(frames, |output| out.extend_from_slice(output.channel(0)));
    assert_eq!(offline.heap_operations(), 0, "block size {block}");
    out
}

/// The same at blocks of 1, 61, 512 and 4,096 (R33).
fn same_at_any_block(editor: &Editor, frames: u64) -> Vec<f32> {
    let want = play(editor, frames, 512);
    for block in [1, 61, 4_096] {
        assert_eq!(play(editor, frames, block), want, "blocks of {block}");
    }
    want
}

/// The frequency of a tone in `out[from..to]`, in Hz, from its rising zero
/// crossings.
fn frequency(out: &[f32], from: usize, to: usize) -> f64 {
    let mut crossings = Vec::new();
    for n in from..to - 1 {
        if out[n] <= 0.0 && out[n + 1] > 0.0 {
            crossings.push(n as f64 + f64::from(-out[n]) / f64::from(out[n + 1] - out[n]));
        }
    }
    let cycles = (crossings.len() - 1) as f64;
    cycles * 48_000.0 / (crossings[crossings.len() - 1] - crossings[0])
}

// Spec: warp-modes R29. A Gain envelope is a share of the clip's Gain, so it
// takes the clip to silence and never above its Gain.
#[test]
fn a_gain_envelope_never_exceeds_the_clips_gain() {
    let (mut editor, audio) = song();
    let clip = add_clip(&mut editor, audio, Ticks::ZERO, Ticks::WHOLE, DC, None);
    editor
        .apply(Command::SetClipGain { clip, gain: 0.5 })
        .unwrap();
    envelope(
        &mut editor,
        clip,
        ClipParam::Gain,
        &[(Ticks::ZERO, 100.0), (Ticks::WHOLE, 0.0)],
    );
    let out = same_at_any_block(&editor, 96_000);
    assert!(out.iter().all(|&s| s <= 0.5 + 1e-6), "never above the Gain");
    assert!((out[0] - 0.5).abs() < 1e-6);
    assert!((out[48_000] - 0.25).abs() < 1e-4, "half way, half the Gain");
    assert!(out[95_999] < 1e-4, "−∞ dB at the share's bottom");
}

// Spec: warp-modes R31. The track's automation of a setting drives every
// audio clip of the track as it plays; a hand move of a clip's setting
// overrides it until Re-Enable Automation.
#[test]
fn a_tracks_clip_automation_drives_its_clips_settings() {
    let (mut editor, audio) = song();
    let half = Ticks::WHOLE / 2;
    let first = add_clip(&mut editor, audio, Ticks::ZERO, half, DC, None);
    let second = add_clip(&mut editor, audio, half, half, DC, None);
    editor
        .apply(Command::SetClipGain {
            clip: second,
            gain: 0.5,
        })
        .unwrap();
    automate(&mut editor, audio, ClipParam::Gain, &[(Ticks::ZERO, 0.25)]);
    let out = same_at_any_block(&editor, 96_000);
    for at in [100, 20_000, 47_900, 48_100, 70_000, 95_000] {
        assert!((out[at] - 0.25).abs() < 1e-6, "frame {at}: {}", out[at]);
    }
    // A hand move of the first clip's Gain turns the automation off, for the
    // track's clips both.
    editor
        .apply(Command::SetClipGain {
            clip: first,
            gain: 0.8,
        })
        .unwrap();
    let out = play(&editor, 96_000, 512);
    assert!((out[20_000] - 0.8).abs() < 1e-6);
    assert!((out[70_000] - 0.5).abs() < 1e-6);
    editor.apply(Command::ReEnableAutomation {}).unwrap();
    let out = play(&editor, 96_000, 512);
    assert!((out[20_000] - 0.25).abs() < 1e-6);
    assert!((out[70_000] - 0.25).abs() < 1e-6);
}

// Spec: warp-modes R25, R26, R29, R33. A Transposition envelope moves the
// pitch of every mode but Re-Pitch (R10), the same at any block size.
#[test]
fn clip_envelopes_modulate_gain_transposition_and_warp_settings() {
    for mode in [
        WarpMode::Beats,
        WarpMode::Tones,
        WarpMode::Texture,
        WarpMode::Complex,
        WarpMode::ComplexPro,
        WarpMode::RePitch,
    ] {
        let (mut editor, audio) = song();
        let clip = add_clip(
            &mut editor,
            audio,
            Ticks::ZERO,
            Ticks::WHOLE,
            SINE,
            Some(mode),
        );
        envelope(
            &mut editor,
            clip,
            ClipParam::Transpose,
            &[(Ticks::ZERO, 12.0)],
        );
        let out = same_at_any_block(&editor, 14_400);
        let hz = frequency(&out, 4_800, 14_400);
        let want = if mode == WarpMode::RePitch {
            440.0
        } else {
            880.0
        };
        assert!(
            (hz - want).abs() < want * 0.03,
            "{mode:?} plays {hz} Hz, not {want}"
        );
    }
}

// Spec: warp-modes R17, R29. Beats reads each segment at the pitch ratio the
// envelope has where the segment begins.
#[test]
fn beats_reads_each_segment_at_the_ratio_it_begins_with() {
    let (mut editor, audio) = song();
    let clip = add_clip(
        &mut editor,
        audio,
        Ticks::ZERO,
        Ticks::WHOLE,
        SINE,
        Some(WarpMode::Beats),
    );
    editor
        .apply(Command::SetClipWarpSettings {
            clip,
            beats: Some(bl_model::BeatsUpdate {
                preserve: Some(Preserve::Eighth),
                ..Default::default()
            }),
            tones: None,
            texture: None,
            complex_pro: None,
        })
        .unwrap();
    // 0 to 12 semitones over the clip's two seconds.
    envelope(
        &mut editor,
        clip,
        ClipParam::Transpose,
        &[(Ticks::ZERO, 0.0), (Ticks::WHOLE, 12.0)],
    );
    let out = same_at_any_block(&editor, 96_000);
    // An eighth note is a quarter of a second, 12,000 frames.
    for (segment, semitones) in [(1, 1.5), (3, 4.5), (5, 7.5)] {
        let start = segment * 12_000;
        let hz = frequency(&out, start + 500, start + 8_500);
        let want = 440.0 * 2f64.powf(semitones / 12.0);
        assert!(
            (hz - want).abs() < want * 0.03,
            "segment {segment} plays {hz} Hz, not {want}"
        );
    }
}

// Spec: warp-modes R30. Sample Offset moves each Beats segment's source by
// its value in sixteenths of the clip's beats.
#[test]
fn sample_offset_moves_each_segments_source() {
    let (mut editor, audio) = song();
    let clip = add_clip(
        &mut editor,
        audio,
        Ticks::ZERO,
        Ticks::WHOLE,
        POSITION,
        Some(WarpMode::Beats),
    );
    editor
        .apply(Command::SetClipWarpSettings {
            clip,
            beats: Some(bl_model::BeatsUpdate {
                preserve: Some(Preserve::Quarter),
                transient_loop_mode: Some(TransientLoopMode::LoopOff),
                ..Default::default()
            }),
            tones: None,
            texture: None,
            complex_pro: None,
        })
        .unwrap();
    let plain = same_at_any_block(&editor, 96_000);
    let beat = 24_000;
    assert!(
        (plain[100] - 100.0 / RAMP).abs() < 1e-6,
        "no offset, no move"
    );
    // Four sixteenths is a beat: every segment takes the audio of the next.
    envelope(
        &mut editor,
        clip,
        ClipParam::SampleOffset,
        &[(Ticks::ZERO, 4.0)],
    );
    let out = same_at_any_block(&editor, 96_000);
    for at in [100, 5_000, beat + 100] {
        let want = (at + beat) as f32 / RAMP;
        assert!((out[at] - want).abs() < 1e-6, "frame {at}: {}", out[at]);
    }
    // A beat earlier: the first segment reads before the file, in silence.
    let (mut editor, audio) = song();
    let clip = add_clip(
        &mut editor,
        audio,
        Ticks::ZERO,
        Ticks::WHOLE,
        POSITION,
        Some(WarpMode::Beats),
    );
    editor
        .apply(Command::SetClipWarpSettings {
            clip,
            beats: Some(bl_model::BeatsUpdate {
                preserve: Some(Preserve::Quarter),
                transient_loop_mode: Some(TransientLoopMode::LoopOff),
                ..Default::default()
            }),
            tones: None,
            texture: None,
            complex_pro: None,
        })
        .unwrap();
    envelope(
        &mut editor,
        clip,
        ClipParam::SampleOffset,
        &[(Ticks::ZERO, -4.0)],
    );
    let out = play(&editor, 96_000, 512);
    assert_eq!(out[100], 0.0);
    assert!((out[beat + 100] - 100.0 / RAMP).abs() < 1e-6);
}

/// A clip of the sine in `mode` transposed five semitones, with Beats cut at
/// each eighth note, so Transient Envelope has segments short enough to act in
/// the first moments.
fn transposed(mode: WarpMode) -> (Editor, TrackId, ClipId) {
    let (mut editor, audio) = song();
    let clip = add_clip(
        &mut editor,
        audio,
        Ticks::ZERO,
        Ticks::WHOLE,
        SINE,
        Some(mode),
    );
    editor
        .apply(Command::SetClipPitch {
            clip,
            transpose: Some(5),
            detune: None,
        })
        .unwrap();
    editor
        .apply(Command::SetClipWarpSettings {
            clip,
            beats: Some(bl_model::BeatsUpdate {
                preserve: Some(Preserve::Eighth),
                ..Default::default()
            }),
            tones: None,
            texture: None,
            complex_pro: None,
        })
        .unwrap();
    (editor, audio, clip)
}

// Spec: warp-modes R33. Automation and envelopes of the modes' controls read
// frame by frame: the same sample at any block size, and not the unmoved
// clip's.
#[test]
fn clip_settings_read_frame_by_frame_at_any_block_size() {
    for (mode, param, to, on_the_track) in [
        (WarpMode::ComplexPro, ClipParam::Formants, 0.0, true),
        (
            WarpMode::ComplexPro,
            ClipParam::ComplexProEnvelope,
            400.0,
            true,
        ),
        (WarpMode::Tones, ClipParam::TonesGrainSize, 90.0, true),
        (WarpMode::Texture, ClipParam::TextureFlux, 90.0, false),
        (WarpMode::Texture, ClipParam::TextureGrainSize, 300.0, false),
        (WarpMode::Beats, ClipParam::TransientEnvelope, -80.0, false),
        (WarpMode::Tones, ClipParam::Detune, 40.0, false),
        (WarpMode::Tones, ClipParam::HighQuality, 0.0, true),
    ] {
        let (mut editor, audio, clip) = transposed(mode);
        let from = param.descriptor().default;
        if on_the_track {
            automate(
                &mut editor,
                audio,
                param,
                &[(Ticks::ZERO, from), (Ticks::WHOLE / 4, to)],
            );
        } else {
            envelope(
                &mut editor,
                clip,
                param,
                &[(Ticks::ZERO, 0.0), (Ticks::WHOLE / 4, to)],
            );
        }
        let moved = same_at_any_block(&editor, 14_400);
        let (still, _, _) = transposed(mode);
        assert_ne!(
            moved,
            play(&still, 14_400, 512),
            "{mode:?} {param:?} is moved"
        );
    }
}
