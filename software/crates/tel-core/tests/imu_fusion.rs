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
//! IMU window and fusion tests.

use proptest::prelude::*;
use tel_asm330::Scaled;
use tel_core::fusion::{Context, Fusion};
use tel_core::imu_window::{ImuSummary, ImuWindow};
use tel_logfmt::records::flags;
use tel_nmea::{parse, Gga, Message, Rmc, Sentence, Tar};

// ---------------------------------------------------------------- IMU window

fn sample(a: [i32; 3], g: [i32; 3], t: i32) -> Scaled {
    Scaled { accel_micro_g: a, gyro_mdps: g, temp_centi_c: t }
}

#[test]
fn empty_window_has_no_summary() {
    let mut w = ImuWindow::new();
    assert_eq!(w.take(), None);
    assert_eq!(w.count(), 0);
}

#[test]
fn mean_and_peak_of_a_known_window() {
    let mut w = ImuWindow::new();
    w.add(&sample([1_000_000, -2_000_000, 0], [100, -300, 5], 2500));
    w.add(&sample([3_000_000, 500_000, -9_000_000], [-100, 100, 5], 2700));
    assert_eq!(w.count(), 2);
    let s = w.take().unwrap();
    assert_eq!(s.accel_mean_ug, [2_000_000, -750_000, -4_500_000]);
    assert_eq!(s.accel_peak_ug, [3_000_000, 2_000_000, 9_000_000], "peak is the largest magnitude, any sign");
    assert_eq!(s.gyro_mean_mdps, [0, -100, 5]);
    assert_eq!(s.gyro_peak_mdps, [100, 300, 5]);
    assert_eq!(s.temp_centi_c, 2600);
    assert_eq!(s.count, 2);
    // taking resets the window
    assert_eq!(w.count(), 0);
    assert_eq!(w.take(), None);
}

#[test]
fn extreme_values_do_not_overflow() {
    let mut w = ImuWindow::new();
    for _ in 0..1000 {
        w.add(&sample([i32::MAX, i32::MIN, i32::MAX], [i32::MIN, i32::MAX, i32::MIN], i32::MAX));
    }
    let s = w.take().unwrap();
    assert_eq!(s.accel_mean_ug, [i32::MAX, i32::MIN, i32::MAX]);
    assert_eq!(s.accel_peak_ug[1], i32::MAX, "|i32::MIN| clamps rather than wrapping");
    assert_eq!(s.gyro_peak_mdps[0], i32::MAX);
    // the mean temperature does not fit i16 and degrades to 0 rather than wrapping to garbage
    assert_eq!(s.temp_centi_c, 0);
}

#[test]
fn count_saturates() {
    let mut w = ImuWindow::new();
    for _ in 0..70_000 {
        w.add(&sample([1, 1, 1], [1, 1, 1], 0));
    }
    assert_eq!(w.take().unwrap().count, u16::MAX);
}

proptest! {
    #[test]
    fn window_matches_i128_reference(
        samples in prop::collection::vec((prop::array::uniform3(any::<i32>()), prop::array::uniform3(any::<i32>()), -4000_i32..12_000), 1..200)
    ) {
        let mut w = ImuWindow::new();
        for (a, g, t) in &samples {
            w.add(&sample(*a, *g, *t));
        }
        let s = w.take().unwrap();
        let n = samples.len() as i128;
        for axis in 0..3 {
            let sum_a: i128 = samples.iter().map(|x| i128::from(x.0[axis])).sum();
            let sum_g: i128 = samples.iter().map(|x| i128::from(x.1[axis])).sum();
            prop_assert_eq!(i128::from(s.accel_mean_ug[axis]), sum_a / n);
            prop_assert_eq!(i128::from(s.gyro_mean_mdps[axis]), sum_g / n);
            let pa = samples.iter().map(|x| i128::from(x.0[axis]).abs()).max().unwrap();
            let pg = samples.iter().map(|x| i128::from(x.1[axis]).abs()).max().unwrap();
            prop_assert_eq!(i128::from(s.accel_peak_ug[axis]), pa.min(i128::from(i32::MAX)));
            prop_assert_eq!(i128::from(s.gyro_peak_mdps[axis]), pg.min(i128::from(i32::MAX)));
        }
        let st: i128 = samples.iter().map(|x| i128::from(x.2)).sum();
        prop_assert_eq!(i128::from(s.temp_centi_c), st / n);
        prop_assert_eq!(usize::from(s.count), samples.len());
    }
}

