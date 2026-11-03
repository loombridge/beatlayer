//! The test guest, written as WebAssembly text so the spike needs no wasm
//! toolchain and builds the same on all three platforms.
//!
//! One core module serves three shapes:
//!
//! - a component whose `process` takes and returns `list<f32>` through the
//!   canonical ABI, with a `cabi_realloc` that hands out the instance's
//!   reserved input buffer, so the host's copy lands in memory the guest
//!   reserved and nothing allocates;
//! - the same module loaded on its own, where the host writes straight into
//!   the memory it exports (`process_direct`);
//! - a component with `tick`, which does nothing, for the cost of the call.
//!
//! The DSP is four feedback combs of reverb-sized delay lines (158 KiB)
//! over an interleaved stereo block, with the read and write positions kept
//! in the guest's memory. [`Reference`] does the same sums in Rust, so a test
//! can compare the guest's output bit for bit.

use std::fmt::Write;

/// Where the input block goes in the guest's memory, and the output block.
pub const INPUT: usize = 0x1000;
pub const OUTPUT: usize = 0x2000;
/// The most floats either buffer holds (4,096 bytes).
pub const MAX_FLOATS: usize = 1024;

const POSITIONS: u32 = 0x100;
const LENGTHS: u32 = 0x200;
const BASES: u32 = 0x210;
const LINES: u32 = 0x10000;

/// The four delay lines' lengths in floats.
pub const COMBS: [usize; 4] = [8936, 9504, 10216, 10848];
const FEEDBACK: f32 = 0.75;
const FEED: f32 = 0.25;
const WET: f32 = 0.25;

/// What the guest's `run` function computes, in Rust.
pub struct Reference {
    lines: Vec<Vec<f32>>,
    positions: [usize; 4],
}

impl Default for Reference {
    fn default() -> Self {
        Self {
            lines: COMBS.iter().map(|&len| vec![0.0; len]).collect(),
            positions: [0; 4],
        }
    }
}

impl Reference {
    pub fn run(&mut self, input: &[f32], output: &mut [f32]) {
        for (x, y) in input.iter().zip(output) {
            let mut acc = 0.0f32;
            for (k, &len) in COMBS.iter().enumerate() {
                let slot = &mut self.lines[k][self.positions[k]];
                let d = *slot;
                acc += d;
                *slot = x * FEED + d * FEEDBACK;
                self.positions[k] += 1;
                if self.positions[k] >= len {
                    self.positions[k] = 0;
                }
            }
            *y = x + acc * WET;
        }
    }
}

/// The module's body: memory of `pages` 64 KiB pages, the exports above, and
/// `filler` extra functions that nothing calls, to give the compiler
/// something the size of a real device to chew on.
pub fn core_body(pages: u32, filler: usize) -> String {
    let run = run_body();
    let mut out = String::new();
    let mut bases = Vec::new();
    let mut at = LINES;
    for len in COMBS {
        bases.push(at);
        at += len as u32 * 4;
    }
    let u32s = |values: &[u32]| {
        values
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .map(|b| format!("\\{b:02x}"))
            .collect::<String>()
    };
    let lens: Vec<u32> = COMBS.iter().map(|&l| l as u32).collect();
    write!(
        out,
        r#"
  (memory (export "memory") {pages})
  (data (i32.const {LENGTHS}) "{lens}")
  (data (i32.const {BASES}) "{bases}")
  (func $run (param $inp i32) (param $outp i32) (param $n i32){run})
  (func (export "process_direct") (param $frames i32)
    (call $run (i32.const {INPUT}) (i32.const {OUTPUT}) (i32.shl (local.get $frames) (i32.const 1))))
  (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
    (if (i32.gt_u (local.get 3) (i32.const {size})) (then unreachable))
    (i32.const {INPUT}))
  (func (export "process") (param $ptr i32) (param $len i32) (result i32)
    (call $run (local.get $ptr) (i32.const {OUTPUT}) (local.get $len))
    (i32.store (i32.const 0) (i32.const {OUTPUT}))
    (i32.store (i32.const 4) (local.get $len))
    (i32.const 0))
  (func (export "tick") (param $frames i32) (result i32) (local.get $frames))
  (func (export "spin") (loop $l (br $l)))
  (func (export "trap_unreachable") unreachable)
  (func (export "trap_oob") (drop (i32.load (i32.const 0x7ffffff0))))
"#,
        lens = u32s(&lens),
        bases = u32s(&bases),
        size = MAX_FLOATS * 4,
    )
    .unwrap();
    for i in 0..filler {
        writeln!(
            out,
            "  (func $filler{i} (param $inp i32) (param $outp i32) (param $n i32){run})"
        )
        .unwrap();
    }
    out
}

/// The comb loop, from locals to the closing parenthesis.
fn run_body() -> String {
    format!(
        r#"
    (local $i i32) (local $k i32) (local $x f32) (local $acc f32)
    (local $pos i32) (local $addr i32) (local $d f32)
    (block $done
      (loop $samples
        (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
        (local.set $x (f32.load (i32.add (local.get $inp) (i32.shl (local.get $i) (i32.const 2)))))
        (local.set $acc (f32.const 0))
        (local.set $k (i32.const 0))
        (loop $combs
          (local.set $pos (i32.load (i32.add (i32.const {POSITIONS}) (i32.shl (local.get $k) (i32.const 2)))))
          (local.set $addr
            (i32.add
              (i32.load (i32.add (i32.const {BASES}) (i32.shl (local.get $k) (i32.const 2))))
              (i32.shl (local.get $pos) (i32.const 2))))
          (local.set $d (f32.load (local.get $addr)))
          (local.set $acc (f32.add (local.get $acc) (local.get $d)))
          (f32.store (local.get $addr)
            (f32.add (f32.mul (local.get $x) (f32.const {FEED})) (f32.mul (local.get $d) (f32.const {FEEDBACK}))))
          (local.set $pos (i32.add (local.get $pos) (i32.const 1)))
          (if (i32.ge_u (local.get $pos) (i32.load (i32.add (i32.const {LENGTHS}) (i32.shl (local.get $k) (i32.const 2)))))
            (then (local.set $pos (i32.const 0))))
          (i32.store (i32.add (i32.const {POSITIONS}) (i32.shl (local.get $k) (i32.const 2))) (local.get $pos))
          (local.set $k (i32.add (local.get $k) (i32.const 1)))
          (br_if $combs (i32.lt_u (local.get $k) (i32.const 4))))
        (f32.store (i32.add (local.get $outp) (i32.shl (local.get $i) (i32.const 2)))
          (f32.add (local.get $x) (f32.mul (local.get $acc) (f32.const {WET}))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $samples)))
  "#
    )
}

/// The guest as a standalone core module.
pub fn module_wat(pages: u32, filler: usize) -> String {
    format!("(module{})", core_body(pages, filler))
}

/// The guest as a component.
pub fn component_wat(pages: u32, filler: usize) -> String {
    format!(
        r#"(component
  (core module $m{body})
  (core instance $i (instantiate $m))
  (func (export "process") (param "input" (list f32)) (result (list f32))
    (canon lift (core func $i "process") (memory (core memory $i "memory")) (realloc (core func $i "cabi_realloc"))))
  (func (export "tick") (param "frames" u32) (result u32)
    (canon lift (core func $i "tick")))
  (func (export "spin") (canon lift (core func $i "spin")))
  (func (export "trap-unreachable") (canon lift (core func $i "trap_unreachable")))
  (func (export "trap-oob") (canon lift (core func $i "trap_oob")))
)"#,
        body = core_body(pages, filler)
    )
}
