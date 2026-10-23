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

use dbc2table::{generate, model_from_dbc, sanitize, GenError, KindModel, Options};

const HEADER: &str = "VERSION \"\"\n\nNS_ :\n\nBS_:\n\nBU_:\n\n";

fn dbc(body: &str) -> String {
    format!("{HEADER}{body}")
}

const GOOD: &str = "\
BO_ 100 MsgA: 8 Vector__XXX
 SG_ speed : 0|16@1+ (0.1,0) [0|6553.5] \"km/h\" Vector__XXX
 SG_ temp : 16|8@1- (1,-40) [-128|127] \"C\" Vector__XXX

BO_ 2147484160 ExtMsg: 8 Vector__XXX
 SG_ be : 7|16@0+ (1,0) [0|65535] \"\" Vector__XXX
 SG_ f : 32|32@1+ (1,0) [0|0] \"\" Vector__XXX

BO_ 50 Early: 2 Vector__XXX
 SG_ b : 0|8@1+ (1,0) [0|255] \"\" Vector__XXX

SIG_VALTYPE_ 2147484160 f : 1;
";

#[test]
fn models_fields_correctly() {
    let m = model_from_dbc(&dbc(GOOD), &Options::default()).unwrap();
    // sorted by (extended, id): standard 50, standard 100, extended 0x200
    let ids: Vec<(bool, u32)> = m.iter().map(|m| (m.extended, m.id)).collect();
    assert_eq!(ids, vec![(false, 50), (false, 100), (true, 0x200)]);
    let a = &m[1];
    assert_eq!(a.name, "MsgA");
    assert_eq!(a.dlc, 8);
    assert_eq!(a.signals[0].kind, KindModel::Unsigned);
    assert_eq!(a.signals[0].factor, 0.1);
    assert_eq!(a.signals[0].unit, "km/h");
    assert_eq!(a.signals[1].kind, KindModel::Signed);
    assert_eq!(a.signals[1].offset, -40.0);
    let e = &m[2];
    assert!(e.signals[0].big_endian);
    assert_eq!(e.signals[0].start_bit, 7);
    assert_eq!(e.signals[1].kind, KindModel::Float32);
}

#[test]
fn rendered_source_has_expected_shape() {
    let src = generate(&dbc(GOOD), "good.dbc", &Options::default()).unwrap();
    assert!(src.starts_with("// @generated"));
    assert!(src.contains("pub static MESSAGES: [MessageDef; 3]"));
    assert!(src.contains("pub const SIGNAL_COUNT: usize = 5;"));
    assert!(src.contains("first_signal: 1,")); // MsgA follows Early's single signal
    assert!(src.contains("first_signal: 3,"));
    assert!(src.contains("Kind::Float32"));
    assert!(src.contains("ByteOrder::BigEndian"));
    assert!(src.contains("extended: true"));
    assert!(src.contains("id: 0x200"));
    assert!(src.contains("factor: 0.1"));
    assert!(src.contains("offset: -40.0"));
}

#[test]
fn rendering_is_deterministic() {
    let a = generate(&dbc(GOOD), "x", &Options::default()).unwrap();
    let b = generate(&dbc(GOOD), "x", &Options::default()).unwrap();
    assert_eq!(a, b);
}

#[test]
fn multiplexed_signals_are_refused() {
    let body = "BO_ 1 M: 8 Vector__XXX\n SG_ sel M : 0|8@1+ (1,0) [0|255] \"\" Vector__XXX\n SG_ x m0 : 8|8@1+ (1,0) [0|255] \"\" Vector__XXX\n";
    assert!(matches!(model_from_dbc(&dbc(body), &Options::default()), Err(GenError::Unsupported(_))));
}

#[test]
fn double_precision_floats_are_refused() {
    let body = "BO_ 1 M: 8 Vector__XXX\n SG_ d : 0|64@1+ (1,0) [0|0] \"\" Vector__XXX\n\nSIG_VALTYPE_ 1 d : 2;\n";
    assert!(matches!(model_from_dbc(&dbc(body), &Options::default()), Err(GenError::Unsupported(_))));
}

#[test]
fn float32_must_be_32_bits() {
    let body = "BO_ 1 M: 8 Vector__XXX\n SG_ d : 0|16@1+ (1,0) [0|0] \"\" Vector__XXX\n\nSIG_VALTYPE_ 1 d : 1;\n";
    assert!(matches!(model_from_dbc(&dbc(body), &Options::default()), Err(GenError::OutOfRange(_))));
}

