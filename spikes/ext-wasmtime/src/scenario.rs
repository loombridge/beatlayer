//! The measurements: calls on a real-time thread, runaway control, traps, and
//! compile and load times. Each returns numbers; `main.rs` prints them and the
//! tests in `tests/` assert on them.

use std::thread;
use std::time::{Duration, Instant};

use wasmtime::{Error, Result, Trap};

use crate::alloc::{self, Counts};
use crate::host::{Comp, Compiled, Direct, Options, Ticker, Wait};
use crate::rt::{self, Realtime};

/// A block of 32 frames at 48 kHz lasts 666.7 µs.
pub const BLOCK_NS: u64 = 666_667;

/// What a call looks like to the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// The host writes into the exported memory of a core module.
    Direct,
    /// A component, `list<f32>` through the canonical ABI into reserved buffers.
    Reserved,
    /// A component, with the result as a `Vec` the host makes.
    Vec,
    /// A component call that does nothing.
    Tick,
}

/// An instance in one of the shapes.
pub enum Guest {
    Direct(Direct),
    Comp(Shape, Comp),
}

impl Guest {
    pub fn new(compiled: &Compiled, shape: Shape) -> Result<Guest> {
        Ok(match shape {
            Shape::Direct => Guest::Direct(Direct::new(&compiled.engine, &compiled.module)?),
            _ => Guest::Comp(shape, Comp::new(&compiled.engine, &compiled.component)?),
        })
    }

    pub fn call(&mut self, input: &[f32], output: &mut [f32], deadline: u64) -> Result<()> {
        match self {
            Guest::Direct(d) => d.process(input, output, deadline),
            Guest::Comp(Shape::Reserved, c) => c.process_reserved(input, output, deadline),
            Guest::Comp(Shape::Vec, c) => {
                let out = c.process_vec(input, deadline)?;
                output.copy_from_slice(&out);
                Ok(())
            }
            Guest::Comp(_, c) => c.tick(deadline).map(|_| ()),
        }
    }
}

/// Nanosecond statistics.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub n: usize,
    pub mean: f64,
    pub p50: u64,
    pub p99: u64,
    pub p999: u64,
    pub min: u64,
    pub max: u64,
}

impl Stats {
    pub fn of(mut ns: Vec<u64>) -> Stats {
        ns.sort_unstable();
        let at = |q: f64| ns[((ns.len() - 1) as f64 * q) as usize];
        Stats {
            n: ns.len(),
            mean: ns.iter().sum::<u64>() as f64 / ns.len() as f64,
            p50: at(0.5),
            p99: at(0.99),
            p999: at(0.999),
            min: ns[0],
            max: *ns.last().unwrap(),
        }
    }
}

/// The result of calling a guest on a real-time thread.
#[derive(Debug)]
pub struct RtReport {
    pub shape: Shape,
    pub calls: usize,
    pub counts: Counts,
    /// Each call timed on its own, with the clock's own cost still in.
    pub per_call: Stats,
    /// The mean of a second pass that timed the whole loop once.
    pub bulk_ns_per_call: f64,
    /// What reading the clock costs, from timing nothing.
    pub clock_ns: f64,
}

/// A block of 32 stereo frames of noise from a fixed sequence.
pub fn noise(frames: usize) -> Vec<f32> {
    let mut seed = 0x2545_f491u32;
    (0..frames * 2)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        })
        .collect()
}

