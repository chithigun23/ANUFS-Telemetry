#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    missing_docs,
    clippy::type_complexity
)]
//! Decoder tests against images written by the real writer.

use futures_executor::block_on;
use proptest::prelude::*;
use tel_decode::export::{csv_escape, export, fmt_fixed, iso8601, session_info, What};
use tel_decode::scan::{read_header, scan, Ended, ScanError, DEFAULT_STOP_AFTER_INVALID};
use tel_decode::source::{BlockSink, BlockSource, MemSource};
use tel_logfmt::block::{encode_data, Block, DataHeader, BLOCK_SIZE, DATA_START, FLAG_FIRST};
use tel_logfmt::records::{
    event, flags, CanFrame, CanRawBatch, Event, Kind, Sample, SessionInfo, SignalsBatch, SCHEMA_VERSION,
};
use tel_logfmt::recovery::Geometry;
use tel_store::{prepare, recover, BlockDevice, CardLog};

struct Dev(MemSource);

impl BlockDevice for Dev {
    type Error = ();
    async fn read_block(&mut self, lba: u32, buf: &mut Block) -> Result<(), ()> {
        *buf = self.0.read_block(lba).map_err(|_| ())?;
        Ok(())
    }
    async fn write_block(&mut self, lba: u32, data: &Block) -> Result<(), ()> {
        self.0.write_block(lba, data).map_err(|_| ())
    }
}

const ID: u32 = 0xFACE_0001;
const BLOCKS: u32 = 3000;

fn geo() -> Geometry {
    Geometry { start_lba: 0, blocks: BLOCKS }
}

fn blank_image() -> Dev {
    Dev(MemSource { blocks: vec![[0_u8; BLOCK_SIZE]; BLOCKS as usize] })
}

fn enc<F: Fn(&mut [u8]) -> Result<usize, E>, E: std::fmt::Debug>(f: F) -> Vec<u8> {
    let mut b = [0_u8; 255];
    let n = f(&mut b).unwrap();
    b[..n].to_vec()
}

fn sample(i: u32, valid: bool) -> Sample {
    Sample {
        t_us: 1_000_000 * u64::from(i) + 5,
        utc_unix_ms: 1_642_152_863_000 + i64::from(i) * 100,
        flags: if valid {
            flags::GNSS_VALID | flags::HEADING_VALID | flags::UTC_VALID | flags::IMU_VALID | flags::CAN1_ALIVE
        } else {
            0
        },
        lat_e9: -35_280_900_000,
        lon_e9: 149_130_000_001,
        alt_mm: 584_123,
        sog_mm_s: 12_345,
        cog_cdeg: 18_050,
        fix_quality: 4,
        num_sv: 27,
        hdop_e2: 67,
        diff_age_ds: 12,
        heading_e6: 50_968_541,
        pitch_e6: -1_124_780,
        baseline_mm: 860,
        acc_heading_e6: 12_547,
        heading_quality: 4,
        heading_sv: 21,
        accel_mean_ug: [1_000_000, -250_000, 5],
        accel_peak_ug: [2_000_000, 1, -3],
        gyro_mean_mdps: [100, -200, 300],
        gyro_peak_mdps: [4000, 5000, 6000],
        imu_temp_centi_c: 3125,
        imu_count: 42,
        nmea_errors: 1,
        overruns: 2,
        can1_frames: 300 + i,
        can2_frames: 400,
    }
}

fn push_all(log: &mut CardLog<4>, dev: &mut Dev, kind: Kind, body: &[u8]) {
    while log.push(kind, body) == Err(tel_logfmt::writer::PushError::Overrun) {
        block_on(log.service(dev));
    }
}

