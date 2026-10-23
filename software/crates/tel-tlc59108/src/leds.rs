//! Five bi-colour status LEDs driven by two TLC59108 chips.
//!
//! Board wiring (netlist 2026-10-13): each LED is common-anode on +5V with its
//! two cathodes on separate chips. `U9` (address pins `0001`, I2C 0x41) drives
//! pin 3 of `LED1..LED5` from `OUT0..OUT4`; `U8` (address pins `0000`, I2C 0x40)
//! drives pin 1 from `OUT0..OUT4`. LED `n` therefore uses channel `n` on both
//! chips.
//!
//! **Which pin is red is not yet verified** (the LED datasheet could not be
//! retrieved). [`ColorMap::ASSUMED`] is a guess; run [`StatusLeds::sweep`]
//! style bring-up (all LEDs one bank at a time) and correct the map.

use crate::{BadChannel, Error, Output, Tlc59108};
use embedded_hal::delay::DelayNs;
use embedded_hal::i2c::I2c;

/// Number of LEDs.
pub const LED_COUNT: usize = 5;

/// Which chip (and therefore which LED pin) a colour is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bank {
    /// `U8`, LED pin 1, nets `LED5..LED9`, I2C 0x40.
    Pin1,
    /// `U9`, LED pin 3, nets `LED0..LED4`, I2C 0x41.
    Pin3,
}

/// Assignment of colours to banks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorMap {
    /// The bank holding the red die; green is the other bank.
    pub red: Bank,
}

impl ColorMap {
    /// UNVERIFIED guess. Confirm on hardware before relying on colours.
    pub const ASSUMED: Self = Self { red: Bank::Pin1 };

    fn green(&self) -> Bank {
        match self.red {
            Bank::Pin1 => Bank::Pin3,
            Bank::Pin3 => Bank::Pin1,
        }
    }
}

/// LED colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    /// Dark.
    Off,
    /// Red die only.
    Red,
    /// Green die only.
    Green,
    /// Both dies (reads as amber or orange).
    Amber,
}

/// The two chips and the colour map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusLeds {
    pin1: Tlc59108,
    pin3: Tlc59108,
    map: ColorMap,
}

impl StatusLeds {
    /// Create the bank. The addresses are fixed by the board: `U8` has address
    /// pins `0000` and `U9` has `0001`.
    pub const fn new(map: ColorMap) -> Self {
        Self { pin1: Tlc59108::from_pins_masked(0b0000), pin3: Tlc59108::from_pins_masked(0b0001), map }
    }

    /// I2C addresses as `(pin 1 bank, pin 3 bank)`.
    pub const fn addresses(&self) -> (u8, u8) {
        (self.pin1.address(), self.pin3.address())
    }

    /// Initialise both chips (all LEDs off). Both are attempted; the first
    /// error is returned.
    pub fn init<I: I2c, D: DelayNs>(&mut self, i2c: &mut I, delay: &mut D) -> Result<(), Error<I::Error>> {
        let a = self.pin1.init(i2c, delay);
        let b = self.pin3.init(i2c, delay);
        a.and(b)
    }

    fn chip(&mut self, bank: Bank) -> &mut Tlc59108 {
        match bank {
            Bank::Pin1 => &mut self.pin1,
            Bank::Pin3 => &mut self.pin3,
        }
    }

    fn drive(chip: &mut Tlc59108, led: usize, on: bool, level: u8) -> Result<(), BadChannel> {
        let out = match (on, level) {
            (false, _) | (true, 0) => Output::Off,
            (true, 255) => Output::On,
            (true, l) => Output::Pwm(l),
        };
        chip.set(led, out)
    }

    /// Set one LED in the shadow. `level` is the brightness 0..=255 of each die
    /// that is lit. Call [`StatusLeds::flush`] to apply.
    pub fn set(&mut self, led: usize, color: Color, level: u8) -> Result<(), BadChannel> {
        if led >= LED_COUNT {
            return Err(BadChannel);
        }
        let (red_on, green_on) = match color {
            Color::Off => (false, false),
            Color::Red => (true, false),
            Color::Green => (false, true),
            Color::Amber => (true, true),
        };
        let red = self.map.red;
        let green = self.map.green();
        Self::drive(self.chip(red), led, red_on, level)?;
        Self::drive(self.chip(green), led, green_on, level)
    }

    /// Set every LED to the same colour in the shadow.
    pub fn set_all(&mut self, color: Color, level: u8) {
        for led in 0..LED_COUNT {
            // `led` is always in range here.
            let _ = self.set(led, color, level);
        }
    }

    /// Apply the shadow to both chips. Both are attempted; the first error is returned.
    pub fn flush<I: I2c>(&self, i2c: &mut I) -> Result<(), Error<I::Error>> {
        let a = self.pin1.flush(i2c);
        let b = self.pin3.flush(i2c);
        a.and(b)
    }

    /// Read both chips back and compare with the shadow.
    pub fn verify<I: I2c>(&self, i2c: &mut I) -> Result<(), Error<I::Error>> {
        let a = self.pin1.verify(i2c);
        let b = self.pin3.verify(i2c);
        a.and(b)
    }

    /// Light every LED on one bank only (for identifying which pin is which
    /// colour at bring-up), then apply.
    pub fn light_bank<I: I2c>(&mut self, i2c: &mut I, bank: Bank, level: u8) -> Result<(), Error<I::Error>> {
        self.set_all(Color::Off, 0);
        for led in 0..LED_COUNT {
            let _ = Self::drive(self.chip(bank), led, true, level);
        }
        self.flush(i2c)
    }
}
