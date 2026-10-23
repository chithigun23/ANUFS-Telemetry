//! Decision logic for the board bring-up self-test.
//!
//! The firmware bring-up application talks to the hardware and feeds raw
//! observations into the functions here, which decide pass or fail and build a
//! report. Keeping the decisions in a crate with no hardware access means every
//! threshold is tested on the PC, and a mistake in a threshold shows up in a
//! test instead of as a confusing result on the bench.
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

use core::fmt::{self, Write};
use tel_asm330::Scaled;
use tel_core::supervisor::{LedColor, LED_COUNT};
use tel_nmea::{FixQuality, Message};

/// Result of one check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The feature works.
    Pass,
    /// The feature is broken or missing.
    Fail,
    /// Not run (for example no card inserted); says nothing about the hardware.
    Skip,
    /// Informational reading; neither good nor bad.
    Info,
}

/// Which status LED a check reports on (same order as the supervisor).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    /// Clocks, buttons, watchdog, power.
    System,
    /// GNSS modules, UART, PPS, reset line.
    Gnss,
    /// SD slot 1.
    Sd1,
    /// SD slot 2.
    Sd2,
    /// IMU, CAN buses, LED drivers.
    Sensors,
}

impl Group {
    /// Index of the LED for this group.
    pub const fn led(self) -> usize {
        match self {
            Self::System => 0,
            Self::Gnss => 1,
            Self::Sd1 => 2,
            Self::Sd2 => 3,
            Self::Sensors => 4,
        }
    }
}

/// A short text that truncates instead of failing when it is too long.
#[derive(Clone, Copy)]
pub struct Detail {
    buf: [u8; Self::CAP],
    len: usize,
}

impl Detail {
    /// Capacity in bytes.
    pub const CAP: usize = 110;

    /// Empty text.
    pub const fn new() -> Self {
        Self { buf: [0; Self::CAP], len: 0 }
    }

    /// Build from a string, truncating on a character boundary if needed.
    pub fn text(s: &str) -> Self {
        let mut d = Self::new();
        let _ = d.write_str(s);
        d
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        self.buf.get(..self.len).and_then(|b| core::str::from_utf8(b).ok()).unwrap_or("")
    }
}

impl Default for Detail {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Detail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

impl PartialEq for Detail {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for Detail {}

impl Write for Detail {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for ch in s.chars() {
            let n = ch.len_utf8();
            let end = match self.len.checked_add(n) {
                Some(e) if e <= Self::CAP => e,
                _ => return Ok(()), // full: silently drop the rest
            };
            let mut tmp = [0_u8; 4];
            let enc = ch.encode_utf8(&mut tmp).as_bytes();
            if let Some(dst) = self.buf.get_mut(self.len..end) {
                dst.copy_from_slice(enc);
            }
            self.len = end;
        }
        Ok(())
    }
}

/// Format into a [`Detail`].
#[macro_export]
macro_rules! detail {
    ($($arg:tt)*) => {{
        let mut d = $crate::Detail::new();
        let _ = core::fmt::Write::write_fmt(&mut d, core::format_args!($($arg)*));
        d
    }};
}

/// One line of the report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Check {
    /// LED group.
    pub group: Group,
    /// Short name, for example `imu.whoami`.
    pub name: &'static str,
    /// Result.
    pub outcome: Outcome,
    /// Human readable detail.
    pub detail: Detail,
}

/// Counts of each outcome.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Passed checks.
    pub pass: u16,
    /// Failed checks.
    pub fail: u16,
    /// Skipped checks.
    pub skip: u16,
    /// Informational readings.
    pub info: u16,
}

/// A fixed-capacity report of up to `N` checks.
#[derive(Clone, Debug)]
pub struct Report<const N: usize> {
    checks: [Option<Check>; N],
    len: usize,
    dropped: u16,
}

impl<const N: usize> Default for Report<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Report<N> {
    /// An empty report.
    pub const fn new() -> Self {
        Self { checks: [None; N], len: 0, dropped: 0 }
    }

    /// Add a check. If the report is full the check is counted as dropped and
    /// the report is treated as failed, so nothing can be silently lost.
    pub fn push(&mut self, group: Group, name: &'static str, outcome: Outcome, detail: Detail) -> Option<&Check> {
        match self.checks.get_mut(self.len) {
            Some(slot) => {
                *slot = Some(Check { group, name, outcome, detail });
                self.len = self.len.saturating_add(1);
                slot.as_ref()
            }
            None => {
                self.dropped = self.dropped.saturating_add(1);
                None
            }
        }
    }

