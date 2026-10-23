//! Bounded, panic-free NMEA 0183 framing and decoding plus the Quectel
//! LC29H(EA) PAIR/PQTM command set.
//!
//! * [`LineAssembler`] turns a byte stream into checksum-verified sentences.
//! * [`parse`] decodes RMC, GGA, `$PQTMTAR` and command responses into
//!   fixed-point values.
//! * [`Command`] encodes the configuration commands.
//! * [`config::Configurator`] verifies and applies the module configuration.
//!
//! The crate is `no_std`, forbids `unsafe`, performs no allocation and avoids
//! floating point.

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

pub mod cmd;
pub mod config;
pub mod framing;
pub mod msg;
mod num;

pub use cmd::{Command, EncodeError};
pub use framing::{checksum, FrameError, LineAssembler, Sentence, Stats, MAX_PAYLOAD};
pub use msg::{
    parse, Date, FixQuality, Gga, Message, ModeIndicator, ParseError, PqtmCommand, PqtmResult, Rmc, Tar, UtcTime,
};
