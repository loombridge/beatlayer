//! The wasmtime host: an engine with a pooling allocator and epoch
//! interruption, a ticker thread, and the guest instances in the shapes the
//! spike compares.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use wasmtime::component::{self, Component, Func, Linker, WasmList};
use wasmtime::{
    Config, Engine, Instance, InstanceAllocationStrategy, Memory, Module, PoolingAllocationConfig,
    Result, Store, TypedFunc,
};

use crate::guest::{INPUT, OUTPUT};

/// The epoch deadline, in ticks, for set-up: instantiating runs guest code, and
/// a deadline of zero stops it at once. Set-up isn't on the audio thread.
const SETUP: u64 = 1 << 40;

/// Which compiler makes the machine code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Cranelift,
    Winch,
}

/// How a linear memory's bounds are checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bounds {
    /// A 4 GiB reservation and a guard region, so an out-of-bounds access is a
    /// page fault the signal handler turns into a trap: no check per access.
    Guard,
    /// A reservation the size of the largest memory and no guard: the compiler
    /// emits a comparison before every access.
    Checked,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub backend: Backend,
    pub bounds: Bounds,
    /// The largest memory an instance may have (R9 says up to 64 MiB).
    pub max_memory: usize,
    /// How many instances the pool holds.
    pub instances: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            backend: if cfg!(feature = "cranelift") {
                Backend::Cranelift
            } else {
                Backend::Winch
            },
            bounds: Bounds::Guard,
            max_memory: 64 << 20,
            instances: 8,
        }
    }
}

/// An engine with the pooling allocator (every instance's memory and table
/// reserved up front) and epoch interruption.
pub fn engine(options: &Options) -> Result<Engine> {
    let mut config = Config::new();
    config.epoch_interruption(true);
    config.wasm_backtrace_max_frames(None);
    #[cfg(feature = "compile")]
    config.strategy(match options.backend {
        Backend::Cranelift => wasmtime::Strategy::Cranelift,
        Backend::Winch => wasmtime::Strategy::Winch,
    });
    if options.bounds == Bounds::Checked {
        config
            .memory_reservation(options.max_memory as u64)
            .memory_guard_size(0)
            .guard_before_linear_memory(false);
    }
    let mut pool = PoolingAllocationConfig::new();
    pool.total_component_instances(options.instances)
        .total_core_instances(options.instances)
        .total_memories(options.instances)
        .total_tables(options.instances)
        .max_memory_size(options.max_memory);
    config.allocation_strategy(InstanceAllocationStrategy::Pooling(pool));
    Engine::new(&config)
}

/// How the ticker waits between ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wait {
    /// `thread::sleep`: costs nothing between ticks, and the system rounds the
    /// period up.
    Sleep,
    /// Spins on the clock: exact, and keeps a core busy. Here to show the floor.
    Spin,
}

