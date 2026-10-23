//! Driver for the TI TLC59108 8-channel constant-current LED driver (I2C), and
//! a two-chip bank that drives five bi-colour LEDs.
//!
//! Facts below come from TI datasheet SLDS156B (July 2015):
//!
//! * 7-bit slave address is `1 0 0 A3 A2 A1 A0` (0x40 to 0x4F).
//! * The control byte carries `AI[2:0]` in bits 7:5 and the register pointer in
//!   bits 4:0. `AI = 100` (0x80) auto-increments over all registers and rolls
//!   over to 0x00 after 0x11.
//! * Registers: MODE1 0x00, MODE2 0x01, PWM0..PWM7 0x02..0x09, GRPPWM 0x0A,
//!   GRPFREQ 0x0B, LEDOUT0 0x0C, LEDOUT1 0x0D, IREF 0x12, EFLAG 0x13.
//! * MODE1 powers up with the oscillator **off** (bit 4 set) and responding to
//!   the LED All Call address; this driver turns the oscillator on and disables
//!   All Call, Sub Call 1 to 3 so a stray broadcast cannot change the LEDs.
//! * `LDRx` in LEDOUT: 00 off, 01 fully on, 10 individual PWM, 11 group PWM.
//! * Software reset call: write A5h, 5Ah to address 0x4B (1001 011).
//!
//! The driver keeps a shadow of the writable registers and sends them in one
//! transaction, so all outputs change together on the STOP condition
//! (MODE2.OCH = 0). `verify` reads them back to detect a chip that has reset.
//!
//! Methods take the bus as an argument so several chips (and other devices)
//! can share one `I2c`.

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

pub mod leds;

use embedded_hal::delay::DelayNs;
use embedded_hal::i2c::I2c;

/// Register addresses.
pub mod reg {
    /// Mode 1.
    pub const MODE1: u8 = 0x00;
    /// Mode 2.
    pub const MODE2: u8 = 0x01;
    /// First brightness register.
    pub const PWM0: u8 = 0x02;
    /// Group duty cycle.
    pub const GRPPWM: u8 = 0x0A;
    /// Group frequency.
    pub const GRPFREQ: u8 = 0x0B;
    /// Output state, channels 3..0.
    pub const LEDOUT0: u8 = 0x0C;
    /// Output state, channels 7..4.
    pub const LEDOUT1: u8 = 0x0D;
    /// Output current gain.
    pub const IREF: u8 = 0x12;
    /// Error flags (read only, not covered by auto-increment).
    pub const EFLAG: u8 = 0x13;
}

/// Control byte: auto-increment over all registers (`AI = 100`).
const AI_ALL: u8 = 0x80;
/// Reserved address of the software reset call.
pub const SWRST_ADDRESS: u8 = 0x4B;
/// The two bytes that must follow the software reset address.
pub const SWRST_BYTES: [u8; 2] = [0xA5, 0x5A];
/// Number of channels.
pub const CHANNELS: usize = 8;
/// Bytes covered by one flush: PWM0..7, GRPPWM, GRPFREQ, LEDOUT0, LEDOUT1.
const FLUSH_LEN: usize = 12;
/// Oscillator start-up time after clearing MODE1.OSC (datasheet: 500 us max).
const OSC_STARTUP_US: u32 = 500;

/// State of one output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// Off (LDR = 00).
    Off,
    /// Fully on at the configured current (LDR = 01).
    On,
    /// Individually dimmed (LDR = 10) with this 8-bit duty cycle.
    Pwm(u8),
}

/// Driver error. `E` is the I2C error type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error<E> {
    /// Bus transfer failed.
    I2c(E),
    /// Channel number is 8 or more.
    BadChannel,
    /// MODE1 did not read back as configured.
    Mode1Mismatch {
        /// Value read (bits 4:0 are compared).
        got: u8,
    },
    /// A register did not read back as written.
    Mismatch {
        /// Register address.
        reg: u8,
        /// Value expected from the shadow.
        expected: u8,
        /// Value read.
        got: u8,
    },
}

