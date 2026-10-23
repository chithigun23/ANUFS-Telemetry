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
//! Checks on the shipped MoTeC table.

use tel_can::Decoded;
use tel_can_motec::{CATALOG, SIGNAL_COUNT};

const CSV: &str = include_str!("../config/motec.csv");

#[test]
fn shipped_catalog_is_valid() {
    assert_eq!(CATALOG.validate(), Ok(()));
    assert_eq!(CATALOG.signal_count(), SIGNAL_COUNT);
}

#[test]
fn checked_in_table_matches_the_csv() {
    let generated = dbc2table::generate_from_csv(CSV, "motec.csv").unwrap();
    let on_disk = include_str!("../src/tables.rs");
    assert_eq!(on_disk.replace("\r\n", "\n"), generated.replace("\r\n", "\n"), "rerun dbc2table for motec.csv");
}

#[test]
fn unconfigured_catalog_ignores_every_frame_safely() {
    // Whatever the table contains, arbitrary frames must never panic.
    for id in [0_u32, 0x118, 0x500, 0x640, 0x7FF] {
        for ext in [false, true] {
            let r = CATALOG.decode(id, ext, &[0xFF; 8], |_, _, _| {});
            assert!(matches!(r, Decoded::UnknownId | Decoded::Ok { .. } | Decoded::TooShort { .. }));
        }
    }
}
