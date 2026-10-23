//! CRC-32 (ISO-HDLC, the zlib/Ethernet CRC): reflected polynomial 0xEDB88320,
//! initial value and final XOR 0xFFFFFFFF. Check value for `"123456789"` is
//! `0xCBF43926`.

// Evaluated at compile time with `i < 256`; an out-of-range index would be a
// compile error, not a run-time panic, so direct indexing is safe here.
#[allow(clippy::indexing_slicing, clippy::arithmetic_side_effects, clippy::cast_possible_truncation)]
const fn make_table() -> [u32; 256] {
    let mut table = [0_u32; 256];
    let mut i: u32 = 0;
    while i < 256 {
        let mut c = i;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i as usize] = c;
        i += 1;
    }
    table
}

static TABLE: [u32; 256] = make_table();

/// Incremental CRC-32.
#[derive(Clone, Copy, Debug)]
pub struct Crc32 {
    state: u32,
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

impl Crc32 {
    /// Start a new checksum.
    pub const fn new() -> Self {
        Self { state: 0xFFFF_FFFF }
    }

    /// Add bytes.
    pub fn update(&mut self, data: &[u8]) {
        let mut c = self.state;
        for &b in data {
            let idx = usize::from((c ^ u32::from(b)).to_le_bytes().first().copied().unwrap_or(0));
            let t = TABLE.get(idx).copied().unwrap_or(0);
            c = t ^ (c >> 8);
        }
        self.state = c;
    }

    /// The checksum of everything added so far.
    pub const fn finish(&self) -> u32 {
        self.state ^ 0xFFFF_FFFF
    }
}

/// CRC-32 of a byte slice.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = Crc32::new();
    c.update(data);
    c.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_check_values() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
        assert_eq!(crc32(b"The quick brown fox jumps over the lazy dog"), 0x414F_A339);
        assert_eq!(crc32(&[0_u8; 32]), 0x190A_55AD);
    }

    #[test]
    fn incremental_equals_one_shot() {
        let data: std::vec::Vec<u8> = (0..=255_u8).cycle().take(1000).collect();
        for split in [0, 1, 7, 500, 999, 1000] {
            let mut c = Crc32::new();
            c.update(&data[..split]);
            c.update(&data[split..]);
            assert_eq!(c.finish(), crc32(&data));
        }
    }

    #[test]
    fn detects_every_single_bit_flip_in_a_block() {
        let mut data = [0xA5_u8; 64];
        let good = crc32(&data);
        for byte in 0..data.len() {
            for bit in 0..8 {
                data[byte] ^= 1 << bit;
                assert_ne!(crc32(&data), good, "byte {byte} bit {bit}");
                data[byte] ^= 1 << bit;
            }
        }
    }
}