// ---------------------------------------------------------------- fusion

const GGA: &[u8] = b"GNGGA,093316.000,3149.332558,N,11706.912570,E,1,28,0.67,54.481,M,-0.337,M,,";
const GGA_RTK: &[u8] = b"GNGGA,093316.000,3149.332558,N,11706.912570,E,4,28,0.67,54.481,M,-0.337,M,1.2,0000";
const RMC: &[u8] = b"GNRMC,093316.000,A,3149.332558,N,11706.912570,E,0.00,237.67,140122,,,A,V";
const RMC_LATER: &[u8] = b"GNRMC,093316.100,A,3149.400000,N,11706.900000,E,12.00,10.00,140122,,,R,V";
const TAR: &[u8] = b"PQTMTAR,1,165034.000,4,,0.860,1.124780,,50.968541,0.254125,,0.012547,21";

fn gga(p: &[u8]) -> Gga {
    match parse(&Sentence::new(p)).unwrap() {
        Message::Gga(g) => g,
        other => panic!("{other:?}"),
    }
}
fn rmc(p: &[u8]) -> Rmc {
    match parse(&Sentence::new(p)).unwrap() {
        Message::Rmc(r) => r,
        other => panic!("{other:?}"),
    }
}
fn tar(p: &[u8]) -> Tar {
    match parse(&Sentence::new(p)).unwrap() {
        Message::Tar(t) => t,
        other => panic!("{other:?}"),
    }
}

fn ctx(now_us: u64) -> Context {
    Context { now_us, ..Context::default() }
}

#[test]
fn full_fresh_sample_same_epoch() {
    let mut f = Fusion::new(100);
    f.on_gga(gga(GGA), 1_000_000);
    f.on_rmc(rmc(RMC), 1_000_500);
    f.on_tar(tar(TAR), 1_001_000);
    let s = f.build(&Context {
        now_us: 1_050_000,
        utc_unix_ms: Some(1_642_152_196_000),
        imu: Some(ImuSummary {
            accel_mean_ug: [1, 2, 3],
            accel_peak_ug: [4, 5, 6],
            gyro_mean_mdps: [7, 8, 9],
            gyro_peak_mdps: [10, 11, 12],
            temp_centi_c: 3100,
            count: 41,
        }),
        flags: flags::CAN1_ALIVE,
        nmea_errors: 3,
        overruns: 2,
        can1_frames: 100,
        can2_frames: 200,
    });
    assert_eq!(s.t_us, 1_050_000);
    assert_eq!(s.utc_unix_ms, 1_642_152_196_000);
    assert_eq!(s.lat_e9, 31_822_209_300);
    assert_eq!(s.lon_e9, 117_115_209_500);
    assert_eq!(s.alt_mm, 54_481);
    assert_eq!((s.fix_quality, s.num_sv, s.hdop_e2), (1, 28, 67));
    assert_eq!(s.diff_age_ds, u16::MAX, "no differential data");
    assert_eq!((s.sog_mm_s, s.cog_cdeg), (0, 23_767));
    assert_eq!(s.heading_e6, 50_968_541);
    assert_eq!(s.pitch_e6, 1_124_780);
    assert_eq!(s.baseline_mm, 860);
    assert_eq!(s.acc_heading_e6, 12_547);
    assert_eq!((s.heading_quality, s.heading_sv), (4, 21));
    assert_eq!(s.accel_mean_ug, [1, 2, 3]);
    assert_eq!(s.imu_count, 41);
    assert_eq!((s.nmea_errors, s.overruns, s.can1_frames, s.can2_frames), (3, 2, 100, 200));
    let want = flags::GNSS_VALID | flags::HEADING_VALID | flags::UTC_VALID | flags::IMU_VALID | flags::CAN1_ALIVE;
    assert_eq!(s.flags, want);
}

#[test]
fn nothing_received_gives_an_all_invalid_sample() {
    let f = Fusion::new(100);
    let s = f.build(&Context { now_us: 5, can1_frames: 9, ..Context::default() });
    assert_eq!(s.flags, 0);
    assert_eq!((s.lat_e9, s.lon_e9, s.heading_e6, s.fix_quality), (0, 0, 0, 0));
    assert_eq!(s.diff_age_ds, u16::MAX);
    assert_eq!(s.can1_frames, 9);
}

