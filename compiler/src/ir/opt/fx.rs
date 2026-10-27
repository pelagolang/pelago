//! A fast hasher for the pass tables. The keys are small integers (values, blocks, instruction
//! numbers), where the default `SipHash` costs more than the work around it; hash flooding is no
//! concern for compiler-internal tables. This is the FxHash of rustc: a multiply and a rotate per word.

use std::hash::{BuildHasherDefault, Hasher};

#[derive(Default, Clone, Copy)]
pub struct FxHasher {
    hash: u64,
}

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl FxHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut w = [0u8; 8];
            w[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(w));
        }
    }

    #[inline]
    fn write_u8(&mut self, n: u8) {
        self.add(n as u64);
    }

    #[inline]
    fn write_u32(&mut self, n: u32) {
        self.add(n as u64);
    }

    #[inline]
    fn write_u64(&mut self, n: u64) {
        self.add(n);
    }

    #[inline]
    fn write_usize(&mut self, n: usize) {
        self.add(n as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

/// Build with `HashMap::default()` (there is no `new` for a custom hasher).
pub type HashMap<K, V> = std::collections::HashMap<K, V, BuildHasherDefault<FxHasher>>;
pub type HashSet<K> = std::collections::HashSet<K, BuildHasherDefault<FxHasher>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn behaves_like_a_map() {
        let mut m: HashMap<u32, u32> = HashMap::default();
        for i in 0..1000 {
            m.insert(i, i * 2);
        }
        assert_eq!(m.len(), 1000);
        assert_eq!(m[&999], 1998);
        let s: HashSet<(u32, u64)> = [(1, 2), (1, 2), (3, 4)].into_iter().collect();
        assert_eq!(s.len(), 2);
    }
}