/// Session 1: closed cleanly. Session 2: abandoned (power loss).
fn build_image() -> Dev {
    let mut dev = blank_image();
    block_on(prepare(&mut dev, geo(), ID)).unwrap();
    for session in 1..=2_u32 {
        let rec = block_on(recover(&mut dev, geo())).unwrap();
        let mut log: CardLog<4> = CardLog::open(geo(), &rec).unwrap();
        assert_eq!(log.session(), session);
        let mut fw = [0_u8; 16];
        fw[..5].copy_from_slice(b"0.1.0");
        let info = SessionInfo {
            schema: SCHEMA_VERSION,
            firmware: fw,
            fix_interval_ms: 100,
            can1_bitrate: 1_000_000,
            can2_bitrate: 500_000,
            imu_odr_hz: 416,
            t_us: 0,
            utc_unix_ms: 1_642_152_863_000,
        };
        push_all(&mut log, &mut dev, Kind::SessionInfo, &enc(|b| info.encode(b)));
        for i in 0..50_u32 {
            push_all(&mut log, &mut dev, Kind::Sample, &enc(|b| sample(i, i % 10 != 9).encode(b)));
        }
        push_all(
            &mut log,
            &mut dev,
            Kind::Event,
            &enc(|b| Event { t_us: 2_500_000, code: event::CARD_FAULT, arg: 1 }.encode(b)),
        );
        let mut cr = CanRawBatch::new(1);
        assert!(cr.try_push(&CanFrame {
            t_us: 1_000,
            id: 0x640,
            extended: false,
            dlc: 8,
            data: [1, 2, 3, 4, 5, 6, 7, 8]
        }));
        assert!(cr.try_push(&CanFrame {
            t_us: 1_500,
            id: 0x18FF_0001,
            extended: true,
            dlc: 2,
            data: [0xAB, 0xCD, 0, 0, 0, 0, 0, 0]
        }));
        push_all(&mut log, &mut dev, Kind::CanRaw, &enc(|b| cr.encode(b)));
        // one SBG signal: EKF_EULER ROLL, raw 1234 -> 0.1234 rad (factor 1e-4)
        let roll = tel_can_sbg::SbgKeys::resolve(&tel_can_sbg::CATALOG).unwrap().roll;
        let mut sb = SignalsBatch::new(2, 3_000_000);
        assert!(sb.try_push(roll.0, 1234));
        push_all(&mut log, &mut dev, Kind::Signals, &enc(|b| sb.encode(b)));
        if session == 1 {
            while log.close() == Err(tel_logfmt::writer::PushError::Overrun) {
                block_on(log.service(&mut dev));
            }
            block_on(log.service(&mut dev));
        } else {
            // abandon without closing: flush what exists (retrying while the ring drains),
            // then "power loss"
            while log.flush() == Err(tel_logfmt::writer::PushError::Overrun) {
                block_on(log.service(&mut dev));
            }
            block_on(log.service(&mut dev));
        }
    }
    dev
}

fn csv_lines(out: &[u8]) -> Vec<String> {
    String::from_utf8(out.to_vec()).unwrap().lines().map(str::to_string).collect()
}

#[test]
fn scan_reports_runs_and_clean_close() {
    let mut dev = build_image();
    let r = scan(&mut dev.0, geo(), DEFAULT_STOP_AFTER_INVALID).unwrap();
    assert_eq!(r.card_id, ID);
    assert_eq!(r.runs.len(), 2);
    assert_eq!((r.runs[0].session, r.runs[1].session), (1, 2));
    assert!(r.runs[0].clean_close);
    assert!(!r.runs[1].clean_close, "session 2 was abandoned");
    assert_eq!(r.runs[0].first_rel, DATA_START);
    assert_eq!(r.runs[1].first_rel, DATA_START + r.runs[0].blocks);
    assert_eq!(r.runs[1].gap_before, 0);
    assert_eq!(r.valid_blocks, r.runs[0].blocks + r.runs[1].blocks);
    assert_eq!(r.ended, Ended::NoMoreData, "stops after a long stretch of blank blocks");
    assert_eq!(r.foreign_blocks, 0);
    let info = session_info(&mut dev.0, geo(), ID, 2, DEFAULT_STOP_AFTER_INVALID).unwrap().unwrap();
    assert_eq!(info.can2_bitrate, 500_000);
    assert_eq!(&info.firmware[..5], b"0.1.0");
}

