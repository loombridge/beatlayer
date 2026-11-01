//! Prints the spike's measurements as Markdown. `ext-wasmtime-spike --help`
//! lists the parts; with none named, all of them run.

use ext_wasmtime_spike::alloc::Counting;

#[global_allocator]
static ALLOC: Counting = Counting;

#[cfg(feature = "compile")]
fn main() -> wasmtime::Result<()> {
    report::main()
}

#[cfg(all(feature = "runtime", not(feature = "compile")))]
fn main() -> wasmtime::Result<()> {
    // The runtime-only build: loads what the full build precompiled.
    use ext_wasmtime_spike::host;
    let path = std::env::args().nth(1).expect("a .cwasm path");
    let engine = host::engine(&host::Options::default())?;
    let component = host::load_component_file(&engine, path.as_ref())?;
    let mut guest = host::Comp::new(&engine, &component)?;
    println!("tick {}", guest.tick(1 << 40)?);
    Ok(())
}

#[cfg(not(feature = "runtime"))]
fn main() {
    println!("the baseline: no wasmtime");
}

#[cfg(feature = "compile")]
mod report {
    use std::time::Duration;

    use ext_wasmtime_spike::host::{Backend, Bounds, Compiled, Options, Wait};
    use ext_wasmtime_spike::scenario::{
        self, BLOCK_NS, CompileReport, Guest, RtReport, Runaway, Shape, TrapReport,
    };
    use wasmtime::Result;

    const PAGES: u32 = 8;
    const FRAMES: usize = 32;

    pub fn main() -> Result<()> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.iter().any(|a| a == "--help") {
            println!(
                "usage: ext-wasmtime-spike [--calls N] [calls] [runaway] [traps] [compile] \
                 [precompile OUT]"
            );
            return Ok(());
        }
        if let Some(at) = args.iter().position(|a| a == "precompile") {
            let out = args.get(at + 1).expect("an output path");
            let engine = ext_wasmtime_spike::host::engine(&Options::default())?;
            let bytes = ext_wasmtime_spike::host::precompile_component(
                &engine,
                &ext_wasmtime_spike::guest::component_wat(PAGES, 0),
            )?;
            std::fs::write(out, bytes).expect("the output file");
            return Ok(());
        }
        let calls = args
            .iter()
            .position(|a| a == "--calls")
            .map(|at| args[at + 1].parse().expect("a number of calls"))
            .unwrap_or(100_000);
        let parts: Vec<&str> = args
            .iter()
            .filter(|a| ["calls", "runaway", "traps", "compile"].contains(&a.as_str()))
            .map(String::as_str)
            .collect();
        let want = |part: &str| parts.is_empty() || parts.contains(&part);

