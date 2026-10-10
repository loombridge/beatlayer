//! RealtimeSanitizer catches what the allocation trap can't. These tests run
//! `bl-rt`'s harness under it, with the trap installed as in every Beatlayer
//! test binary.
//!
//! Each `child_*` test breaks one real-time rule. RTSan stops a process at
//! its first violation, so the checks run each child in a process of its own
//! and read its report.

bl_rt::install_trap!();

use std::hint::black_box;
use std::process::{Command, Output};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Duration;

use bl_rt::log::Level;

// Without `--cfg bl_rtsan` (`rtsan/.cargo/config.toml`), this doesn't build.
const _: () = assert!(
    bl_rt::sanitize::ACTIVE,
    "bl-rt wasn't built with --cfg bl_rtsan; run `cargo xtask rtsan`"
);

fn run_child(name: &str) -> Output {
    Command::new(std::env::current_exe().unwrap())
        .args([
            name,
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .output()
        .unwrap()
}

/// Runs a child and checks that RTSan stopped it, naming `call`.
fn assert_caught(name: &str, call: &str) {
    let output = run_child(name);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{name} wasn't stopped:\n{stderr}");
    assert!(
        stderr.contains("ERROR: RealtimeSanitizer"),
        "{name} failed without an RTSan report:\n{stderr}"
    );
    assert!(
        stderr.contains(call),
        "the report doesn't name `{call}`:\n{stderr}"
    );
    // Show the report in the test's own output, as evidence.
    eprintln!("--- {name} ---\n{stderr}");
}

#[test]
#[ignore = "breaks a real-time rule on purpose; run by a test below"]
fn child_takes_a_mutex() {
    // Another thread holds the lock, so taking it must wait: the case that
    // causes priority inversion. (Rust's mutex is a futex on Linux, which
    // reaches the kernel, and so RTSan, only when it has to wait. On macOS it
    // is a pthread mutex, which RTSan sees either way.)
    let mutex = Arc::new(Mutex::new(0u32));
    let holding = Arc::new(Barrier::new(2));
    let holder = {
        let mutex = Arc::clone(&mutex);
        let holding = Arc::clone(&holding);
        thread::spawn(move || {
            let guard = mutex.lock().unwrap();
            holding.wait();
            thread::sleep(Duration::from_millis(200));
            drop(guard);
        })
    };
    holding.wait();
    bl_rt::rt_test!({
        *mutex.lock().unwrap() += 1;
    });
    holder.join().unwrap();
}

#[test]
#[ignore = "breaks a real-time rule on purpose; run by a test below"]
fn child_sleeps() {
    bl_rt::rt_test!({ thread::sleep(Duration::from_micros(10)) });
}

#[test]
#[ignore = "breaks a real-time rule on purpose; run by a test below"]
fn child_allocates() {
    bl_rt::rt_test!({
        black_box(Box::new([0u8; 32]));
    });
}

#[test]
fn a_test_that_takes_a_mutex_fails_under_rtsan() {
    // Linux reports the futex system call that a waiting mutex makes.
    let call = if cfg!(target_os = "macos") {
        "pthread_mutex_lock"
    } else {
        "Intercepted call to real-time unsafe function"
    };
    assert_caught("child_takes_a_mutex", call);
}

#[test]
fn a_test_that_sleeps_fails_under_rtsan() {
    assert_caught("child_sleeps", "sleep");
}

#[test]
fn a_test_that_allocates_fails_under_rtsan() {
    assert_caught("child_allocates", "malloc");
}

/// What a real-time thread does every block, done under RTSan: clock reads,
/// atomics, the logger, and the denormal guard. A violation here aborts the
/// whole test binary, which fails the run.
#[test]
fn the_harness_itself_passes_under_rtsan() {
    thread::spawn(move || {
        let report = bl_rt::prepare_current_thread(&bl_rt::RtThreadConfig {
            period: Duration::from_millis(5),
            promote: false,
            workgroup: None,
        });
        assert!(report.sanitizer_active);
        let counter = std::sync::atomic::AtomicU64::new(0);
        let mut writer = bl_rt::log::Writer::new();
        for block in 0..100u64 {
            bl_rt::rt_test!({
                let began = std::time::Instant::now();
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                bl_rt::rt_log!(Level::Debug, "block {} took {}", block, began.elapsed());
                bl_rt::rt_log!(to: writer, Level::Debug, "block {}", block);
                assert!(bl_rt::denormals::flushing() || !bl_rt::denormals::SUPPORTED);
            });
        }
        assert_eq!(counter.into_inner(), 100);
    })
    .join()
    .unwrap();
}
