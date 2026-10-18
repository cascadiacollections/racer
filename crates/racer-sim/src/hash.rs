//! FNV-1a, the one hash the sim uses for determinism checks. DESIGN §6, §9.
//!
//! Split out of Salt Line's `replay.rs`, verbatim: here both the track digests
//! (`track::track_hash`) and the run state hash (`replay::state_hash`) need
//! it, and neither should depend on the other.
//!
//! FNV-1a is not a strong hash and does not need to be. Its job is to notice
//! that two runs differ by a single bit, on every platform, with no library:
//! one xor and one multiply per byte, all wrapping integer arithmetic.

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a. Chosen because it is trivially identical on every platform and has
/// no library dependency — the sim crate takes none.
#[derive(Clone, Copy, Debug)]
pub struct Hasher(u64);

impl Default for Hasher {
    fn default() -> Self {
        Hasher(FNV_OFFSET)
    }
}

impl Hasher {
    /// A hasher at the FNV-1a offset basis.
    pub fn new() -> Self {
        Self::default()
    }
    /// The hash of everything written so far.
    pub fn finish(self) -> u64 {
        self.0
    }
    /// Mix in one byte: the FNV-1a step, xor then multiply.
    pub fn write_u8(&mut self, v: u8) {
        self.0 ^= v as u64;
        self.0 = self.0.wrapping_mul(FNV_PRIME);
    }
    /// Mix in eight bytes, little-endian whatever the host, so the hash is
    /// the same on every platform.
    pub fn write_u64(&mut self, v: u64) {
        for b in v.to_le_bytes() {
            self.write_u8(b);
        }
    }
    /// Sign-extended to 64 bits, then as [`Hasher::write_u64`].
    pub fn write_i32(&mut self, v: i32) {
        self.write_u64(v as i64 as u64);
    }
    /// Widened to 64 bits, then as [`Hasher::write_u64`].
    pub fn write_u32(&mut self, v: u32) {
        self.write_u64(v as u64);
    }
    /// Widened to 64 bits, so 32- and 64-bit targets agree.
    pub fn write_usize(&mut self, v: usize) {
        self.write_u64(v as u64);
    }
    /// Hashes the bit pattern, so a one-ULP difference anywhere shows up.
    pub fn write_f64(&mut self, v: f64) {
        self.write_u64(v.to_bits());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_published_fnv1a_vectors() {
        // FNV-1a 64 test vectors from the reference: "" and "a".
        assert_eq!(Hasher::new().finish(), 0xcbf2_9ce4_8422_2325);
        let mut h = Hasher::new();
        h.write_u8(b'a');
        assert_eq!(h.finish(), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn floats_hash_by_bit_pattern() {
        // 0.0 and -0.0 compare equal but are different bits; a determinism
        // hash must tell them apart.
        let (mut a, mut b) = (Hasher::new(), Hasher::new());
        a.write_f64(0.0);
        b.write_f64(-0.0);
        assert_ne!(a.finish(), b.finish());
    }

    #[test]
    fn widths_are_normalised_before_hashing() {
        let (mut a, mut b, mut c) = (Hasher::new(), Hasher::new(), Hasher::new());
        a.write_u32(7);
        b.write_usize(7);
        c.write_u64(7);
        assert_eq!(a.finish(), c.finish());
        assert_eq!(b.finish(), c.finish());
    }
}