    /// Checks recorded so far.
    pub fn iter(&self) -> impl Iterator<Item = &Check> {
        self.checks.iter().take(self.len).flatten()
    }

    /// Number of checks that did not fit.
    pub const fn dropped(&self) -> u16 {
        self.dropped
    }

    /// Counts of each outcome.
    pub fn summary(&self) -> Summary {
        let mut s = Summary::default();
        for c in self.iter() {
            let slot = match c.outcome {
                Outcome::Pass => &mut s.pass,
                Outcome::Fail => &mut s.fail,
                Outcome::Skip => &mut s.skip,
                Outcome::Info => &mut s.info,
            };
            *slot = slot.saturating_add(1);
        }
        s
    }

    /// Overall verdict: fail if anything failed or any check was dropped, pass if
    /// something passed and nothing failed, otherwise skip.
    pub fn overall(&self) -> Outcome {
        let s = self.summary();
        if s.fail > 0 || self.dropped > 0 {
            Outcome::Fail
        } else if s.pass > 0 {
            Outcome::Pass
        } else {
            Outcome::Skip
        }
    }

    /// Verdict for one LED group.
    pub fn group_outcome(&self, group: Group) -> Outcome {
        let (mut pass, mut fail) = (false, false);
        for c in self.iter().filter(|c| c.group == group) {
            match c.outcome {
                Outcome::Pass => pass = true,
                Outcome::Fail => fail = true,
                Outcome::Skip | Outcome::Info => {}
            }
        }
        if fail {
            Outcome::Fail
        } else if pass {
            Outcome::Pass
        } else {
            Outcome::Skip
        }
    }

    /// LED colours: green all passed, red something failed, off nothing ran.
    pub fn led_colors(&self) -> [LedColor; LED_COUNT] {
        let color = |g: Group| match self.group_outcome(g) {
            Outcome::Pass => LedColor::Green,
            Outcome::Fail => LedColor::Red,
            Outcome::Skip | Outcome::Info => LedColor::Off,
        };
        let mut out = [LedColor::Off; LED_COUNT];
        for g in [Group::System, Group::Gnss, Group::Sd1, Group::Sd2, Group::Sensors] {
            if let Some(slot) = out.get_mut(g.led()) {
                *slot = color(g);
            }
        }
        // A dropped check means the report is incomplete: make the system LED red.
        if self.dropped > 0 {
            if let Some(slot) = out.get_mut(Group::System.led()) {
                *slot = LedColor::Red;
            }
        }
        out
    }
}

// ------------------------------------------------------------------ IMU

/// Mean, minimum and maximum of one axis.
#[derive(Clone, Copy)]
struct AxisStats {
    mean: i64,
    spread: i64,
}

fn axis_stats(samples: &[Scaled], pick: impl Fn(&Scaled) -> i32) -> AxisStats {
    let count = i64::try_from(samples.len()).unwrap_or(i64::MAX).max(1);
    let (mut sum, mut min, mut max) = (0_i64, i64::MAX, i64::MIN);
    for s in samples {
        let v = i64::from(pick(s));
        sum = sum.saturating_add(v);
        min = min.min(v);
        max = max.max(v);
    }
    AxisStats { mean: sum.checked_div(count).unwrap_or(0), spread: max.saturating_sub(min).max(0) }
}

fn get(v: &[i32; 3], i: usize) -> i32 {
    v.get(i).copied().unwrap_or(0)
}

