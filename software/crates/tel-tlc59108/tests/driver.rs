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
//! Driver tests against a simulator of two TLC59108 chips on one I2C bus.

use embedded_hal::delay::DelayNs;
use embedded_hal::i2c::{Error as HalError, ErrorKind, ErrorType, I2c, NoAcknowledgeSource, Operation};
use proptest::prelude::*;
use tel_tlc59108::leds::{Bank, Color, ColorMap, StatusLeds, LED_COUNT};
use tel_tlc59108::{output_current_microamps, reg, reset_all, Error, Output, Tlc59108};

// ---------------------------------------------------------------- simulator

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BusError {
    Nack,
    Fault,
}
impl HalError for BusError {
    fn kind(&self) -> ErrorKind {
        match self {
            Self::Nack => ErrorKind::NoAcknowledge(NoAcknowledgeSource::Address),
            Self::Fault => ErrorKind::Bus,
        }
    }
}

#[derive(Clone)]
struct Chip {
    address: u8,
    regs: [u8; 0x14],
    ptr: u8,
    ai: u8,
}

impl Chip {
    fn new(address: u8) -> Self {
        let mut c = Self { address, regs: [0; 0x14], ptr: 0, ai: 0 };
        c.power_up();
        c
    }

    fn power_up(&mut self) {
        self.regs = [0; 0x14];
        self.regs[0x00] = 0b0001_0001; // OSC = 1 (off), ALLCALL = 1
        self.regs[0x0E] = 0x92;
        self.regs[0x0F] = 0x94;
        self.regs[0x10] = 0x98;
        self.regs[0x11] = 0x90; // LED All Call address 1001 000
        self.regs[0x12] = 0xFF; // IREF
        self.regs[0x13] = 0xFF; // EFLAG
        self.ptr = 0;
        self.ai = 0;
    }

    fn responds_to(&self, addr: u8) -> bool {
        addr == self.address || (addr == 0x48 && self.regs[0] & 1 != 0)
    }

    fn set_control(&mut self, c: u8) {
        self.ai = c >> 5;
        self.ptr = c & 0x1F;
        assert!(
            self.ai == 0b000 || self.ai == 0b100,
            "driver used an auto-increment mode this test does not model: {:03b}",
            self.ai
        );
        assert!(self.ptr <= 0x13, "register pointer out of range");
    }

    fn advance(&mut self) {
        if self.ai == 0b100 {
            self.ptr = if self.ptr >= 0x11 { 0x00 } else { self.ptr + 1 };
        }
    }

    fn write(&mut self, v: u8) {
        let p = usize::from(self.ptr);
        match self.ptr {
            0x00 => self.regs[0] = v & 0x1F, // bits 7:5 are read only
            0x13 => {}                       // EFLAG read only
            _ => self.regs[p] = v,
        }
        self.advance();
    }

    fn read(&mut self) -> u8 {
        let p = usize::from(self.ptr);
        let v = if self.ptr == 0x00 { (self.ai << 5) | (self.regs[0] & 0x1F) } else { self.regs[p] };
        self.advance();
        v
    }
}

struct Bus {
    chips: Vec<Chip>,
    transactions: usize,
    fail_on: Option<usize>,
    log: Vec<(u8, Vec<u8>)>, // (address, bytes written)
    swrst_count: usize,
}

impl Bus {
    fn new(addresses: &[u8]) -> Self {
        Self {
            chips: addresses.iter().map(|a| Chip::new(*a)).collect(),
            transactions: 0,
            fail_on: None,
            log: Vec::new(),
            swrst_count: 0,
        }
    }
    fn chip(&self, addr: u8) -> &Chip {
        self.chips.iter().find(|c| c.address == addr).unwrap()
    }
    fn chip_mut(&mut self, addr: u8) -> &mut Chip {
        self.chips.iter_mut().find(|c| c.address == addr).unwrap()
    }
}

impl ErrorType for Bus {
    type Error = BusError;
}

