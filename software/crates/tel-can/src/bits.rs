//! Bit-field extraction and insertion with DBC bit numbering.
//!
//! * `LittleEndian` (DBC `@1`, Intel): `start_bit` is the position of the least
//!   significant bit; bit `n` lives in byte `n / 8`, bit `n % 8`, and
//!   significance increases with `n`.
//! * `BigEndian` (DBC `@0`, Motorola): `start_bit` is the position of the most
//!   significant bit in the same `n / 8`, `n % 8` numbering; moving toward the
//!   least significant bit steps down within a byte and then jumps to bit 7 of
//!   the next byte.
//!
//! Both functions return `None` instead of reading or writing out of range.

/// Byte order of a signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ByteOrder {
    /// DBC `@1`.
    LittleEndian,
    /// DBC `@0`.
    BigEndian,
}

/// Largest field width supported.
pub const MAX_BITS: u8 = 64;

fn bit_at(data: &[u8], pos: u16) -> Option<u64> {
    let byte = data.get(usize::from(pos / 8))?;
    Some(u64::from(byte.checked_shr(u32::from(pos % 8))? & 1))
}

/// Position that follows `pos` when walking from the MSB toward the LSB of a
/// big-endian signal.
fn next_big_endian(pos: u16) -> Option<u16> {
    if pos % 8 == 0 {
        pos.checked_add(15)
    } else {
        pos.checked_sub(1)
    }
}

/// Extract `len` bits (1 to 64) as an unsigned value.
pub fn extract(data: &[u8], start_bit: u16, len: u8, order: ByteOrder) -> Option<u64> {
    if len == 0 || len > MAX_BITS {
        return None;
    }
    let mut value: u64 = 0;
    match order {
        ByteOrder::LittleEndian => {
            for i in 0..u16::from(len) {
                let bit = bit_at(data, start_bit.checked_add(i)?)?;
                value |= bit.checked_shl(u32::from(i))?;
            }
        }
        ByteOrder::BigEndian => {
            let mut pos = start_bit;
            for k in 0..len {
                let bit = bit_at(data, pos)?;
                // MSB first: the first bit read has weight 2^(len-1).
                let weight = u32::from(len.checked_sub(1)?.checked_sub(k)?);
                value |= bit.checked_shl(weight)?;
                if k.checked_add(1)? < len {
                    pos = next_big_endian(pos)?;
                }
            }
        }
    }
    Some(value)
}

/// Insert the low `len` bits of `value` into `data`, leaving all other bits
/// untouched. Returns `None` if the field does not fit or `value` has bits set
/// above `len`.
pub fn insert(data: &mut [u8], start_bit: u16, len: u8, order: ByteOrder, value: u64) -> Option<()> {
    if len == 0 || len > MAX_BITS {
        return None;
    }
    if len < MAX_BITS && value.checked_shr(u32::from(len))? != 0 {
        return None;
    }
    // Validate the whole field first so a failure leaves `data` unchanged.
    extract(data, start_bit, len, order)?;
    let set = |data: &mut [u8], pos: u16, bit: u64| -> Option<()> {
        let byte = data.get_mut(usize::from(pos / 8))?;
        let mask = 1_u8 << (pos % 8);
        if bit != 0 {
            *byte |= mask;
        } else {
            *byte &= !mask;
        }
        Some(())
    };
    match order {
        ByteOrder::LittleEndian => {
            for i in 0..u16::from(len) {
                let bit = value.checked_shr(u32::from(i))? & 1;
                set(data, start_bit.checked_add(i)?, bit)?;
            }
        }
        ByteOrder::BigEndian => {
            let mut pos = start_bit;
            for k in 0..len {
                let weight = u32::from(len.checked_sub(1)?.checked_sub(k)?);
                let bit = value.checked_shr(weight)? & 1;
                set(data, pos, bit)?;
                if k.checked_add(1)? < len {
                    pos = next_big_endian(pos)?;
                }
            }
        }
    }
    Some(())
}

