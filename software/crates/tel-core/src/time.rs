//! Time helpers: extending a hardware counter to 64 bits and converting UTC
//! calendar fields to Unix milliseconds. Pure, checked integer arithmetic.

use tel_nmea::{Date, UtcTime};

/// Extends a free-running hardware counter of `BITS` bits (at 1 MHz, so one
/// count is one microsecond) to a monotonic 64-bit microsecond value.
///
/// It must be called with a fresh reading at least once per half counter
/// period (about 35 minutes for 32 bits, 32 ms for 16 bits); otherwise a wrap
/// could be missed. A reading that appears to go backwards by less than half a
/// period is treated as a stale reading and ignored (the previous value is
/// returned), which keeps the output monotonic.
#[derive(Clone, Copy, Debug)]
pub struct TimerExtender<const BITS: u32> {
    last_raw: u32,
    high: u64,
    primed: bool,
}

impl<const BITS: u32> Default for TimerExtender<BITS> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const BITS: u32> TimerExtender<BITS> {
    const MASK: u64 = if BITS >= 32 { 0xFFFF_FFFF } else { (1_u64 << BITS) - 1 };
    const PERIOD: u64 = Self::MASK + 1;

    /// A new extender; the first reading defines the origin.
    pub const fn new() -> Self {
        Self { last_raw: 0, high: 0, primed: false }
    }

    /// Convert a raw counter reading (only the low `BITS` bits are used).
    pub fn extend(&mut self, raw: u32) -> u64 {
        let raw = u64::from(raw) & Self::MASK;
        let last = u64::from(self.last_raw);
        if !self.primed {
            self.primed = true;
            self.last_raw = u32::try_from(raw).unwrap_or(0);
            return raw;
        }
        // Forward distance modulo the period.
        let fwd = raw.wrapping_sub(last) & Self::MASK;
        if fwd >= Self::PERIOD / 2 {
            // "Backwards" (stale reading): keep the previous value.
            return self.high.saturating_add(last);
        }
        if raw < last {
            self.high = self.high.saturating_add(Self::PERIOD);
        }
        self.last_raw = u32::try_from(raw).unwrap_or(0);
        self.high.saturating_add(raw)
    }
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: u8) -> Option<u8> {
    Some(match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => return None,
    })
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Hinnant's algorithm).
fn days_from_civil(y: i64, m: u8, d: u8) -> Option<i64> {
    let m = i64::from(m);
    let d = i64::from(d);
    let y = if m <= 2 { y.checked_sub(1)? } else { y };
    let era = y.div_euclid(400);
    let yoe = y.checked_sub(era.checked_mul(400)?)?;
    let mp = if m > 2 { m.checked_sub(3)? } else { m.checked_add(9)? };
    let doy = (153_i64.checked_mul(mp)?.checked_add(2)?.checked_div(5)?).checked_add(d.checked_sub(1)?)?;
    let doe =
        yoe.checked_mul(365)?.checked_add(yoe.checked_div(4)?)?.checked_sub(yoe.checked_div(100)?)?.checked_add(doy)?;
    era.checked_mul(146_097)?.checked_add(doe)?.checked_sub(719_468)
}

