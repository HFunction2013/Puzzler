//! A tiny bitset used as the board bitboard inside the search.
//!
//! The solver only needs a handful of operations, and keeping them in one
//! `Vec<u64>` avoids per-node allocations: masks are built once, and the
//! occupancy word array is mutated in place while searching.

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct BitSet {
    words: Vec<u64>,
}

#[inline]
pub const fn words_for(bits: usize) -> usize {
    bits.div_ceil(64)
}

impl BitSet {
    pub fn new(bits: usize) -> Self {
        Self {
            words: vec![0; words_for(bits)],
        }
    }

    #[inline]
    pub fn words(&self) -> &[u64] {
        &self.words
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
        self.words.iter().map(|w| w.count_ones()).sum()
    }

    /// `self |= other`
    #[inline]
    pub fn or_assign(&mut self, other: &Self) {
        for (dst, src) in self.words.iter_mut().zip(other.words.iter()) {
            *dst |= *src;
        }
    }

    /// `self ^= other`. Used to undo a placement: a placement never overlaps
    /// what is already on the board, so XOR removes exactly its bits.
    #[inline]
    pub fn xor_assign(&mut self, other: &Self) {
        for (dst, src) in self.words.iter_mut().zip(other.words.iter()) {
            *dst ^= *src;
        }
    }

    #[inline]
    pub fn intersects(&self, other: &Self) -> bool {
        self.words
            .iter()
            .zip(other.words.iter())
            .any(|(a, b)| a & b != 0)
    }

    /// The lowest set bit, if any.
    #[inline]
    #[allow(dead_code)]
    pub fn first_set(&self) -> Option<usize> {
        self.words.iter().enumerate().find_map(|(i, &w)| {
            if w == 0 {
                None
            } else {
                Some(i * 64 + w.trailing_zeros() as usize)
            }
        })
    }

    /// Collects the set bit indices in ascending order.
    pub fn indices(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.count() as usize);
        for (i, &word) in self.words.iter().enumerate() {
            let mut w = word;
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
