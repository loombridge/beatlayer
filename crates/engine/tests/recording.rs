//! WP-2.13's acceptance in the engine: recording armed tracks' input into
//! rings, and Monitor Auto closing the input while a track's clips play
//! (the routing spec, R11).
//!
//! - A take starts on the frame playback started on, holds exactly the
//!   frames played, and ends when the transport stops; its rings are closed
//!   for the writer.
//! - A recording is moved earlier by the plan's delay: the frames that
//!   moves it past aren't written (WP-2.13).
//! - Punch-In and Punch-Out write only between the loop brace's ends,
//!   whether or not the Loop switch is on, and playback carries on past
//!   the punch-out point.
//! - A ring too full for a block's samples ends the take before them and
//!   counts them as dropped: nothing is dropped silently.
//! - Under Monitor Auto a track's input closes while its clips play and
//!   opens again when they end.
//! - Every block runs as a real-time section under the allocation trap,
//!   recording included.
//!
//! Routing and transport commands run by ID with JSON arguments, as an
//! agent runs them (D17).

bl_rt::install_trap!();

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bl_commands::Registry as Commands;
use bl_core::time::{Frames, Ticks};
use bl_engine::{
    DeviceInput, LoopRegion, MediaMap, RecordTrack, RecordingPlan, Registry, TakeBounds,
    TransportCommand,
};
use bl_media::SampleData;
use bl_model::{Channel, ContentHash, TrackId, TrackKind, registry};
use common::{Rig, track};
use rtrb::{Consumer, RingBuffer};
use serde_json::{Value, json};

