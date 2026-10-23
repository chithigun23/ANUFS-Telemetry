//! Driver for the ST ASM330LHHX automotive 6-axis IMU over SPI.
//!
//! Register addresses, bit positions, full-scale encodings, sensitivities and
//! the SPI protocol follow datasheet DS13197 Rev 9:
//!
//! * SPI mode 3 (the HAL must be configured for it), up to 10 MHz. The first
//!   byte is `0x80 | address` for a read and `address` for a write; the
//!   address auto-increments when `CTRL3_C.IF_INC` is set.
//! * `WHO_AM_I` (0x0F) reads 0x6B.
//! * Accelerometer full scale `FS[1:0]`: 00 = 2 g, 01 = 16 g, 10 = 4 g, 11 = 8 g.
//! * Gyroscope full scale `FS[1:0]`: 00 = 250, 01 = 500, 10 = 1000,
//!   11 = 2000 dps, with `FS_125` and `FS_4000` bits for the extremes.
//! * Temperature: 256 LSB per degree C, 0 LSB at 25 degrees C.
//!
//! Every configuration write is read back and compared, so a bus or device
//! fault during start-up is reported rather than silently producing bad data.

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

use embedded_hal::delay::DelayNs;
use embedded_hal::spi::{Operation, SpiDevice};

/// Register addresses.
pub mod reg {
    /// Interrupt 1 routing.
    pub const INT1_CTRL: u8 = 0x0D;
    /// Device identification, reads [`super::WHO_AM_I_VALUE`].
    pub const WHO_AM_I: u8 = 0x0F;
    /// Accelerometer control.
    pub const CTRL1_XL: u8 = 0x10;
    /// Gyroscope control.
    pub const CTRL2_G: u8 = 0x11;
    /// Control 3: BDU, IF_INC, SW_RESET.
    pub const CTRL3_C: u8 = 0x12;
    /// Control 4: I2C_disable.
    pub const CTRL4_C: u8 = 0x13;
    /// Data-ready status.
    pub const STATUS_REG: u8 = 0x1E;
    /// First output register (temperature low byte).
    pub const OUT_TEMP_L: u8 = 0x20;
}

/// Expected `WHO_AM_I` value.
pub const WHO_AM_I_VALUE: u8 = 0x6B;

const CTRL3_SW_RESET: u8 = 1 << 0;
const CTRL3_IF_INC: u8 = 1 << 2;
const CTRL3_BDU: u8 = 1 << 6;
const CTRL4_I2C_DISABLE: u8 = 1 << 2;
const INT1_DRDY_XL: u8 = 1 << 0;
const INT1_DRDY_G: u8 = 1 << 1;

/// Time the device needs after power is applied before it answers reliably.
pub const TURN_ON_MS: u32 = 35;

/// Burst length covering temperature, gyro and accelerometer output.
const BURST_LEN: usize = 14;

/// Output data rate. The code is the value written to `ODR_XL`/`ODR_G`
/// (high-performance mode, the power-on default of `CTRL6_C.XL_HM_MODE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Odr {
    /// 12.5 Hz.
    Hz12_5 = 1,
    /// 26 Hz.
    Hz26 = 2,
    /// 52 Hz.
    Hz52 = 3,
    /// 104 Hz.
    Hz104 = 4,
    /// 208 Hz.
    Hz208 = 5,
    /// 416 Hz.
    Hz416 = 6,
    /// 833 Hz.
    Hz833 = 7,
    /// 1667 Hz.
    Hz1667 = 8,
    /// 3333 Hz.
    Hz3333 = 9,
    /// 6667 Hz.
    Hz6667 = 10,
}

impl Odr {
    /// The 4-bit register code.
    pub const fn code(self) -> u8 {
        self as u8
    }
}

/// Accelerometer full-scale range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccelRange {
    /// +/- 2 g.
    G2,
    /// +/- 4 g.
    G4,
    /// +/- 8 g.
    G8,
    /// +/- 16 g.
    G16,
}

impl AccelRange {
    /// `FS[1:0]_XL` bits.
    pub const fn bits(self) -> u8 {
        match self {
            Self::G2 => 0b00,
            Self::G16 => 0b01,
            Self::G4 => 0b10,
            Self::G8 => 0b11,
        }
    }

    /// Sensitivity in micro-g per LSB (datasheet: 0.061, 0.122, 0.244, 0.488 mg/LSB).
    pub const fn micro_g_per_lsb(self) -> i64 {
        match self {
            Self::G2 => 61,
            Self::G4 => 122,
            Self::G8 => 244,
            Self::G16 => 488,
        }
    }
}

/// Gyroscope full-scale range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GyroRange {
    /// +/- 125 dps.
    Dps125,
    /// +/- 250 dps.
    Dps250,
    /// +/- 500 dps.
    Dps500,
    /// +/- 1000 dps.
    Dps1000,
    /// +/- 2000 dps.
    Dps2000,
    /// +/- 4000 dps.
    Dps4000,
}