/// Calls the guest `calls` times (and `calls` more, timed as one) on a thread
/// that enters a real-time section after one warm-up block, with the
/// allocation counter running for every call. `deadline` is in epoch ticks;
/// an engine without a ticker never reaches it. With `section` false the
/// thread doesn't enter the section, for shapes that allocate.
pub fn rt_calls(
    guest: Guest,
    shape: Shape,
    frames: usize,
    calls: usize,
    section: bool,
) -> Result<RtReport> {
    thread::scope(|scope| {
        thread::Builder::new()
            .name("audio".into())
            .spawn_scoped(scope, move || {
                let mut guest = guest;
                let input = noise(frames);
                let mut output = vec![0.0; frames * 2];
                let mut times = vec![0u64; calls];
                let deadline = 1 << 40;
                // The warm-up: the first call on a thread touches wasmtime's
                // thread-locals, which allocate on macOS, so it comes before
                // the section, as an audio thread's set-up does.
                for _ in 0..16 {
                    guest.call(&input, &mut output, deadline)?;
                }
                alloc::start();
                let rt = section.then(Realtime::enter);
                let mut clock = Duration::ZERO;
                for time in times.iter_mut() {
                    let t = Instant::now();
                    *time = t.elapsed().as_nanos() as u64;
                    clock += t.elapsed();
                }
                for time in times.iter_mut() {
                    let t = Instant::now();
                    guest.call(&input, &mut output, deadline)?;
                    *time = t.elapsed().as_nanos() as u64;
                }
                let t = Instant::now();
                for _ in 0..calls {
                    guest.call(&input, &mut output, deadline)?;
                }
                let bulk = t.elapsed();
                drop(rt);
                let counts = alloc::stop();
                Ok(RtReport {
                    shape,
                    calls,
                    counts,
                    per_call: Stats::of(times),
                    bulk_ns_per_call: bulk.as_nanos() as f64 / calls as f64,
                    clock_ns: clock.as_nanos() as f64 / calls as f64,
                })
            })
            .expect("a thread starts")
            .join()
            .expect("the audio thread didn't panic")
    })
}

/// How a call to a runaway guest ended.
#[derive(Debug)]
pub struct Runaway {
    pub wait: Wait,
    pub tick: Duration,
    pub deadline_ticks: u64,
    /// The time from the call to its return, over all runs.
    pub stop: Stats,
    /// The ticker's mean period over the whole run, which `sleep` makes longer
    /// than asked.
    pub achieved_tick_ns: f64,
    /// Runs whose error wasn't an epoch interrupt.
    pub other_errors: usize,
}

/// Starts a ticker at `tick`, then `runs` times calls a guest that never
/// returns, on a real-time thread, and times how long each takes to stop.
pub fn runaway(
    compiled: &Compiled,
    wait: Wait,
    tick: Duration,
    deadline_ticks: u64,
    runs: usize,
) -> Result<Runaway> {
    let ticker = Ticker::start(&compiled.engine, tick, wait);
    let begun = Instant::now();
    let mut times = Vec::with_capacity(runs);
    let mut other_errors = 0;
    for _ in 0..runs {
        let mut guest = Comp::new(&compiled.engine, &compiled.component)?;
        let result = thread::scope(|scope| {
            scope
                .spawn(|| {
                    // The first call on a thread: the warm-up.
                    let _ = guest.tick(1 << 40);
                    let section = Realtime::enter();
                    let t = Instant::now();
                    // The spin itself does nothing a section forbids; the
                    // trap that ends it allocates (see `trap`), so it runs
                    // unchecked.
                    let result = rt::unchecked(|| guest.call("spin", deadline_ticks));
                    let took = t.elapsed();
                    drop(section);
                    (result, took)
                })
                .join()
                .expect("the thread didn't panic")
        });
        match result {
            (Err(e), took) if is_trap(&e, Trap::Interrupt) => times.push(took.as_nanos() as u64),
            (_, took) => {
                other_errors += 1;
                times.push(took.as_nanos() as u64);
            }
        }
    }
    let achieved_tick_ns = begun.elapsed().as_nanos() as f64 / ticker.ticks().max(1) as f64;
    Ok(Runaway {
        wait,
        tick,
        deadline_ticks,
        stop: Stats::of(times),
        achieved_tick_ns,
        other_errors,
    })
}

pub fn is_trap(error: &Error, trap: Trap) -> bool {
    error.downcast_ref::<Trap>() == Some(&trap)
}