#[test]
fn samples_export_values_gating_and_session_filter() {
    let mut dev = build_image();
    let mut out = Vec::new();
    let (stats, _) = export(&mut dev.0, geo(), ID, Some(1), What::Samples, 256, &mut out).unwrap();
    assert_eq!(stats.rows, 50);
    assert_eq!(stats.undecodable, 0);
    let lines = csv_lines(&out);
    assert_eq!(lines.len(), 51);
    let header: Vec<&str> = lines[0].split(',').collect();
    let col = |name: &str| header.iter().position(|h| *h == name).unwrap_or_else(|| panic!("no column {name}"));
    let row = |i: usize| lines[1 + i].split(',').map(str::to_string).collect::<Vec<String>>();
    // every row has the same number of fields as the header
    for l in &lines[1..] {
        assert_eq!(l.split(',').count(), header.len());
    }
    let r0 = row(0);
    assert_eq!(r0[col("session")], "1");
    assert_eq!(r0[col("t_s")], "0.000005");
    assert_eq!(r0[col("utc")], "2022-01-14T09:34:23.000Z");
    assert_eq!(r0[col("lat_deg")], "-35.280900000");
    assert_eq!(r0[col("lon_deg")], "149.130000001");
    assert_eq!(r0[col("alt_m")], "584.123");
    assert_eq!(r0[col("sog_mps")], "12.345");
    assert_eq!(r0[col("cog_deg")], "180.50");
    assert_eq!(r0[col("hdop")], "0.67");
    assert_eq!(r0[col("diff_age_s")], "1.2");
    assert_eq!(r0[col("heading_deg")], "50.968541");
    assert_eq!(r0[col("pitch_deg")], "-1.124780");
    assert_eq!(r0[col("baseline_m")], "0.860");
    assert_eq!(r0[col("accel_mean_x_g")], "1.000000");
    assert_eq!(r0[col("accel_mean_y_g")], "-0.250000");
    assert_eq!(r0[col("accel_mean_z_g")], "0.000005");
    assert_eq!(r0[col("gyro_peak_z_dps")], "6.000");
    assert_eq!(r0[col("imu_temp_c")], "31.25");
    assert_eq!(r0[col("gnss_valid")], "1");
    assert_eq!(r0[col("can1_alive")], "1");
    assert_eq!(r0[col("can2_alive")], "0");
    assert_eq!(r0[col("can1_frames")], "300");
    // sample 9 is flagged invalid: every gated column is empty, counters remain
    let r9 = row(9);
    for name in ["utc", "lat_deg", "alt_m", "heading_deg", "accel_mean_x_g", "imu_temp_c"] {
        assert_eq!(r9[col(name)], "", "{name} must be empty when invalid");
    }
    assert_eq!(r9[col("gnss_valid")], "0");
    assert_eq!(r9[col("nmea_errors")], "1");
    assert_eq!(r9[col("can1_frames")], "309");
    // all sessions
    let mut out_all = Vec::new();
    let (stats, _) = export(&mut dev.0, geo(), ID, None, What::Samples, 256, &mut out_all).unwrap();
    assert_eq!(stats.rows, 100);
    // a session that does not exist
    let mut none = Vec::new();
    assert_eq!(export(&mut dev.0, geo(), ID, Some(9), What::Samples, 256, &mut none).unwrap().0.rows, 0);
}