impl GyroRange {
    /// The `CTRL2_G` low nibble: `FS[1:0]_G`, `FS_125`, `FS_4000`.
    pub const fn bits(self) -> u8 {
        match self {
            Self::Dps250 => 0b00 << 2,
            Self::Dps500 => 0b01 << 2,
            Self::Dps1000 => 0b10 << 2,
            Self::Dps2000 => 0b11 << 2,
            Self::Dps125 => 1 << 1,
            Self::Dps4000 => 1 << 0,
        }
    }

    /// Sensitivity in micro-degrees per second per LSB (datasheet: 4.375,
    /// 8.75, 17.5, 35, 70, 140 mdps/LSB).
    pub const fn micro_dps_per_lsb(self) -> i64 {
        match self {
            Self::Dps125 => 4_375,
            Self::Dps250 => 8_750,
            Self::Dps500 => 17_500,
            Self::Dps1000 => 35_000,
            Self::Dps2000 => 70_000,
            Self::Dps4000 => 140_000,
        }
    }
}

/// Sensor configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Accelerometer range.
    pub accel_range: AccelRange,
    /// Accelerometer output data rate.
    pub accel_odr: Odr,
    /// Gyroscope range.
    pub gyro_range: GyroRange,
    /// Gyroscope output data rate.
    pub gyro_odr: Odr,
    /// Route the data-ready signals to INT1.
    pub drdy_on_int1: bool,
}

impl Default for Config {
    /// +/-16 g, +/-2000 dps, 416 Hz, data-ready on INT1: suits a car chassis.
    fn default() -> Self {
        Self {
            accel_range: AccelRange::G16,
            accel_odr: Odr::Hz416,
            gyro_range: GyroRange::Dps2000,
            gyro_odr: Odr::Hz416,
            drdy_on_int1: true,
        }
    }
}

impl Config {
    /// `CTRL1_XL` value for this configuration (LPF2 disabled).
    pub const fn ctrl1_xl(&self) -> u8 {
        (self.accel_odr.code() << 4) | (self.accel_range.bits() << 2)
    }

    /// `CTRL2_G` value for this configuration.
    pub const fn ctrl2_g(&self) -> u8 {
        (self.gyro_odr.code() << 4) | self.gyro_range.bits()
    }

    /// `INT1_CTRL` value for this configuration.
    pub const fn int1_ctrl(&self) -> u8 {
        if self.drdy_on_int1 {
            INT1_DRDY_XL | INT1_DRDY_G
        } else {
            0
        }
    }
}

/// Driver error. `E` is the SPI error type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error<E> {
    /// SPI transfer failed.
    Spi(E),
    /// `WHO_AM_I` did not read 0x6B.
    WrongDevice(u8),
    /// A configuration register did not read back what was written.
    ConfigMismatch {
        /// Register address.
        reg: u8,
        /// Value written.
        expected: u8,
        /// Value read back.
        got: u8,
    },
    /// `SW_RESET` did not clear in time.
    ResetTimeout,
}

/// Raw output counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Raw {
    /// Temperature counts (256 per degree C, 0 at 25 degrees C).
    pub temp: i16,
    /// Gyroscope X, Y, Z counts.
    pub gyro: [i16; 3],
    /// Accelerometer X, Y, Z counts.
    pub accel: [i16; 3],
}

impl Raw {
    /// Parse the 14 bytes starting at `OUT_TEMP_L`: temperature, gyro XYZ,
    /// accelerometer XYZ, each little-endian two's complement.
    pub fn from_burst(b: &[u8; BURST_LEN]) -> Self {
        let word = |i: usize| -> i16 {
            let lo = b.get(i).copied().unwrap_or(0);
            let hi = b.get(i.saturating_add(1)).copied().unwrap_or(0);
            i16::from_le_bytes([lo, hi])
        };
        Self { temp: word(0), gyro: [word(2), word(4), word(6)], accel: [word(8), word(10), word(12)] }
    }
}

/// Raw counts converted to integer engineering units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scaled {
    /// Acceleration in micro-g.
    pub accel_micro_g: [i32; 3],
    /// Angular rate in milli-degrees per second.
    pub gyro_mdps: [i32; 3],
    /// Temperature in hundredths of a degree C.
    pub temp_centi_c: i32,
}

fn to_i32(v: i64) -> i32 {
    i32::try_from(v).unwrap_or(if v < 0 { i32::MIN } else { i32::MAX })
}

impl Raw {
    /// Convert with the sensitivities of the configured ranges. The results
    /// always fit in an `i32` for every range, so no value ever saturates.
    pub fn scale(&self, accel: AccelRange, gyro: GyroRange) -> Scaled {
        let a = |c: i16| to_i32(i64::from(c).saturating_mul(accel.micro_g_per_lsb()));
        let g = |c: i16| to_i32(i64::from(c).saturating_mul(gyro.micro_dps_per_lsb()) / 1000);
        let t = (i64::from(self.temp).saturating_mul(100) / 256).saturating_add(2500);
        Scaled {
            accel_micro_g: [a(self.accel[0]), a(self.accel[1]), a(self.accel[2])],
            gyro_mdps: [g(self.gyro[0]), g(self.gyro[1]), g(self.gyro[2])],
            temp_centi_c: to_i32(t),
        }
    }
}