        println!("## {} {}", std::env::consts::OS, std::env::consts::ARCH);
        println!(
            "\nwasmtime 50.0.1; RealtimeSanitizer {}; a block of {FRAMES} frames at 48 kHz is \
             {} µs, a quarter of it {} µs",
            if scenario::sanitizer() { "on" } else { "off" },
            BLOCK_NS / 1000,
            BLOCK_NS / 4000,
        );
        if want("calls") {
            calls_table(calls)?;
        }
        if want("runaway") {
            runaway_table()?;
        }
        if want("traps") {
            traps_table()?;
        }
        if want("compile") {
            compile_table()?;
        }
        Ok(())
    }

    fn name(options: &Options) -> String {
        format!("{:?}, {:?} bounds", options.backend, options.bounds)
    }

    fn us(ns: u64) -> String {
        format!("{:.2}", ns as f64 / 1000.0)
    }

    fn calls_table(calls: usize) -> Result<()> {
        println!("\n### Calls on a real-time thread, {calls} blocks of {FRAMES} frames\n");
        println!(
            "| Build | Shape | Allocations | Frees | Mean µs | p50 µs | p99 µs | p99.9 µs | Max µs | \
             Bulk mean µs |"
        );
        println!("|---|---|---|---|---|---|---|---|---|---|");
        let mut clock = 0.0;
        for backend in [Backend::Cranelift, Backend::Winch] {
            for bounds in [Bounds::Guard, Bounds::Checked] {
                let options = Options {
                    backend,
                    bounds,
                    ..Options::default()
                };
                let compiled = Compiled::new(&options, PAGES, 0)?;
                for shape in [Shape::Direct, Shape::Reserved, Shape::Vec, Shape::Tick] {
                    let guest = Guest::new(&compiled, shape)?;
                    let section = shape != Shape::Vec;
                    let r = scenario::rt_calls(guest, shape, FRAMES, calls, section)?;
                    clock = r.clock_ns;
                    calls_row(&name(&options), &r, section);
                }
            }
        }
        println!(
            "\nMean µs and the percentiles time each call with the clock's own cost still in \
             (about {clock:.0} ns); bulk mean times the whole loop once. `Vec` runs outside \
             the real-time section because it allocates."
        );
        Ok(())
    }

    fn calls_row(build: &str, r: &RtReport, section: bool) {
        let s = &r.per_call;
        println!(
            "| {build} | {:?}{} | {} | {} | {:.2} | {} | {} | {} | {} | {:.2} |",
            r.shape,
            if section { "" } else { " (no section)" },
            r.counts.allocs,
            r.counts.frees,
            s.mean / 1000.0,
            us(s.p50),
            us(s.p99),
            us(s.p999),
            us(s.max),
            r.bulk_ns_per_call / 1000.0,
        );
    }

    fn runaway_table() -> Result<()> {
        println!("\n### A runaway call stopped by epoch interruption\n");
        println!(
            "| Ticker | Tick | Deadline (ticks) | Achieved tick µs | Stop min µs | Stop p50 µs | Stop p99 µs | \
             Stop max µs | Not an interrupt |"
        );
        println!("|---|---|---|---|---|---|---|---|---|");
        let compiled = Compiled::new(&Options::default(), PAGES, 0)?;
        let runs = [
            (Wait::Sleep, 25, 4),
            (Wait::Sleep, 50, 2),
            (Wait::Sleep, 50, 3),
            (Wait::Sleep, 100, 1),
            (Wait::Sleep, 100, 2),
            (Wait::Sleep, 250, 1),
            (Wait::Spin, 10, 4),
            (Wait::Spin, 25, 4),
            (Wait::Spin, 50, 3),
        ];
        for (wait, tick, deadline) in runs {
            let tick = Duration::from_micros(tick);
            let r: Runaway = scenario::runaway(&compiled, wait, tick, deadline, 200)?;
            println!(
                "| {:?} | {} µs | {} | {:.1} | {} | {} | {} | {} | {} |",
                r.wait,
                r.tick.as_micros(),
                r.deadline_ticks,
                r.achieved_tick_ns / 1000.0,
                us(r.stop.min),
                us(r.stop.p50),
                us(r.stop.p99),
                us(r.stop.max),
                r.other_errors,
            );
        }
        Ok(())
    }

    fn traps_table() -> Result<()> {
        println!("\n### A guest that traps, on a real-time thread\n");
        println!(
            "| Bounds | Export | Trap | Took µs | Allocations | Frees | Same instance refuses a call | \
             New instance runs |"
        );
        println!("|---|---|---|---|---|---|---|---|");
        for bounds in [Bounds::Guard, Bounds::Checked] {
            let options = Options {
                bounds,
                ..Options::default()
            };
            let compiled = Compiled::new(&options, PAGES, 0)?;
            for export in ["trap-unreachable", "trap-oob"] {
                let r: TrapReport = scenario::trap(&compiled, export, false)?;
                println!(
                    "| {bounds:?} | {} | {:?} | {} | {} | {} | {} | {} |",
                    r.export,
                    r.trap,
                    r.took.as_micros(),
                    r.counts.allocs,
                    r.counts.frees,
                    r.poisoned,
                    r.replacement_runs,
                );
            }
        }
        Ok(())
    }

    fn compile_table() -> Result<()> {
        println!("\n### Compile and load\n");
        println!(
            "| Backend | Filler functions | Text bytes | Machine code bytes | Engine ms | Compile ms | \
             Load from memory ms | Load from file ms | Instantiate p50 µs | Instantiate max µs |"
        );
        println!("|---|---|---|---|---|---|---|---|---|---|");
        for backend in [Backend::Cranelift, Backend::Winch] {
            for filler in [0, 300] {
                let options = Options {
                    backend,
                    ..Options::default()
                };
                let r: CompileReport = scenario::compile(&options, PAGES, filler)?;
                println!(
                    "| {backend:?} | {} | {} | {} | {:.1} | {:.1} | {:.2} | {:.2} | {} | {} |",
                    r.filler,
                    r.wat_bytes,
                    r.machine_code_bytes,
                    r.engine_ns as f64 / 1e6,
                    r.compile_ns as f64 / 1e6,
                    r.deserialize_ns as f64 / 1e6,
                    r.deserialize_file_ns as f64 / 1e6,
                    us(r.instantiate.p50),
                    us(r.instantiate.max),
                );
            }
        }
        Ok(())
    }
}