/// Runs a document command by ID, as an agent does.
fn run(editor: &mut bl_model::Editor, id: &str, args: Value) {
    let mut commands = Commands::new();
    registry::register(&mut commands).unwrap();
    let spec = commands.get(id).unwrap();
    bl_commands::validate(spec.args.as_ref().unwrap(), &args).unwrap();
    registry::run(editor, id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
}

/// An audio track whose Audio From is Ext. In `channel`, armed.
fn armed(rig: &mut Rig, channel: u16) -> TrackId {
    let id = track(&mut rig.editor, TrackKind::Audio, None);
    run(
        &mut rig.editor,
        "routing.set_input",
        json!({
            "track": id,
            "input": { "type": "external", "channel": { "type": "mono", "channel": channel } },
        }),
    );
    run(
        &mut rig.editor,
        "mixer.set_arm",
        json!({ "track": id, "arm": true }),
    );
    id
}

/// A recording plan with one ring per track, `capacity` samples each, and
/// the rings' reading ends and drop counters.
fn plan(
    tracks: &[(TrackId, Channel)],
    capacity: usize,
) -> (RecordingPlan, Vec<Consumer<f32>>, Vec<Arc<AtomicU64>>) {
    let mut planned = Vec::new();
    let mut consumers = Vec::new();
    let mut dropped = Vec::new();
    for &(track, channel) in tracks {
        let (producer, consumer) = RingBuffer::new(capacity);
        let count = Arc::new(AtomicU64::new(0));
        dropped.push(Arc::clone(&count));
        planned.push(RecordTrack {
            track,
            channel,
            producer,
            dropped: count,
        });
        consumers.push(consumer);
    }
    (
        RecordingPlan {
            tracks: planned,
            midi: Vec::new(),
            delay: 0,
            session: Vec::new(),
            launches: None,
        },
        consumers,
        dropped,
    )
}

/// One block of the device's input: four channels, the third carrying the
/// device frame's own number, so a recorded file shows which frames it
/// holds. The rest is silence.
struct Input {
    channels: Vec<Vec<f32>>,
}

impl Input {
    /// A block of `frames` frames starting at device frame `next`, which is
    /// moved on.
    fn ramp(next: &mut u32, frames: usize) -> Self {
        let mut channels = vec![vec![0.0; frames]; 4];
        for (i, sample) in channels[2].iter_mut().enumerate() {
            *sample = (*next + i as u32) as f32;
        }
        *next += frames as u32;
        Self { channels }
    }
}

impl DeviceInput for Input {
    fn channels(&self) -> usize {
        self.channels.len()
    }

    fn channel(&self, index: usize) -> &[f32] {
        &self.channels[index]
    }
}

/// Drains a ring's samples.
fn drain(ring: &mut Consumer<f32>) -> Vec<f32> {
    let mut samples = Vec::new();
    while let Ok(sample) = ring.pop() {
        samples.push(sample);
    }
    samples
}

// Spec: routing R11 (WP-2.13). A take starts where playback started, holds
// the frames played, and ends at the stop; the input is recorded whether
// the track hears it or not.
#[test]
fn a_take_holds_each_armed_tracks_input_from_the_frame_playback_started_on() {
    let mut rig = Rig::with_input(256, 4);
    let a = armed(&mut rig, 3);
    // Monitor Off: the track hears nothing, and the take records its input
    // all the same.
    run(
        &mut rig.editor,
        "routing.set_monitor",
        json!({ "track": a, "monitor": "off" }),
    );
    rig.sync();

    let (plan, mut rings, _) = plan(&[(a, Channel::Mono(3))], 1 << 12);
    rig.sender.record(rig.collector.handle().wrap(plan));
    // Playback starts on two quarter notes (1 second), so the take starts
    // on its frame.
    rig.sender
        .transport(TransportCommand::SetInsertMarker(Ticks::QUARTER * 2));
    rig.sender.transport(TransportCommand::SetRecording(true));
    rig.sender.transport(TransportCommand::Play);
    let mut next = 0;
    for _ in 0..4 {
        let input = Input::ramp(&mut next, 256);
        rig.block_with_input(256, &input);
    }
    rig.sender.transport(TransportCommand::Stop);
    let input = Input::ramp(&mut next, 256);
    rig.block_with_input(256, &input);

    let state = rig.monitor.state();
    assert!(!state.recording, "the stop ends the recording");
    assert_eq!(
        state.take,
        Some(TakeBounds {
            start: Frames(48_000),
            end: Frames(49_024),
            wrap: None,
        }),
        "four blocks of 256 frames from the frame playback started on"
    );
    // The plan went with the take's end, so the writer sees the rings
    // closed and drains exactly the take's frames. The drop thread frees
    // it; the test collects instead of waiting.
    rig.collector.collect();
    assert!(rings[0].is_abandoned(), "the plan was dropped");
    let samples = drain(&mut rings[0]);
    assert_eq!(samples.len(), 1024);
    for (i, sample) in samples.iter().enumerate() {
        assert_eq!(*sample, i as f32, "frame {i} is the one the device had");
    }
}

// Spec: routing R11 (WP-2.13). A ring too full ends the take before the
// frames it can't take, and counts them: a slow writer drops nothing
// silently.
#[test]
fn a_full_ring_ends_the_take_and_counts_its_dropped_frames() {
    let mut rig = Rig::with_input(8, 4);
    let a = armed(&mut rig, 3);
    rig.sync();

    // A ring of 16 samples takes two 8-frame blocks and no more.
    let (plan, mut rings, dropped) = plan(&[(a, Channel::Mono(3))], 16);
    rig.sender.record(rig.collector.handle().wrap(plan));
    rig.sender.transport(TransportCommand::SetRecording(true));
    rig.sender.transport(TransportCommand::Play);
    let mut next = 0;
    for _ in 0..3 {
        let input = Input::ramp(&mut next, 8);
        rig.block_with_input(8, &input);
    }

    let state = rig.monitor.state();
    assert!(!state.recording, "the take ended when its ring filled");
    assert_eq!(
        state.take,
        Some(TakeBounds {
            start: Frames::ZERO,
            end: Frames(16),
            wrap: None,
        }),
        "the two blocks the ring took"
    );
    assert_eq!(dropped[0].load(Ordering::Relaxed), 8, "the third block");
    let samples = drain(&mut rings[0]);
    assert_eq!(samples.len(), 16, "the file holds the take's frames");
}

// Spec: recording R11 (WP-2.13). The Arrangement loop's wrap doesn't end the
// take: it goes on into the next pass, so the file holds the part before the
// loop, then every pass, one after another, and the take names the loop it
// went round.
#[test]
fn a_take_goes_on_through_the_loops_wraps() {
    let mut rig = Rig::with_input(256, 4);
    let a = armed(&mut rig, 3);
    rig.sync();

    let (plan, mut rings, _) = plan(&[(a, Channel::Mono(3))], 1 << 18);
    rig.sender.record(rig.collector.handle().wrap(plan));
    // At 120 bpm a quarter note is 24,000 frames: the loop is the second
    // one, and playback starts a quarter note before it.
    rig.sender
        .transport(TransportCommand::SetLoop(Some(LoopRegion {
            start: Ticks::QUARTER,
            end: Ticks::QUARTER * 2,
        })));
    rig.sender.transport(TransportCommand::SetRecording(true));
    rig.sender.transport(TransportCommand::Play);
    // The lead-in, two full passes, and 1,000 frames of a third.
    let mut next = 0;
    for _ in 0..285 {
        let input = Input::ramp(&mut next, 256);
        rig.block_with_input(256, &input);
    }
    let input = Input::ramp(&mut next, 40);
    rig.block_with_input(40, &input);
    rig.sender.transport(TransportCommand::Stop);
    let input = Input::ramp(&mut next, 256);
    rig.block_with_input(256, &input);

    let state = rig.monitor.state();
    assert_eq!(
        state.take,
        Some(TakeBounds {
            start: Frames::ZERO,
            end: Frames(25_000),
            wrap: Some((Frames(24_000), Frames(48_000))),
        }),
        "one take, 1,000 frames into the third pass"
    );
    rig.collector.collect();
    let samples = drain(&mut rings[0]);
    assert_eq!(samples.len(), 73_000);
    for (i, sample) in samples.iter().enumerate() {
        assert_eq!(*sample, i as f32, "frame {i} follows the one before it");
    }
}

// Spec: recording R11 (WP-2.13). A locate away from the loop still ends the
// take, as it ends one outside a loop.
#[test]
fn a_locate_inside_a_loop_still_ends_the_take() {
    let mut rig = Rig::with_input(256, 4);
    let a = armed(&mut rig, 3);
    rig.sync();

    let (plan, _rings, _) = plan(&[(a, Channel::Mono(3))], 1 << 18);
    rig.sender.record(rig.collector.handle().wrap(plan));
    rig.sender
        .transport(TransportCommand::SetLoop(Some(LoopRegion {
            start: Ticks::QUARTER,
            end: Ticks::QUARTER * 2,
        })));
    rig.sender.transport(TransportCommand::SetRecording(true));
    rig.sender.transport(TransportCommand::Play);
    let mut next = 0;
    for _ in 0..2 {
        let input = Input::ramp(&mut next, 256);
        rig.block_with_input(256, &input);
    }
    rig.sender
        .transport(TransportCommand::Locate(Ticks::QUARTER * 3));
    let input = Input::ramp(&mut next, 256);
    rig.block_with_input(256, &input);
    let state = rig.monitor.state();
    assert!(!state.recording, "the locate ended the take");
    assert_eq!(state.take.map(|take| take.wrap), Some(None));
}

/// A DC file of `value`, playable by a clip.
fn dc_file(value: f32) -> Arc<SampleData> {
    let channel = vec![value; 48_000];
    Arc::new(SampleData::new(48_000, vec![channel.clone(), channel]).unwrap())
}

// Spec: routing R11 (WP-2.13). Under Monitor Auto a track's input closes
// while its clips play; past the clip it opens again.
#[test]
fn an_auto_tracks_input_closes_while_its_clips_play_and_opens_after_them() {
    let mut media = MediaMap::new();
    let hash = ContentHash([7; 32]);
    media.insert(hash, dc_file(0.25));
    let mut rig = Rig::with_input_and_registry(256, 4, |registry: &mut Registry| {
        registry.set_media(Arc::new(media.clone()));
    });
    let a = armed(&mut rig, 3);
    let clip = rig.editor.new_id();
    rig.editor
        .apply(bl_model::Command::CreateClip {
            track: a,
            clip: bl_model::Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::QUARTER,
                bl_model::ClipData::Audio(bl_model::AudioData::new(hash)),
            ),
        })
        .unwrap();
    rig.sync();

    // The input carries 0.5 on channel 3; the clip carries 0.25, so which
    // one plays is visible in the samples.
    let level = |v: f32| {
        let channels = vec![vec![v; 256]; 4];
        Input { channels }
    };
    rig.sender.transport(TransportCommand::Play);
    // A 20 ms glide closes the input as the clip starts; after it, the
    // clip alone is heard.
    rig.render_with_input(1920, 256, &level(0.5));
    let last = rig.last();
    assert!(
        last.iter().all(|&s| s == 0.25),
        "the clip alone plays while it sounds: {last:?}"
    );

    // Past the clip's end the input opens again.
    rig.sender.transport(TransportCommand::Locate(Ticks::WHOLE));
    rig.render_with_input(1920, 256, &level(0.5));
    let last = rig.last();
    assert!(
        last.iter().all(|&s| s == 0.5),
        "the input is heard when no clip plays: {last:?}"
    );
}

