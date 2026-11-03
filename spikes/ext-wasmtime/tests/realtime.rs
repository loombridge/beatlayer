//! The spike's acceptance points, as tests. With RTSAN_LIB_PATH set at build
//! time they run under RealtimeSanitizer (build.rs links it); every test takes
//! a lock because the allocation counter is global.

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use ext_wasmtime_spike::alloc::Counting;
use ext_wasmtime_spike::host::{Backend, Bounds, Compiled, Options, Wait};
use ext_wasmtime_spike::scenario::{self, BLOCK_NS, Guest, Shape};
use wasmtime::Trap;

#[global_allocator]
static ALLOC: Counting = Counting;

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner())
}

const PAGES: u32 = 8;
const CALLS: usize = 100_000;

fn compiled(backend: Backend, bounds: Bounds) -> Compiled {
    let options = Options {
        backend,
        bounds,
        ..Options::default()
    };
    Compiled::new(&options, PAGES, 0).unwrap()
}

/// Spec R25 and R9: 100,000 calls on a real-time thread, nothing allocated
/// after instantiation, in every way the host can hand a block over that
/// doesn't make a `Vec`.
#[test]
fn a_hundred_thousand_calls_allocate_nothing() {
    let _one = serial();
    for backend in [Backend::Cranelift, Backend::Winch] {
        for bounds in [Bounds::Guard, Bounds::Checked] {
            let compiled = compiled(backend, bounds);
            for shape in [Shape::Direct, Shape::Reserved, Shape::Tick] {
                let guest = Guest::new(&compiled, shape).unwrap();
                let r = scenario::rt_calls(guest, shape, 32, CALLS, true).unwrap();
                assert_eq!(
                    (r.counts.allocs, r.counts.frees),
                    (0, 0),
                    "{backend:?} {bounds:?} {shape:?} allocated on the audio thread"
                );
            }
        }
    }
}

/// What a typical binding does, for the report: one allocation a call.
#[test]
fn a_result_made_as_a_vec_allocates_every_call() {
    let _one = serial();
    let compiled = compiled(Backend::Cranelift, Bounds::Guard);
    let guest = Guest::new(&compiled, Shape::Vec).unwrap();
    let r = scenario::rt_calls(guest, Shape::Vec, 32, 1000, false).unwrap();
    assert!(r.counts.allocs >= 2000, "{:?}", r.counts);
}

/// Spec R11: a runaway call ends in an epoch interrupt, and, with a ticker that
/// keeps time, inside a block. (A sleeping ticker's accuracy is the report's
/// subject: it depends on the platform's timers.)
#[test]
fn a_runaway_is_stopped_within_a_block() {
    let _one = serial();
    let compiled = compiled(Backend::Cranelift, Bounds::Guard);
    let r = scenario::runaway(&compiled, Wait::Spin, Duration::from_micros(10), 4, 100).unwrap();
    assert_eq!(r.other_errors, 0);
    assert!(r.stop.p50 < BLOCK_NS, "{r:?}");
    let r = scenario::runaway(&compiled, Wait::Sleep, Duration::from_micros(100), 2, 50).unwrap();
    assert_eq!(r.other_errors, 0);
}

/// Spec R10: a trap stops the call, poisons that instance, and leaves the pool
/// able to make another.
#[test]
fn a_trap_ends_the_instance_and_nothing_else() {
    let _one = serial();
    for bounds in [Bounds::Guard, Bounds::Checked] {
        let compiled = compiled(Backend::Cranelift, bounds);
        for (export, want) in [
            ("trap-unreachable", Trap::UnreachableCodeReached),
            ("trap-oob", Trap::MemoryOutOfBounds),
        ] {
            let r = scenario::trap(&compiled, export, false).unwrap();
            assert_eq!(r.trap, Some(want), "{bounds:?} {export}");
            assert!(r.poisoned && r.replacement_runs, "{r:?}");
        }
    }
}

/// The sanitizer is live: the same section, with a call that allocates, stops
/// the process. Runs the test binary again to see it stop.
#[cfg(ext_rtsan)]
#[test]
fn the_sanitizer_stops_an_allocating_call() {
    use std::process::Command;
    if std::env::var_os("EXT_SPIKE_CHILD").is_some() {
        let compiled = compiled(Backend::Cranelift, Bounds::Guard);
        let guest = Guest::new(&compiled, Shape::Vec).unwrap();
        let _ = scenario::rt_calls(guest, Shape::Vec, 32, 10, true);
        return;
    }
    let _one = serial();
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "the_sanitizer_stops_an_allocating_call",
            "--nocapture",
        ])
        .env("EXT_SPIKE_CHILD", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "the child wasn't stopped");
    assert!(stderr.contains("RealtimeSanitizer"), "{stderr}");
}