/// Convert a UTC date and time of day to Unix milliseconds. Returns `None`
/// for an impossible date (for example 31 June). A leap second (`second == 60`)
/// maps to the instant of the following minute.
pub fn unix_ms(date: Date, time: UtcTime) -> Option<i64> {
    let y = i64::from(date.year);
    if date.day == 0 || date.day > days_in_month(y, date.month)? {
        return None;
    }
    let days = days_from_civil(y, date.month, date.day)?;
    let secs = days
        .checked_mul(86_400)?
        .checked_add(i64::from(time.hour).checked_mul(3600)?)?
        .checked_add(i64::from(time.minute).checked_mul(60)?)?
        .checked_add(i64::from(time.second))?;
    secs.checked_mul(1000)?.checked_add(i64::from(time.millis))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: u16, m: u8, day: u8) -> Date {
        Date { day, month: m, year: y }
    }
    fn t(h: u8, m: u8, s: u8, ms: u16) -> UtcTime {
        UtcTime { hour: h, minute: m, second: s, millis: ms }
    }

    // Reference values computed independently with Python's datetime.
    #[test]
    fn unix_ms_matches_python_reference() {
        let cases = [
            (d(2000, 1, 1), t(0, 0, 0, 0), 946_684_800_000_i64),
            (d(2022, 1, 14), t(9, 34, 23, 0), 1_642_152_863_000),
            (d(2024, 2, 29), t(12, 0, 0, 500), 1_709_208_000_500),
            (d(2099, 12, 31), t(23, 59, 59, 999), 4_102_444_799_999),
            (d(2100, 3, 1), t(0, 0, 0, 0), 4_107_542_400_000),
            (d(2038, 1, 19), t(3, 14, 8, 0), 2_147_483_648_000),
            (d(2025, 10, 23), t(14, 25, 6, 250), 1_761_229_506_250),
            (d(2000, 2, 29), t(0, 0, 0, 0), 951_782_400_000),
            (d(2023, 12, 31), t(23, 59, 59, 0), 1_704_067_199_000),
        ];
        for (date, time, want) in cases {
            assert_eq!(unix_ms(date, time), Some(want), "{date:?} {time:?}");
        }
    }

    #[test]
    fn impossible_dates_are_rejected() {
        assert_eq!(unix_ms(d(2023, 2, 29), t(0, 0, 0, 0)), None); // not a leap year
        assert_eq!(unix_ms(d(2100, 2, 29), t(0, 0, 0, 0)), None); // century rule
        assert_eq!(unix_ms(d(2024, 4, 31), t(0, 0, 0, 0)), None);
        assert_eq!(unix_ms(d(2024, 13, 1), t(0, 0, 0, 0)), None);
        assert_eq!(unix_ms(d(2024, 0, 1), t(0, 0, 0, 0)), None);
        assert_eq!(unix_ms(d(2024, 1, 0), t(0, 0, 0, 0)), None);
        assert!(unix_ms(d(2000, 2, 29), t(0, 0, 0, 0)).is_some()); // 400-year rule leap
        assert!(unix_ms(d(2024, 2, 29), t(0, 0, 0, 0)).is_some());
    }

    #[test]
    fn leap_second_maps_to_the_next_minute() {
        let a = unix_ms(d(2016, 12, 31), t(23, 59, 60, 0)).unwrap();
        let b = unix_ms(d(2017, 1, 1), t(0, 0, 0, 0)).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn every_day_from_2000_to_2100_advances_by_exactly_one_day() {
        let mut prev = unix_ms(d(2000, 1, 1), t(0, 0, 0, 0)).unwrap();
        for year in 2000..=2100_u16 {
            for month in 1..=12_u8 {
                let dim = days_in_month(i64::from(year), month).unwrap();
                for day in 1..=dim {
                    if year == 2000 && month == 1 && day == 1 {
                        continue;
                    }
                    let cur = unix_ms(d(year, month, day), t(0, 0, 0, 0)).unwrap();
                    assert_eq!(cur - prev, 86_400_000, "{year}-{month}-{day}");
                    prev = cur;
                }
            }
        }
    }

    #[test]
    fn extender_32_bit_handles_wraps() {
        // Every step is below half the period (the documented contract).
        let mut e = TimerExtender::<32>::new();
        assert_eq!(e.extend(100), 100);
        assert_eq!(e.extend(0x4000_0000), 0x4000_0000);
        assert_eq!(e.extend(0x7FFF_0000), 0x7FFF_0000);
        assert_eq!(e.extend(0xBFFF_0000), 0xBFFF_0000);
        assert_eq!(e.extend(0xFFFF_FF00), 0xFFFF_FF00);
        assert_eq!(e.extend(0x10), 0x1_0000_0010, "wrapped once");
        assert_eq!(e.extend(0x4000_0000), 0x1_4000_0000);
        // a reading far "behind" the last one is a stale value: time must not go backwards
        assert_eq!(e.extend(0xFFFF_FF00), 0x1_4000_0000);
        assert_eq!(e.extend(0x4000_0100), 0x1_4000_0100);
    }

    #[test]
    fn extender_16_bit_handles_wraps_and_stale_readings() {
        let mut e = TimerExtender::<16>::new();
        assert_eq!(e.extend(0xFFF0), 0xFFF0);
        assert_eq!(e.extend(0x0010), 0x1_0010, "wrap");
        // a reading that is slightly behind (stale) must not move time backwards
        assert_eq!(e.extend(0x0008), 0x1_0010);
        assert_eq!(e.extend(0x0020), 0x1_0020);
    }

    #[test]
    fn extender_ignores_bits_above_the_counter_width() {
        let mut e = TimerExtender::<16>::new();
        assert_eq!(e.extend(0xABCD_0042), 0x42);
        assert_eq!(e.extend(0xFFFF_0050), 0x50);
    }
}
