//! A small seeded generator, so every simulated run replays from its seed.

/// `SplitMix64`: fast, well distributed, and dependency free. A generator
/// may first draw from a tape of bytes, so a coverage-guided fuzzer can
/// steer the simulation's choices toward interleavings seeds rarely reach.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
    tape: Vec<u8>,
    read: usize,
}

impl Rng {
    /// Creates a generator for `seed`.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            state: seed,
            tape: Vec::new(),
            read: 0,
        }
    }

    /// Creates a generator whose draws come from `tape`, eight bytes each,
    /// until it runs out, and from `seed` after that.
    #[must_use]
    pub fn from_tape(seed: u64, tape: &[u8]) -> Self {
        Self {
            tape: tape.to_vec(),
            ..Self::new(seed)
        }
    }

    /// Returns the next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        if self.read < self.tape.len() {
            let end = (self.read + 8).min(self.tape.len());
            let mut bytes = [0; 8];
            bytes[..end - self.read]
                .copy_from_slice(&self.tape[self.read..end]);
            self.read = end;
            return u64::from_le_bytes(bytes);
        }
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Returns a value in `0..n`, or 0 when `n` is 0.
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }

    /// Returns a `usize` in `0..n`, or 0 when `n` is 0.
    pub fn index(&mut self, n: usize) -> usize {
        usize::try_from(self.below(n as u64)).unwrap_or(0)
    }

    /// Returns a value in `low..=high`.
    pub fn range(&mut self, low: u64, high: u64) -> u64 {
        low + self.below(high.saturating_sub(low) + 1)
    }

    /// Returns true with probability `per_mille / 1000`.
    pub fn chance(&mut self, per_mille: u64) -> bool {
        self.below(1000) < per_mille
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tape_supplies_the_first_draws_then_the_seed_takes_over() {
        let mut taped = Rng::from_tape(9, &[1, 0, 0, 0, 0, 0, 0, 0, 2]);
        assert_eq!(taped.next_u64(), 1);
        assert_eq!(taped.next_u64(), 2, "a short last draw is zero-padded");
        let mut seeded = Rng::new(9);
        for _ in 0..4 {
            assert_eq!(taped.next_u64(), seeded.next_u64());
        }
    }
}
