//! Audio streams under RealtimeSanitizer: every block the runner hands to a
//! callback is a real-time section, so the runner's own work must pass, and a
//! callback that breaks the rules must be stopped. Run with
//! `cargo xtask rtsan`; in a build without RTSan linked these skip and say so.

bl_rt::install_trap!();

use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use bl_audio_io::{
    AudioBuffer, AudioBufferMut, AudioCallback, Backend, DeviceSelector, NullBackend, NullOptions,
    Pacing, PortRequest, ProcessContext, StreamRequest,
};

fn active() -> bool {
    if !bl_rt::sanitize::ACTIVE {
        eprintln!(
            "skipped: RealtimeSanitizer isn't linked into this build; run `cargo xtask rtsan`"
        );
    }
    bl_rt::sanitize::ACTIVE
}

/// A sine, as `bl-cli tone` plays, that also counts its blocks.
struct Sine {
    phase: f32,
    blocks: Arc<AtomicU64>,
}

impl AudioCallback for Sine {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        _: &AudioBuffer<'_>,
        output: &mut AudioBufferMut<'_>,
    ) {
        let step = 440.0 / ctx.sample_rate as f32;
        for channel in output.iter_mut() {
            let mut phase = self.phase;
            for sample in channel.iter_mut() {
                *sample = (phase * std::f32::consts::TAU).sin() * 0.001;
                phase = (phase + step).fract();
            }
        }
        self.phase = (self.phase + step * ctx.frames as f32).fract();
        self.blocks.fetch_add(1, Ordering::Relaxed);
    }
}

fn request(buffer: u32) -> StreamRequest {
    StreamRequest {
        output: Some(PortRequest {
            device: DeviceSelector::Default,
            channels: Some(2),
        }),
        input: Some(PortRequest::default()),
        sample_rate: Some(48_000),
        buffer_size: Some(buffer),
    }
}

/// Runs `callback` on a real-time-paced null stream for `seconds`.
fn play(callback: impl AudioCallback, seconds: f64) -> bl_audio_io::StreamStats {
    let backend = NullBackend::with_options(NullOptions {
        pacing: Pacing::RealTime,
        frame_limit: Some((seconds * 48_000.0) as u64),
    });
    let mut stream = backend
        .open_stream(&request(64), Box::new(callback))
        .unwrap();
    stream.start().unwrap();
    let began = Instant::now();
    while stream.stats().frames < (seconds * 48_000.0) as u64 {
        assert!(
            began.elapsed() < Duration::from_secs(30),
            "the stream stalled"
        );
        thread::sleep(Duration::from_millis(5));
    }
    stream.stop().unwrap();
    stream.stats()
}

#[test]
fn a_stream_runs_clean_under_rtsan() {
    let rtsan = active();
    let blocks = Arc::new(AtomicU64::new(0));
    let stats = play(
        Sine {
            phase: 0.0,
            blocks: Arc::clone(&blocks),
        },
        0.5,
    );
    assert_eq!(blocks.load(Ordering::Relaxed), 375);
    assert_eq!(stats.heap_operations, 0);
    let report = stats.audio_thread.expect("a set-up report");
    assert_eq!(report.sanitizer_active, rtsan);
}

/// Takes a mutex another thread holds, inside the callback.
struct Locker {
    shared: Arc<Mutex<u64>>,
}

impl AudioCallback for Locker {
    fn process(&mut self, _: &ProcessContext, _: &AudioBuffer<'_>, _: &mut AudioBufferMut<'_>) {
        *self.shared.lock().unwrap() += 1;
    }
}

#[test]
#[ignore = "breaks a real-time rule on purpose; run by a test below"]
fn child_callback_takes_a_mutex() {
    let shared = Arc::new(Mutex::new(0u64));
    let holding = Arc::new(Barrier::new(2));
    let holder = {
        let shared = Arc::clone(&shared);
        let holding = Arc::clone(&holding);
        thread::spawn(move || {
            let guard = shared.lock().unwrap();
            holding.wait();
            thread::sleep(Duration::from_millis(500));
            drop(guard);
        })
    };
    holding.wait();
    play(Locker { shared }, 0.1);
    holder.join().unwrap();
}

#[test]
fn a_callback_that_takes_a_mutex_is_stopped_by_rtsan() {
    if !active() {
        return;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "child_callback_takes_a_mutex",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "the child wasn't stopped:\n{stderr}"
    );
    assert!(stderr.contains("ERROR: RealtimeSanitizer"), "{stderr}");
    if cfg!(target_os = "macos") {
        assert!(stderr.contains("pthread_mutex_lock"), "{stderr}");
    }
    // The report's stack runs through the runner into the callback.
    assert!(stderr.contains("Locker"), "{stderr}");
    eprintln!("--- child_callback_takes_a_mutex ---\n{stderr}");
}
