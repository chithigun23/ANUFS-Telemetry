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
//! Verify the generated table against the vendored DBC using an independent,
//! deliberately simple line parser (not the parser the generator uses), and
//! check real decode results.

use tel_can::{insert, ByteOrder, Decoded, Kind};
use tel_can_sbg::{signal_id, tables, SbgKeys, CATALOG, SIGNAL_COUNT};

const DBC: &str = include_str!("../dbc/sbgCan.dbc");
const SKIP: &str = include_str!("../dbc/skip.txt");

fn options() -> dbc2table::Options {
    dbc2table::Options::from_skip_file(SKIP)
}

struct MiniSig {
    name: String,
    start: u16,
    len: u8,
    little: bool,
    signed: bool,
    factor: f64,
    offset: f64,
    unit: String,
}
struct MiniMsg {
    id: u32,
    name: String,
    dlc: u8,
    sigs: Vec<MiniSig>,
}

fn mini_parse(text: &str) -> Vec<MiniMsg> {
    let mut msgs: Vec<MiniMsg> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        if let Some(rest) = line.strip_prefix("BO_ ") {
            // BO_ <id> <name>: <dlc> <tx>
            let mut it = rest.split_whitespace();
            let id: u32 = it.next().unwrap().parse().unwrap();
            let name = it.next().unwrap().trim_end_matches(':').to_string();
            let dlc: u8 = it.next().unwrap().parse().unwrap();
            msgs.push(MiniMsg { id, name, dlc, sigs: Vec::new() });
        } else if let Some(rest) = line.trim_start().strip_prefix("SG_ ") {
            // SG_ name : start|len@order+ (factor,offset) [min|max] "unit" rx
            let (name, rest) = rest.split_once(" : ").unwrap();
            let mut it = rest.split_whitespace();
            let layout = it.next().unwrap(); // 48|8@1+
            let (pos, ord) = layout.split_once('@').unwrap();
            let (start, len) = pos.split_once('|').unwrap();
            let little = ord.starts_with('1');
            let signed = ord.ends_with('-');
            let scale = it.next().unwrap().trim_matches(|c| c == '(' || c == ')');
            let (factor, offset) = scale.split_once(',').unwrap();
            let unit = rest.split('"').nth(1).unwrap().to_string();
            msgs.last_mut().unwrap().sigs.push(MiniSig {
                name: name.trim().to_string(),
                start: start.parse().unwrap(),
                len: len.parse().unwrap(),
                little,
                signed,
                factor: factor.parse().unwrap(),
                offset: offset.parse().unwrap(),
                unit,
            });
        }
    }
    msgs
}

#[test]
fn table_is_valid_and_has_expected_size() {
    assert_eq!(CATALOG.validate(), Ok(()));
    // 77 messages and 212 signals in the DBC, minus the two skipped messages
    // (one signal each).
    assert_eq!(CATALOG.messages().len(), 75);
    assert_eq!(SIGNAL_COUNT, 210);
    assert_eq!(CATALOG.signal_count(), SIGNAL_COUNT);
}

#[test]
fn table_matches_dbc_field_for_field() {
    let skip = options().skip;
    assert_eq!(skip.len(), 2);
    let all = mini_parse(DBC);
    assert_eq!(all.len(), 77);
    let mut mini: Vec<MiniMsg> = all.into_iter().filter(|m| !skip.contains(&m.name)).collect();
    mini.sort_by_key(|m| m.id);
    assert_eq!(mini.len(), tables::MESSAGES.len());
    let mut total = 0;
    for (m, t) in mini.iter().zip(tables::MESSAGES.iter()) {
        assert_eq!(m.id, t.id, "{}", m.name);
        assert!(!t.extended);
        assert_eq!(m.name, t.name);
        assert_eq!(m.dlc, t.dlc, "{}", m.name);
        assert_eq!(m.sigs.len(), t.signals.len(), "{}", m.name);
        for (a, b) in m.sigs.iter().zip(t.signals.iter()) {
            let ctx = format!("{}.{}", m.name, a.name);
            assert_eq!(a.name, b.name, "{ctx}");
            assert_eq!(a.start, b.start_bit, "{ctx}");
            assert_eq!(a.len, b.len, "{ctx}");
            assert_eq!(a.little, matches!(b.order, ByteOrder::LittleEndian), "{ctx}");
            assert_eq!(a.signed, b.kind == Kind::Signed, "{ctx}");
            assert_eq!(a.factor, b.factor, "{ctx}");
            assert_eq!(a.offset, b.offset, "{ctx}");
            assert_eq!(a.unit, b.unit, "{ctx}");
            total += 1;
        }
    }
    assert_eq!(total, 210);
    for name in &skip {
        assert!(CATALOG.messages().iter().all(|m| m.name != name), "{name} must be absent");
    }
}

#[test]
fn checked_in_table_is_not_stale() {
    let generated = dbc2table::generate(DBC, "sbgCan.dbc", &options()).unwrap();
    let on_disk = include_str!("../src/tables.rs");
    assert_eq!(on_disk.replace("\r\n", "\n"), generated.replace("\r\n", "\n"));
}

