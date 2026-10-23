//! SBG Systems Ellipse CAN decoding.
//!
//! [`tables`] is generated from SBG's `sbgCan.dbc` (MIT licensed, see
//! `dbc/LICENSE-sbgECom.md`) by `tools/dbc2table`; do not edit it by hand.
//! [`SbgKeys`] resolves the signals the logger records to catalog-wide
//! [`SignalId`]s once at start-up, so per-frame work is a table lookup.
//!
//! The Ellipse sends **no** CAN messages until they are enabled with SBG's
//! serial or Ethernet tools; CAN cannot configure the device.

#![no_std]

/// Generated message table. Do not edit by hand.
#[rustfmt::skip]
pub mod tables;

use tel_can::{Catalog, SignalId};

pub use tables::{CATALOG, SIGNAL_COUNT};

/// Newest-value store sized for the whole SBG catalog.
pub type Latest = tel_can::Latest<SIGNAL_COUNT>;

/// Look up a signal by message and signal name. Linear search: use at
/// start-up, not per frame.
pub fn signal_id(catalog: &Catalog, message: &str, signal: &str) -> Option<SignalId> {
    for m in catalog.messages() {
        if m.name == message {
            for (i, s) in m.signals.iter().enumerate() {
                if s.name == signal {
                    return u16::try_from(i).ok().and_then(|i| m.first_signal.checked_add(i)).map(SignalId);
                }
            }
        }
    }
    None
}

/// Signal ids of the quantities the logger records from the Ellipse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SbgKeys {
    /// `EKF_EULER.ROLL`, radians x 1e-4.
    pub roll: SignalId,
    /// `EKF_EULER.PITCH`.
    pub pitch: SignalId,
    /// `EKF_EULER.YAW`.
    pub yaw: SignalId,
    /// `EKF_VEL_NED.VELOCITY_N`, m/s x 0.01.
    pub vel_n: SignalId,
    /// `EKF_VEL_NED.VELOCITY_E`.
    pub vel_e: SignalId,
    /// `EKF_VEL_NED.VELOCITY_D`.
    pub vel_d: SignalId,
    /// `EKF_POS.LATITUDE`, degrees x 1e-7.
    pub latitude: SignalId,
    /// `EKF_POS.LONGITUDE`.
    pub longitude: SignalId,
    /// `EKF_ALTITUDE.ALTITUDE`, metres x 0.001.
    pub altitude: SignalId,
    /// `IMU_ACCEL.ACCEL_X`, m/s^2 x 0.01.
    pub accel_x: SignalId,
    /// `IMU_ACCEL.ACCEL_Y`.
    pub accel_y: SignalId,
    /// `IMU_ACCEL.ACCEL_Z`.
    pub accel_z: SignalId,
    /// `IMU_GYRO.GYRO_X`, rad/s x 0.001.
    pub gyro_x: SignalId,
    /// `IMU_GYRO.GYRO_Y`.
    pub gyro_y: SignalId,
    /// `IMU_GYRO.GYRO_Z`.
    pub gyro_z: SignalId,
    /// `STATUS_03.SOLUTION`: EKF solution status bit field.
    pub solution_status: SignalId,
    /// `STATUS_01.GENERAL`: general status bit field.
    pub general_status: SignalId,
}

impl SbgKeys {
    /// Resolve every key signal, or return the name of the first one missing
    /// (which means the generated table and this list have drifted apart).
    pub fn resolve(catalog: &Catalog) -> Result<Self, &'static str> {
        let id = |msg: &'static str, sig: &'static str| signal_id(catalog, msg, sig).ok_or(sig);
        Ok(Self {
            roll: id("SBG_ECAN_MSG_EKF_EULER", "ROLL")?,
            pitch: id("SBG_ECAN_MSG_EKF_EULER", "PITCH")?,
            yaw: id("SBG_ECAN_MSG_EKF_EULER", "YAW")?,
            vel_n: id("SBG_ECAN_MSG_EKF_VEL_NED", "VELOCITY_N")?,
            vel_e: id("SBG_ECAN_MSG_EKF_VEL_NED", "VELOCITY_E")?,
            vel_d: id("SBG_ECAN_MSG_EKF_VEL_NED", "VELOCITY_D")?,
            latitude: id("SBG_ECAN_MSG_EKF_POS", "LATITUDE")?,
            longitude: id("SBG_ECAN_MSG_EKF_POS", "LONGITUDE")?,
            altitude: id("SBG_ECAN_MSG_EKF_ALTITUDE", "ALTITUDE")?,
            accel_x: id("SBG_ECAN_MSG_IMU_ACCEL", "ACCEL_X")?,
            accel_y: id("SBG_ECAN_MSG_IMU_ACCEL", "ACCEL_Y")?,
            accel_z: id("SBG_ECAN_MSG_IMU_ACCEL", "ACCEL_Z")?,
            gyro_x: id("SBG_ECAN_MSG_IMU_GYRO", "GYRO_X")?,
            gyro_y: id("SBG_ECAN_MSG_IMU_GYRO", "GYRO_Y")?,
            gyro_z: id("SBG_ECAN_MSG_IMU_GYRO", "GYRO_Z")?,
            solution_status: id("SBG_ECAN_MSG_STATUS_03", "SOLUTION")?,
            general_status: id("SBG_ECAN_MSG_STATUS_01", "GENERAL")?,
        })
    }

    /// All key signal ids in a fixed order (the order they are logged).
    pub fn all(&self) -> [SignalId; 17] {
        [
            self.roll,
            self.pitch,
            self.yaw,
            self.vel_n,
            self.vel_e,
            self.vel_d,
            self.latitude,
            self.longitude,
            self.altitude,
            self.accel_x,
            self.accel_y,
            self.accel_z,
            self.gyro_x,
            self.gyro_y,
            self.gyro_z,
            self.solution_status,
            self.general_status,
        ]
    }
}