/// Plays `blocks` blocks of `block` frames of the ramp from song frame 0,
/// recording the third input channel into one track's ring, and returns
/// that ring and the rig.
fn record_ramp(rig: &mut Rig, delay: i64, block: usize, blocks: usize) -> Consumer<f32> {
    let a = armed(rig, 3);
    rig.sync();
    let (mut plan, mut rings, _) = plan(&[(a, Channel::Mono(3))], 1 << 17);
    plan.delay = delay;
    rig.sender.record(rig.collector.handle().wrap(plan));
    rig.sender.transport(TransportCommand::SetRecording(true));
    rig.sender.transport(TransportCommand::Play);
    let mut next = 0;
    for _ in 0..blocks {
        let input = Input::ramp(&mut next, block);
        rig.block_with_input(block, &input);
    }
    rings.remove(0)
}

// Spec: recording R7 (WP-2.13). A recording moved earlier by `delay` frames
// leaves out the first `delay` frames of input, across blocks too, while the
// take still covers every frame played: the file's first sample is the input
// that sounded when the take began.
#[test]
fn a_delay_moves_the_recording_earlier_by_leaving_out_its_first_frames() {
    for delay in [100, 300] {
        let mut rig = Rig::with_input(256, 4);
        let mut ring = record_ramp(&mut rig, delay, 256, 4);
        rig.sender.transport(TransportCommand::Stop);
        rig.block(256);
        let take = rig.monitor.state().take.unwrap();
        let samples = drain(&mut ring);
        assert_eq!(take.start, Frames::ZERO);
        assert_eq!(take.end, Frames(1024), "the take covers the frames played");
        assert_eq!(samples.len(), 1024 - delay as usize);
        for (i, sample) in samples.iter().enumerate() {
            assert_eq!(
                *sample,
                (i as i64 + delay) as f32,
                "delay {delay}, frame {i}"
            );
        }
    }
}