#[test]
fn events_can_raw_and_signals_exports() {
    let mut dev = build_image();
    let mut out = Vec::new();
    export(&mut dev.0, geo(), ID, Some(1), What::Events, 256, &mut out).unwrap();
    let l = csv_lines(&out);
    assert_eq!(l[0], "session,t_s,code,name,arg");
    assert_eq!(l[1], "1,2.500000,3,card_fault,1");
    assert_eq!(l.len(), 2);

    let mut out = Vec::new();
    export(&mut dev.0, geo(), ID, Some(1), What::CanRaw, 256, &mut out).unwrap();
    let l = csv_lines(&out);
    assert_eq!(l[0], "session,t_s,bus,id,extended,dlc,data");
    assert_eq!(l[1], "1,0.001000,1,0x640,0,8,0102030405060708");
    assert_eq!(l[2], "1,0.001500,1,0x18FF0001,1,2,ABCD");

    let mut out = Vec::new();
    export(&mut dev.0, geo(), ID, Some(1), What::Signals, 256, &mut out).unwrap();
    let l = csv_lines(&out);
    assert_eq!(l[0], "session,t_s,bus,message,signal,raw,physical,unit");
    assert_eq!(l.len(), 2);
    let f: Vec<&str> = l[1].split(',').collect();
    assert_eq!(f[2], "2");
    assert_eq!(f[3], "SBG_ECAN_MSG_EKF_EULER");
    assert_eq!(f[4], "ROLL");
    assert_eq!(f[5], "1234");
    let phys: f64 = f[6].parse().unwrap();
    assert!((phys - 0.1234).abs() < 1e-9, "{phys}");
    assert_eq!(f[7], "rad");
}

#[test]
fn a_damaged_block_splits_the_run_and_is_reported() {
    let mut dev = build_image();
    let before = scan(&mut dev.0, geo(), 256).unwrap();
    let victim = before.runs[0].first_rel + before.runs[0].blocks / 2;
    dev.0.blocks[victim as usize][100] ^= 0xFF; // bit rot in the middle of session 1
    let r = scan(&mut dev.0, geo(), 256).unwrap();
    assert_eq!(r.invalid_blocks, 1);
    assert_eq!(r.valid_blocks, before.valid_blocks - 1);
    // session 1 is now two runs with a one block gap, then session 2
    assert_eq!(r.runs.len(), 3);
    assert_eq!((r.runs[0].session, r.runs[1].session, r.runs[2].session), (1, 1, 2));
    assert_eq!(r.runs[1].gap_before, 1);
    assert!(r.runs[1].clean_close);
    // export of session 1 still returns everything that survives
    let mut out = Vec::new();
    let (stats, _) = export(&mut dev.0, geo(), ID, Some(1), What::Samples, 256, &mut out).unwrap();
    assert!(stats.rows > 0 && stats.rows < 50);
    assert_eq!(stats.undecodable, 0, "a damaged block drops whole records, never half of one");
}

#[test]
fn foreign_blocks_are_counted_and_skipped() {
    let mut dev = build_image();
    let before = scan(&mut dev.0, geo(), 256).unwrap();
    // overwrite one block with a valid block of another card
    let mut b = [0_u8; BLOCK_SIZE];
    encode_data(&mut b, &DataHeader { card_id: 0x0BAD_C0DE, session: 1, seq: 0, used: 0, flags: FLAG_FIRST }, &[])
        .unwrap();
    dev.0.blocks[(before.runs[0].first_rel + 3) as usize] = b;
    let r = scan(&mut dev.0, geo(), 256).unwrap();
    assert_eq!(r.foreign_blocks, 1);
    assert_eq!(r.invalid_blocks, 0);
}