/// Judge a set of samples taken with the board sitting still.
///
/// Pass when the acceleration magnitude is close to 1 g (any orientation),
/// the readings are steady, the gyroscope offset is small and the die
/// temperature is plausible.
pub fn imu_static(samples: &[Scaled]) -> (Outcome, Detail) {
    let n = samples.len();
    if n < 8 {
        return (Outcome::Fail, detail!("only {n} samples, need at least 8"));
    }
    let ax = axis_stats(samples, |s| get(&s.accel_micro_g, 0));
    let ay = axis_stats(samples, |s| get(&s.accel_micro_g, 1));
    let az = axis_stats(samples, |s| get(&s.accel_micro_g, 2));
    let gx = axis_stats(samples, |s| get(&s.gyro_mdps, 0));
    let gy = axis_stats(samples, |s| get(&s.gyro_mdps, 1));
    let gz = axis_stats(samples, |s| get(&s.gyro_mdps, 2));
    let temp = axis_stats(samples, |s| s.temp_centi_c);

    let sq = |v: i64| i128::from(v).saturating_mul(i128::from(v));
    let mag2 = sq(ax.mean).saturating_add(sq(ay.mean)).saturating_add(sq(az.mean));
    // magnitude between 0.85 g and 1.15 g, in micro-g squared
    let lo = 850_000_i128 * 850_000;
    let hi = 1_150_000_i128 * 1_150_000;
    let spread = ax.spread.max(ay.spread).max(az.spread);
    let offset = gx.mean.abs().max(gy.mean.abs()).max(gz.mean.abs());

    if mag2 < lo || mag2 > hi {
        return (
            Outcome::Fail,
            detail!("|a| outside 0.85..1.15 g: mean ({}, {}, {}) mg", ax.mean / 1000, ay.mean / 1000, az.mean / 1000),
        );
    }
    if spread > 150_000 {
        return (Outcome::Fail, detail!("readings not steady: spread {} mg (is the board moving?)", spread / 1000));
    }
    if offset > 5_000 {
        return (Outcome::Fail, detail!("gyro offset {offset} mdps too large (is the board moving?)"));
    }
    if !(-1_000..=8_000).contains(&temp.mean) {
        return (Outcome::Fail, detail!("die temperature {} cC implausible", temp.mean));
    }
    (
        Outcome::Pass,
        detail!(
            "a=({}, {}, {}) mg, gyro offset max {} mdps, {}.{:02} C, {} samples",
            ax.mean / 1000,
            ay.mean / 1000,
            az.mean / 1000,
            offset,
            temp.mean / 100,
            (temp.mean % 100).abs(),
            n
        ),
    )
}

// ------------------------------------------------------------------ GNSS

/// What was seen on a GNSS UART during a listening window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GnssTally {
    /// Baud rate used.
    pub baud: u32,
    /// Sentences with a good checksum.
    pub frames_ok: u32,
    /// Sentences with a bad checksum.
    pub checksum_errors: u32,
    /// RMC sentences.
    pub rmc: u32,
    /// GGA sentences.
    pub gga: u32,
    /// `$PQTMTAR` sentences.
    pub tar: u32,
    /// Best fix quality seen (0 = none).
    pub best_quality: u8,
    /// Largest satellite count seen in GGA.
    pub max_sv: u8,
    /// An RMC with status A (valid) was seen.
    pub rmc_valid: bool,
}

impl GnssTally {
    /// Account for one decoded message.
    pub fn feed(&mut self, msg: &Message) {
        self.frames_ok = self.frames_ok.saturating_add(1);
        match msg {
            Message::Rmc(r) => {
                self.rmc = self.rmc.saturating_add(1);
                self.rmc_valid |= r.valid;
            }
            Message::Gga(g) => {
                self.gga = self.gga.saturating_add(1);
                let q = match g.quality {
                    FixQuality::Invalid => 0,
                    other => other.as_u8(),
                };
                self.best_quality = self.best_quality.max(q);
                self.max_sv = self.max_sv.max(g.num_sv);
            }
            Message::Tar(_) => self.tar = self.tar.saturating_add(1),
            _ => {}
        }
    }

    /// Account for a sentence that passed its checksum but is of a kind (or
    /// shape) the decoder does not use.
    pub fn feed_unparsed(&mut self) {
        self.frames_ok = self.frames_ok.saturating_add(1);
    }

    /// Account for a sentence that failed its checksum.
    pub fn feed_error(&mut self) {
        self.checksum_errors = self.checksum_errors.saturating_add(1);
    }
}

/// Is the NMEA stream alive and clean? A missing satellite fix is not a failure
/// (indoors, or antenna not connected): it is reported separately.
pub fn gnss_stream(t: &GnssTally) -> (Outcome, Detail) {
    if t.frames_ok == 0 && t.checksum_errors == 0 {
        return (Outcome::Fail, detail!("no data at {} baud (check UART4, level shifter U12, module power)", t.baud));
    }
    if t.frames_ok < 5 {
        return (
            Outcome::Fail,
            detail!(
                "only {} good and {} bad sentences at {} baud (wrong baud?)",
                t.frames_ok,
                t.checksum_errors,
                t.baud
            ),
        );
    }
    // more than 5 percent bad sentences means a wiring or baud problem
    if u64::from(t.checksum_errors).saturating_mul(20) > u64::from(t.frames_ok) {
        return (
            Outcome::Fail,
            detail!("{} bad checksums among {} sentences at {} baud", t.checksum_errors, t.frames_ok, t.baud),
        );
    }
    (
        Outcome::Pass,
        detail!(
            "{} sentences ({} RMC, {} GGA, {} PQTMTAR), {} bad, at {} baud",
            t.frames_ok,
            t.rmc,
            t.gga,
            t.tar,
            t.checksum_errors,
            t.baud
        ),
    )
}