#[test]
fn key_signals_resolve() {
    let keys = SbgKeys::resolve(&CATALOG).unwrap();
    let all = keys.all();
    let mut ids: Vec<u16> = all.iter().map(|s| s.0).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 17, "key signal ids must be distinct");
    assert!(all.iter().all(|s| usize::from(s.0) < SIGNAL_COUNT));
}

#[test]
fn unknown_signal_names_do_not_resolve() {
    assert_eq!(signal_id(&CATALOG, "SBG_ECAN_MSG_EKF_EULER", "NOPE"), None);
    assert_eq!(signal_id(&CATALOG, "NOPE", "ROLL"), None);
}

#[test]
fn ekf_euler_decodes_to_physical_values() {
    // SBG_ECAN_MSG_EKF_EULER (0x132 = 306): ROLL 0|16 s, PITCH 16|16 s, YAW 32|16 s, x1e-4 rad.
    let keys = SbgKeys::resolve(&CATALOG).unwrap();
    let to_raw = |rad: f64| -> u64 { u64::from((rad / 1e-4).round() as i16 as u16) };
    let mut frame = [0_u8; 6];
    insert(&mut frame, 0, 16, ByteOrder::LittleEndian, to_raw(0.1234)).unwrap();
    insert(&mut frame, 16, 16, ByteOrder::LittleEndian, to_raw(-0.5)).unwrap();
    insert(&mut frame, 32, 16, ByteOrder::LittleEndian, to_raw(3.0)).unwrap();
    let mut seen = Vec::new();
    let r = CATALOG.decode(306, false, &frame, |id, sig, raw| seen.push((id, sig.physical(raw))));
    assert!(matches!(r, Decoded::Ok { signals: 3, .. }));
    let get = |id| seen.iter().find(|(i, _)| *i == id).unwrap().1;
    assert!((get(keys.roll) - 0.1234).abs() < 1e-9);
    assert!((get(keys.pitch) - -0.5).abs() < 1e-9);
    assert!((get(keys.yaw) - 3.0).abs() < 1e-9);
}

#[test]
fn ekf_position_decodes_latitude_longitude() {
    // SBG_ECAN_MSG_EKF_POS (0x134 = 308): LATITUDE 0|32 signed, LONGITUDE 32|32 signed, x1e-7 deg.
    let keys = SbgKeys::resolve(&CATALOG).unwrap();
    // Canberra: -35.2809, 149.1300
    let lat_raw = (-35.2809_f64 / 1e-7).round() as i32;
    let lon_raw = (149.1300_f64 / 1e-7).round() as i32;
    let mut frame = [0_u8; 8];
    insert(&mut frame, 0, 32, ByteOrder::LittleEndian, u64::from(lat_raw as u32)).unwrap();
    insert(&mut frame, 32, 32, ByteOrder::LittleEndian, u64::from(lon_raw as u32)).unwrap();
    let mut lat = None;
    let mut lon = None;
    CATALOG.decode(308, false, &frame, |id, sig, raw| {
        if id == keys.latitude {
            lat = Some(sig.physical(raw));
        }
        if id == keys.longitude {
            lon = Some(sig.physical(raw));
        }
    });
    assert!((lat.unwrap() - -35.2809).abs() < 1e-7);
    assert!((lon.unwrap() - 149.1300).abs() < 1e-7);
}

#[test]
fn every_message_decodes_an_all_ones_and_all_zero_frame_without_error() {
    for m in CATALOG.messages() {
        for fill in [0x00_u8, 0xFF, 0xA5] {
            let frame = [fill; 8];
            let mut n = 0;
            let r = CATALOG.decode(m.id, m.extended, &frame, |_, _, _| n += 1);
            assert!(matches!(r, Decoded::Ok { .. }), "{}", m.name);
            assert_eq!(n, m.signals.len(), "{}", m.name);
            // too-short frames are refused for every message that has a payload
            if m.dlc > 0 {
                let short = &frame[..usize::from(m.dlc) - 1];
                assert!(matches!(CATALOG.decode(m.id, m.extended, short, |_, _, _| {}), Decoded::TooShort { .. }));
            }
        }
    }
}

#[test]
fn signed_extremes_decode_correctly_for_every_signed_signal() {
    for m in CATALOG.messages() {
        for s in m.signals {
            if s.kind != Kind::Signed {
                continue;
            }
            let mut frame = [0_u8; 8];
            let min_raw = 1_u64 << (s.len - 1); // sign bit only = most negative value
            insert(&mut frame, s.start_bit, s.len, s.order, min_raw).unwrap();
            let mut got = None;
            CATALOG.decode(m.id, m.extended, &frame, |_, sig, raw| {
                if std::ptr::eq(sig, s) {
                    got = Some(sig.raw_to_i64(raw));
                }
            });
            let expect = -(1_i128 << (s.len - 1));
            assert_eq!(i128::from(got.unwrap()), expect, "{}.{}", m.name, s.name);
        }
    }
}