#[test]
fn stale_data_is_never_presented_as_current() {
    let mut f = Fusion::new(100); // stale after 2 * 100 + 100 = 300 ms
    f.on_gga(gga(GGA), 1_000_000);
    f.on_rmc(rmc(RMC), 1_000_000);
    f.on_tar(tar(TAR), 1_000_000);
    let at = |now| f.build(&ctx(now)).flags;
    assert_eq!(
        at(1_300_000) & (flags::GNSS_VALID | flags::HEADING_VALID),
        flags::GNSS_VALID | flags::HEADING_VALID,
        "exactly at the limit is fresh"
    );
    assert_eq!(at(1_300_001) & (flags::GNSS_VALID | flags::HEADING_VALID), 0, "one microsecond later is stale");
    let s = f.build(&ctx(2_000_000));
    assert_eq!((s.lat_e9, s.lon_e9, s.alt_mm, s.sog_mm_s, s.heading_e6, s.fix_quality), (0, 0, 0, 0, 0, 0));
}

#[test]
fn staleness_scales_with_the_fix_interval() {
    let mut f = Fusion::new(1000); // 1 Hz: stale after 2100 ms
    f.on_gga(gga(GGA), 0);
    assert!(f.build(&ctx(2_100_000)).flags & flags::GNSS_VALID != 0);
    assert!(f.build(&ctx(2_100_001)).flags & flags::GNSS_VALID == 0);
}

#[test]
fn a_timestamp_in_the_future_is_not_treated_as_stale() {
    let mut f = Fusion::new(100);
    f.on_gga(gga(GGA), 5_000_000);
    assert!(f.build(&ctx(4_000_000)).flags & flags::GNSS_VALID != 0);
}

#[test]
fn invalid_fixes_are_not_reported_valid() {
    let mut f = Fusion::new(100);
    f.on_rmc(rmc(b"GNRMC,000000.000,V,,,,,,,,,,N,V"), 100);
    f.on_gga(gga(b"GNGGA,000000.000,,,,,0,00,99.99,,M,,M,,"), 100);
    f.on_tar(tar(b"PQTMTAR,1,165034.000,0,,,,,,,,,0"), 100);
    let s = f.build(&ctx(200));
    assert_eq!(s.flags, 0);
}

#[test]
fn rmc_alone_provides_position_with_quality_from_the_mode_indicator() {
    for (mode, q) in [("A", 1), ("D", 2), ("F", 5), ("R", 4), ("E", 1), ("M", 1)] {
        let sentence = format!("GNRMC,093316.000,A,3149.332558,N,11706.912570,E,1.00,90.00,140122,,,{mode},V");
        let mut f = Fusion::new(100);
        f.on_rmc(rmc(sentence.as_bytes()), 10);
        let s = f.build(&ctx(20));
        assert!(s.flags & flags::GNSS_VALID != 0, "{mode}");
        assert_eq!(s.fix_quality, q, "mode {mode}");
        assert_eq!(s.sog_mm_s, 514);
        assert_eq!(s.cog_cdeg, 9000);
        assert_eq!(s.alt_mm, 0, "RMC has no altitude");
    }
}

#[test]
fn different_epochs_are_never_mixed_the_newer_message_wins() {
    let mut f = Fusion::new(100);
    f.on_gga(gga(GGA), 1_000_000); // epoch 093316.000
    f.on_rmc(rmc(RMC_LATER), 1_100_000); // epoch 093316.100, newer
    let s = f.build(&ctx(1_150_000));
    // position comes from the newer RMC (mode R = RTK fixed), not from the older GGA
    assert_eq!(s.fix_quality, 4);
    assert_eq!(s.lat_e9, 31_823_333_333);
    assert_eq!(s.alt_mm, 0, "the older GGA's altitude must not leak into the newer epoch");
    assert_eq!(s.sog_mm_s, 6173);
    // and the reverse: GGA newer than RMC
    let mut f = Fusion::new(100);
    f.on_rmc(rmc(RMC), 1_000_000);
    f.on_gga(gga(GGA_RTK), 1_100_000);
    // same time stamp 093316.000 on both: that IS the same epoch and merges
    let s = f.build(&ctx(1_150_000));
    assert_eq!(s.fix_quality, 4);
    assert_eq!(s.alt_mm, 54_481);
    assert_eq!(s.cog_cdeg, 23_767);
    assert_eq!(s.diff_age_ds, 12);
}