/// Software reset every TLC59108 on the bus. Wait at least 1 ms afterwards
/// and call [`Tlc59108::init`] again.
pub fn reset_all<I: I2c>(i2c: &mut I) -> Result<(), Error<I::Error>> {
    i2c.write(SWRST_ADDRESS, &SWRST_BYTES).map_err(Error::I2c)
}

/// Output current in microamps for a given `IREF` code and external resistor.
///
/// Datasheet: `Iout = (1.26 V / Rext * 15) * CG`, `CG = VG * 3^(CM - 1)`,
/// `VG = (1 + HC) * (1 + D / 64) / 4`, with `IREF = {CM, HC, CC[5:0]}` and
/// `D = CC`. Returns `None` for `rext_ohm == 0`.
pub fn output_current_microamps(rext_ohm: u32, iref_code: u8) -> Option<u32> {
    if rext_ohm == 0 {
        return None;
    }
    let cm_high = iref_code & 0x80 != 0;
    let hc = u64::from((iref_code >> 6) & 1);
    let d = u64::from(iref_code & 0x3F);
    // 1.26 V * 15 expressed in micro-volts: 18_900_000.
    let numerator = 18_900_000_u64.checked_mul(hc.checked_add(1)?)?.checked_mul(d.checked_add(64)?)?;
    let denominator = 256_u64.checked_mul(u64::from(rext_ohm))?.checked_mul(if cm_high { 1 } else { 3 })?;
    u32::try_from(numerator.checked_div(denominator)?).ok()
}

/// One TLC59108 on the bus. Holds the address and a shadow of its registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tlc59108 {
    address: u8,
    pwm: [u8; CHANNELS],
    mode: [u8; CHANNELS],
    grp_pwm: u8,
    grp_freq: u8,
}

impl Tlc59108 {
    /// Create a driver for the chip whose address pins `A3..A0` form
    /// `pins` (0 to 15). Returns `None` if `pins` is out of range.
    pub const fn new(pins: u8) -> Option<Self> {
        if pins > 0x0F {
            return None;
        }
        Some(Self::from_pins_masked(pins))
    }

    /// Like [`Tlc59108::new`] but keeps only the low four bits of `pins`, so it
    /// cannot fail. For board constants.
    pub const fn from_pins_masked(pins: u8) -> Self {
        Self { address: 0x40 | (pins & 0x0F), pwm: [0; CHANNELS], mode: [0; CHANNELS], grp_pwm: 0, grp_freq: 0 }
    }

    /// The 7-bit I2C address.
    pub const fn address(&self) -> u8 {
        self.address
    }

    fn ledout(&self) -> [u8; 2] {
        let pack = |lo: usize| -> u8 {
            let mut v = 0_u8;
            for (i, m) in self.mode.iter().skip(lo).take(4).enumerate() {
                let shift = u32::try_from(i).unwrap_or(0).saturating_mul(2);
                v |= (m & 0b11).checked_shl(shift).unwrap_or(0);
            }
            v
        };
        [pack(0), pack(4)]
    }

    fn image(&self) -> [u8; FLUSH_LEN] {
        let mut b = [0_u8; FLUSH_LEN];
        for (dst, src) in b.iter_mut().zip(self.pwm.iter()) {
            *dst = *src;
        }
        let [l0, l1] = self.ledout();
        if let Some(x) = b.get_mut(8) {
            *x = self.grp_pwm;
        }
        if let Some(x) = b.get_mut(9) {
            *x = self.grp_freq;
        }
        if let Some(x) = b.get_mut(10) {
            *x = l0;
        }
        if let Some(x) = b.get_mut(11) {
            *x = l1;
        }
        b
    }

