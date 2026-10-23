//! Crash-safe append-only block log.
//!
//! See [`block`] for the on-card layout, [`records`] for the record schema,
//! [`writer`] for the sans-IO writer and [`recovery`] for start-up recovery.
//! The crate does no I/O itself: callers perform block reads and writes, which
//! makes it identical on the MCU (SDMMC or SPI), in tests and on the PC.

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

pub mod block;
pub mod crc;
pub mod records;
pub mod recovery;
pub mod writer;
