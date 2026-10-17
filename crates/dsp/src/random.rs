//! A seeded random source for the effects' sample-and-hold, random and
//! wander shapes (the device framework spec, R29).

/// A random source whose values depend only on its seed, so a render
/// repeats run after run. It is a plain value: nothing in it allocates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Random {
    state: u64,
}

impl Random {
    /// A source for `seed`. Seeds that differ by one (device IDs, say) give
    /// unrelated streams: the seed is mixed before it is used.
    #[must_use]
    pub fn seeded(seed: u64) -> Self {
        let mut random = Self { state: seed };
        // Two rounds put a small seed well away from zero.
        random.next_u64();
        random.next_u64();
        random
    }

    /// The next 64 random bits (SplitMix64).
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// The next value, from −1 up to (not including) 1.
    pub fn next_bipolar(&mut self) -> f64 {
        // The top 53 bits make an exact f64 in [0, 1).
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: device-framework R29
    #[test]
    fn a_seed_gives_the_same_stream_and_other_seeds_other_streams() {
        let stream = |seed| {
            let mut random = Random::seeded(seed);
            (0..64).map(|_| random.next_bipolar()).collect::<Vec<_>>()
        };
        assert_eq!(stream(1), stream(1));
        assert_ne!(stream(1), stream(2));
        assert!(stream(1).iter().all(|v| (-1.0..1.0).contains(v)));
        let mean = stream(1).iter().sum::<f64>() / 64.0;
        assert!(mean.abs() < 0.3, "centred: {mean}");
    }
}