// Spec: recording R7 (WP-2.13). A delay below zero leaves the input whole; the
// shell starts the clip later.
#[test]
fn a_negative_delay_leaves_the_input_whole() {
    let mut rig = Rig::with_input(256, 4);
    let mut ring = record_ramp(&mut rig, -100, 256, 4);
    rig.sender.transport(TransportCommand::Stop);
    rig.block(256);
    let take = rig.monitor.state().take.unwrap();
    assert_eq!((take.start, take.end), (Frames::ZERO, Frames(1024)));
    let samples = drain(&mut ring);
    assert_eq!(samples.len(), 1024);
    assert_eq!(samples[0], 0.0);
    assert_eq!(samples[1023], 1023.0);
}

/// Sets the loop brace to the second quarter note, 24,000 to 48,000 at
/// 120 BPM and 48 kHz, with the Loop switch off, and the punch switches.
fn punch_rig(punch_in: bool, punch_out: bool) -> Rig {
    let mut rig = Rig::with_input(256, 4);
    run(
        &mut rig.editor,
        "transport.set_loop",
        json!({ "start": Ticks::QUARTER.get(), "length": Ticks::QUARTER.get() }),
    );
    run(
        &mut rig.editor,
        "transport.set_punch_in",
        json!({ "enabled": punch_in }),
    );
    run(
        &mut rig.editor,
        "transport.set_punch_out",
        json!({ "enabled": punch_out }),
    );
    rig
}

