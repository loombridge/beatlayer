//! The capacity target (the architecture, "Requirements that shape the
//! design"): 128 audio tracks with an EQ and a compressor each, at 48 kHz in
//! blocks of 128 frames, on the multicore scheduler (WP-5.37), with no
//! dropout. It runs the song on the null device paced like a sound card, so
//! the engine has a real-time thread and a deadline, and prints the stream's
//! counters.
//!
//! It takes real time and the machine to itself, so it doesn't run with the
//! other tests:
//!
//! ```text
//! cargo nextest run --release -p bl-cli --test capacity --run-ignored only --no-capture
//! ```
//!
//! `BL_CAPACITY_SECONDS` sets how long it plays (600, ten minutes, by
//! default), `BL_CAPACITY_WORKERS` how many workers run beside the audio
//! thread (as many as the machine suggests by default; `0` plays on the
//! audio thread alone), and `BL_CAPACITY_TRACKS` the tracks (128).

bl_rt::install_trap!();

use std::time::{Duration, Instant};

use bl_audio_io::{
    AudioBuffer, AudioBufferMut, AudioCallback, Backend, NullBackend, NullOptions, Pacing,
    PortRequest, ProcessContext, StreamRequest,
};
use bl_core::time::SampleRate;
use bl_engine::{Engine, EngineConfig, Pool, PoolConfig, Processor, Registry};
use rtrb::{Consumer, RingBuffer};

/// The engine as a stream's callback, taking its processor when it arrives.
struct Play {
    incoming: Consumer<Processor>,
    processor: Option<Processor>,
}

impl AudioCallback for Play {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        _input: &AudioBuffer<'_>,
        output: &mut AudioBufferMut<'_>,
    ) {
        if self.processor.is_none() {
            self.processor = self.incoming.pop().ok();
        }
        let Some(processor) = self.processor.as_mut() else {
            return;
        };
        let mut done = 0;
        while done < ctx.frames {
            let n = (ctx.frames - done).min(processor.config().max_frames);
            let block = processor.process(n);
            for c in 0..output.channels() {
                let from = block.channel(c.min(block.channels() - 1));
                output.channel_mut(c)[done..done + n].copy_from_slice(from);
            }
            done += n;
        }
    }
}

fn variable(name: &str) -> Option<usize> {
    std::env::var(name).ok().map(|value| {
        value
            .parse()
            .unwrap_or_else(|_| panic!("{name} is `{value}`: set it to a number"))
    })
}

#[test]
#[ignore = "plays for ten minutes of real time on a machine of its own"]
fn the_capacity_target_plays_without_a_dropout() {
    let seconds = variable("BL_CAPACITY_SECONDS").unwrap_or(600);
    let tracks = variable("BL_CAPACITY_TRACKS").unwrap_or(128);
    let rate = SampleRate::HZ_48000;
    let config = EngineConfig::new(rate, 128);

    let backend = NullBackend::with_options(NullOptions {
        dropout_after: None,
        pacing: Pacing::RealTime,
        frame_limit: None,
        channels: 2,
    });
    let request = StreamRequest {
        output: Some(PortRequest::default()),
        input: None,
        sample_rate: Some(rate.hz()),
        buffer_size: Some(128),
    };
    let (mut handoff, incoming) = RingBuffer::new(1);
    let mut stream = backend
        .open_stream(
            &request,
            Box::new(Play {
                incoming,
                processor: None,
            }),
        )
        .unwrap();

    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    let (mut engine, mut processor) = Engine::start(config, registry).unwrap();
    let period = Duration::from_secs_f64(128.0 / f64::from(rate.hz()));
    let mut pool_config = PoolConfig::for_stream(period, stream.workgroup(), true);
    if let Some(workers) = variable("BL_CAPACITY_WORKERS") {
        pool_config.workers = workers;
    }
    let pool = Pool::start(&config, &pool_config).unwrap();
    let workers = pool.workers();
    let monitor = pool.monitor();
    processor.set_pool((workers > 0).then_some(pool));

    let (editor, _) = bl_cli::capacity::song(tracks);
    engine.sync(editor.document());
    engine.play();
    assert!(engine.flush(Duration::from_secs(30)), "the first program");
    handoff.push(processor).ok().unwrap();

    println!("{tracks} tracks, each a tone, an EQ and a compressor; 48 kHz, blocks of 128 frames");
    println!("{workers} workers beside the audio thread, for {seconds} s");
    stream.start().unwrap();
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(seconds as u64) {
        std::thread::sleep(Duration::from_millis(250));
    }
    stream.stop().unwrap();
    let stats = stream.stats();
    let state = engine.state().clone();
    println!(
        "{} blocks, {} dropouts, longest callback {:.3} ms of 2.667 ms, \
         peak engine load {:.1}%, {} heap operations on the audio thread, \
         {} on the workers, {} worker panics",
        stats.blocks,
        stats.dropouts,
        stats.longest_block.as_secs_f64() * 1000.0,
        state.cpu_load_max * 100.0,
        stats.heap_operations,
        monitor.heap_operations(),
        monitor.panics(),
    );
    assert_eq!(stats.dropouts, 0, "{stats:?}");
    assert_eq!(stats.heap_operations, 0);
    assert_eq!(monitor.heap_operations(), 0);
    assert_eq!(monitor.panics(), 0);
    drop(stream);
}
