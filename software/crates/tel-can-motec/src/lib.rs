//! MoTeC M150 ECU, PDM15 and C125 dash CAN decoding.
//!
//! MoTeC message layouts are defined by each car's own configuration (M1 Build
//! for the ECU, PDM Manager for the PDM, the dash configuration for the C125),
//! so there is no universal table. [`tables`] is generated from
//! `config/motec.csv` by `tools/dbc2table`; fill that file in when the
//! configuration is available and regenerate (see the comments in the CSV).
//!
//! Until then the catalog is valid but empty, which decodes nothing. The
//! firmware logs every CAN1 frame raw in the meantime, so no data is lost and
//! the signals can be decoded on the PC later.

#![no_std]

/// Generated message table. Do not edit by hand.
#[rustfmt::skip]
pub mod tables;

pub use tables::{CATALOG, SIGNAL_COUNT};

/// Newest-value store sized for the whole MoTeC catalog.
pub type Latest = tel_can::Latest<SIGNAL_COUNT>;
