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
//! Catalog validation, lookup, decode and the latest-value store.

use proptest::prelude::*;
use tel_can::{ingest, insert, ByteOrder, Catalog, CatalogError, Decoded, Kind, Latest, MessageDef, Signal, SignalId};

const fn sig(name: &'static str, start_bit: u16, len: u8, order: ByteOrder, kind: Kind) -> Signal {
    Signal { name, start_bit, len, order, kind, factor: 1.0, offset: 0.0, unit: "" }
}

const LE: ByteOrder = ByteOrder::LittleEndian;
const BE: ByteOrder = ByteOrder::BigEndian;

static SIGS_A: [Signal; 3] = [
    sig("speed", 0, 16, LE, Kind::Unsigned),
    sig("temp", 16, 8, LE, Kind::Signed),
    sig("flag", 24, 1, LE, Kind::Unsigned),
];
static SIGS_B: [Signal; 2] = [sig("rpm", 7, 16, BE, Kind::Unsigned), sig("trim", 23, 12, BE, Kind::Signed)];
static SIGS_C: [Signal; 1] = [sig("ratio", 0, 32, LE, Kind::Float32)];

static MSGS: [MessageDef; 4] = [
    MessageDef { id: 0x100, extended: false, name: "A", dlc: 4, first_signal: 0, signals: &SIGS_A },
    MessageDef { id: 0x640, extended: false, name: "B", dlc: 8, first_signal: 3, signals: &SIGS_B },
    MessageDef { id: 0x7FF, extended: false, name: "C", dlc: 4, first_signal: 5, signals: &SIGS_C },
    MessageDef { id: 0x100, extended: true, name: "A_ext", dlc: 4, first_signal: 6, signals: &SIGS_A },
];
const CAT: Catalog = Catalog::new(&MSGS);

#[test]
fn fixture_is_valid() {
    assert_eq!(CAT.validate(), Ok(()));
    assert_eq!(CAT.signal_count(), 9);
}

#[test]
fn find_uses_id_and_extended_flag() {
    assert_eq!(CAT.find(0x100, false).unwrap().name, "A");
    assert_eq!(CAT.find(0x100, true).unwrap().name, "A_ext");
    assert_eq!(CAT.find(0x640, false).unwrap().name, "B");
    assert!(CAT.find(0x640, true).is_none());
    assert!(CAT.find(0x101, false).is_none());
    assert!(CAT.find(u32::MAX, true).is_none());
}

#[test]
fn decode_known_values_both_byte_orders() {
    // message A: speed=0x1234, temp=-5, flag=1
    let mut a = [0_u8; 4];
    insert(&mut a, 0, 16, LE, 0x1234).unwrap();
    insert(&mut a, 16, 8, LE, 0xFB).unwrap();
    insert(&mut a, 24, 1, LE, 1).unwrap();
    let mut got = Vec::new();
    let r = CAT.decode(0x100, false, &a, |id, s, raw| got.push((id.0, s.name, s.raw_to_i64(raw))));
    assert!(matches!(r, Decoded::Ok { signals: 3, .. }));
    assert_eq!(got, vec![(0, "speed", 0x1234), (1, "temp", -5), (2, "flag", 1)]);

    // message B (big endian): rpm=0xBEEF at start 7, trim=-100 (12 bit) at start 23
    let mut b = [0_u8; 8];
    insert(&mut b, 7, 16, BE, 0xBEEF).unwrap();
    insert(&mut b, 23, 12, BE, 0xF9C).unwrap(); // -100 in 12 bits
    assert_eq!(&b[..2], &[0xBE, 0xEF]);
    let mut got = Vec::new();
    CAT.decode(0x640, false, &b, |id, s, raw| got.push((id.0, s.name, s.raw_to_i64(raw))));
    assert_eq!(got, vec![(3, "rpm", 0xBEEF), (4, "trim", -100)]);
}

#[test]
fn float_signal_decodes_via_physical() {
    let bits = 1.5_f32.to_bits();
    let data = bits.to_le_bytes();
    let mut raw_seen = 0;
    CAT.decode(0x7FF, false, &data, |_, s, raw| {
        raw_seen = raw;
        assert!((s.physical(raw) - 1.5).abs() < 1e-12);
    });
    assert_eq!(raw_seen, u64::from(bits));
}

