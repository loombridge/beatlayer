//! WP-1.5's acceptance for the UI's playhead: while the engine plays on a
//! real-time audio thread (the null device, paced like a sound card), a
//! second thread standing in for the UI reads the engine's state through
//! its triple buffer, without a lock, and every state it sees is the
//! playhead the engine played: right for its clock, through a start on an
//! exact frame and a loop that wraps, with its frames and ticks agreeing.
//! Every block runs under the allocation trap. The spec is
//! the transport spec, rule R20.
//!
//! `cargo test -p bl-engine --test engine_tests -- playhead:: --nocapture` prints what the
//! UI thread saw.

use crate::common;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use bl_audio_io::{
    AudioBuffer, AudioBufferMut, AudioCallback, Backend, NullBackend, NullOptions, Pacing,
    PortRequest, ProcessContext, StreamRequest,
};
use bl_core::time::{Frames, TempoMap, Ticks};
use bl_engine::testing::{self, Probe};
use bl_engine::{Engine, EngineConfig, Processor, TransportCommand, When};
use bl_model::{Command, Editor, TrackKind};
use common::{RATE, device, track};

/// The engine as a stream's callback. The stream runs it under the
/// allocation trap and counts heap use and dropouts.
struct Play {
    processor: Processor,
}

impl AudioCallback for Play {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        _input: &AudioBuffer<'_>,
        output: &mut AudioBufferMut<'_>,
    ) {
        let mut done = 0;
        while done < ctx.frames {
            let n = (ctx.frames - done).min(self.processor.config().max_frames);
            let block = self.processor.process(n);
            for c in 0..output.channels() {
                let from = block.channel(c.min(block.channels() - 1));
                output.channel_mut(c)[done..done + n].copy_from_slice(from);
            }
            done += n;
        }
    }
}

/// Where the song is, playing from the frame of the engine clock `start`
/// at song frame 0, looping from `first` to `last`, after `clock` frames.
fn expected(clock: u64, start: u64, first: i64, last: i64) -> (bool, i64) {
    if clock <= start {
        return (false, 0);
    }
    let played = (clock - start) as i64;
    let position = if played < last {
        played
    } else {
        first + (played - last) % (last - first)
    };
    (true, position)
}

#[test]
fn the_ui_thread_sees_the_playhead_the_engine_played() {
    let probe = Arc::new(Probe::default());
    let config = EngineConfig::new(RATE, 128);
    let (mut engine, processor) = Engine::start(config, testing::registry(&probe)).unwrap();

    // A loop from beat 2 to beat 3 at 120 BPM: half a second to a second.
    let mut editor = Editor::new();
    let a = track(&mut editor, TrackKind::Midi, None);
    device(&mut editor, a, testing::POSITION, &[]);
    editor
        .apply(Command::SetLoop {
            start: Ticks::QUARTER,
            length: Ticks::QUARTER,
        })
        .unwrap();
    editor
        .apply(Command::SetLoopEnabled { enabled: true })
        .unwrap();
    let tempo = TempoMap::default();
    let (first, last) = (24_000, 48_000);
    // Play starts on frame 4,801 of the engine clock, which no block of
    // 128 starts on.
    let start = 4_801;
    engine.sync(editor.document());
    engine.transport_at(TransportCommand::Play, When::Frame(start));
    assert!(engine.flush(Duration::from_secs(5)));

    let backend = NullBackend::with_options(NullOptions {
        dropout_after: None,
        pacing: Pacing::RealTime,
        frame_limit: None,
        channels: 2,
    });
    let request = StreamRequest {
        output: Some(PortRequest::default()),
        input: None,
        sample_rate: Some(RATE.hz()),
        buffer_size: Some(128),
    };
    let callback = Play { processor };
    let mut stream = backend.open_stream(&request, Box::new(callback)).unwrap();
    stream.start().unwrap();

    // The UI's side: read the state as often as a fast display would, and
    // more, until the song has looped a few times.
    let began = Instant::now();
    let mut seen = 0u64;
    let mut last_clock = 0;
    let mut wraps_seen = 0;
    let mut last_position = None;
    loop {
        let state = engine.state();
        if state.clock != last_clock {
            // Spec: transport R20
            let (playing, position) = expected(state.clock, start, first, last);
            assert_eq!(
                (state.playing, state.position),
                (playing, Frames(position)),
                "the state after clock {}",
                state.clock
            );
            assert_eq!(
                state.position_ticks,
                tempo.frames_to_ticks(state.position, RATE)
            );
            assert!(state.clock > last_clock, "the clock never runs backwards");
            if let Some(before) = last_position
                && position < before
            {
                wraps_seen += 1;
            }
            last_position = Some(position);
            last_clock = state.clock;
            seen += 1;
        }
        if state.clock > 140_000 {
            break;
        }
        assert!(
            began.elapsed() < Duration::from_secs(20),
            "the stream stalled at clock {}",
            state.clock
        );
        thread::sleep(Duration::from_micros(300));
    }
    stream.stop().unwrap();
    let stats = stream.stats();
    eprintln!(
        "the UI thread saw {seen} states over {last_clock} frames, {wraps_seen} of them just \
         after the loop wrapped; the audio thread ran {} blocks with {} dropouts and {} heap \
         operations",
        stats.blocks, stats.dropouts, stats.heap_operations
    );
    assert!(seen > 100, "only {seen} states seen");
    assert!(wraps_seen >= 3, "only {wraps_seen} wraps seen");
    assert!(stats.audio_thread.unwrap().trap_installed);
    assert_eq!(stats.heap_operations, 0);
    // A dropout is a block whose callback took longer than the block lasts:
    // a wall-clock result that other processes on a busy machine can cause.
    // It's judged at full depth (pushes to main and the nightly run), which
    // runs this test with the runner to itself; every run checks the rest.
    if common::full_depth() {
        assert_eq!(stats.dropouts, 0, "{stats:?}");
    }
}