/// What a guest that traps does on a real-time thread.
#[derive(Debug)]
pub struct TrapReport {
    pub export: &'static str,
    pub trap: Option<Trap>,
    pub took: Duration,
    /// What the trap's path allocated and freed on the audio thread.
    pub counts: Counts,
    /// Whether the instance, after the trap, refuses another call.
    pub poisoned: bool,
    /// Whether a new instance from the same pool then runs.
    pub replacement_runs: bool,
}

/// Calls a trapping export on a real-time thread, then the same instance
/// again, then a new instance. With `checked` the thread's real-time section
/// stays on for the trap as well, and RealtimeSanitizer stops the process at
/// the allocation the trap makes; without it, the trap runs unchecked and its
/// allocations are counted.
pub fn trap(compiled: &Compiled, export: &'static str, checked: bool) -> Result<TrapReport> {
    let mut guest = Comp::new(&compiled.engine, &compiled.component)?;
    let (result, took, counts, again) = thread::scope(|scope| {
        scope
            .spawn(|| {
                let _ = guest.tick(1 << 40);
                let section = Realtime::enter();
                alloc::start();
                let t = Instant::now();
                let result = if checked {
                    guest.call(export, 1 << 40)
                } else {
                    rt::unchecked(|| guest.call(export, 1 << 40))
                };
                let took = t.elapsed();
                let counts = alloc::stop();
                drop(section);
                // Outside the section: refusing a call makes an error.
                let again = guest.tick(1 << 40);
                (result, took, counts, again)
            })
            .join()
            .expect("the thread didn't panic")
    });
    let trap = result.err().and_then(|e| e.downcast_ref::<Trap>().copied());
    let mut replacement = Comp::new(&compiled.engine, &compiled.component)?;
    Ok(TrapReport {
        export,
        trap,
        took,
        counts,
        poisoned: again.is_err(),
        replacement_runs: replacement.tick(1 << 40).is_ok(),
    })
}

/// Compile and load times, and sizes, for one backend and one size of guest.
#[derive(Debug)]
pub struct CompileReport {
    pub filler: usize,
    pub wat_bytes: usize,
    pub engine_ns: u128,
    pub compile_ns: u128,
    pub machine_code_bytes: usize,
    pub deserialize_ns: u128,
    pub deserialize_file_ns: u128,
    pub instantiate: Stats,
}

pub fn compile(options: &Options, pages: u32, filler: usize) -> Result<CompileReport> {
    let t = Instant::now();
    let engine = crate::host::engine(options)?;
    let engine_ns = t.elapsed().as_nanos();
    let wat = crate::guest::component_wat(pages, filler);
    let t = Instant::now();
    let bytes = crate::host::precompile_component(&engine, &wat)?;
    let compile_ns = t.elapsed().as_nanos();
    let t = Instant::now();
    let component = crate::host::load_component(&engine, &bytes)?;
    let deserialize_ns = t.elapsed().as_nanos();
    let dir = std::env::temp_dir().join(format!("ext-wasmtime-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temporary directory");
    let path = dir.join("guest.cwasm");
    std::fs::write(&path, &bytes).expect("a temporary file");
    let t = Instant::now();
    let _from_file = crate::host::load_component_file(&engine, &path)?;
    let deserialize_file_ns = t.elapsed().as_nanos();
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
    let mut times = Vec::new();
    for _ in 0..200 {
        let t = Instant::now();
        let guest = Comp::new(&engine, &component)?;
        times.push(t.elapsed().as_nanos() as u64);
        drop(guest);
    }
    Ok(CompileReport {
        filler,
        wat_bytes: wat.len(),
        engine_ns,
        compile_ns,
        machine_code_bytes: bytes.len(),
        deserialize_ns,
        deserialize_file_ns,
        instantiate: Stats::of(times),
    })
}

/// Whether this process has RealtimeSanitizer's runtime.
pub fn sanitizer() -> bool {
    rt::ACTIVE
}
