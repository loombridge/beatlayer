//! What an audio thread runs, under RealtimeSanitizer, built as a release
//! build runs it: this binary doesn't install the allocation trap, so the
//! real-time path touches none of the trap's thread-locals. (macOS allocates
//! a thread's thread-local storage with `malloc` the first time the thread
//! touches it, and RTSan reports that.)
//!
//! The set-up for a thread an audio API owns runs here on a thread made with
//! `pthread_create`, as Core Audio or an ASIO driver makes theirs, inside a
//! real-time section from its first instruction: RTSan stops the process at
//! anything it doesn't allow. What RTSan doesn't intercept, because it
//! doesn't block, is listed in `bl_rt::AudioThreadSetup`'s documentation.

use std::ffi::c_void;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use bl_audio_io::{
    AudioBuffer, AudioBufferMut, AudioCallback, Backend, DeviceSelector, NullBackend, NullOptions,
    Pacing, PortRequest, ProcessContext, StreamRequest,
};
#[cfg(target_os = "macos")]
use bl_rt::WorkgroupJoin;
use bl_rt::{AudioThreadSetup, Promotion, RtThreadConfig, ThreadReport};

// Without `--cfg bl_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "bl-rt wasn't built with --cfg bl_rtsan; run `cargo xtask rtsan`"
);

/// What a raw thread is given, and what it hands back.
struct Job {
    setup: AudioThreadSetup,
    report: Option<ThreadReport>,
    /// Whether to undo the set-up before the thread ends, as a backend's own
    /// thread does; otherwise the thread just ends, as an audio API's may.
    release: bool,
}

/// The thread's whole life: a real-time section from its first instruction,
/// with the set-up inside.
extern "C" fn audio_thread(job: *mut c_void) -> *mut c_void {
    // SAFETY: `on_a_fresh_thread` passes a live, exclusive `Job` and waits
    // for this thread to end before touching it again.
    let job = unsafe { &mut *job.cast::<Job>() };
    let _section = bl_rt::Realtime::enter();
    job.report = Some(job.setup.enter());
    if job.release {
        job.setup.release();
    }
    std::ptr::null_mut()
}

/// Runs the set-up on a thread no Rust code has run on before.
fn on_a_fresh_thread(job: &mut Job) {
    let mut thread: libc::pthread_t = 0;
    // SAFETY: `audio_thread` matches the start routine's signature, `job`
    // outlives the thread because it's joined below, and `thread` is
    // writable.
    let created = unsafe {
        libc::pthread_create(
            &mut thread,
            std::ptr::null(),
            audio_thread,
            std::ptr::from_mut(job).cast(),
        )
    };
    assert_eq!(created, 0, "pthread_create failed");
    // SAFETY: `thread` was just created and is joined once.
    let joined = unsafe { libc::pthread_join(thread, std::ptr::null_mut()) };
    assert_eq!(joined, 0, "pthread_join failed");
}

/// Real-time priority, and on macOS a workgroup a fresh thread really joins.
fn config() -> RtThreadConfig {
    RtThreadConfig {
        period: Duration::from_micros(1_333),
        promote: true,
        #[cfg(target_os = "macos")]
        workgroup: Some(
            bl_rt::WorkInterval::new("bl-rtsan audio thread")
                .unwrap()
                .workgroup()
                .clone(),
        ),
        #[cfg(not(target_os = "macos"))]
        workgroup: None,
    }
}

