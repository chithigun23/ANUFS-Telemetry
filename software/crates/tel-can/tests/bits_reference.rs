#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    missing_docs
)]
//! Cross-check the bit extractor against an independent, deliberately naive
//! model, and verify insert/extract round trips.

use proptest::prelude::*;
use tel_can::{extract, insert, sign_extend, ByteOrder};

/// Bit positions of a signal listed from least to most significant.
fn positions(order: ByteOrder, start: u32, len: u32) -> Vec<u32> {
    match order {
        ByteOrder::LittleEndian => (0..len).map(|i| start + i).collect(),
        ByteOrder::BigEndian => {
            // Walk from the MSB down using the DBC "sawtooth".
            let mut v = Vec::new();
            let mut pos = start as i64;
            for _ in 0..len {
                v.push(pos as u32);
                // Within a byte move to the next lower bit; from bit 0 jump to
                // bit 7 of the next byte.
                pos = if pos % 8 == 0 { pos + 15 } else { pos - 1 };
            }
            v.reverse();
            v
        }
    }
}

fn ref_extract(data: &[u8], order: ByteOrder, start: u32, len: u32) -> Option<u64> {
    let mut value = 0_u64;
    for (i, p) in positions(order, start, len).into_iter().enumerate() {
        let byte = *data.get((p / 8) as usize)?;
        let bit = (byte >> (p % 8)) & 1;
        value |= u64::from(bit) << i;
    }
    Some(value)
}

fn order_strategy() -> impl Strategy<Value = ByteOrder> {
    prop_oneof![Just(ByteOrder::LittleEndian), Just(ByteOrder::BigEndian)]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(20_000))]

    #[test]
    fn matches_reference(
        data in prop::collection::vec(any::<u8>(), 0..=8),
        start in 0_u16..80,
        len in 1_u8..=64,
        order in order_strategy(),
    ) {
        prop_assert_eq!(
            extract(&data, start, len, order),
            ref_extract(&data, order, u32::from(start), u32::from(len))
        );
    }

    #[test]
    fn never_panics_on_any_input(
        data in prop::collection::vec(any::<u8>(), 0..=16),
        start in any::<u16>(),
        len in any::<u8>(),
        order in order_strategy(),
        value in any::<u64>(),
    ) {
        let _ = extract(&data, start, len, order);
        let mut copy = data.clone();
        let _ = insert(&mut copy, start, len, order, value);
        let _ = sign_extend(value, len);
    }

    #[test]
    fn insert_extract_roundtrip_and_isolation(
        data in prop::collection::vec(any::<u8>(), 8..=8),
        start in 0_u16..64,
        len in 1_u8..=64,
        order in order_strategy(),
        raw in any::<u64>(),
    ) {
        let masked = if len == 64 { raw } else { raw & ((1_u64 << len) - 1) };
        let mut buf = data.clone();
        let fits = extract(&data, start, len, order).is_some();
        let r = insert(&mut buf, start, len, order, masked);
        prop_assert_eq!(r.is_some(), fits);
        if fits {
            prop_assert_eq!(extract(&buf, start, len, order), Some(masked));
            // Every bit outside the field is unchanged.
            let field: std::collections::HashSet<u32> =
                positions(order, u32::from(start), u32::from(len)).into_iter().collect();
            for pos in 0..64_u32 {
                if !field.contains(&pos) {
                    let a = (data[(pos / 8) as usize] >> (pos % 8)) & 1;
                    let b = (buf[(pos / 8) as usize] >> (pos % 8)) & 1;
                    prop_assert_eq!(a, b, "bit {} changed", pos);
                }
            }
        } else {
            prop_assert_eq!(buf, data); // failure leaves the frame untouched
        }
    }

    #[test]
    fn insert_rejects_values_wider_than_the_field(
        start in 0_u16..32, len in 1_u8..32, order in order_strategy(), extra in 1_u64..1000,
    ) {
        let mut buf = [0_u8; 8];
        let too_big = (1_u64 << len) + extra - 1 + (1_u64 << len); // always has a bit set at or above `len`
        prop_assert_eq!(insert(&mut buf, start, len, order, too_big), None);
        prop_assert_eq!(buf, [0_u8; 8]);
    }

    #[test]
    fn sign_extend_matches_i128_reference(len in 1_u8..=64, raw in any::<u64>()) {
        let masked = if len == 64 { raw } else { raw & ((1_u64 << len) - 1) };
        let expect: i128 = if len == 64 {
            i128::from(masked as i64)
        } else if masked >> (len - 1) & 1 == 1 {
            i128::from(masked) - (1_i128 << len)
        } else {
            i128::from(masked)
        };
        prop_assert_eq!(i128::from(sign_extend(masked, len)), expect);
    }
}