/// Sign-extend the low `len` bits of `raw` to an `i64`. A width of 0 is
/// invalid and yields 0; a width of 64 reinterprets the bits.
pub fn sign_extend(raw: u64, len: u8) -> i64 {
    if len == 0 {
        return 0;
    }
    let shift = u32::from(MAX_BITS.saturating_sub(len.min(MAX_BITS)));
    // Move the sign bit to bit 63, reinterpret as signed, then shift back.
    let shifted = raw.checked_shl(shift).unwrap_or(raw);
    let signed = i64::from_ne_bytes(shifted.to_ne_bytes());
    signed.checked_shr(shift).unwrap_or(signed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ByteOrder::{BigEndian as BE, LittleEndian as LE};

    #[test]
    fn little_endian_known_answers() {
        let d = [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0];
        assert_eq!(extract(&d, 0, 8, LE), Some(0x12));
        assert_eq!(extract(&d, 0, 16, LE), Some(0x3412));
        assert_eq!(extract(&d, 0, 32, LE), Some(0x7856_3412));
        assert_eq!(extract(&d, 4, 8, LE), Some(0x41)); // straddles bytes: high nibble of b0 then low nibble of b1
        assert_eq!(extract(&d, 48, 16, LE), Some(0xF0DE));
        assert_eq!(extract(&d, 0, 64, LE), Some(0xF0DE_BC9A_7856_3412));
        assert_eq!(extract(&d, 7, 1, LE), Some(0));
        assert_eq!(extract(&d, 4, 1, LE), Some(1));
    }

    #[test]
    fn big_endian_known_answers() {
        let d = [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0];
        // start_bit 7 is the MSB of byte 0.
        assert_eq!(extract(&d, 7, 8, BE), Some(0x12));
        assert_eq!(extract(&d, 7, 16, BE), Some(0x1234));
        assert_eq!(extract(&d, 7, 32, BE), Some(0x1234_5678));
        assert_eq!(extract(&d, 7, 64, BE), Some(0x1234_5678_9ABC_DEF0));
        // MSB at bit 3 of byte 0: low nibble of b0 (high part), then high nibble of b1.
        assert_eq!(extract(&d, 3, 8, BE), Some(0x23));
        // 12 bit field starting at byte 1 MSB.
        assert_eq!(extract(&d, 15, 12, BE), Some(0x345));
        assert_eq!(extract(&d, 55, 16, BE), Some(0xDEF0));
    }

    #[test]
    fn single_bits() {
        let d = [0b1000_0001, 0b0000_0010];
        assert_eq!(extract(&d, 0, 1, LE), Some(1));
        assert_eq!(extract(&d, 7, 1, LE), Some(1));
        assert_eq!(extract(&d, 9, 1, LE), Some(1));
        assert_eq!(extract(&d, 8, 1, LE), Some(0));
        assert_eq!(extract(&d, 7, 1, BE), Some(1));
        assert_eq!(extract(&d, 9, 1, BE), Some(1));
    }

    #[test]
    fn out_of_range_is_none() {
        let d = [0_u8; 2];
        assert_eq!(extract(&d, 0, 17, LE), None);
        assert_eq!(extract(&d, 9, 8, LE), None);
        assert_eq!(extract(&d, 16, 1, LE), None);
        assert_eq!(extract(&d, 0, 0, LE), None);
        assert_eq!(extract(&d, 0, 65, LE), None);
        assert_eq!(extract(&d, 7, 17, BE), None);
        // MSB at bit 0 of byte 0: next bits are 15,14,...,8 (all in byte 1), so 9 bits fit and the 10th would be bit 23.
        assert_eq!(extract(&d, 0, 9, BE), Some(0));
        assert_eq!(extract(&d, 0, 10, BE), None);
        assert_eq!(extract(&[], 0, 1, LE), None);
        assert_eq!(extract(&d, u16::MAX, 8, LE), None);
        assert_eq!(extract(&d, u16::MAX, 8, BE), None);
    }

    #[test]
    fn insert_then_extract_roundtrip_both_orders() {
        for order in [LE, BE] {
            let mut d = [0_u8; 8];
            insert(&mut d, if order == LE { 5 } else { 12 }, 11, order, 0x5A7).unwrap();
            assert_eq!(extract(&d, if order == LE { 5 } else { 12 }, 11, order), Some(0x5A7));
        }
    }

    #[test]
    fn insert_leaves_other_bits_alone_and_is_atomic_on_failure() {
        let mut d = [0xFF_u8; 4];
        insert(&mut d, 8, 8, LE, 0x00).unwrap();
        assert_eq!(d, [0xFF, 0x00, 0xFF, 0xFF]);
        // value too wide
        let mut e = [0xAA_u8; 2];
        assert_eq!(insert(&mut e, 0, 4, LE, 0x10), None);
        assert_eq!(e, [0xAA, 0xAA]);
        // field does not fit: nothing written
        assert_eq!(insert(&mut e, 12, 8, LE, 0xFF), None);
        assert_eq!(e, [0xAA, 0xAA]);
        assert_eq!(insert(&mut e, 0, 0, LE, 0), None);
    }

    #[test]
    fn sign_extension() {
        assert_eq!(sign_extend(0xFF, 8), -1);
        assert_eq!(sign_extend(0x80, 8), -128);
        assert_eq!(sign_extend(0x7F, 8), 127);
        assert_eq!(sign_extend(0x8000, 16), -32768);
        assert_eq!(sign_extend(0x1, 1), -1);
        assert_eq!(sign_extend(0xFFFF_FFFF, 32), -1);
        assert_eq!(sign_extend(u64::MAX, 64), -1);
        assert_eq!(sign_extend(5, 64), 5);
    }
}
