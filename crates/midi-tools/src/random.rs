//! Beatlayer's own seeded generator (the MIDI Tools spec, R20): the same
//! seed gives the same numbers on every platform, since it is integer
//! arithmetic only. It is SplitMix64.

/// A seeded generator.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// A generator for an application's seed.
    #[must_use]
    pub fn new(seed: u32) -> Self {
        Self(u64::from(seed))
    }

    /// The next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A whole number from 0 up to but not including `n`; 0 for an `n` of 0.
    pub fn below(&mut self, n: u32) -> u32 {
        // The share of 32 bits that n takes: the bias is under one in four
        // billion of n, far below what a note can show.
        (((self.next_u64() >> 32) * u64::from(n)) >> 32) as u32
    }

    /// A number from 0 up to but not including 1.
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_numbers_everywhere() {
        // SplitMix64's published sequence for a seed of 0.
        let mut rng = Rng::new(0);
        assert_eq!(rng.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(rng.next_u64(), 0x6E78_9E6A_A1B9_65F4);
        assert_eq!(rng.next_u64(), 0x06C4_5D18_8009_454F);
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        assert_eq!(a.unit().to_bits(), b.unit().to_bits());
        assert_ne!(Rng::new(7).next_u64(), Rng::new(8).next_u64());
    }

    #[test]
    fn draws_stay_in_their_range() {
        let mut rng = Rng::new(u32::MAX);
        for _ in 0..1000 {
            assert!(rng.below(7) < 7);
            assert!((0.0..1.0).contains(&rng.unit()));
        }
        assert_eq!(rng.below(0), 0);
    }
}
