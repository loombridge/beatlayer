//! The guest computes what the Rust reference computes, in every shape.

use ext_wasmtime_spike::alloc::Counting;
use ext_wasmtime_spike::guest::Reference;
use ext_wasmtime_spike::host::{Comp, Compiled, Direct, Options};

#[global_allocator]
static ALLOC: Counting = Counting;

/// Blocks of stereo frames from a fixed sequence.
fn blocks(count: usize, frames: usize) -> Vec<Vec<f32>> {
    let mut seed = 0x2545_f491u32;
    (0..count)
        .map(|_| {
            (0..frames * 2)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
                })
                .collect()
        })
        .collect()
}

#[test]
fn every_shape_matches_the_reference() {
    let compiled = Compiled::new(&Options::default(), 8, 0).unwrap();
    let mut direct = Direct::new(&compiled.engine, &compiled.module).unwrap();
    let mut reserved = Comp::new(&compiled.engine, &compiled.component).unwrap();
    let mut vec = Comp::new(&compiled.engine, &compiled.component).unwrap();
    let mut reference = Reference::default();
    for block in blocks(300, 32) {
        let mut want = vec![0.0; block.len()];
        reference.run(&block, &mut want);
        let mut got = vec![0.0; block.len()];
        direct.process(&block, &mut got, u64::MAX / 2).unwrap();
        assert_eq!(want, got, "direct");
        reserved
            .process_reserved(&block, &mut got, u64::MAX / 2)
            .unwrap();
        assert_eq!(want, got, "reserved");
        assert_eq!(want, vec.process_vec(&block, u64::MAX / 2).unwrap(), "vec");
    }
}
