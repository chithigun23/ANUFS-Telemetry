//! Telemetry core logic, independent of any hardware.
//!
//! * [`time`]: counter extension and UTC conversion.
//! * [`clock`]: PPS-disciplined UTC clock.
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
        clippy::arithmetic_side_effects,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_possible_wrap
    )
)]

#[cfg(test)]
extern crate std;

pub mod clock;
pub mod fusion;
pub mod imu_window;
pub mod supervisor;
pub mod time;