#[test]
fn physical_scaling() {
    let s = Signal { factor: 0.01, offset: -40.0, ..sig("t", 0, 16, LE, Kind::Unsigned) };
    assert!((s.physical(6500) - 25.0).abs() < 1e-9);
    let s = Signal { factor: 0.5, offset: 0.0, ..sig("t", 0, 8, LE, Kind::Signed) };
    assert!((s.physical(0xFE) - -1.0).abs() < 1e-9);
}

#[test]
fn unknown_and_short_frames_deliver_nothing() {
    let mut calls = 0;
    assert_eq!(CAT.decode(0x555, false, &[0; 8], |_, _, _| calls += 1), Decoded::UnknownId);
    assert_eq!(CAT.decode(0x100, false, &[0; 3], |_, _, _| calls += 1), Decoded::TooShort { need: 4, have: 3 });
    assert_eq!(CAT.decode(0x100, false, &[], |_, _, _| calls += 1), Decoded::TooShort { need: 4, have: 0 });
    assert_eq!(calls, 0);
}

#[test]
fn longer_frames_are_accepted() {
    let mut n = 0;
    let r = CAT.decode(0x100, false, &[0; 8], |_, _, _| n += 1);
    assert!(matches!(r, Decoded::Ok { signals: 3, .. }));
    assert_eq!(n, 3);
}

#[test]
fn latest_store_tracks_values_age_and_counts() {
    let mut latest: Latest<9> = Latest::new();
    assert_eq!(latest.get(SignalId(0)), None);
    let frame = [0x34, 0x12, 0xFB, 1];
    assert!(matches!(ingest(&CAT, &mut latest, 0x100, false, &frame, 1_000), Decoded::Ok { .. }));
    let s = latest.get(SignalId(0)).unwrap();
    assert_eq!((s.raw, s.stamp_us, s.count), (0x1234, 1_000, 1));
    ingest(&CAT, &mut latest, 0x100, false, &frame, 2_500);
    assert_eq!(latest.get(SignalId(0)).unwrap().count, 2);
    assert_eq!(latest.age_us(SignalId(0), 4_000), Some(1_500));
    assert_eq!(latest.age_us(SignalId(0), 100), Some(0)); // clock behind: no underflow
    assert_eq!(latest.age_us(SignalId(8), 100), None);
    latest.clear();
    assert_eq!(latest.get(SignalId(0)), None);
}

#[test]
fn latest_store_rejects_out_of_range_ids_without_panicking() {
    let mut latest: Latest<2> = Latest::new();
    assert!(latest.update(SignalId(1), 5, 0));
    assert!(!latest.update(SignalId(2), 5, 0));
    assert!(!latest.update(SignalId(u16::MAX), 5, 0));
    assert_eq!(latest.rejected(), 2);
    assert_eq!(latest.get(SignalId(2)), None);
    // A catalog larger than the store: extra signals are counted, not stored.
    ingest(&CAT, &mut latest, 0x640, false, &[0; 8], 7);
    assert_eq!(latest.rejected(), 4);
}

#[test]
fn update_counter_saturates() {
    let mut latest: Latest<1> = Latest::new();
    for _ in 0..3 {
        latest.update(SignalId(0), 1, 0);
    }
    assert_eq!(latest.get(SignalId(0)).unwrap().count, 3);
}

// ---- validate() negative cases ---------------------------------------------

macro_rules! leak {
    ($e:expr) => {
        Box::leak(Box::new($e))
    };
}