// Spec: recording R6 (WP-2.13). Playing from before the brace with both
// switches on, the take is exactly the brace, and the Loop switch stays off.
// Playback carries on past the punch-out point with Arrangement Record
// still on, and nothing more is written.
#[test]
fn punch_records_only_between_the_brace_ends_and_playback_carries_on() {
    let mut rig = punch_rig(true, true);
    let mut ring = record_ramp(&mut rig, 0, 256, 300);
    let state = rig.monitor.state();
    assert!(state.playing && state.recording, "recording stays on");
    assert!(state.position > Frames(48_000), "playback carried on");
    assert_eq!(
        state.take,
        Some(TakeBounds {
            start: Frames(24_000),
            end: Frames(48_000),
            wrap: None,
        })
    );
    rig.collector.collect();
    assert!(
        ring.is_abandoned(),
        "the take closed its ring at the punch-out"
    );
    let samples = drain(&mut ring);
    assert_eq!(samples.len(), 24_000);
    assert_eq!(samples[0], 24_000.0, "the brace's first frame");
    assert_eq!(samples[23_999], 47_999.0, "the brace's last frame");
}

// Spec: recording R6 (WP-2.13). With Punch-In alone the take runs from the
// brace's start until Stop; with Punch-Out alone, from play start to the
// brace's end.
#[test]
fn each_punch_switch_alone_moves_only_its_own_end() {
    let mut rig = punch_rig(true, false);
    let mut ring = record_ramp(&mut rig, 0, 256, 300);
    rig.sender.transport(TransportCommand::Stop);
    rig.block(256);
    let take = rig.monitor.state().take.unwrap();
    assert_eq!(take.start, Frames(24_000));
    assert_eq!(take.end, Frames(300 * 256));
    assert_eq!(drain(&mut ring)[0], 24_000.0);

    let mut rig = punch_rig(false, true);
    let mut ring = record_ramp(&mut rig, 0, 256, 300);
    let take = rig.monitor.state().take.unwrap();
    assert_eq!(
        take,
        TakeBounds {
            start: Frames::ZERO,
            end: Frames(48_000),
            wrap: None,
        }
    );
    assert_eq!(drain(&mut ring).len(), 48_000);
}

// Spec: recording R9, R11 (WP-2.13). With Punch-Out on, the Loop's wrap ends
// the take, and the recording with it, instead of going on into the next
// pass.
#[test]
fn the_loops_wrap_ends_a_take_that_has_punch_out_on() {
    let mut rig = punch_rig(false, true);
    run(
        &mut rig.editor,
        "transport.set_loop_enabled",
        json!({ "enabled": true }),
    );
    let mut ring = record_ramp(&mut rig, 0, 256, 300);
    let state = rig.monitor.state();
    assert!(!state.recording, "the wrap ended the recording");
    assert_eq!(
        state.take,
        Some(TakeBounds {
            start: Frames::ZERO,
            end: Frames(48_000),
            wrap: None,
        })
    );
    assert_eq!(drain(&mut ring).len(), 48_000);
}

// Spec: recording R6 (WP-2.13). Playing from inside the brace records from
// play start.
#[test]
fn playing_from_inside_the_brace_records_from_play_start() {
    let mut rig = punch_rig(true, true);
    let a = armed(&mut rig, 3);
    rig.sync();
    let (plan, mut rings, _) = plan(&[(a, Channel::Mono(3))], 1 << 17);
    rig.sender.record(rig.collector.handle().wrap(plan));
    rig.sender.transport(TransportCommand::SetInsertMarker(
        Ticks::QUARTER + Ticks::QUARTER / 2,
    ));
    rig.sender.transport(TransportCommand::SetRecording(true));
    rig.sender.transport(TransportCommand::Play);
    let mut next = 0;
    for _ in 0..100 {
        let input = Input::ramp(&mut next, 256);
        rig.block_with_input(256, &input);
    }
    // The brace ends at 48,000 and play started at 36,000.
    assert_eq!(
        rig.monitor.state().take,
        Some(TakeBounds {
            start: Frames(36_000),
            end: Frames(48_000),
            wrap: None,
        })
    );
    assert_eq!(drain(&mut rings[0]).len(), 12_000);
}
