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
//! CSV signal-list import, including an end-to-end decode through `tel-can`.

use dbc2table::{generate_from_csv, model_from_csv, GenError, KindModel, CSV_HEADER};
use tel_can::{insert, ByteOrder, Catalog, Decoded, Kind, MessageDef, Signal};

fn csv(rows: &str) -> String {
    format!("{CSV_HEADER}\n{rows}")
}

const GOOD: &str = "\
# a comment
0x640,0,ECU_A,8,rpm,7,16,be,u,1,0,rpm
0x640,0,ECU_A,8,tps,23,12,be,u,0.1,0,%
0x640,0,ECU_A,8,coolant,55,8,be,s,1,-40,C
0x118,0,PDM_A,6,vbat,0,16,le,u,0.01,0,V
0x118,0,PDM_A,6,flag,16,1,le,u,1,0,
0x18FF0001,1,EXT_A,4,ratio,0,32,le,f32,1,0,
";

#[test]
fn parses_and_groups_rows() {
    let m = model_from_csv(&csv(GOOD)).unwrap();
    // standard 0x118, standard 0x640, extended 0x18FF0001
    let keys: Vec<(bool, u32)> = m.iter().map(|m| (m.extended, m.id)).collect();
    assert_eq!(keys, vec![(false, 0x118), (false, 0x640), (true, 0x18FF_0001)]);
    assert_eq!(m[1].signals.len(), 3);
    assert_eq!(m[1].signals[1].factor, 0.1);
    assert_eq!(m[1].signals[2].offset, -40.0);
    assert_eq!(m[1].signals[2].kind, KindModel::Signed);
    assert!(m[1].signals[0].big_endian);
    assert_eq!(m[2].signals[0].kind, KindModel::Float32);
    assert_eq!(m[0].signals[1].unit, "");
}

#[test]
fn empty_and_header_only_inputs_give_an_empty_table() {
    assert!(model_from_csv("").unwrap().is_empty());
    assert!(model_from_csv("# only comments\n\n").unwrap().is_empty());
    assert!(model_from_csv(&csv("")).unwrap().is_empty());
    let src = generate_from_csv(&csv(""), "empty.csv").unwrap();
    assert!(src.contains("MessageDef; 0]"));
    assert!(src.contains("SIGNAL_COUNT: usize = 0"));
}

#[test]
fn header_is_required_and_checked() {
    assert!(matches!(model_from_csv("id,extended\n"), Err(GenError::Parse(_))));
    assert!(matches!(model_from_csv("0x1,0,M,8,s,0,8,le,u,1,0,\n"), Err(GenError::Parse(_))));
    // whitespace inside the header is tolerated
    assert!(model_from_csv(&CSV_HEADER.replace(',', ", ")).is_ok());
}

#[test]
fn bad_rows_are_rejected_with_the_line_number() {
    let cases = [
        "zz,0,M,8,s,0,8,le,u,1,0,",         // id
        "0x1,2,M,8,s,0,8,le,u,1,0,",        // extended
        "0x1,0,M,8,s,0,8,xx,u,1,0,",        // order
        "0x1,0,M,8,s,0,8,le,q,1,0,",        // kind
        "0x1,0,M,8,s,0,8,le,u,abc,0,",      // factor
        "0x1,0,M,8,s,0,8,le,u,1,0,u,extra", // too many fields
        "0x1,0,M,8,s,0,8,le,u,1",           // too few fields
        "0x1,0,,8,s,0,8,le,u,1,0,",         // empty message name
        "0x1,0,M,8,,0,8,le,u,1,0,",         // empty signal name
        "0x1,0,M,300,s,0,8,le,u,1,0,",      // dlc not a u8
    ];
    for row in cases {
        let err = model_from_csv(&csv(row)).unwrap_err();
        assert!(err.to_string().contains("line 2") || matches!(err, GenError::OutOfRange(_)), "{row}: {err}");
    }
}

#[test]
fn semantic_checks() {
    // id too large for 11 bits
    assert!(matches!(model_from_csv(&csv("0x800,0,M,8,s,0,8,le,u,1,0,")), Err(GenError::OutOfRange(_))));
    // dlc above 8
    assert!(matches!(model_from_csv(&csv("0x1,0,M,9,s,0,8,le,u,1,0,")), Err(GenError::OutOfRange(_))));
    // width 0 and 65
    assert!(matches!(model_from_csv(&csv("0x1,0,M,8,s,0,0,le,u,1,0,")), Err(GenError::OutOfRange(_))));
    assert!(matches!(model_from_csv(&csv("0x1,0,M,8,s,0,65,le,u,1,0,")), Err(GenError::OutOfRange(_))));
    // float of the wrong width
    assert!(matches!(model_from_csv(&csv("0x1,0,M,8,s,0,16,le,f32,1,0,")), Err(GenError::OutOfRange(_))));
    // signal runs past the declared length
    assert!(matches!(model_from_csv(&csv("0x1,0,M,2,s,8,16,le,u,1,0,")), Err(GenError::SignalOutsideFrame(_))));
    // non-finite scaling
    assert!(matches!(model_from_csv(&csv("0x1,0,M,8,s,0,8,le,u,inf,0,")), Err(GenError::OutOfRange(_))));
    assert!(matches!(model_from_csv(&csv("0x1,0,M,8,s,0,8,le,u,NaN,0,")), Err(GenError::OutOfRange(_))));
}