/// Report the satellite fix state: informational, never a failure.
pub fn gnss_fix(t: &GnssTally) -> (Outcome, Detail) {
    if t.best_quality == 0 {
        (Outcome::Info, detail!("no fix yet ({} satellites seen; needs an antenna with sky view)", t.max_sv))
    } else {
        (Outcome::Info, detail!("fix quality {} with {} satellites", t.best_quality, t.max_sv))
    }
}

/// Judge pulse-per-second edge counts over a window of `window_ms`.
pub fn pps(pulses: u32, window_ms: u32, expect_fix: bool) -> (Outcome, Detail) {
    // At 1 Hz, a window of N ms should hold about N/1000 pulses (allow one either way).
    let expected = window_ms / 1000;
    if pulses == 0 {
        if expect_fix {
            return (Outcome::Fail, detail!("no PPS pulses in {} ms although the module has a fix", window_ms));
        }
        return (Outcome::Info, detail!("no PPS pulses (the module only pulses with a fix)"));
    }
    if pulses.abs_diff(expected) > 1 {
        return (Outcome::Fail, detail!("{} pulses in {} ms, expected about {}", pulses, window_ms, expected));
    }
    (Outcome::Pass, detail!("{} pulses in {} ms", pulses, window_ms))
}

/// Compare the MCU clock against the GNSS pulse-per-second signal.
///
/// `span_us` is the time between the first and last pulse as measured by the
/// MCU, `intervals` the number of one second intervals between them. The
/// crystal and PLL should agree with the GNSS second to within a few hundred
/// parts per million; a gross error means the wrong clock source or a bad
/// crystal. Reported but not judged when `crystal_in_use` is false.
pub fn clock_vs_pps(span_us: u64, intervals: u32, crystal_in_use: bool) -> (Outcome, Detail) {
    if intervals < 3 {
        return (Outcome::Info, detail!("only {intervals} intervals, need at least 3 for a clock comparison"));
    }
    let expected = u64::from(intervals).saturating_mul(1_000_000);
    let diff = i128::from(span_us).saturating_sub(i128::from(expected));
    let ppm = diff.saturating_mul(1_000_000).checked_div(i128::from(expected)).unwrap_or(0);
    if !crystal_in_use {
        return (
            Outcome::Info,
            detail!("MCU clock is {ppm} ppm off the GNSS second (running on the internal oscillator)"),
        );
    }
    if ppm.abs() > 200 {
        return (
            Outcome::Fail,
            detail!("MCU clock is {ppm} ppm off the GNSS second (limit 200): check crystal Y1 and PLL setup"),
        );
    }
    (Outcome::Pass, detail!("MCU clock is {ppm} ppm from the GNSS second over {intervals} s"))
}

// ------------------------------------------------------------------ CAN, SD, misc

/// Judge a CAN loopback run: every frame sent must come back identical.
pub fn can_loopback(sent: u32, received: u32, mismatches: u32, what: &'static str) -> (Outcome, Detail) {
    if sent == 0 {
        return (Outcome::Fail, detail!("{what}: nothing was sent"));
    }
    if received < sent {
        return (Outcome::Fail, detail!("{what}: sent {sent}, received {received} (check transceiver, TX/RX wiring)"));
    }
    if mismatches > 0 {
        return (Outcome::Fail, detail!("{what}: {mismatches} of {sent} frames came back different"));
    }
    (Outcome::Pass, detail!("{what}: {sent} frames sent and received intact"))
}

/// Judge an SD card identification: it must report a plausible capacity.
pub fn sd_card(blocks: u64) -> (Outcome, Detail) {
    // 2^16 blocks is 32 MB; the largest card we expect to see is far below 2^36 blocks (32 TB).
    if !(65_536..=(1_u64 << 36)).contains(&blocks) {
        return (Outcome::Fail, detail!("implausible capacity of {blocks} blocks"));
    }
    let mb = blocks / 2048;
    (Outcome::Pass, detail!("{} blocks ({} MB)", blocks, mb))
}