#[test]
fn duplicate_ids_are_refused() {
    let body = "BO_ 7 A: 1 Vector__XXX\n SG_ a : 0|8@1+ (1,0) [0|0] \"\" Vector__XXX\n\nBO_ 7 B: 1 Vector__XXX\n SG_ b : 0|8@1+ (1,0) [0|0] \"\" Vector__XXX\n";
    assert_eq!(model_from_dbc(&dbc(body), &Options::default()), Err(GenError::DuplicateId(7, false)));
}

#[test]
fn same_id_standard_and_extended_is_fine() {
    let body = "BO_ 7 A: 1 Vector__XXX\n SG_ a : 0|8@1+ (1,0) [0|0] \"\" Vector__XXX\n\nBO_ 2147483655 B: 1 Vector__XXX\n SG_ b : 0|8@1+ (1,0) [0|0] \"\" Vector__XXX\n";
    assert_eq!(model_from_dbc(&dbc(body), &Options::default()).unwrap().len(), 2);
}

#[test]
fn signals_that_overrun_the_frame_are_refused_unless_skipped() {
    let body = "BO_ 4 Short: 4 Vector__XXX\n SG_ s : 32|32@1+ (1,0) [0|0] \"\" Vector__XXX\n\nBO_ 5 Ok: 1 Vector__XXX\n SG_ o : 0|8@1+ (1,0) [0|0] \"\" Vector__XXX\n";
    let err = model_from_dbc(&dbc(body), &Options::default()).unwrap_err();
    assert!(matches!(err, GenError::SignalOutsideFrame(ref s) if s.contains("Short.s")), "{err}");
    let skip = Options { skip: vec!["Short".into()] };
    let m = model_from_dbc(&dbc(body), &skip).unwrap();
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].name, "Ok");
}

#[test]
fn big_endian_overrun_is_detected() {
    // MSB at bit 0 with 10 bits needs byte 2 in a 2 byte frame.
    let body = "BO_ 4 Short: 2 Vector__XXX\n SG_ s : 0|10@0+ (1,0) [0|0] \"\" Vector__XXX\n";
    assert!(matches!(model_from_dbc(&dbc(body), &Options::default()), Err(GenError::SignalOutsideFrame(_))));
}

#[test]
fn unknown_skip_name_is_an_error() {
    let skip = Options { skip: vec!["DoesNotExist".into()] };
    assert_eq!(model_from_dbc(&dbc(GOOD), &skip), Err(GenError::UnknownSkip("DoesNotExist".into())));
}

#[test]
fn skip_file_parsing() {
    let o = Options::from_skip_file("# comment\n\nA\n  B  # trailing\n#C\n");
    assert_eq!(o.skip, vec!["A".to_string(), "B".to_string()]);
}

#[test]
fn skipped_names_are_recorded_in_the_output() {
    let skip = Options { skip: vec!["Early".into()] };
    let src = generate(&dbc(GOOD), "x", &skip).unwrap();
    assert!(src.contains("// Skipped messages: Early"));
    assert!(!src.contains("\"Early\""));
}

#[test]
fn out_of_range_values_are_refused() {
    let body = "BO_ 1 M: 9 Vector__XXX\n SG_ a : 0|8@1+ (1,0) [0|0] \"\" Vector__XXX\n";
    assert!(matches!(model_from_dbc(&dbc(body), &Options::default()), Err(GenError::OutOfRange(_))));
    let body = "BO_ 1 M: 8 Vector__XXX\n SG_ a : 64|8@1+ (1,0) [0|0] \"\" Vector__XXX\n";
    assert!(model_from_dbc(&dbc(body), &Options::default()).is_err());
    let body = "BO_ 1 M: 8 Vector__XXX\n SG_ a : 0|65@1+ (1,0) [0|0] \"\" Vector__XXX\n";
    assert!(model_from_dbc(&dbc(body), &Options::default()).is_err());
}

#[test]
fn garbage_is_a_parse_error() {
    assert!(matches!(model_from_dbc("this is not a dbc", &Options::default()), Err(GenError::Parse(_))));
}

#[test]
fn sanitize_only_touches_single_value_ranges() {
    let text = " SG_ a : 0|8@1+ (1,0) [100] \"s\" X\n SG_ b : 0|8@1+ (1,0) [0|255] \"\" X\nBO_ 1 M [5]: 1 X\n";
    let (out, n) = sanitize(text);
    assert_eq!(n, 1);
    assert!(out.contains("[100|100]"));
    assert!(out.contains("[0|255]"));
    assert!(out.contains("BO_ 1 M [5]: 1 X"), "non-SG_ lines untouched");
}