#[test]
fn gga_newer_than_rmc_from_a_different_epoch_uses_only_gga() {
    let g_later = b"GNGGA,093316.200,3149.500000,N,11706.800000,E,2,20,1.00,10.000,M,0.000,M,,";
    let mut f = Fusion::new(100);
    f.on_rmc(rmc(RMC), 1_000_000); // epoch .000, older
    f.on_gga(gga(g_later), 1_200_000); // epoch .200, newer
    let s = f.build(&ctx(1_250_000));
    assert_eq!(s.fix_quality, 2);
    assert_eq!(s.alt_mm, 10_000);
    assert_eq!((s.sog_mm_s, s.cog_cdeg), (0, 0), "the older RMC's speed must not be attached to the newer position");
}

#[test]
fn differential_age_is_clamped() {
    let mut f = Fusion::new(100);
    let big = b"GNGGA,093316.000,3149.332558,N,11706.912570,E,4,28,0.67,54.481,M,-0.337,M,99999.0,0000";
    f.on_gga(gga(big), 10);
    assert_eq!(f.build(&ctx(20)).diff_age_ds, u16::MAX - 1, "clamped below the 'absent' marker");
}

#[test]
fn heading_requires_a_valid_quality_and_a_heading_value() {
    let mut f = Fusion::new(100);
    f.on_tar(tar(b"PQTMTAR,1,165034.000,4,,0.860,1.0,,,0.25,,0.012,21"), 10); // heading missing
    assert_eq!(f.build(&ctx(20)).flags & flags::HEADING_VALID, 0);
    f.on_tar(tar(b"PQTMTAR,1,165034.000,0,,0.860,1.0,,50.0,0.25,,0.012,21"), 10); // quality 0
    assert_eq!(f.build(&ctx(20)).flags & flags::HEADING_VALID, 0);
    f.on_tar(tar(TAR), 10);
    assert!(f.build(&ctx(20)).flags & flags::HEADING_VALID != 0);
}

#[test]
fn building_is_deterministic_and_does_not_consume_state() {
    let mut f = Fusion::new(100);
    f.on_gga(gga(GGA), 10);
    f.on_rmc(rmc(RMC), 10);
    let a = f.build(&ctx(20));
    let b = f.build(&ctx(20));
    assert_eq!(a, b);
}

proptest! {
    /// Whatever arrives and whenever, a sample never panics and a clear flag
    /// always means the matching fields are zero.
    #[test]
    fn flags_and_fields_agree(
        gga_t in prop::option::of(0_u64..2_000_000), rmc_t in prop::option::of(0_u64..2_000_000), tar_t in prop::option::of(0_u64..2_000_000),
        now in 0_u64..3_000_000, with_imu in any::<bool>(), with_utc in any::<bool>(),
    ) {
        let mut f = Fusion::new(100);
        if let Some(t) = gga_t { f.on_gga(gga(GGA), t); }
        if let Some(t) = rmc_t { f.on_rmc(rmc(RMC), t); }
        if let Some(t) = tar_t { f.on_tar(tar(TAR), t); }
        let c = Context {
            now_us: now,
            utc_unix_ms: with_utc.then_some(123),
            imu: with_imu.then_some(ImuSummary { count: 3, accel_mean_ug: [1, 1, 1], ..ImuSummary::default() }),
            ..Context::default()
        };
        let s = f.build(&c);
        if s.flags & flags::GNSS_VALID == 0 {
            prop_assert_eq!((s.lat_e9, s.lon_e9, s.alt_mm, s.fix_quality, s.num_sv), (0, 0, 0, 0, 0));
        }
        if s.flags & flags::HEADING_VALID == 0 {
            prop_assert_eq!((s.heading_e6, s.baseline_mm, s.heading_quality), (0, 0, 0));
        }
        if s.flags & flags::IMU_VALID == 0 {
            prop_assert_eq!((s.imu_count, s.accel_mean_ug), (0, [0, 0, 0]));
        }
        prop_assert_eq!(s.flags & flags::UTC_VALID != 0, with_utc);
        prop_assert_eq!(s.t_us, now);
    }
}