#[test]
fn validate_detects_each_class_of_error() {
    let sigs: &'static [Signal] = leak!([sig("x", 0, 8, LE, Kind::Unsigned)]);
    let mk = |id, ext, dlc, first, signals: &'static [Signal]| MessageDef {
        id,
        extended: ext,
        name: "m",
        dlc,
        first_signal: first,
        signals,
    };
    let check = |msgs: Vec<MessageDef>| -> Result<(), CatalogError> {
        let s: &'static [MessageDef] = Box::leak(msgs.into_boxed_slice());
        Catalog::new(s).validate()
    };
    assert_eq!(check(vec![]), Ok(()));
    // unsorted and duplicate ids
    assert_eq!(
        check(vec![mk(2, false, 1, 0, sigs), mk(1, false, 1, 1, sigs)]),
        Err(CatalogError::NotSorted { message: 1 })
    );
    assert_eq!(
        check(vec![mk(1, false, 1, 0, sigs), mk(1, false, 1, 1, sigs)]),
        Err(CatalogError::NotSorted { message: 1 })
    );
    // standard before extended is the required order
    assert_eq!(
        check(vec![mk(1, true, 1, 0, sigs), mk(1, false, 1, 1, sigs)]),
        Err(CatalogError::NotSorted { message: 1 })
    );
    assert_eq!(check(vec![mk(0x800, false, 1, 0, sigs)]), Err(CatalogError::IdOutOfRange { message: 0 }));
    assert_eq!(check(vec![mk(0x2000_0000, true, 1, 0, sigs)]), Err(CatalogError::IdOutOfRange { message: 0 }));
    assert_eq!(check(vec![mk(1, false, 9, 0, sigs)]), Err(CatalogError::DlcTooLarge { message: 0 }));
    assert_eq!(check(vec![mk(1, false, 1, 5, sigs)]), Err(CatalogError::BadSignalNumbering { message: 0 }));
    // signal outside the frame
    assert_eq!(check(vec![mk(1, false, 0, 0, sigs)]), Err(CatalogError::SignalOutsideFrame { message: 0, signal: 0 }));
    let wide: &'static [Signal] = leak!([sig("w", 4, 16, LE, Kind::Unsigned)]);
    assert_eq!(check(vec![mk(1, false, 2, 0, wide)]), Err(CatalogError::SignalOutsideFrame { message: 0, signal: 0 }));
    // bad widths
    let zero: &'static [Signal] = leak!([sig("z", 0, 0, LE, Kind::Unsigned)]);
    assert_eq!(check(vec![mk(1, false, 8, 0, zero)]), Err(CatalogError::BadWidth { message: 0, signal: 0 }));
    let big: &'static [Signal] = leak!([sig("b", 0, 65, LE, Kind::Unsigned)]);
    assert_eq!(check(vec![mk(1, false, 8, 0, big)]), Err(CatalogError::BadWidth { message: 0, signal: 0 }));
    let f16: &'static [Signal] = leak!([sig("f", 0, 16, LE, Kind::Float32)]);
    assert_eq!(check(vec![mk(1, false, 8, 0, f16)]), Err(CatalogError::BadWidth { message: 0, signal: 0 }));
}

#[test]
fn big_endian_signal_overrunning_the_frame_is_caught() {
    // MSB at bit 0 of byte 0 walks bits 15..8 of byte 1, then would reach bit 23 (byte 2).
    let s: &'static [Signal] = leak!([sig("be", 0, 10, BE, Kind::Unsigned)]);
    let m: &'static [MessageDef] =
        leak!([MessageDef { id: 1, extended: false, name: "m", dlc: 2, first_signal: 0, signals: s }]);
    assert_eq!(Catalog::new(m).validate(), Err(CatalogError::SignalOutsideFrame { message: 0, signal: 0 }));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10_000))]

    /// Any frame, any id: decoding never panics and is all-or-nothing.
    #[test]
    fn decode_is_total_and_all_or_nothing(
        id in prop::sample::select(vec![0x100_u32, 0x640, 0x7FF, 0x555, 0, u32::MAX]),
        ext in any::<bool>(),
        data in prop::collection::vec(any::<u8>(), 0..=12),
    ) {
        let mut n = 0_u16;
        let r = CAT.decode(id, ext, &data, |_, _, _| n += 1);
        match r {
            Decoded::Ok { message, signals } => {
                prop_assert_eq!(signals, n);
                prop_assert_eq!(usize::from(signals), message.signals.len());
                prop_assert!(data.len() >= usize::from(message.dlc));
            }
            Decoded::TooShort { .. } | Decoded::UnknownId => prop_assert_eq!(n, 0),
        }
    }

    /// Values written with `insert` come back unchanged through the catalog.
    #[test]
    fn encode_decode_roundtrip_message_a(speed in any::<u16>(), temp in any::<i8>(), flag in 0_u64..2) {
        let mut a = [0_u8; 4];
        insert(&mut a, 0, 16, LE, u64::from(speed)).unwrap();
        insert(&mut a, 16, 8, LE, u64::from(temp as u8)).unwrap();
        insert(&mut a, 24, 1, LE, flag).unwrap();
        let mut got = Vec::new();
        CAT.decode(0x100, false, &a, |_, s, raw| got.push(s.raw_to_i64(raw)));
        prop_assert_eq!(got, vec![i64::from(speed), i64::from(temp), flag as i64]);
    }
}