/// The set-up for a thread an audio API owns, on two fresh threads in turn,
/// as when Core Audio moves its callback: the first ends without undoing it
/// (its workgroup exit hook leaves), the second undoes it before it ends.
/// RTSan stops the process if anything in either set-up allocates, locks,
/// sleeps, or does I/O.
#[test]
fn audio_thread_set_up_passes_under_rtsan() {
    assert!(!bl_rt::trap::is_installed(), "a release-shaped binary");
    let (setup, mut reports) = AudioThreadSetup::new(config());
    let mut job = Job {
        setup,
        report: None,
        release: false,
    };
    for release in [false, true] {
        job.release = release;
        on_a_fresh_thread(&mut job);
        let report = job.report.take().expect("the thread set itself up");
        eprintln!("set-up on a fresh thread (release: {release}): {report:?}");
        assert!(report.sanitizer_active);
        assert_eq!(report.denormals_flushed, bl_rt::denormals::SUPPORTED);
        #[cfg(target_os = "macos")]
        {
            assert!(report.priority.is_realtime(), "{}", report.priority);
            assert_eq!(report.workgroup, WorkgroupJoin::Joined);
        }
        #[cfg(target_os = "linux")]
        assert_eq!(report.promotion, Promotion::Pending);
    }
    assert_eq!(job.setup.setups(), 2);
    // On Linux a helper thread raises the thread, or reports why it couldn't
    // (CI machines may have neither rtprio limits nor rtkit).
    let began = Instant::now();
    while reports.latest().expect("a report").promotion == Promotion::Pending {
        assert!(began.elapsed() < Duration::from_secs(10), "no answer");
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(target_os = "macos")]
thread_local! {
    static TOUCHED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Touches a Rust thread-local first thing on a fresh thread, inside a
/// real-time section: what the set-up must never do on macOS.
#[cfg(target_os = "macos")]
extern "C" fn touches_a_thread_local(_: *mut c_void) -> *mut c_void {
    let _section = bl_rt::Realtime::enter();
    TOUCHED.with(|touched| touched.set(touched.get() + 1));
    std::ptr::null_mut()
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "breaks a real-time rule on purpose; run by a test below"]
fn child_touches_a_thread_local_on_a_fresh_thread() {
    let mut thread: libc::pthread_t = 0;
    // SAFETY: as in `on_a_fresh_thread`; the routine takes no argument.
    unsafe {
        assert_eq!(
            libc::pthread_create(
                &mut thread,
                std::ptr::null(),
                touches_a_thread_local,
                std::ptr::null_mut()
            ),
            0
        );
        libc::pthread_join(thread, std::ptr::null_mut());
    }
}

/// The fresh-thread check above is sensitive: on macOS, the first touch of
/// a thread-local makes the system `malloc` that thread's storage, and RTSan
/// stops the process. That's why the set-up for an audio API's thread keeps
/// to no thread-locals there.
#[cfg(target_os = "macos")]
#[test]
fn a_thread_local_on_a_fresh_thread_is_stopped_by_rtsan() {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "child_touches_a_thread_local_on_a_fresh_thread",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "not stopped:\n{stderr}");
    assert!(stderr.contains("`malloc` in real-time context"), "{stderr}");
    assert!(stderr.contains("_tlv_get_addr"), "{stderr}");
    eprintln!("--- child_touches_a_thread_local_on_a_fresh_thread ---\n{stderr}");
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

/// A whole stream: the null device's thread sets itself up as a real-time
/// section, then runs 375 blocks, each one a real-time section.
#[test]
fn a_stream_runs_clean_under_rtsan() {
    let blocks = Arc::new(AtomicU64::new(0));
    let stats = play(
        Sine {
            phase: 0.0,
            blocks: Arc::clone(&blocks),
        },
        0.5,
    );
    assert_eq!(blocks.load(Ordering::Relaxed), 375);
    let report = stats.audio_thread.expect("a set-up report");
    assert!(report.sanitizer_active);
    assert_eq!(stats.audio_thread_setups, 1);
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
    // The report's stack runs through the runner into the callback. Naming
    // functions needs a symbolizer (`atos` on macOS; on Linux, the
    // `llvm-symbolizer` that `cargo xtask rtsan` finds next to the runtime).
    if stderr.contains("invalid path to external symbolizer") {
        eprintln!("note: no llvm-symbolizer, so the report can't name the callback");
    } else {
        assert!(stderr.contains("Locker"), "{stderr}");
    }
    eprintln!("--- child_callback_takes_a_mutex ---\n{stderr}");
}
