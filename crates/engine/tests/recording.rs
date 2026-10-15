//! WP-2.13's acceptance in the engine: recording armed tracks' input into
//! rings, and Monitor Auto closing the input while a track's clips play
//! (the routing spec, R11).
//!
//! - A take starts on the frame playback started on, holds exactly the
//!   frames played, and ends when the transport stops; its rings are closed
//!   for the writer.
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
    DeviceInput, MediaMap, RecordTrack, RecordingPlan, Registry, TakeBounds, TransportCommand,
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
    (RecordingPlan { tracks: planned }, consumers, dropped)
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
        }),
        "the two blocks the ring took"
    );
    assert_eq!(dropped[0].load(Ordering::Relaxed), 8, "the third block");
    let samples = drain(&mut rings[0]);
    assert_eq!(samples.len(), 16, "the file holds the take's frames");
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
