//! A tiny inline bitset used as the board bitboard inside the search.
//!
//! Storage is `[u64; 4]` (up to 256 cells) so that placement masks live
//! inline in the `Placement` struct — no per-placement heap allocation,
//! better cache locality, and the compiler can unroll the word loop.

/// Maximum board size this bitset supports (256 cells).
const MAX_WORDS: usize = 4;

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct BitSet {
    words: [u64; MAX_WORDS],
    len: usize,
}

#[inline]
pub const fn words_for(bits: usize) -> usize {
    bits.div_ceil(64)
}

impl BitSet {
    pub fn new(bits: usize) -> Self {
        let len = words_for(bits).min(MAX_WORDS);
        debug_assert!(
            bits <= MAX_WORDS * 64,
            "board too large for inline BitSet (max {} cells)",
            MAX_WORDS * 64
        );
        Self {
            words: [0u64; MAX_WORDS],
            len,
        }
    }

    #[inline]
    pub fn words(&self) -> &[u64] {
        &self.words[..self.len]
    }

    #[inline]
    pub fn set(&mut self, bit: usize) {
        self.words[bit / 64] |= 1u64 << (bit % 64);
    }

    #[inline]
    pub fn get(&self, bit: usize) -> bool {
        self.words[bit / 64] & (1u64 << (bit % 64)) != 0
    }

    #[inline]
    pub fn count(&self) -> u32 {
        self.words[..self.len].iter().map(|w| w.count_ones()).sum()
    }

    /// `self |= other`
    #[inline]
    pub fn or_assign(&mut self, other: &Self) {
        for i in 0..self.len {
            self.words[i] |= other.words[i];
        }
    }

    /// `self ^= other`. Used to undo a placement: a placement never overlaps
    /// what is already on the board, so XOR removes exactly its bits.
    #[inline]
    pub fn xor_assign(&mut self, other: &Self) {
        for i in 0..self.len {
            self.words[i] ^= other.words[i];
        }
    }

    #[inline]
    pub fn intersects(&self, other: &Self) -> bool {
        for i in 0..self.len {
            if self.words[i] & other.words[i] != 0 {
                return true;
            }
        }
        false
    }

    /// The lowest set bit, if any.
    #[inline]
    #[allow(dead_code)]
    pub fn first_set(&self) -> Option<usize> {
        for i in 0..self.len {
            if self.words[i] != 0 {
                return Some(i * 64 + self.words[i].trailing_zeros() as usize);
            }
        }
        None
    }

    /// Collects the set bit indices in ascending order.
    pub fn indices(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.count() as usize);
        for i in 0..self.len {
            let mut w = self.words[i];
            while w != 0 {
                let bit = w.trailing_zeros() as usize;
                out.push((i * 64 + bit) as u32);
                w &= w - 1;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_ops() {
        let mut a = BitSet::new(200);
        a.set(3);
        a.set(70);
        a.set(199);
        assert_eq!(a.count(), 3);
        assert_eq!(a.first_set(), Some(3));
        assert_eq!(a.indices(), vec![3, 70, 199]);

        let mut b = BitSet::new(200);
        b.set(70);
        b.set(199);
        assert!(a.intersects(&b));
        a.xor_assign(&b);
        assert_eq!(a.indices(), vec![3]);
        assert!(!a.intersects(&b));
    }
}
