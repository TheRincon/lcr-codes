//! MSB-first bit vector and cursor.
//!
//! Everything on the LC Code wire is a bit stream that does not respect byte boundaries:
//! the compact frame header is 15 bits (§9.1), the format region is 15 bits expanded to
//! 30 (§7), and chroma symbols are 1 or 2 bits (§5). One shared representation keeps the
//! bit order unambiguous — **MSB first, always** — rather than leaving each module to
//! pick.

use alloc::vec::Vec;

/// Growable MSB-first bit buffer.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct BitVec {
    bytes: Vec<u8>,
    len: usize,
}

impl BitVec {
    /// Empty buffer.
    pub const fn new() -> BitVec {
        BitVec {
            bytes: Vec::new(),
            len: 0,
        }
    }

    /// Empty buffer with room for `bits` bits.
    pub fn with_capacity(bits: usize) -> BitVec {
        BitVec {
            bytes: Vec::with_capacity(bits.div_ceil(8)),
            len: 0,
        }
    }

    /// Wrap whole bytes, MSB of `bytes[0]` becoming bit 0.
    pub fn from_bytes(bytes: &[u8]) -> BitVec {
        BitVec {
            bytes: bytes.to_vec(),
            len: bytes.len() * 8,
        }
    }

    /// Number of bits held.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when no bits are held.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Append one bit.
    #[inline]
    pub fn push(&mut self, bit: bool) {
        if self.len % 8 == 0 {
            self.bytes.push(0);
        }
        if bit {
            let i = self.len / 8;
            self.bytes[i] |= 0x80 >> (self.len % 8);
        }
        self.len += 1;
    }

    /// Append the low `n` bits of `value`, most significant of those first.
    pub fn push_bits(&mut self, value: u32, n: u32) {
        debug_assert!(n <= 32);
        for i in (0..n).rev() {
            self.push((value >> i) & 1 == 1);
        }
    }

    /// Append every bit of `bytes`.
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.push_bits(b as u32, 8);
        }
    }

    /// Append all of `other`.
    pub fn extend(&mut self, other: &BitVec) {
        for i in 0..other.len {
            self.push(other.get(i));
        }
    }

    /// Read bit `i`. Out-of-range reads return `false` rather than panicking, because
    /// the module walk legitimately runs past the end of a short codeword.
    #[inline]
    pub fn get(&self, i: usize) -> bool {
        if i >= self.len {
            return false;
        }
        self.bytes[i / 8] & (0x80 >> (i % 8)) != 0
    }

    /// Overwrite bit `i`. Panics if `i >= len`.
    #[inline]
    pub fn set(&mut self, i: usize, bit: bool) {
        assert!(i < self.len, "bit index {i} out of range {}", self.len);
        let mask = 0x80u8 >> (i % 8);
        if bit {
            self.bytes[i / 8] |= mask;
        } else {
            self.bytes[i / 8] &= !mask;
        }
    }

    /// Zero-extend to `n` bits. No-op if already at least that long.
    pub fn pad_to(&mut self, n: usize) {
        while self.len < n {
            self.push(false);
        }
    }

    /// Truncate to `n` bits. No-op if already shorter.
    pub fn truncate(&mut self, n: usize) {
        if n < self.len {
            self.len = n;
            self.bytes.truncate(n.div_ceil(8));
            // Clear the tail of the final partial byte so equality and `as_bytes` are
            // canonical.
            if n % 8 != 0 {
                let last = n / 8;
                self.bytes[last] &= !(0xFFu8 >> (n % 8));
            }
        }
    }

    /// Underlying bytes, zero-padded in the final partial byte.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consume into the padded byte vector.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Copy out `n` bytes starting at bit `at`. Missing bits read as zero.
    pub fn to_bytes_from(&self, at: usize, n: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(n);
        for j in 0..n {
            let mut b = 0u8;
            for k in 0..8 {
                b = (b << 1) | self.get(at + j * 8 + k) as u8;
            }
            out.push(b);
        }
        out
    }

    /// Read `n` bits starting at `at` as an integer, MSB first.
    pub fn read_bits(&self, at: usize, n: u32) -> u32 {
        let mut v = 0u32;
        for i in 0..n as usize {
            v = (v << 1) | self.get(at + i) as u32;
        }
        v
    }

    /// Iterate bits in order.
    pub fn iter(&self) -> impl Iterator<Item = bool> + '_ {
        (0..self.len).map(move |i| self.get(i))
    }
}

impl core::fmt::Debug for BitVec {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "BitVec[{}]:", self.len)?;
        for i in 0..self.len.min(64) {
            write!(f, "{}", self.get(i) as u8)?;
        }
        if self.len > 64 {
            write!(f, "…")?;
        }
        Ok(())
    }
}

impl FromIterator<bool> for BitVec {
    fn from_iter<T: IntoIterator<Item = bool>>(iter: T) -> Self {
        let mut v = BitVec::new();
        for b in iter {
            v.push(b);
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_read_roundtrip() {
        let mut b = BitVec::new();
        b.push_bits(0b1011, 4);
        b.push_bits(0x5A, 8);
        assert_eq!(b.len(), 12);
        assert_eq!(b.read_bits(0, 4), 0b1011);
        assert_eq!(b.read_bits(4, 8), 0x5A);
    }

    #[test]
    fn bytes_roundtrip() {
        let data = [0xDE, 0xAD, 0xBE, 0xEF];
        let b = BitVec::from_bytes(&data);
        assert_eq!(b.to_bytes_from(0, 4), data);
    }

    #[test]
    fn truncate_is_canonical() {
        let mut a = BitVec::from_bytes(&[0xFF, 0xFF]);
        a.truncate(9);
        let mut b = BitVec::new();
        b.push_bits(0b1_1111_1111, 9);
        assert_eq!(a, b);
        assert_eq!(a.as_bytes(), &[0xFF, 0x80]);
    }

    #[test]
    fn reads_past_end_are_zero() {
        let b = BitVec::from_bytes(&[0xFF]);
        assert!(!b.get(8));
        assert_eq!(b.read_bits(4, 8), 0b1111_0000);
    }
}