#[test]
fn inconsistent_repeats_are_rejected() {
    let renamed = "0x1,0,A,8,x,0,8,le,u,1,0,\n0x1,0,B,8,y,8,8,le,u,1,0,";
    assert!(matches!(model_from_csv(&csv(renamed)), Err(GenError::Parse(_))));
    let redlc = "0x1,0,A,8,x,0,8,le,u,1,0,\n0x1,0,A,4,y,8,8,le,u,1,0,";
    assert!(matches!(model_from_csv(&csv(redlc)), Err(GenError::Parse(_))));
    let dupsig = "0x1,0,A,8,x,0,8,le,u,1,0,\n0x1,0,A,8,x,8,8,le,u,1,0,";
    assert!(matches!(model_from_csv(&csv(dupsig)), Err(GenError::Parse(_))));
}

#[test]
fn same_id_standard_and_extended_are_different_messages() {
    let rows = "0x1,0,S,8,x,0,8,le,u,1,0,\n0x1,1,E,8,y,0,8,le,u,1,0,";
    assert_eq!(model_from_csv(&csv(rows)).unwrap().len(), 2);
}

/// Build a runtime catalog from the model (leaking to get 'static) and prove
/// the values written with `insert` come back through `Catalog::decode`.
#[test]
fn csv_to_decoded_values_end_to_end() {
    let models = model_from_csv(&csv(GOOD)).unwrap();
    let mut defs: Vec<MessageDef> = Vec::new();
    let mut first = 0_u16;
    for m in &models {
        let signals: Vec<Signal> = m
            .signals
            .iter()
            .map(|s| Signal {
                name: Box::leak(s.name.clone().into_boxed_str()),
                start_bit: s.start_bit,
                len: s.len,
                order: if s.big_endian { ByteOrder::BigEndian } else { ByteOrder::LittleEndian },
                kind: match s.kind {
                    KindModel::Unsigned => Kind::Unsigned,
                    KindModel::Signed => Kind::Signed,
                    KindModel::Float32 => Kind::Float32,
                },
                factor: s.factor,
                offset: s.offset,
                unit: Box::leak(s.unit.clone().into_boxed_str()),
            })
            .collect();
        let n = signals.len() as u16;
        defs.push(MessageDef {
            id: m.id,
            extended: m.extended,
            name: Box::leak(m.name.clone().into_boxed_str()),
            dlc: m.dlc,
            first_signal: first,
            signals: Box::leak(signals.into_boxed_slice()),
        });
        first += n;
    }
    let cat = Catalog::new(Box::leak(defs.into_boxed_slice()));
    assert_eq!(cat.validate(), Ok(()));

    // ECU_A: rpm=6500 (BE 16 @7), tps=250 -> 25.0% (BE 12 @23), coolant=85C -> raw 125 (BE s8 @55 offset -40)
    let mut f = [0_u8; 8];
    insert(&mut f, 7, 16, ByteOrder::BigEndian, 6500).unwrap();
    insert(&mut f, 23, 12, ByteOrder::BigEndian, 250).unwrap();
    insert(&mut f, 55, 8, ByteOrder::BigEndian, 125).unwrap();
    assert_eq!(&f[..2], &6500_u16.to_be_bytes());
    let mut got = Vec::new();
    let r = cat.decode(0x640, false, &f, |_, s, raw| got.push((s.name, s.physical(raw))));
    assert!(matches!(r, Decoded::Ok { signals: 3, .. }));
    assert_eq!(got[0].0, "rpm");
    assert!((got[0].1 - 6500.0).abs() < 1e-9);
    assert!((got[1].1 - 25.0).abs() < 1e-9);
    assert!((got[2].1 - 85.0).abs() < 1e-9);

    // extended float message
    let data = 2.5_f32.to_le_bytes();
    let mut v = None;
    cat.decode(0x18FF_0001, true, &data, |_, s, raw| v = Some(s.physical(raw)));
    assert!((v.unwrap() - 2.5).abs() < 1e-12);
    // same id in the wrong id space is unknown
    assert_eq!(cat.decode(0x18FF_0001, false, &data, |_, _, _| {}), Decoded::UnknownId);
}
