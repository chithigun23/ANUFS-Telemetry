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
//! Driver tests against a register-level simulator of the ASM330LHHX.

use embedded_hal::delay::DelayNs;
use embedded_hal::spi::{Error as HalError, ErrorKind, ErrorType, Operation, SpiDevice};
use proptest::prelude::*;
use tel_asm330::{reg, AccelRange, Asm330, Config, Error, GyroRange, Odr, Raw, WHO_AM_I_VALUE};

// ---------------------------------------------------------------- simulator

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BusFault;
impl HalError for BusFault {
    fn kind(&self) -> ErrorKind {
        ErrorKind::Other
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Access {
    Read { addr: u8, len: usize },
    Write { addr: u8, data: Vec<u8> },
}

struct Sim {
    regs: [u8; 128],
    who: u8,
    /// Reads of CTRL3_C needed before SW_RESET clears.
    reset_polls_needed: u32,
    reset_polls_left: u32,
    resetting: bool,
    /// Fail the n-th transaction (0-based).
    fail_on: Option<usize>,
    /// Writes to this register keep only the bits in the mask.
    stuck: Option<(u8, u8)>,
    transactions: usize,
    log: Vec<Access>,
}

impl Sim {
    fn new() -> Self {
        let mut s = Self {
            regs: [0; 128],
            who: WHO_AM_I_VALUE,
            reset_polls_needed: 1,
            reset_polls_left: 0,
            resetting: false,
            fail_on: None,
            stuck: None,
            transactions: 0,
            log: Vec::new(),
        };
        s.defaults();
        s
    }

    fn defaults(&mut self) {
        self.regs = [0; 128];
        self.regs[usize::from(reg::CTRL3_C)] = 0b0000_0100; // IF_INC defaults to 1
    }

    fn set_data(&mut self, raw: &Raw) {
        let mut b = Vec::new();
        b.extend(raw.temp.to_le_bytes());
        for v in raw.gyro.iter().chain(raw.accel.iter()) {
            b.extend(v.to_le_bytes());
        }
        let base = usize::from(reg::OUT_TEMP_L);
        self.regs[base..base + 14].copy_from_slice(&b);
    }

    fn auto_inc(&self) -> bool {
        self.regs[usize::from(reg::CTRL3_C)] & 0b100 != 0
    }

    fn read_byte(&mut self, addr: u8) -> u8 {
        if addr == reg::WHO_AM_I {
            return self.who;
        }
        if addr == reg::CTRL3_C && self.resetting {
            if self.reset_polls_left > 0 {
                self.reset_polls_left -= 1;
            }
            if self.reset_polls_left == 0 {
                self.resetting = false;
                self.regs[usize::from(reg::CTRL3_C)] &= !1;
            }
        }
        self.regs[usize::from(addr & 0x7F)]
    }

    fn write_byte(&mut self, addr: u8, value: u8) {
        let a = usize::from(addr & 0x7F);
        if addr == reg::WHO_AM_I || (reg::STATUS_REG..0x2E).contains(&addr) {
            return; // read-only
        }
        if addr == reg::CTRL3_C && value & 1 != 0 {
            self.defaults();
            self.regs[a] = 0b0000_0100 | 1;
            self.resetting = true;
            self.reset_polls_left = self.reset_polls_needed;
            return;
        }
        let stored = match self.stuck {
            Some((r, mask)) if r == addr => value & mask,
            _ => value,
        };
        self.regs[a] = stored;
    }
}

impl ErrorType for Sim {
    type Error = BusFault;
}

impl SpiDevice for Sim {
    fn transaction(&mut self, ops: &mut [Operation<'_, u8>]) -> Result<(), BusFault> {
        let n = self.transactions;
        self.transactions += 1;
        if self.fail_on == Some(n) {
            return Err(BusFault);
        }
        let mut it = ops.iter_mut();
        let Some(Operation::Write(cmd)) = it.next() else { panic!("transaction must start with a write") };
        let first = cmd[0];
        let read = first & 0x80 != 0;
        let mut addr = first & 0x7F;
        if !read {
            let data = cmd[1..].to_vec();
            for d in &data {
                self.write_byte(addr, *d);
                if self.auto_inc() {
                    addr = addr.wrapping_add(1);
                }
            }
            self.log.push(Access::Write { addr: first & 0x7F, data });
            assert!(it.next().is_none(), "write transactions carry no further operations");
            return Ok(());
        }
        assert_eq!(cmd.len(), 1, "read command is a single address byte");
        let Some(Operation::Read(buf)) = it.next() else { panic!("read must follow the address byte") };
        let len = buf.len();
        for slot in buf.iter_mut() {
            *slot = self.read_byte(addr);
            if self.auto_inc() {
                addr = addr.wrapping_add(1);
            }
        }
        self.log.push(Access::Read { addr: first & 0x7F, len });
        Ok(())
    }
}

struct NoDelay;
impl DelayNs for NoDelay {
    fn delay_ns(&mut self, _ns: u32) {}
}

fn init_default(sim: Sim) -> (Asm330<Sim>, Result<(), Error<BusFault>>) {
    let mut d = Asm330::new(sim);
    let r = d.init(Config::default(), &mut NoDelay);
    (d, r)
}

// ---------------------------------------------------------------- tests

#[test]
fn init_writes_the_expected_register_values() {
    let (d, r) = init_default(Sim::new());
    assert_eq!(r, Ok(()));
    let sim = d.release();
    // CTRL3_C: BDU (bit 6) + IF_INC (bit 2)
    assert_eq!(sim.regs[usize::from(reg::CTRL3_C)], 0x44);
    // CTRL4_C: I2C_disable (bit 2)
    assert_eq!(sim.regs[usize::from(reg::CTRL4_C)], 0x04);
    // INT1_CTRL: DRDY_XL | DRDY_G
    assert_eq!(sim.regs[usize::from(reg::INT1_CTRL)], 0x03);
    // CTRL2_G: ODR 416 Hz (0b0110) << 4 | FS 2000 dps (0b11) << 2
    assert_eq!(sim.regs[usize::from(reg::CTRL2_G)], 0x6C);
    // CTRL1_XL: ODR 416 Hz << 4 | FS 16 g (0b01) << 2
    assert_eq!(sim.regs[usize::from(reg::CTRL1_XL)], 0x64);
}

#[test]
fn init_order_enables_outputs_last() {
    let (d, r) = init_default(Sim::new());
    assert_eq!(r, Ok(()));
    let writes: Vec<u8> = d
        .release()
        .log
        .iter()
        .filter_map(|a| if let Access::Write { addr, .. } = a { Some(*addr) } else { None })
        .collect();
    // reset first, outputs (CTRL2_G, CTRL1_XL) last
    assert_eq!(writes.first(), Some(&reg::CTRL3_C));
    assert_eq!(&writes[writes.len() - 2..], &[reg::CTRL2_G, reg::CTRL1_XL]);
}

#[test]
fn spi_framing_read_bit_and_address_masking() {
    let (d, _) = init_default(Sim::new());
    let sim = d.release();
    // first access is the WHO_AM_I read
    assert_eq!(sim.log[0], Access::Read { addr: reg::WHO_AM_I, len: 1 });
    // every logged address is 7-bit
    for a in &sim.log {
        let addr = match a {
            Access::Read { addr, .. } | Access::Write { addr, .. } => *addr,
        };
        assert!(addr < 0x80);
    }
}

#[test]
fn datasheet_encodings() {
    // Accelerometer FS bits: 00 = 2g, 01 = 16g, 10 = 4g, 11 = 8g
    assert_eq!(AccelRange::G2.bits(), 0b00);
    assert_eq!(AccelRange::G16.bits(), 0b01);
    assert_eq!(AccelRange::G4.bits(), 0b10);
    assert_eq!(AccelRange::G8.bits(), 0b11);
    // Gyro: FS[1:0] in bits 3:2, FS_125 bit 1, FS_4000 bit 0
    assert_eq!(GyroRange::Dps250.bits(), 0b0000);
    assert_eq!(GyroRange::Dps500.bits(), 0b0100);
    assert_eq!(GyroRange::Dps1000.bits(), 0b1000);
    assert_eq!(GyroRange::Dps2000.bits(), 0b1100);
    assert_eq!(GyroRange::Dps125.bits(), 0b0010);
    assert_eq!(GyroRange::Dps4000.bits(), 0b0001);
    // ODR codes (high performance column)
    let odrs = [
        (Odr::Hz12_5, 0b0001),
        (Odr::Hz26, 0b0010),
        (Odr::Hz52, 0b0011),
        (Odr::Hz104, 0b0100),
        (Odr::Hz208, 0b0101),
        (Odr::Hz416, 0b0110),
        (Odr::Hz833, 0b0111),
        (Odr::Hz1667, 0b1000),
        (Odr::Hz3333, 0b1001),
        (Odr::Hz6667, 0b1010),
    ];
    for (o, code) in odrs {
        assert_eq!(o.code(), code);
    }
    // Sensitivities from Table 2
    assert_eq!(
        [AccelRange::G2, AccelRange::G4, AccelRange::G8, AccelRange::G16].map(AccelRange::micro_g_per_lsb),
        [61, 122, 244, 488]
    );
    assert_eq!(
        [
            GyroRange::Dps125,
            GyroRange::Dps250,
            GyroRange::Dps500,
            GyroRange::Dps1000,
            GyroRange::Dps2000,
            GyroRange::Dps4000
        ]
        .map(GyroRange::micro_dps_per_lsb),
        [4_375, 8_750, 17_500, 35_000, 70_000, 140_000]
    );
}

#[test]
fn every_range_and_rate_combination_is_written_and_verified() {
    let accels = [AccelRange::G2, AccelRange::G4, AccelRange::G8, AccelRange::G16];
    let gyros = [
        GyroRange::Dps125,
        GyroRange::Dps250,
        GyroRange::Dps500,
        GyroRange::Dps1000,
        GyroRange::Dps2000,
        GyroRange::Dps4000,
    ];
    let odrs = [Odr::Hz12_5, Odr::Hz104, Odr::Hz416, Odr::Hz6667];
    for a in accels {
        for g in gyros {
            for o in odrs {
                for drdy in [false, true] {
                    let cfg = Config { accel_range: a, accel_odr: o, gyro_range: g, gyro_odr: o, drdy_on_int1: drdy };
                    let mut d = Asm330::new(Sim::new());
                    assert_eq!(d.init(cfg, &mut NoDelay), Ok(()), "{cfg:?}");
                    assert_eq!(d.config(), cfg);
                    let s = d.release();
                    assert_eq!(s.regs[usize::from(reg::CTRL1_XL)], cfg.ctrl1_xl());
                    assert_eq!(s.regs[usize::from(reg::CTRL2_G)], cfg.ctrl2_g());
                    assert_eq!(s.regs[usize::from(reg::INT1_CTRL)], if drdy { 3 } else { 0 });
                }
            }
        }
    }
}

#[test]
fn wrong_device_is_rejected_before_any_write() {
    let mut sim = Sim::new();
    sim.who = 0x6A;
    let (d, r) = init_default(sim);
    assert_eq!(r, Err(Error::WrongDevice(0x6A)));
    assert!(d.release().log.iter().all(|a| matches!(a, Access::Read { .. })), "must not write to an unknown device");
}

#[test]
fn reset_that_completes_slowly_is_waited_for() {
    let mut sim = Sim::new();
    sim.reset_polls_needed = 15;
    let (_, r) = init_default(sim);
    assert_eq!(r, Ok(()));
}

#[test]
fn reset_that_never_completes_times_out() {
    let mut sim = Sim::new();
    sim.reset_polls_needed = 1000;
    let (d, r) = init_default(sim);
    assert_eq!(r, Err(Error::ResetTimeout));
    // nothing was configured
    assert_eq!(d.release().regs[usize::from(reg::CTRL1_XL)], 0);
}

#[test]
fn readback_mismatch_is_reported_with_register_and_values() {
    // CTRL1_XL stuck: bit 6 cannot be set, so 0x64 (ODR 0b0110) reads back as 0x24 (ODR 0b0010)
    let mut sim = Sim::new();
    sim.stuck = Some((reg::CTRL1_XL, !0x40));
    let (_, r) = init_default(sim);
    assert_eq!(r, Err(Error::ConfigMismatch { reg: reg::CTRL1_XL, expected: 0x64, got: 0x24 }));
}

#[test]
fn mismatch_in_any_configuration_register_is_caught() {
    for r in [reg::CTRL3_C, reg::CTRL4_C, reg::INT1_CTRL, reg::CTRL2_G, reg::CTRL1_XL] {
        let mut sim = Sim::new();
        sim.stuck = Some((r, 0x00)); // register ignores writes
        let (_, res) = init_default(sim);
        assert!(matches!(res, Err(Error::ConfigMismatch { reg, .. }) if reg == r), "{r:#x}: {res:?}");
    }
}

#[test]
fn every_single_bus_fault_is_reported_and_the_driver_recovers() {
    // Count transactions in a clean run.
    let (clean, r) = init_default(Sim::new());
    assert_eq!(r, Ok(()));
    let total = clean.release().transactions;
    for n in 0..total {
        let mut sim = Sim::new();
        sim.fail_on = Some(n);
        let mut d = Asm330::new(sim);
        let r = d.init(Config::default(), &mut NoDelay);
        assert_eq!(r, Err(Error::Spi(BusFault)), "fault at transaction {n}");
        // Remove the fault and try again on the same hardware: it must work.
        let mut sim = d.release();
        sim.fail_on = None;
        let mut d = Asm330::new(sim);
        assert_eq!(d.init(Config::default(), &mut NoDelay), Ok(()), "retry after fault at {n}");
    }
}

#[test]
fn read_raw_parses_the_burst_little_endian_and_signed() {
    let raw = Raw { temp: -300, gyro: [1, -2, 0x7FFF], accel: [i16::MIN, 0x1234, -1] };
    let (mut d, r) = init_default(Sim::new());
    assert_eq!(r, Ok(()));
    let mut sim = d.release();
    sim.set_data(&raw);
    d = Asm330::new(sim);
    assert_eq!(d.read_raw(), Ok(raw));
    let sim = d.release();
    assert_eq!(sim.log.last(), Some(&Access::Read { addr: reg::OUT_TEMP_L, len: 14 }));
}

#[test]
fn status_flags_decode() {
    let (d, _) = init_default(Sim::new());
    let mut sim = d.release();
    // STATUS_REG is read-only in the simulator, so patch the backing array directly.
    for (byte, (xl, g, t)) in [
        (0b000, (false, false, false)),
        (0b001, (true, false, false)),
        (0b010, (false, true, false)),
        (0b100, (false, false, true)),
        (0b111, (true, true, true)),
    ] {
        sim.regs[usize::from(reg::STATUS_REG)] = byte;
        let mut d = Asm330::new(sim);
        let s = d.status().unwrap();
        assert_eq!((s.accel_ready, s.gyro_ready, s.temp_ready), (xl, g, t));
        sim = d.release();
    }
}

#[test]
fn scaling_known_values() {
    // +1 g at +/-2 g range is 16393 counts: 16393 * 61 = 999 973 ug
    let raw = Raw { temp: 0, gyro: [0; 3], accel: [16_393, 0, -16_393] };
    let s = raw.scale(AccelRange::G2, GyroRange::Dps250);
    assert_eq!(s.accel_micro_g, [999_973, 0, -999_973]);
    // 1000 counts at 2000 dps = 70 000 mdps
    let raw = Raw { temp: 0, gyro: [1000, -1000, 0], accel: [0; 3] };
    let s = raw.scale(AccelRange::G2, GyroRange::Dps2000);
    assert_eq!(s.gyro_mdps, [70_000, -70_000, 0]);
    // 125 dps range: 1000 counts * 4.375 mdps = 4375 mdps
    let s = raw.scale(AccelRange::G2, GyroRange::Dps125);
    assert_eq!(s.gyro_mdps, [4_375, -4_375, 0]);
}

#[test]
fn temperature_conversion() {
    let t = |raw: i16| Raw { temp: raw, ..Raw::default() }.scale(AccelRange::G2, GyroRange::Dps250).temp_centi_c;
    assert_eq!(t(0), 2500); // 0 LSB at 25 C
    assert_eq!(t(256), 2600);
    assert_eq!(t(-10_240), -1500); // -40 counts/degree * 256 -> -15 C
    assert_eq!(t(i16::MAX), 32_767 * 100 / 256 + 2500);
    assert_eq!(t(i16::MIN), -32_768 * 100 / 256 + 2500);
}

#[test]
fn scaled_values_never_saturate_for_any_range() {
    let extremes = Raw { temp: i16::MIN, gyro: [i16::MIN, i16::MAX, 0], accel: [i16::MIN, i16::MAX, 0] };
    for a in [AccelRange::G2, AccelRange::G4, AccelRange::G8, AccelRange::G16] {
        for g in [
            GyroRange::Dps125,
            GyroRange::Dps250,
            GyroRange::Dps500,
            GyroRange::Dps1000,
            GyroRange::Dps2000,
            GyroRange::Dps4000,
        ] {
            let s = extremes.scale(a, g);
            assert_eq!(i64::from(s.accel_micro_g[0]), i64::from(i16::MIN) * a.micro_g_per_lsb());
            assert_eq!(i64::from(s.accel_micro_g[1]), i64::from(i16::MAX) * a.micro_g_per_lsb());
            assert_eq!(i64::from(s.gyro_mdps[0]), i64::from(i16::MIN) * g.micro_dps_per_lsb() / 1000);
            assert_eq!(i64::from(s.gyro_mdps[1]), i64::from(i16::MAX) * g.micro_dps_per_lsb() / 1000);
        }
    }
}

proptest! {
    #[test]
    fn scaling_matches_i128_reference(
        temp in any::<i16>(),
        gyro in prop::array::uniform3(any::<i16>()),
        accel in prop::array::uniform3(any::<i16>()),
        ai in 0_usize..4, gi in 0_usize..6,
    ) {
        let a = [AccelRange::G2, AccelRange::G4, AccelRange::G8, AccelRange::G16][ai];
        let g = [GyroRange::Dps125, GyroRange::Dps250, GyroRange::Dps500, GyroRange::Dps1000, GyroRange::Dps2000, GyroRange::Dps4000][gi];
        let s = Raw { temp, gyro, accel }.scale(a, g);
        for i in 0..3 {
            prop_assert_eq!(i128::from(s.accel_micro_g[i]), i128::from(accel[i]) * i128::from(a.micro_g_per_lsb()));
            prop_assert_eq!(i128::from(s.gyro_mdps[i]), i128::from(gyro[i]) * i128::from(g.micro_dps_per_lsb()) / 1000);
        }
        prop_assert_eq!(i128::from(s.temp_centi_c), i128::from(temp) * 100 / 256 + 2500);
    }

    #[test]
    fn burst_roundtrip(temp in any::<i16>(), gyro in prop::array::uniform3(any::<i16>()), accel in prop::array::uniform3(any::<i16>())) {
        let raw = Raw { temp, gyro, accel };
        let mut sim = Sim::new();
        sim.set_data(&raw);
        let mut d = Asm330::new(sim);
        // read without init: IF_INC defaults to 1, matching the datasheet
        prop_assert_eq!(d.read_raw(), Ok(raw));
    }
}
