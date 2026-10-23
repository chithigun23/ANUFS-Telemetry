//! Table-driven CAN signal extraction.
//!
//! The crate separates *data* from *code*: message and signal layouts are
//! static tables (generated from a DBC file or written by hand), and a single
//! small, bounds-checked routine ([`bits::extract`]) turns frame bytes into raw
//! values. The firmware logs raw bits; physical scaling is applied by host
//! tools using the same tables.
//!
//! `no_std`, no allocation, no `unsafe`.

#![no_std]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )
)]

#[cfg(test)]
extern crate std;

pub mod bits;
pub mod catalog;
pub mod latest;

pub use bits::{extract, insert, sign_extend, ByteOrder};
pub use catalog::{Catalog, CatalogError, Decoded, Kind, MessageDef, Signal, SignalId};
pub use latest::{ingest, Latest, Sample};