/// Does block 0 end with the 0x55 0xAA boot signature (a normally formatted card)?
pub fn boot_signature(block0: &[u8; 512]) -> bool {
    block0.get(510) == Some(&0x55) && block0.get(511) == Some(&0xAA)
}

/// Judge a read-back of a written pattern.
pub fn readback(written: &[u8], read: &[u8]) -> (Outcome, Detail) {
    if written.len() != read.len() {
        return (Outcome::Fail, detail!("length mismatch {} vs {}", written.len(), read.len()));
    }
    match written.iter().zip(read).position(|(a, b)| a != b) {
        None => (Outcome::Pass, detail!("{} bytes written and read back identically", written.len())),
        Some(i) => (Outcome::Fail, detail!("first difference at byte {i}")),
    }
}

/// An I2C address scan result for the two LED drivers.
pub fn led_drivers(u8_present: bool, u9_present: bool) -> (Outcome, Detail) {
    match (u8_present, u9_present) {
        (true, true) => (Outcome::Pass, detail!("both LED drivers answer (0x40 and 0x41)")),
        (false, false) => (Outcome::Fail, detail!("no LED driver answers: check SDA/SCL (PA8, PB4), reset PB5, 3.3 V")),
        (false, true) => (Outcome::Fail, detail!("U8 (0x40) does not answer")),
        (true, false) => (Outcome::Fail, detail!("U9 (0x41) does not answer")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use tel_nmea::{Gga, Rmc, Tar};

    fn still(az_ug: i32) -> Scaled {
        Scaled { accel_micro_g: [10_000, -5_000, az_ug], gyro_mdps: [300, -200, 100], temp_centi_c: 2_450 }
    }

    fn rmc(valid: bool) -> Message {
        Message::Rmc(Rmc {
            time: None,
            valid,
            lat_e9: None,
            lon_e9: None,
            sog_mm_s: None,
            cog_cdeg: None,
            date: None,
            mode: None,
        })
    }

    fn gga(q: FixQuality, sv: u8) -> Message {
        Message::Gga(Gga {
            time: None,
            lat_e9: None,
            lon_e9: None,
            quality: q,
            num_sv: sv,
            hdop_e2: None,
            alt_mm: None,
            geoid_sep_mm: None,
            diff_age_ds: None,
        })
    }

    #[test]
    fn detail_truncates_without_panic() {
        let long = "x".repeat(500);
        let d = Detail::text(&long);
        assert_eq!(d.as_str().len(), Detail::CAP);
        let d = detail!("{}", long);
        assert_eq!(d.as_str().len(), Detail::CAP);
    }

    #[test]
    fn detail_truncates_on_char_boundary() {
        let s = "\u{e9}".repeat(100); // two bytes each
        let d = Detail::text(&s);
        assert!(d.as_str().len() <= Detail::CAP);
        assert_eq!(d.as_str().len() % 2, 0);
        assert!(d.as_str().chars().all(|c| c == '\u{e9}'));
    }

    #[test]
    fn imu_pass_flat() {
        let v = [still(1_000_000); 16];
        let (o, d) = imu_static(&v);
        assert_eq!(o, Outcome::Pass, "{d:?}");
        assert!(d.as_str().contains("24.50 C"), "{d:?}");
    }

    #[test]
    fn imu_pass_any_orientation() {
        let s = Scaled { accel_micro_g: [-990_000, 20_000, 30_000], gyro_mdps: [0, 0, 0], temp_centi_c: 3_000 };
        assert_eq!(imu_static(&[s; 10]).0, Outcome::Pass);
    }

    #[test]
    fn imu_rejects_too_few_samples() {
        assert_eq!(imu_static(&[still(1_000_000); 7]).0, Outcome::Fail);
        assert_eq!(imu_static(&[]).0, Outcome::Fail);
    }

    #[test]
    fn imu_rejects_zero_and_wrong_gravity() {
        assert_eq!(imu_static(&[still(0); 16]).0, Outcome::Fail);
        assert_eq!(imu_static(&[still(2_000_000); 16]).0, Outcome::Fail);
        assert_eq!(imu_static(&[still(840_000); 16]).0, Outcome::Fail);
        assert_eq!(imu_static(&[still(1_160_000); 16]).0, Outcome::Fail);
        assert_eq!(imu_static(&[still(860_000); 16]).0, Outcome::Pass);
        assert_eq!(imu_static(&[still(1_140_000); 16]).0, Outcome::Pass);
    }

    #[test]
    fn imu_rejects_motion_and_gyro_offset() {
        let mut v = [still(1_000_000); 16];
        v[3].accel_micro_g[2] = 1_300_000;
        v[4].accel_micro_g[2] = 700_000;
        assert_eq!(imu_static(&v).0, Outcome::Fail);
        let mut g = [still(1_000_000); 16];
        for s in &mut g {
            s.gyro_mdps = [6_000, 0, 0];
        }
        assert_eq!(imu_static(&g).0, Outcome::Fail);
        for s in &mut g {
            s.gyro_mdps = [-5_000, 0, 0];
        }
        assert_eq!(imu_static(&g).0, Outcome::Pass);
    }

    #[test]
    fn imu_rejects_bad_temperature() {
        let mut v = [still(1_000_000); 16];
        for s in &mut v {
            s.temp_centi_c = 9_000;
        }
        assert_eq!(imu_static(&v).0, Outcome::Fail);
        for s in &mut v {
            s.temp_centi_c = -1_001;
        }
        assert_eq!(imu_static(&v).0, Outcome::Fail);
        for s in &mut v {
            s.temp_centi_c = -1_000;
        }
        assert_eq!(imu_static(&v).0, Outcome::Pass);
    }

    #[test]
    fn imu_extreme_values_do_not_overflow() {
        let s =
            Scaled { accel_micro_g: [i32::MAX, i32::MIN, i32::MAX], gyro_mdps: [i32::MIN; 3], temp_centi_c: i32::MAX };
        assert_eq!(imu_static(&[s; 20]).0, Outcome::Fail);
    }

    proptest! {
        #[test]
        fn imu_never_panics(a in prop::array::uniform3(any::<i32>()), g in prop::array::uniform3(any::<i32>()), t in any::<i32>(), n in 0_usize..40) {
            let s = Scaled { accel_micro_g: a, gyro_mdps: g, temp_centi_c: t };
            let v = std::vec![s; n];
            let _ = imu_static(&v);
        }
    }

    #[test]
    fn gnss_no_data_fails() {
        let t = GnssTally { baud: 115_200, ..GnssTally::default() };
        let (o, d) = gnss_stream(&t);
        assert_eq!(o, Outcome::Fail);
        assert!(d.as_str().contains("115200"));
    }

    #[test]
    fn gnss_garbage_fails() {
        let t = GnssTally { baud: 9_600, checksum_errors: 400, frames_ok: 2, ..GnssTally::default() };
        assert_eq!(gnss_stream(&t).0, Outcome::Fail);
        let t = GnssTally { baud: 9_600, checksum_errors: 50, ..GnssTally::default() };
        assert_eq!(gnss_stream(&t).0, Outcome::Fail);
    }

    #[test]
    fn gnss_error_rate_boundary() {
        let ok = |good, bad| GnssTally { baud: 460_800, frames_ok: good, checksum_errors: bad, ..GnssTally::default() };
        assert_eq!(gnss_stream(&ok(100, 5)).0, Outcome::Pass); // exactly 5 percent
        assert_eq!(gnss_stream(&ok(100, 6)).0, Outcome::Fail);
        assert_eq!(gnss_stream(&ok(5, 0)).0, Outcome::Pass);
        assert_eq!(gnss_stream(&ok(4, 0)).0, Outcome::Fail);
    }

    #[test]
    fn gnss_tally_counts() {
        let mut t = GnssTally { baud: 460_800, ..GnssTally::default() };
        t.feed(&rmc(false));
        assert!(!t.rmc_valid);
        t.feed(&rmc(true));
        t.feed(&gga(FixQuality::Invalid, 3));
        t.feed(&gga(FixQuality::RtkFixed, 17));
        t.feed(&gga(FixQuality::Gps, 9));
        t.feed(&Message::Tar(Tar {
            version: 1,
            time: None,
            quality: FixQuality::Invalid,
            baseline_mm: None,
            pitch_e6: None,
            heading_e6: None,
            acc_pitch_e6: None,
            acc_heading_e6: None,
            used_sv: None,
        }));
        t.feed_error();
        t.feed_unparsed();
        assert_eq!((t.frames_ok, t.rmc, t.gga, t.tar, t.checksum_errors), (7, 2, 3, 1, 1));
        assert!(t.rmc_valid);
        assert_eq!(t.best_quality, 4);
        assert_eq!(t.max_sv, 17);
        assert_eq!(gnss_fix(&t).0, Outcome::Info);
        assert!(gnss_fix(&t).1.as_str().contains("quality 4"));
        assert!(gnss_fix(&GnssTally::default()).1.as_str().contains("no fix"));
    }

    #[test]
    fn pps_windows() {
        assert_eq!(pps(10, 10_000, true).0, Outcome::Pass);
        assert_eq!(pps(9, 10_000, true).0, Outcome::Pass);
        assert_eq!(pps(11, 10_000, true).0, Outcome::Pass);
        assert_eq!(pps(8, 10_000, true).0, Outcome::Fail);
        assert_eq!(pps(12, 10_000, true).0, Outcome::Fail);
        assert_eq!(pps(0, 10_000, true).0, Outcome::Fail);
        assert_eq!(pps(0, 10_000, false).0, Outcome::Info);
        assert_eq!(pps(5_000, 10_000, false).0, Outcome::Fail);
        assert_eq!(pps(u32::MAX, 0, true).0, Outcome::Fail);
    }

    #[test]
    fn clock_vs_pps_cases() {
        // exact
        assert_eq!(clock_vs_pps(9_000_000, 9, true).0, Outcome::Pass);
        // 100 ppm fast and slow pass
        assert_eq!(clock_vs_pps(9_000_900, 9, true).0, Outcome::Pass);
        assert_eq!(clock_vs_pps(8_999_100, 9, true).0, Outcome::Pass);
        // 201 ppm fails, 200 passes
        assert_eq!(clock_vs_pps(10_002_000, 10, true).0, Outcome::Pass);
        assert_eq!(clock_vs_pps(10_002_010, 10, true).0, Outcome::Fail);
        assert_eq!(clock_vs_pps(9_997_990, 10, true).0, Outcome::Fail);
        // a clock that is 1 percent off is only reported when not on the crystal
        assert_eq!(clock_vs_pps(10_100_000, 10, false).0, Outcome::Info);
        assert_eq!(clock_vs_pps(10_100_000, 10, true).0, Outcome::Fail);
        // too few intervals
        assert_eq!(clock_vs_pps(2_000_000, 2, true).0, Outcome::Info);
        assert_eq!(clock_vs_pps(0, 0, true).0, Outcome::Info);
        assert!(clock_vs_pps(10_000_500, 10, true).1.as_str().contains("50 ppm"));
    }

    proptest! {
        #[test]
        fn clock_vs_pps_never_panics(span in any::<u64>(), n in any::<u32>(), c in any::<bool>()) {
            let _ = clock_vs_pps(span, n, c);
        }
    }

    #[test]
    fn can_loopback_cases() {
        assert_eq!(can_loopback(100, 100, 0, "CAN1").0, Outcome::Pass);
        assert_eq!(can_loopback(0, 0, 0, "CAN1").0, Outcome::Fail);
        assert_eq!(can_loopback(100, 99, 0, "CAN1").0, Outcome::Fail);
        assert_eq!(can_loopback(100, 100, 1, "CAN1").0, Outcome::Fail);
        assert!(can_loopback(100, 99, 0, "CAN2").1.as_str().starts_with("CAN2"));
    }

    #[test]
    fn sd_capacity_bounds() {
        assert_eq!(sd_card(0).0, Outcome::Fail);
        assert_eq!(sd_card(65_535).0, Outcome::Fail);
        assert_eq!(sd_card(65_536).0, Outcome::Pass);
        assert_eq!(sd_card(1 << 36).0, Outcome::Pass);
        assert_eq!(sd_card((1 << 36) + 1).0, Outcome::Fail);
        assert_eq!(sd_card(u64::MAX).0, Outcome::Fail);
        assert!(sd_card(62_333_952).1.as_str().contains("30436 MB"));
    }

    #[test]
    fn boot_signature_detected() {
        let mut b = [0_u8; 512];
        assert!(!boot_signature(&b));
        b[510] = 0x55;
        assert!(!boot_signature(&b));
        b[511] = 0xAA;
        assert!(boot_signature(&b));
        b[510] = 0xAA;
        assert!(!boot_signature(&b));
    }

    #[test]
    fn readback_cases() {
        assert_eq!(readback(&[1, 2, 3], &[1, 2, 3]).0, Outcome::Pass);
        assert_eq!(readback(&[], &[]).0, Outcome::Pass);
        let (o, d) = readback(&[1, 2, 3], &[1, 9, 3]);
        assert_eq!(o, Outcome::Fail);
        assert!(d.as_str().contains("byte 1"));
        assert_eq!(readback(&[1, 2], &[1, 2, 3]).0, Outcome::Fail);
    }

    #[test]
    fn led_driver_cases() {
        assert_eq!(led_drivers(true, true).0, Outcome::Pass);
        assert_eq!(led_drivers(true, false).0, Outcome::Fail);
        assert_eq!(led_drivers(false, true).0, Outcome::Fail);
        assert_eq!(led_drivers(false, false).0, Outcome::Fail);
        assert!(led_drivers(true, false).1.as_str().contains("U9"));
        assert!(led_drivers(false, true).1.as_str().contains("U8"));
    }

    #[test]
    fn report_summary_and_overall() {
        let mut r = Report::<8>::new();
        assert_eq!(r.overall(), Outcome::Skip);
        r.push(Group::Sensors, "a", Outcome::Info, Detail::new());
        assert_eq!(r.overall(), Outcome::Skip);
        r.push(Group::Sensors, "b", Outcome::Pass, Detail::new());
        assert_eq!(r.overall(), Outcome::Pass);
        r.push(Group::Sd2, "c", Outcome::Skip, Detail::new());
        assert_eq!(r.overall(), Outcome::Pass);
        r.push(Group::Gnss, "d", Outcome::Fail, Detail::new());
        assert_eq!(r.overall(), Outcome::Fail);
        assert_eq!(r.summary(), Summary { pass: 1, fail: 1, skip: 1, info: 1 });
        assert_eq!(r.iter().count(), 4);
    }

    #[test]
    fn report_leds_follow_groups() {
        let mut r = Report::<8>::new();
        r.push(Group::System, "clk", Outcome::Pass, Detail::new());
        r.push(Group::Gnss, "u10", Outcome::Pass, Detail::new());
        r.push(Group::Gnss, "u13", Outcome::Fail, Detail::new());
        r.push(Group::Sd1, "sd1", Outcome::Skip, Detail::new());
        r.push(Group::Sensors, "imu", Outcome::Pass, Detail::new());
        r.push(Group::Sensors, "pps", Outcome::Info, Detail::new());
        let c = r.led_colors();
        assert_eq!(c[Group::System.led()], LedColor::Green);
        assert_eq!(c[Group::Gnss.led()], LedColor::Red);
        assert_eq!(c[Group::Sd1.led()], LedColor::Off);
        assert_eq!(c[Group::Sd2.led()], LedColor::Off);
        assert_eq!(c[Group::Sensors.led()], LedColor::Green);
    }

    #[test]
    fn group_led_indices_are_distinct_and_in_range() {
        let all = [Group::System, Group::Gnss, Group::Sd1, Group::Sd2, Group::Sensors];
        for (i, a) in all.iter().enumerate() {
            assert!(a.led() < LED_COUNT);
            for b in &all[i + 1..] {
                assert_ne!(a.led(), b.led());
            }
        }
    }

    #[test]
    fn overflowing_report_fails_loudly() {
        let mut r = Report::<2>::new();
        assert!(r.push(Group::Sensors, "a", Outcome::Pass, Detail::new()).is_some());
        assert!(r.push(Group::Sensors, "b", Outcome::Pass, Detail::new()).is_some());
        assert!(r.push(Group::Sensors, "c", Outcome::Pass, Detail::new()).is_none());
        assert_eq!(r.dropped(), 1);
        assert_eq!(r.overall(), Outcome::Fail);
        assert_eq!(r.led_colors()[Group::System.led()], LedColor::Red);
        assert_eq!(r.iter().count(), 2);
    }

    proptest! {
        #[test]
        fn report_never_panics(items in prop::collection::vec((0_u8..5, 0_u8..4), 0..40)) {
            let mut r = Report::<16>::new();
            for (g, o) in items {
                let g = [Group::System, Group::Gnss, Group::Sd1, Group::Sd2, Group::Sensors][usize::from(g)];
                let o = [Outcome::Pass, Outcome::Fail, Outcome::Skip, Outcome::Info][usize::from(o)];
                r.push(g, "x", o, Detail::new());
            }
            let s = r.summary();
            prop_assert!(usize::from(s.pass) + usize::from(s.fail) + usize::from(s.skip) + usize::from(s.info) <= 16);
            let _ = r.led_colors();
        }
    }
}