#[test]
fn reused_session_numbers_are_separate_runs() {
    let mut dev = blank_image();
    block_on(prepare(&mut dev, geo(), ID)).unwrap();
    let put = |dev: &mut Dev, rel: u32, session: u32, seq: u32| {
        let mut b = [0_u8; BLOCK_SIZE];
        let flags = if seq == 0 { FLAG_FIRST } else { 0 };
        encode_data(&mut b, &DataHeader { card_id: ID, session, seq, used: 0, flags }, &[]).unwrap();
        dev.0.blocks[rel as usize] = b;
    };
    for (i, seq) in (0..3_u32).enumerate() {
        put(&mut dev, DATA_START + i as u32, 1, seq);
    }
    for (i, seq) in (0..2_u32).enumerate() {
        put(&mut dev, 20 + i as u32, 1, seq); // same session number again after a hole
    }
    let r = scan(&mut dev.0, geo(), 256).unwrap();
    assert_eq!(r.runs.len(), 2);
    assert_eq!(r.runs[0].blocks, 3);
    assert_eq!(r.runs[1].first_rel, 20);
    assert_eq!(r.runs[1].gap_before, 20 - (DATA_START + 3));
}

#[test]
fn unprepared_cards_and_wrong_geometry_are_reported() {
    let mut blank = blank_image();
    assert!(matches!(read_header(&mut blank.0, geo()), Err(ScanError::NotPrepared)));
    assert!(matches!(scan(&mut blank.0, geo(), 256), Err(ScanError::NotPrepared)));
    let mut dev = build_image();
    let wrong = Geometry { start_lba: 0, blocks: BLOCKS - 1 };
    assert!(matches!(scan(&mut dev.0, wrong, 256), Err(ScanError::GeometryMismatch { found }) if found == BLOCKS));
    // wrong start block: the header is not there
    let shifted = Geometry { start_lba: 5, blocks: BLOCKS - 5 };
    assert!(matches!(scan(&mut dev.0, shifted, 256), Err(ScanError::NotPrepared)));
}

/// A source that counts reads, to prove the scan stops early.
struct Counting {
    inner: MemSource,
    reads: usize,
}
impl BlockSource for Counting {
    fn read_block(&mut self, lba: u32) -> std::io::Result<Block> {
        self.reads += 1;
        self.inner.read_block(lba)
    }
    fn len_blocks(&mut self) -> std::io::Result<u64> {
        self.inner.len_blocks()
    }
}

#[test]
fn the_scan_stops_after_the_data_ends_instead_of_reading_the_whole_card() {
    let dev = build_image();
    let mut src = Counting { inner: dev.0, reads: 0 };
    let r = scan(&mut src, geo(), 256).unwrap();
    assert_eq!(r.ended, Ended::NoMoreData);
    let used = (r.runs[1].first_rel + r.runs[1].blocks) as usize;
    assert!(src.reads <= used + 256 + 4, "{} reads for {used} used blocks", src.reads);
    assert!((src.reads as u32) < BLOCKS / 2);
}

#[test]
fn a_medium_shorter_than_the_region_ends_the_scan_gracefully() {
    let dev = build_image();
    let mut short = MemSource { blocks: dev.0.blocks[..120].to_vec() };
    let r = scan(&mut short, geo(), 256).unwrap();
    assert_eq!(r.ended, Ended::MediaEnd);
}

#[test]
fn prepare_validates_the_device_size_and_makes_an_empty_log() {
    let mut small = MemSource { blocks: vec![[0; BLOCK_SIZE]; 10] };
    assert!(tel_decode::prepare(&mut small, Geometry { start_lba: 0, blocks: 100 }, 1).is_err());
    assert!(
        tel_decode::prepare(&mut small, Geometry { start_lba: 8, blocks: 5 }, 1).is_err(),
        "start + blocks must fit"
    );
    let mut dev = MemSource { blocks: vec![[0; BLOCK_SIZE]; 200] };
    tel_decode::prepare(&mut dev, Geometry { start_lba: 20, blocks: 100 }, 0xABCD).unwrap();
    let g = Geometry { start_lba: 20, blocks: 100 };
    assert_eq!(read_header(&mut dev, g).unwrap().card_id, 0xABCD);
    let r = scan(&mut dev, g, 256).unwrap();
    assert!(r.runs.is_empty());
    // the firmware side recovers from a card prepared by the tool
    let mut d = Dev(dev);
    let rec = block_on(recover(&mut d, g)).unwrap();
    assert_eq!((rec.card_id, rec.last_session, rec.next_rel), (0xABCD, 0, DATA_START));
}