/// A thread that advances the engine's epoch every `period`. A call's deadline
/// is a number of these ticks, so a call is stopped between one tick less and
/// the deadline's worth of ticks after it started.
pub struct Ticker {
    stop: Arc<AtomicBool>,
    ticks: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl Ticker {
    pub fn start(engine: &Engine, period: Duration, wait: Wait) -> Ticker {
        let stop = Arc::new(AtomicBool::new(false));
        let ticks = Arc::new(AtomicU64::new(0));
        let thread = thread::Builder::new()
            .name("epoch-ticker".into())
            .spawn({
                let (engine, stop, ticks) = (engine.clone(), stop.clone(), ticks.clone());
                move || {
                    let mut next = Instant::now() + period;
                    while !stop.load(Ordering::Relaxed) {
                        match wait {
                            Wait::Sleep => thread::sleep(period),
                            Wait::Spin => {
                                while Instant::now() < next && !stop.load(Ordering::Relaxed) {
                                    std::hint::spin_loop();
                                }
                                next += period;
                            }
                        }
                        engine.increment_epoch();
                        ticks.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })
            .expect("a thread starts");
        Ticker {
            stop,
            ticks,
            thread: Some(thread),
        }
    }

    pub fn ticks(&self) -> u64 {
        self.ticks.load(Ordering::Relaxed)
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The guest compiled both ways.
pub struct Compiled {
    pub engine: Engine,
    pub component: Component,
    pub module: Module,
}

#[cfg(feature = "compile")]
impl Compiled {
    pub fn new(options: &Options, pages: u32, filler: usize) -> Result<Compiled> {
        let engine = engine(options)?;
        let component = Component::new(&engine, crate::guest::component_wat(pages, filler))?;
        let module = Module::new(&engine, crate::guest::module_wat(pages, filler))?;
        Ok(Compiled {
            engine,
            component,
            module,
        })
    }
}

/// Machine code for the component, as a file's worth of bytes.
#[cfg(feature = "compile")]
pub fn precompile_component(engine: &Engine, wat: &str) -> Result<Vec<u8>> {
    engine.precompile_component(wat.as_bytes())
}

/// Loads precompiled machine code. The bytes must come from this engine
/// configuration's own `precompile_component`; a cache that holds anything
/// else is a hole.
pub fn load_component(engine: &Engine, bytes: &[u8]) -> Result<Component> {
    // SAFETY: the caller's bytes are trusted output of `precompile_component`
    // for an engine with the same configuration. The spike's caches are its
    // own; a product would key and checksum them.
    unsafe { Component::deserialize(engine, bytes) }
}

/// Like [`load_component`], mapping the file instead of reading it.
pub fn load_component_file(engine: &Engine, path: &Path) -> Result<Component> {
    // SAFETY: as in `load_component`; the file isn't changed while mapped.
    unsafe { Component::deserialize_file(engine, path) }
}

/// The guest as a core module, written to directly.
pub struct Direct {
    pub store: Store<()>,
    memory: Memory,
    process: TypedFunc<(i32,), ()>,
}

impl Direct {
    pub fn new(engine: &Engine, module: &Module) -> Result<Direct> {
        let mut store = Store::new(engine, ());
        store.set_epoch_deadline(SETUP);
        let instance = Instance::new(&mut store, module, &[])?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .expect("the guest exports its memory");
        let process = instance.get_typed_func(&mut store, "process_direct")?;
        Ok(Direct {
            store,
            memory,
            process,
        })
    }

    /// One block: the host writes `input` into the guest's input buffer, the
    /// guest runs, the host reads the output buffer. `deadline` is in epoch
    /// ticks. shortcut: panics on a block over `MAX_FLOATS`, the spike's blocks are 32 frames.
    pub fn process(&mut self, input: &[f32], output: &mut [f32], deadline: u64) -> Result<()> {
        let data = self.memory.data_mut(&mut self.store);
        let (words, _) = data[INPUT..INPUT + input.len() * 4].as_chunks_mut::<4>();
        for (word, x) in words.iter_mut().zip(input) {
            *word = x.to_le_bytes();
        }
        self.store.set_epoch_deadline(deadline);
        self.process
            .call(&mut self.store, (input.len() as i32 / 2,))?;
        let data = self.memory.data(&self.store);
        let (words, _) = data[OUTPUT..OUTPUT + output.len() * 4].as_chunks::<4>();
        for (y, word) in output.iter_mut().zip(words) {
            *y = f32::from_le_bytes(*word);
        }
        Ok(())
    }
}

/// The guest as a component, called through the canonical ABI.
pub struct Comp {
    pub store: Store<()>,
    instance: component::Instance,
    process: Func,
    tick: Func,
}

impl Comp {
    pub fn new(engine: &Engine, component: &Component) -> Result<Comp> {
        let linker = Linker::new(engine);
        let mut store = Store::new(engine, ());
        store.set_epoch_deadline(SETUP);
        let instance = linker.instantiate(&mut store, component)?;
        let process = instance
            .get_func(&mut store, "process")
            .expect("the guest exports process");
        let tick = instance
            .get_func(&mut store, "tick")
            .expect("the guest exports tick");
        Ok(Comp {
            store,
            instance,
            process,
            tick,
        })
    }

    /// One block with the guest's buffers reserved: the lowered input lands in
    /// the guest's input buffer (its `cabi_realloc` hands that out), and the
    /// output is read in place from guest memory.
    pub fn process_reserved(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        deadline: u64,
    ) -> Result<()> {
        self.store.set_epoch_deadline(deadline);
        let process = self
            .process
            .typed::<(&[f32],), (WasmList<f32>,)>(&self.store)?;
        let (list,) = process.call(&mut self.store, (input,))?;
        // `WasmList` has raw slice access only for integer types, so each float
        // is lifted from guest memory on its own.
        for (y, x) in output.iter_mut().zip(list.iter(&mut self.store)?) {
            *y = x?;
        }
        Ok(())
    }

    /// One block the way a guest binding would hand it over by default: the
    /// result is a `Vec`, made by the host.
    pub fn process_vec(&mut self, input: &[f32], deadline: u64) -> Result<Vec<f32>> {
        self.store.set_epoch_deadline(deadline);
        let process = self.process.typed::<(&[f32],), (Vec<f32>,)>(&self.store)?;
        let (out,) = process.call(&mut self.store, (input,))?;
        Ok(out)
    }

    /// A call that does nothing: what crossing into the guest costs.
    pub fn tick(&mut self, deadline: u64) -> Result<u32> {
        self.store.set_epoch_deadline(deadline);
        let tick = self.tick.typed::<(u32,), (u32,)>(&self.store)?;
        Ok(tick.call(&mut self.store, (32,))?.0)
    }

    /// Calls a no-argument export (`spin`, `trap-unreachable`, `trap-oob`).
    pub fn call(&mut self, name: &str, deadline: u64) -> Result<()> {
        let func = self
            .instance
            .get_func(&mut self.store, name)
            .expect("the guest exports it");
        let func = func.typed::<(), ()>(&self.store)?;
        self.store.set_epoch_deadline(deadline);
        func.call(&mut self.store, ())
    }
}
