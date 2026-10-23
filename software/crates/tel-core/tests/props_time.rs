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
//! Property tests for time handling.

use proptest::prelude::*;
use tel_core::time::{unix_ms, TimerExtender};
use tel_nmea::{Date, UtcTime};

proptest! {
    /// Reading the counter at arbitrary intervals shorter than half a period
    /// reconstructs the true cumulative time exactly (32-bit).
    #[test]
    fn extender32_reconstructs_true_time(steps in prop::collection::vec(1_u64..0x7FFF_FFFF, 1..200), start in any::<u32>()) {
        let mut e = TimerExtender::<32>::new();
        let mut truth: u64 = u64::from(start);
        prop_assert_eq!(e.extend(start), truth);
        for s in steps {
            truth += s;
            prop_assert_eq!(e.extend((truth & 0xFFFF_FFFF) as u32), truth);
        }
    }

    #[test]
    fn extender16_reconstructs_true_time(steps in prop::collection::vec(1_u64..0x7FFF, 1..400), start in any::<u16>()) {
        let mut e = TimerExtender::<16>::new();
        let mut truth: u64 = u64::from(start);
        prop_assert_eq!(e.extend(u32::from(start)), truth);
        for s in steps {
            truth += s;
            prop_assert_eq!(e.extend((truth & 0xFFFF) as u32), truth);
        }
    }

    /// Arbitrary (even garbage) readings never panic and never move time backwards.
    #[test]
    fn extender_is_total_and_monotonic(readings in prop::collection::vec(any::<u32>(), 1..300)) {
        let mut e = TimerExtender::<32>::new();
        let mut prev = 0_u64;
        for r in readings {
            let v = e.extend(r);
            prop_assert!(v >= prev, "time went backwards: {} -> {}", prev, v);
            prev = v;
        }
    }

    /// Unix time agrees with a day-by-day reference built independently.
    #[test]
    fn unix_ms_matches_a_reference_calendar(
        year in 2000_u16..2100, month in 1_u8..13, day in 1_u8..32,
        h in 0_u8..24, m in 0_u8..60, s in 0_u8..60, ms in 0_u16..1000,
    ) {
        // reference: count days by walking years and months
        fn leap(y: u32) -> bool { (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 }
        let dim = |y: u32, mo: u8| -> u32 {
            match mo { 1|3|5|7|8|10|12 => 31, 4|6|9|11 => 30, _ => if leap(y) { 29 } else { 28 } }
        };
        let valid = u32::from(day) <= dim(u32::from(year), month);
        let got = unix_ms(Date { day, month, year }, UtcTime { hour: h, minute: m, second: s, millis: ms });
        prop_assert_eq!(got.is_some(), valid);
        if valid {
            let mut days: i64 = 0;
            for y in 1970..u32::from(year) { days += if leap(y) { 366 } else { 365 }; }
            for mo in 1..month { days += i64::from(dim(u32::from(year), mo)); }
            days += i64::from(day) - 1;
            let secs = days * 86_400 + i64::from(h) * 3600 + i64::from(m) * 60 + i64::from(s);
            prop_assert_eq!(got.unwrap(), secs * 1000 + i64::from(ms));
        }
    }
}