#[test]
fn formatting_matches_references() {
    assert_eq!(fmt_fixed(0, 3), "0.000");
    assert_eq!(fmt_fixed(1500, 3), "1.500");
    assert_eq!(fmt_fixed(-1500, 3), "-1.500");
    assert_eq!(fmt_fixed(-5, 3), "-0.005");
    assert_eq!(fmt_fixed(7, 0), "7");
    assert_eq!(fmt_fixed(i64::MIN, 9), "-9223372036.854775808");
    assert_eq!(fmt_fixed(i64::MAX, 6), "9223372036854.775807");
    for (ms, s) in [
        (0_i64, "1970-01-01T00:00:00.000Z"),
        (946_684_800_000, "2000-01-01T00:00:00.000Z"),
        (1_642_152_863_000, "2022-01-14T09:34:23.000Z"),
        (1_709_208_000_500, "2024-02-29T12:00:00.500Z"),
        (4_102_444_799_999, "2099-12-31T23:59:59.999Z"),
        (1_761_229_506_250, "2025-10-23T14:25:06.250Z"),
        (-1, "1969-12-31T23:59:59.999Z"),
        (951_782_400_000, "2000-02-29T00:00:00.000Z"),
    ] {
        assert_eq!(iso8601(ms), s);
    }
    assert_eq!(csv_escape("plain"), "plain");
    assert_eq!(csv_escape("a,b"), "\"a,b\"");
    assert_eq!(csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
}

proptest! {
    /// Formatting never panics for any value and round-trips through string parsing.
    #[test]
    fn fmt_fixed_is_total_and_exact(v in any::<i64>(), d in 0_u32..10) {
        let s = fmt_fixed(v, d);
        let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
        let parsed: i128 = digits.parse().unwrap();
        let signed = if s.starts_with('-') { -parsed } else { parsed };
        prop_assert_eq!(signed, i128::from(v));
        if d > 0 {
            prop_assert_eq!(s.split('.').nth(1).unwrap().len(), d as usize);
        }
    }

    #[test]
    fn iso8601_is_total(ms in any::<i64>()) {
        let s = iso8601(ms);
        prop_assert!(s.ends_with('Z'));
    }

    /// For years 2000 to 2099 the formatted date parses back to the same instant.
    #[test]
    fn iso8601_roundtrips_through_the_tel_core_converter(secs in 946_684_800_i64..4_102_444_799, ms in 0_i64..1000) {
        let unix_ms = secs * 1000 + ms;
        let s = iso8601(unix_ms);
        let y: u16 = s[0..4].parse().unwrap();
        let mo: u8 = s[5..7].parse().unwrap();
        let d: u8 = s[8..10].parse().unwrap();
        let h: u8 = s[11..13].parse().unwrap();
        let mi: u8 = s[14..16].parse().unwrap();
        let sec: u8 = s[17..19].parse().unwrap();
        let milli: u16 = s[20..23].parse().unwrap();
        // independent day count: walk years and months
        fn leap(y: u32) -> bool { (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 }
        let dim = |y: u32, m: u8| -> i64 { match m { 1|3|5|7|8|10|12 => 31, 4|6|9|11 => 30, _ => if leap(y) { 29 } else { 28 } } };
        let mut days: i64 = 0;
        for yy in 1970..u32::from(y) { days += if leap(yy) { 366 } else { 365 }; }
        for m in 1..mo { days += dim(u32::from(y), m); }
        days += i64::from(d) - 1;
        let back = ((days * 24 + i64::from(h)) * 60 + i64::from(mi)) * 60 + i64::from(sec);
        prop_assert_eq!(back * 1000 + i64::from(milli), unix_ms);
    }
}