/// `STATUS_REG` flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// New accelerometer data.
    pub accel_ready: bool,
    /// New gyroscope data.
    pub gyro_ready: bool,
    /// New temperature data.
    pub temp_ready: bool,
}

/// The driver.
pub struct Asm330<SPI> {
    spi: SPI,
    config: Config,
}

impl<SPI: SpiDevice> Asm330<SPI> {
    /// Wrap an SPI device (mode 3). Call [`Asm330::init`] before reading.
    pub fn new(spi: SPI) -> Self {
        Self { spi, config: Config::default() }
    }

    /// Release the SPI device.
    pub fn release(self) -> SPI {
        self.spi
    }

    /// The configuration applied by the last successful [`Asm330::init`].
    pub fn config(&self) -> Config {
        self.config
    }

    fn read_into(&mut self, address: u8, buf: &mut [u8]) -> Result<(), Error<SPI::Error>> {
        let cmd = [0x80 | (address & 0x7F)];
        self.spi.transaction(&mut [Operation::Write(&cmd), Operation::Read(buf)]).map_err(Error::Spi)
    }

    fn read_reg(&mut self, address: u8) -> Result<u8, Error<SPI::Error>> {
        let mut b = [0_u8; 1];
        self.read_into(address, &mut b)?;
        Ok(b[0])
    }

    fn write_reg(&mut self, address: u8, value: u8) -> Result<(), Error<SPI::Error>> {
        let cmd = [address & 0x7F, value];
        self.spi.write(&cmd).map_err(Error::Spi)
    }

    /// Write a register and read it back; fail if it does not match.
    fn write_verified(&mut self, address: u8, value: u8) -> Result<(), Error<SPI::Error>> {
        self.write_reg(address, value)?;
        let got = self.read_reg(address)?;
        if got == value {
            Ok(())
        } else {
            Err(Error::ConfigMismatch { reg: address, expected: value, got })
        }
    }

    /// Read `WHO_AM_I`.
    pub fn who_am_i(&mut self) -> Result<u8, Error<SPI::Error>> {
        self.read_reg(reg::WHO_AM_I)
    }

    /// Reset and configure the device.
    ///
    /// The caller must have waited [`TURN_ON_MS`] after power-up. Sequence:
    /// check `WHO_AM_I`, software reset (polled for completion), block data
    /// update on, I2C off, then ranges and rates, each verified by readback.
    pub fn init<D: DelayNs>(&mut self, config: Config, delay: &mut D) -> Result<(), Error<SPI::Error>> {
        let id = self.who_am_i()?;
        if id != WHO_AM_I_VALUE {
            return Err(Error::WrongDevice(id));
        }
        self.write_reg(reg::CTRL3_C, CTRL3_SW_RESET)?;
        let mut cleared = false;
        for _ in 0..20 {
            delay.delay_ms(1);
            if self.read_reg(reg::CTRL3_C)? & CTRL3_SW_RESET == 0 {
                cleared = true;
                break;
            }
        }
        if !cleared {
            return Err(Error::ResetTimeout);
        }
        // Bring everything to a known state with outputs off, then enable.
        self.write_verified(reg::CTRL3_C, CTRL3_BDU | CTRL3_IF_INC)?;
        self.write_verified(reg::CTRL4_C, CTRL4_I2C_DISABLE)?;
        self.write_verified(reg::INT1_CTRL, config.int1_ctrl())?;
        self.write_verified(reg::CTRL2_G, config.ctrl2_g())?;
        self.write_verified(reg::CTRL1_XL, config.ctrl1_xl())?;
        self.config = config;
        Ok(())
    }

    /// Read the data-ready flags.
    pub fn status(&mut self) -> Result<Status, Error<SPI::Error>> {
        let s = self.read_reg(reg::STATUS_REG)?;
        Ok(Status { accel_ready: s & 0b001 != 0, gyro_ready: s & 0b010 != 0, temp_ready: s & 0b100 != 0 })
    }

    /// Read temperature, gyroscope and accelerometer in one burst. With
    /// block-data-update enabled the burst is internally consistent.
    pub fn read_raw(&mut self) -> Result<Raw, Error<SPI::Error>> {
        let mut b = [0_u8; BURST_LEN];
        self.read_into(reg::OUT_TEMP_L, &mut b)?;
        Ok(Raw::from_burst(&b))
    }

    /// Read and scale with the configured ranges.
    pub fn read_scaled(&mut self) -> Result<Scaled, Error<SPI::Error>> {
        let raw = self.read_raw()?;
        Ok(raw.scale(self.config.accel_range, self.config.gyro_range))
    }
}