    /// Change one output in the shadow. Call [`Tlc59108::flush`] to apply.
    pub fn set(&mut self, channel: usize, output: Output) -> Result<(), BadChannel> {
        let (mode, duty) = match output {
            Output::Off => (0b00, None),
            Output::On => (0b01, None),
            Output::Pwm(d) => (0b10, Some(d)),
        };
        let m = self.mode.get_mut(channel).ok_or(BadChannel)?;
        *m = mode;
        if let Some(d) = duty {
            if let Some(p) = self.pwm.get_mut(channel) {
                *p = d;
            }
        }
        Ok(())
    }

    /// The shadow state of an output.
    pub fn get(&self, channel: usize) -> Option<Output> {
        let m = *self.mode.get(channel)?;
        let p = *self.pwm.get(channel)?;
        Some(match m {
            0b00 => Output::Off,
            0b01 => Output::On,
            _ => Output::Pwm(p),
        })
    }

    /// Turn the oscillator on, disable the broadcast addresses, clear all
    /// outputs and verify the result.
    pub fn init<I: I2c, D: DelayNs>(&mut self, i2c: &mut I, delay: &mut D) -> Result<(), Error<I::Error>> {
        self.pwm = [0; CHANNELS];
        self.mode = [0; CHANNELS];
        self.grp_pwm = 0;
        self.grp_freq = 0;
        // MODE1: OSC = 0 (on), SUB1..3 = 0, ALLCALL = 0. MODE2: defaults.
        // Auto-increment is needed so the second byte lands in MODE2 and not in MODE1 again.
        i2c.write(self.address, &[AI_ALL | reg::MODE1, 0x00, 0x00]).map_err(Error::I2c)?;
        delay.delay_us(OSC_STARTUP_US);
        self.flush(i2c)?;
        self.verify(i2c)
    }

    /// Write every output register in one transaction.
    pub fn flush<I: I2c>(&self, i2c: &mut I) -> Result<(), Error<I::Error>> {
        let img = self.image();
        let mut frame = [0_u8; FLUSH_LEN + 1];
        if let Some(c) = frame.first_mut() {
            *c = AI_ALL | reg::PWM0;
        }
        for (dst, src) in frame.iter_mut().skip(1).zip(img.iter()) {
            *dst = *src;
        }
        i2c.write(self.address, &frame).map_err(Error::I2c)
    }

    /// Read MODE1 and the output registers back and compare them to the shadow.
    pub fn verify<I: I2c>(&self, i2c: &mut I) -> Result<(), Error<I::Error>> {
        let mut m1 = [0_u8; 1];
        i2c.write_read(self.address, &[reg::MODE1], &mut m1).map_err(Error::I2c)?;
        let got = m1.first().copied().unwrap_or(0xFF);
        // Bits 7:5 report the auto-increment state; compare bits 4:0 only.
        if got & 0x1F != 0 {
            return Err(Error::Mode1Mismatch { got });
        }
        let mut back = [0_u8; FLUSH_LEN];
        i2c.write_read(self.address, &[AI_ALL | reg::PWM0], &mut back).map_err(Error::I2c)?;
        let want = self.image();
        for (i, (w, g)) in want.iter().zip(back.iter()).enumerate() {
            if w != g {
                let r = reg::PWM0.saturating_add(u8::try_from(i).unwrap_or(0));
                return Err(Error::Mismatch { reg: r, expected: *w, got: *g });
            }
        }
        Ok(())
    }

    /// Read the error flag register (open-circuit and over-temperature, by channel).
    pub fn error_flags<I: I2c>(&self, i2c: &mut I) -> Result<u8, Error<I::Error>> {
        let mut b = [0_u8; 1];
        i2c.write_read(self.address, &[reg::EFLAG], &mut b).map_err(Error::I2c)?;
        Ok(b.first().copied().unwrap_or(0))
    }

    /// Set the output current gain code `{CM, HC, CC[5:0]}` (register IREF).
    pub fn set_current_code<I: I2c>(&self, i2c: &mut I, code: u8) -> Result<(), Error<I::Error>> {
        i2c.write(self.address, &[reg::IREF, code]).map_err(Error::I2c)
    }
}

/// Channel number out of range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadChannel;