impl I2c for Bus {
    fn transaction(&mut self, address: u8, ops: &mut [Operation<'_>]) -> Result<(), BusError> {
        let n = self.transactions;
        self.transactions += 1;
        if self.fail_on == Some(n) {
            return Err(BusError::Fault);
        }
        if address == 0x4B {
            // Software reset call: exactly A5h, 5Ah.
            let ok = matches!(ops, [Operation::Write(b)] if **b == [0xA5, 0x5A]);
            if !ok {
                return Err(BusError::Nack);
            }
            for c in &mut self.chips {
                c.power_up();
            }
            self.swrst_count += 1;
            return Ok(());
        }
        let Some(idx) = self.chips.iter().position(|c| c.responds_to(address)) else { return Err(BusError::Nack) };
        let mut written = Vec::new();
        let mut first_write = true;
        for op in ops.iter_mut() {
            match op {
                Operation::Write(bytes) => {
                    for b in bytes.iter() {
                        written.push(*b);
                        let chip = &mut self.chips[idx];
                        if first_write {
                            chip.set_control(*b);
                            first_write = false;
                        } else {
                            chip.write(*b);
                        }
                    }
                    first_write = true; // a following Write after a repeated start would be a new control byte
                }
                Operation::Read(buf) => {
                    for slot in buf.iter_mut() {
                        *slot = self.chips[idx].read();
                    }
                }
            }
        }
        self.log.push((address, written));
        Ok(())
    }
}

#[derive(Default)]
struct RecDelay {
    total_us: u64,
}
impl DelayNs for RecDelay {
    fn delay_ns(&mut self, ns: u32) {
        self.total_us += u64::from(ns) / 1000;
    }
}

// ---------------------------------------------------------------- chip tests

#[test]
fn address_pins_map_to_0x40_through_0x4f() {
    for pins in 0..=15_u8 {
        assert_eq!(Tlc59108::new(pins).unwrap().address(), 0x40 + pins);
    }
    assert!(Tlc59108::new(16).is_none());
    assert!(Tlc59108::new(255).is_none());
    // the masked constructor never fails
    assert_eq!(Tlc59108::from_pins_masked(0xF1).address(), 0x41);
}

#[test]
fn init_enables_the_oscillator_and_disables_broadcast_addresses() {
    let mut bus = Bus::new(&[0x40]);
    assert_eq!(bus.chip(0x40).regs[0], 0x11, "datasheet power-up: oscillator off, All Call on");
    let mut chip = Tlc59108::new(0).unwrap();
    let mut delay = RecDelay::default();
    chip.init(&mut bus, &mut delay).unwrap();
    assert_eq!(bus.chip(0x40).regs[0], 0x00, "MODE1: oscillator on, no All Call, no Sub Call");
    assert_eq!(bus.chip(0x40).regs[1], 0x00, "MODE2 defaults");
    assert!(delay.total_us >= 500, "must wait for the oscillator (500 us)");
    // All Call address no longer answers: a broadcast cannot change the LEDs.
    let r = bus.write(0x48, &[0x80 | reg::PWM0, 0xFF]);
    assert_eq!(r, Err(BusError::Nack));
}

#[test]
fn set_and_flush_pack_registers_exactly() {
    let mut bus = Bus::new(&[0x40]);
    let mut chip = Tlc59108::new(0).unwrap();
    chip.init(&mut bus, &mut RecDelay::default()).unwrap();
    chip.set(0, Output::On).unwrap();
    chip.set(1, Output::Pwm(0x80)).unwrap();
    chip.set(2, Output::Off).unwrap();
    chip.set(3, Output::On).unwrap();
    chip.set(4, Output::Pwm(0x11)).unwrap();
    chip.set(7, Output::On).unwrap();
    bus.log.clear();
    let before = bus.transactions;
    chip.flush(&mut bus).unwrap();
    assert_eq!(bus.transactions - before, 1, "all registers must change in one transaction");
    let (addr, bytes) = &bus.log[0];
    assert_eq!(*addr, 0x40);
    // control byte: AI = 100, pointer = PWM0
    assert_eq!(bytes[0], 0x82);
    // PWM0..7: only ch1 and ch4 carry duty values
    assert_eq!(&bytes[1..9], &[0, 0x80, 0, 0, 0x11, 0, 0, 0]);
    assert_eq!(bytes[9], 0, "GRPPWM");
    assert_eq!(bytes[10], 0, "GRPFREQ");
    // LEDOUT0: ch0 = 01, ch1 = 10, ch2 = 00, ch3 = 01 -> 01 00 10 01
    assert_eq!(bytes[11], 0b01_00_10_01);
    // LEDOUT1: ch4 = 10, ch5 = 00, ch6 = 00, ch7 = 01 -> 01 00 00 10
    assert_eq!(bytes[12], 0b01_00_00_10);
    assert_eq!(bytes.len(), 13);
    // the chip now holds exactly that
    let regs = &bus.chip(0x40).regs;
    assert_eq!(regs[0x0C], 0x49);
    assert_eq!(regs[0x0D], 0x42);
}

#[test]
fn shadow_reads_back_what_was_set() {
    let mut chip = Tlc59108::new(1).unwrap();
    chip.set(0, Output::Pwm(7)).unwrap();
    chip.set(1, Output::On).unwrap();
    chip.set(2, Output::Off).unwrap();
    assert_eq!(chip.get(0), Some(Output::Pwm(7)));
    assert_eq!(chip.get(1), Some(Output::On));
    assert_eq!(chip.get(2), Some(Output::Off));
    assert_eq!(chip.get(8), None);
    assert!(chip.set(8, Output::On).is_err());
    assert!(chip.set(usize::MAX, Output::On).is_err());
    // switching a channel from Pwm to Off keeps the duty but reports Off
    chip.set(0, Output::Off).unwrap();
    assert_eq!(chip.get(0), Some(Output::Off));
}

#[test]
fn verify_passes_after_flush_and_detects_a_reset_chip() {
    let mut bus = Bus::new(&[0x40]);
    let mut chip = Tlc59108::new(0).unwrap();
    chip.init(&mut bus, &mut RecDelay::default()).unwrap();
    chip.set(3, Output::Pwm(200)).unwrap();
    chip.flush(&mut bus).unwrap();
    assert_eq!(chip.verify(&mut bus), Ok(()));
    // A brown-out or glitch resets the chip: oscillator off again.
    bus.chip_mut(0x40).power_up();
    assert_eq!(chip.verify(&mut bus), Err(Error::Mode1Mismatch { got: 0x11 }));
    // Re-initialising recovers it, and the shadow is rebuilt from scratch.
    chip.init(&mut bus, &mut RecDelay::default()).unwrap();
    assert_eq!(chip.get(3), Some(Output::Off));
    assert_eq!(chip.verify(&mut bus), Ok(()));
}

#[test]
fn verify_detects_a_single_corrupted_register() {
    let mut bus = Bus::new(&[0x40]);
    let mut chip = Tlc59108::new(0).unwrap();
    chip.init(&mut bus, &mut RecDelay::default()).unwrap();
    chip.set(2, Output::Pwm(100)).unwrap();
    chip.flush(&mut bus).unwrap();
    bus.chip_mut(0x40).regs[0x04] ^= 0x10; // PWM2 bit flip
    assert_eq!(chip.verify(&mut bus), Err(Error::Mismatch { reg: 0x04, expected: 100, got: 100 ^ 0x10 }));
    bus.chip_mut(0x40).regs[0x04] ^= 0x10;
    bus.chip_mut(0x40).regs[0x0C] ^= 0x02; // LEDOUT0 bit flip
    assert!(matches!(chip.verify(&mut bus), Err(Error::Mismatch { reg: 0x0C, .. })));
}

#[test]
fn every_single_bus_fault_during_init_is_reported_and_recoverable() {
    let mut clean = Bus::new(&[0x40]);
    let mut c = Tlc59108::new(0).unwrap();
    c.init(&mut clean, &mut RecDelay::default()).unwrap();
    let total = clean.transactions;
    for n in 0..total {
        let mut bus = Bus::new(&[0x40]);
        bus.fail_on = Some(n);
        let mut chip = Tlc59108::new(0).unwrap();
        assert_eq!(chip.init(&mut bus, &mut RecDelay::default()), Err(Error::I2c(BusError::Fault)), "fault {n}");
        bus.fail_on = None;
        assert_eq!(chip.init(&mut bus, &mut RecDelay::default()), Ok(()), "retry after fault {n}");
    }
}

#[test]
fn missing_chip_is_reported_as_nack() {
    let mut bus = Bus::new(&[0x41]);
    let mut chip = Tlc59108::new(0).unwrap();
    assert_eq!(chip.init(&mut bus, &mut RecDelay::default()), Err(Error::I2c(BusError::Nack)));
}

#[test]
fn error_flags_and_current_code_use_the_registers_outside_auto_increment() {
    let mut bus = Bus::new(&[0x40]);
    let mut chip = Tlc59108::new(0).unwrap();
    chip.init(&mut bus, &mut RecDelay::default()).unwrap();
    assert_eq!(chip.error_flags(&mut bus), Ok(0xFF));
    bus.chip_mut(0x40).regs[0x13] = 0b0000_0100;
    assert_eq!(chip.error_flags(&mut bus), Ok(0b0000_0100));
    chip.set_current_code(&mut bus, 0xC0).unwrap();
    assert_eq!(bus.chip(0x40).regs[0x12], 0xC0);
}

#[test]
fn software_reset_call_resets_all_chips() {
    let mut bus = Bus::new(&[0x40, 0x41]);
    let mut a = Tlc59108::new(0).unwrap();
    let mut b = Tlc59108::new(1).unwrap();
    a.init(&mut bus, &mut RecDelay::default()).unwrap();
    b.init(&mut bus, &mut RecDelay::default()).unwrap();
    reset_all(&mut bus).unwrap();
    assert_eq!(bus.swrst_count, 1);
    assert!(matches!(a.verify(&mut bus), Err(Error::Mode1Mismatch { .. })));
    assert!(matches!(b.verify(&mut bus), Err(Error::Mode1Mismatch { .. })));
}

// ---------------------------------------------------------------- current

#[test]
fn output_current_matches_datasheet_examples() {
    // Default code 0xFF with the board's 1.87k: 1.26 V / 1870 * 15 * (127/128)
    let i = output_current_microamps(1870, 0xFF).unwrap();
    let expect = 1.26 / 1870.0 * 15.0 * (127.0 / 128.0) * 1e6;
    assert!((f64::from(i) - expect).abs() < 1.0, "{i} vs {expect}");
    // {CM,HC,CC} = {1,1,0}: VG = 1/2
    let i = output_current_microamps(1870, 0b1100_0000).unwrap();
    assert!((f64::from(i) - 1.26 / 1870.0 * 15.0 * 0.5 * 1e6).abs() < 1.0);
    // {0,0,0}: VG = 1/4, CG = (1/4) * 3^-1 = 1/12
    let i = output_current_microamps(1870, 0).unwrap();
    assert!((f64::from(i) - 1.26 / 1870.0 * 15.0 / 12.0 * 1e6).abs() < 1.0);
    assert_eq!(output_current_microamps(0, 0xFF), None);
}

proptest! {
    #[test]
    fn current_is_monotonic_in_cc_and_never_panics(r in 1_u32..1_000_000, cm in any::<bool>(), hc in any::<bool>()) {
        let base = (u8::from(cm) << 7) | (u8::from(hc) << 6);
        let mut prev = 0;
        for cc in 0..64_u8 {
            let i = output_current_microamps(r, base | cc).unwrap();
            prop_assert!(i >= prev);
            prev = i;
        }
    }

    #[test]
    fn any_code_any_resistor_is_total(r in any::<u32>(), code in any::<u8>()) {
        let _ = output_current_microamps(r, code);
    }
}

// ---------------------------------------------------------------- status LEDs

fn leds_on_bus() -> (StatusLeds, Bus) {
    let mut bus = Bus::new(&[0x40, 0x41]);
    let mut leds = StatusLeds::new(ColorMap { red: Bank::Pin3 });
    leds.init(&mut bus, &mut RecDelay::default()).unwrap();
    (leds, bus)
}

#[test]
fn status_leds_use_the_board_addresses() {
    let leds = StatusLeds::new(ColorMap::ASSUMED);
    assert_eq!(leds.addresses(), (0x40, 0x41)); // U8 (pin 1), U9 (pin 3)
}

#[test]
fn colours_land_on_the_right_chip_and_channel() {
    let (mut leds, mut bus) = leds_on_bus();
    // red = Pin3 = U9 = 0x41; green = Pin1 = U8 = 0x40
    leds.set(0, Color::Red, 255).unwrap();
    leds.set(1, Color::Green, 255).unwrap();
    leds.set(2, Color::Amber, 128).unwrap();
    leds.set(3, Color::Off, 255).unwrap();
    leds.set(4, Color::Red, 0).unwrap(); // zero brightness is off
    leds.flush(&mut bus).unwrap();
    let u9 = &bus.chip(0x41).regs; // red
    let u8_ = &bus.chip(0x40).regs; // green
                                    // channel k: LEDOUT nibble pairs: ch0..3 in 0x0C
    let mode = |regs: &[u8; 0x14], ch: usize| -> u8 { (regs[0x0C + ch / 4] >> ((ch % 4) * 2)) & 0b11 };
    assert_eq!(mode(u9, 0), 0b01); // red On
    assert_eq!(mode(u8_, 0), 0b00);
    assert_eq!(mode(u9, 1), 0b00);
    assert_eq!(mode(u8_, 1), 0b01); // green On
    assert_eq!(mode(u9, 2), 0b10); // amber: both PWM
    assert_eq!(mode(u8_, 2), 0b10);
    assert_eq!(u9[0x02 + 2], 128);
    assert_eq!(u8_[0x02 + 2], 128);
    assert_eq!(mode(u9, 3), 0b00);
    assert_eq!(mode(u8_, 3), 0b00);
    assert_eq!(mode(u9, 4), 0b00);
    // unused channels 5..7 stay off
    for ch in 5..8 {
        assert_eq!(mode(u9, ch), 0);
        assert_eq!(mode(u8_, ch), 0);
    }
    assert_eq!(leds.verify(&mut bus), Ok(()));
}

#[test]
fn colour_map_swaps_the_banks() {
    let mut bus = Bus::new(&[0x40, 0x41]);
    let mut leds = StatusLeds::new(ColorMap { red: Bank::Pin1 });
    leds.init(&mut bus, &mut RecDelay::default()).unwrap();
    leds.set(0, Color::Red, 255).unwrap();
    leds.flush(&mut bus).unwrap();
    assert_eq!(bus.chip(0x40).regs[0x0C] & 0b11, 0b01, "red is on the pin 1 bank (U8, 0x40)");
    assert_eq!(bus.chip(0x41).regs[0x0C] & 0b11, 0b00);
}

#[test]
fn out_of_range_led_is_rejected_without_side_effects() {
    let (mut leds, mut bus) = leds_on_bus();
    let snapshot = leds;
    assert!(leds.set(LED_COUNT, Color::Red, 255).is_err());
    assert!(leds.set(usize::MAX, Color::Red, 255).is_err());
    assert_eq!(leds, snapshot);
    leds.flush(&mut bus).unwrap();
}

#[test]
fn set_all_and_light_bank_for_bringup() {
    let (mut leds, mut bus) = leds_on_bus();
    leds.set_all(Color::Green, 255);
    leds.flush(&mut bus).unwrap();
    assert_eq!(bus.chip(0x40).regs[0x0C], 0x55); // channels 0..3 on
    assert_eq!(bus.chip(0x40).regs[0x0D] & 0b11, 0b01); // channel 4 on
                                                        // Identify the bank wired to LED pin 3, nothing else lit.
    leds.light_bank(&mut bus, Bank::Pin3, 255).unwrap();
    assert_eq!(bus.chip(0x41).regs[0x0C], 0x55);
    assert_eq!(bus.chip(0x40).regs[0x0C], 0x00);
    assert_eq!(bus.chip(0x40).regs[0x0D], 0x00);
}

#[test]
fn one_dead_chip_does_not_stop_the_other_from_being_driven() {
    let mut bus = Bus::new(&[0x41]); // U8 (0x40) is absent
    let mut leds = StatusLeds::new(ColorMap { red: Bank::Pin3 });
    let r = leds.init(&mut bus, &mut RecDelay::default());
    assert_eq!(r, Err(Error::I2c(BusError::Nack)));
    assert_eq!(bus.chip(0x41).regs[0x00], 0x00, "the working chip was still initialised");
    leds.set(0, Color::Red, 255).unwrap();
    assert_eq!(leds.flush(&mut bus), Err(Error::I2c(BusError::Nack)));
    assert_eq!(bus.chip(0x41).regs[0x0C] & 0b11, 0b01, "the working chip still received its update");
}

proptest! {
    /// Any sequence of LED updates produces a shadow the hardware matches.
    #[test]
    fn random_updates_always_verify(ops in prop::collection::vec((0_usize..6, 0_u8..4, any::<u8>()), 0..60)) {
        let (mut leds, mut bus) = leds_on_bus();
        for (led, c, level) in ops {
            let colour = [Color::Off, Color::Red, Color::Green, Color::Amber][usize::from(c)];
            let r = leds.set(led, colour, level);
            prop_assert_eq!(r.is_ok(), led < LED_COUNT);
        }
        leds.flush(&mut bus).unwrap();
        prop_assert_eq!(leds.verify(&mut bus), Ok(()));
    }
}
