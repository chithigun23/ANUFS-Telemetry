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
//! System test: a scripted drive through the whole firmware data path, with
//! faults, read back with the host tool.
//!
//! Hardware is replaced by scripted inputs; every piece of logic between the
//! pins and the card is the real code: NMEA framing and parsing, the PPS
//! clock, fusion, the IMU window, CAN decoding, the supervisor, the block log
//! and the storage driver.

use futures_executor::block_on;
use tel_asm330::Scaled;
use tel_can::{ingest, insert, ByteOrder, Latest};
use tel_can_sbg::{SbgKeys, CATALOG, SIGNAL_COUNT};
use tel_core::clock::PpsClock;
use tel_core::fusion::{Context, Fusion};
use tel_core::imu_window::ImuWindow;
use tel_core::supervisor::{Action, Card, Config, Event, GnssHealth, Supervisor, Task};
use tel_core::time::unix_ms;
use tel_decode::export::{export, What};
use tel_decode::scan::scan;
use tel_decode::source::{BlockSink, BlockSource, MemSource};
use tel_logfmt::block::{Block, BLOCK_SIZE};
use tel_logfmt::records::{CanFrame, CanRawBatch, Event as LogEvent, Kind, SignalsBatch};
use tel_logfmt::recovery::Geometry;
use tel_logfmt::writer::PushError;
use tel_nmea::{parse, LineAssembler, Message};
use tel_store::{prepare, recover, BlockDevice, CardLog, Service};

// ---------------------------------------------------------------- simulated cards

struct SimCard {
    mem: MemSource,
    writes: usize,
    fail_after: Option<usize>,
}

impl BlockDevice for SimCard {
    type Error = ();
    async fn read_block(&mut self, lba: u32, buf: &mut Block) -> Result<(), ()> {
        *buf = self.mem.read_block(lba).map_err(|_| ())?;
        Ok(())
    }
    async fn write_block(&mut self, lba: u32, data: &Block) -> Result<(), ()> {
        if self.fail_after.is_some_and(|n| self.writes >= n) {
            return Err(());
        }
        self.writes += 1;
        self.mem.write_block(lba, data).map_err(|_| ())
    }
}

const BLOCKS: u32 = 4000;
fn geo() -> Geometry {
    Geometry { start_lba: 0, blocks: BLOCKS }
}

// ---------------------------------------------------------------- NMEA generation

fn checksum(s: &str) -> u8 {
    s.bytes().fold(0, |a, b| a ^ b)
}

fn sentence(body: &str) -> Vec<u8> {
    format!("${body}*{:02X}\r\n", checksum(body)).into_bytes()
}

/// ddmm.mmmmmm / dddmm.mmmmmm from an unsigned 1e-9 degree value.
fn nmea_coord(abs_e9: i64, deg_digits: usize) -> String {
    let deg = abs_e9 / 1_000_000_000;
    let min_e9 = (abs_e9 % 1_000_000_000) * 60;
    let min_e6 = (min_e9 + 500) / 1000; // minutes in 1e-6, rounded
    format!("{deg:0w$}{:02}.{:06}", min_e6 / 1_000_000, min_e6 % 1_000_000, w = deg_digits)
}

const T0_UNIX_MS: i64 = 1_761_229_500_000; // 2025-10-23 14:25:00 UTC, an exact second
const BOOT_US: u64 = 5_000_000;
const LAT0_E9: i64 = -35_280_900_000;
const LON0_E9: i64 = 149_130_000_000;

fn hhmmss(unix_ms: i64) -> String {
    let ms_of_day = unix_ms.rem_euclid(86_400_000);
    format!(
        "{:02}{:02}{:02}.{:03}",
        ms_of_day / 3_600_000,
        (ms_of_day / 60_000) % 60,
        (ms_of_day / 1000) % 60,
        ms_of_day % 1000
    )
}

fn rmc_gga_tar(k: i64) -> [Vec<u8>; 3] {
    let utc = T0_UNIX_MS + k * 100;
    let t = hhmmss(utc);
    let lat = nmea_coord((LAT0_E9 + k * 50).abs(), 2); // moving slowly north (toward zero)
    let lon = nmea_coord(LON0_E9 + k * 70, 3);
    let heading_e6 = 90_000_000 + k * 10_000;
    [
        sentence(&format!("GNRMC,{t},A,{lat},S,{lon},E,{}.{:02},180.50,231025,,,R,V", 5 + k / 100, k % 100)),
        sentence(&format!("GNGGA,{t},{lat},S,{lon},E,4,24,0.80,584.123,M,-0.337,M,1.0,0000")),
        sentence(&format!(
            "PQTMTAR,1,{t},4,,0.860,0.500000,,{}.{:06},0.200000,,0.050000,19",
            heading_e6 / 1_000_000,
            heading_e6 % 1_000_000
        )),
    ]
}

// ---------------------------------------------------------------- the run

/// Run the scripted drive; returns (card A, card B, counters).
fn drive() -> (SimCard, SimCard, usize, usize) {
    let blank =
        || SimCard { mem: MemSource { blocks: vec![[0; BLOCK_SIZE]; BLOCKS as usize] }, writes: 0, fail_after: None };
    let (mut a, mut b) = (blank(), blank());
    block_on(prepare(&mut a, geo(), 0xAAAA_0001)).unwrap();
    block_on(prepare(&mut b, geo(), 0xBBBB_0002)).unwrap();

    let mut sup = Supervisor::new(Config::default());
    sup.step(Event::Tick { now_ms: 0 });
    let mut logs: [Option<CardLog<4>>; 2] = [None, None];
    let mut devs = [a, b];
    // boot: both cards mount fine
    for (i, card) in [Card::Sd1, Card::Sd2].into_iter().enumerate() {
        let ok = block_on(recover(&mut devs[i], geo())).is_ok();
        sup.step(Event::CardMounted { card, ok });
    }
    sup.step(Event::Gnss(GnssHealth { configured: true, ..GnssHealth::default() }));

    let mut asm = LineAssembler::new();
    let mut fusion = Fusion::new(100);
    let mut clock = PpsClock::default();
    let mut imu = ImuWindow::new();
    let mut latest: Latest<SIGNAL_COUNT> = Latest::new();
    let keys = SbgKeys::resolve(&CATALOG).unwrap();
    let mut can_batch = CanRawBatch::new(2);
    let mut frames2 = 0_u32;
    let mut overruns = 0_u32;
    let mut mount_events: Vec<Action> = Vec::new();

    let mut garbage_injected = 0_usize;
    let mut checksum_errors_injected = 0_usize;

    for k in 0..300_i64 {
        let epoch_mono = BOOT_US + (k as u64) * 100_000;
        let jitter = (k as u64 * 37) % 3000; // up to 3 ms of serial/processing delay
        let arrival = epoch_mono + 20_000 + jitter;
        let now_ms = arrival / 1000;

        // PPS edge once per second, captured by hardware at the exact epoch
        if k % 10 == 0 {
            clock.on_pps(epoch_mono);
        }

        // IMU at ~416 Hz: ~41 samples per 100 ms window
        for n in 0..41_i32 {
            imu.add(&Scaled {
                accel_micro_g: [1_000_000 + n, -2_000 + k as i32, 3],
                gyro_mdps: [n, 0, -n],
                temp_centi_c: 3100,
            });
        }
        // SBG EKF_EULER (0x132) on CAN2 at 100 Hz: 10 frames per window; roll counts up
        for f in 0..10_u64 {
            let t = epoch_mono + f * 10_000;
            let roll_raw = (k * 10 + f as i64) as u64 & 0xFFFF;
            let mut data = [0_u8; 6];
            insert(&mut data, 0, 16, ByteOrder::LittleEndian, roll_raw).unwrap();
            ingest(&CATALOG, &mut latest, 306, false, &data, t);
            frames2 += 1;
            if f == 0
                && !can_batch.try_push(&CanFrame {
                    t_us: t,
                    id: 306,
                    extended: false,
                    dlc: 6,
                    data: [data[0], data[1], data[2], data[3], data[4], data[5], 0, 0],
                })
            {
                // batch full: not expected in this test
                panic!("raw batch full");
            }
        }

        // The three sentences of this epoch arrive as a byte stream with noise.
        let mut stream = Vec::new();
        if k % 53 == 7 {
            stream.extend_from_slice(b"\x00\xFFnoise$$\r\n"); // garbage burst
            garbage_injected += 1;
        }
        let [rmc, gga, tar] = rmc_gga_tar(k);
        let mut sentences = vec![gga, tar, rmc];
        if k == 120 {
            // corrupt one sentence: flip a digit inside the GGA payload
            let g = &mut sentences[0];
            g[12] ^= 0x01;
            checksum_errors_injected += 1;
        }
        for s in sentences {
            stream.extend(s);
        }
        let mut rmc_seen = false;
        for &byte in &stream {
            if let Some(Ok(s)) = asm.push(byte) {
                match parse(&s) {
                    Ok(Message::Rmc(r)) => {
                        // RMC closes the epoch: label the pulse, then build the sample
                        if let (Some(d), Some(t)) = (r.date, r.time) {
                            if let Some(ms) = unix_ms(d, t) {
                                clock.on_time_message(ms, arrival);
                            }
                        }
                        fusion.on_rmc(r, arrival);
                        rmc_seen = true;
                    }
                    Ok(Message::Gga(g)) => fusion.on_gga(g, arrival),
                    Ok(Message::Tar(t)) => fusion.on_tar(t, arrival),
                    _ => {}
                }
            }
        }
        assert!(rmc_seen);

        // supervisor housekeeping
        for task in [Task::Gnss, Task::Imu, Task::Logger, Task::Supervisor] {
            sup.step(Event::Heartbeat { task, now_ms });
        }
        sup.step(Event::ImuOk { ok: true });
        sup.step(Event::CanAlive { bus: 2, alive: true });
        let health = GnssHealth { configured: true, fix: true, rtk_fixed: true, heading: true };
        let mut acts = sup.step(Event::Gnss(health));
        let t = sup.step(Event::Tick { now_ms });
        mount_events.extend(acts.iter().chain(t.iter()));
        let _ = &mut acts;

        // open sessions the supervisor asked for
        for act in mount_events.drain(..) {
            if let Action::OpenSession(card) = act {
                let i = if card == Card::Sd1 { 0 } else { 1 };
                let rec = block_on(recover(&mut devs[i], geo())).unwrap();
                logs[i] = Some(CardLog::open(geo(), &rec).unwrap());
            }
        }

        // build the record
        let sample = fusion.build(&Context {
            now_us: arrival,
            utc_unix_ms: clock.utc_unix_ms_at(arrival),
            imu: imu.take(),
            flags: tel_logfmt::records::flags::CAN2_ALIVE
                | if clock.is_locked(arrival) { tel_logfmt::records::flags::PPS_LOCKED } else { 0 },
            nmea_errors: asm.stats().checksum_errors,
            overruns,
            can1_frames: 0,
            can2_frames: frames2,
        });
        let mut body = [0_u8; 255];
        let n = sample.encode(&mut body).unwrap();
        let mut sig = SignalsBatch::new(2, arrival);
        for id in keys.all() {
            if let Some(s) = latest.get(id) {
                sig.try_push(id.0, s.raw);
            }
        }
        let mut sig_body = [0_u8; 255];
        let sn = sig.encode(&mut sig_body).unwrap();
        let mut raw_body = [0_u8; 255];
        let rn = can_batch.encode(&mut raw_body).unwrap();
        can_batch.clear();

        // card A dies half way through
        if k == 150 {
            devs[0].fail_after = Some(devs[0].writes);
        }
        for i in 0..2 {
            let card = if i == 0 { Card::Sd1 } else { Card::Sd2 };
            let Some(log) = logs[i].as_mut() else { continue };
            if log.is_faulted() {
                continue;
            }
            for (kind, data) in
                [(Kind::Sample, &body[..n]), (Kind::Signals, &sig_body[..sn]), (Kind::CanRaw, &raw_body[..rn])]
            {
                loop {
                    match log.push(kind, data) {
                        Ok(()) => break,
                        Err(PushError::Overrun) => {
                            overruns += 1;
                            block_on(log.service(&mut devs[i]));
                        }
                        Err(e) => panic!("{e:?}"),
                    }
                }
            }
            if let Service::Faulted = block_on(log.service(&mut devs[i])) {
                let acts = sup.step(Event::WriteFailed { card });
                // the supervisor logs the fault on the surviving card
                for act in acts.iter() {
                    if let Action::LogEvent { code, arg } = act {
                        let ev = LogEvent { t_us: arrival, code, arg };
                        let mut eb = [0_u8; 32];
                        let en = ev.encode(&mut eb).unwrap();
                        let j = 1 - i;
                        if let Some(other) = logs[j].as_mut() {
                            if !other.is_faulted() {
                                let _ = other.push(Kind::Event, &eb[..en]);
                            }
                        }
                    }
                }
            }
        }
        assert!(sup.should_kick_watchdog(now_ms), "watchdog must be fed during a healthy run (k = {k})");
    }
    // clean shutdown of the surviving card
    let healthy = if logs[1].as_ref().is_some_and(|l| !l.is_faulted()) { 1 } else { 0 };
    if let Some(log) = logs[healthy].as_mut() {
        while log.close() == Err(PushError::Overrun) {
            block_on(log.service(&mut devs[healthy]));
        }
        block_on(log.service(&mut devs[healthy]));
    }
    let _ = garbage_injected;
    let [a, b] = devs;
    (a, b, checksum_errors_injected, garbage_injected)
}

fn rows(card: &mut SimCard, id: u32, what: What) -> (Vec<String>, Vec<Vec<String>>) {
    let mut out = Vec::new();
    export(&mut card.mem, geo(), id, None, what, 256, &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    let mut lines = text.lines();
    let header: Vec<String> = lines.next().unwrap().split(',').map(str::to_string).collect();
    let body = lines.map(|l| l.split(',').map(str::to_string).collect()).collect();
    (header, body)
}

#[test]
fn full_drive_with_card_failure_is_logged_and_readable() {
    let (mut a, mut b, injected_errors, _) = drive();

    // ---- card B (healthy): complete log
    let report = scan(&mut b.mem, geo(), 256).unwrap();
    assert_eq!(report.runs.len(), 1);
    assert!(report.runs[0].clean_close);
    assert_eq!(report.invalid_blocks, 0);
    let (h, samples) = rows(&mut b, 0xBBBB_0002, What::Samples);
    assert_eq!(samples.len(), 300, "one record per GNSS epoch");
    let col = |name: &str| h.iter().position(|c| c == name).unwrap_or_else(|| panic!("no column {name}"));

    // times strictly increase by about 100 ms
    let t: Vec<f64> = samples.iter().map(|r| r[col("t_s")].parse().unwrap()).collect();
    for w in t.windows(2) {
        let dt = w[1] - w[0];
        assert!((0.096..=0.104).contains(&dt), "sample spacing {dt}");
    }
    // position, speed and heading track the script exactly
    let first = &samples[0];
    assert_eq!(first[col("lat_deg")], "-35.280900000");
    assert_eq!(first[col("lon_deg")], "149.130000000");
    assert_eq!(first[col("alt_m")], "584.123");
    assert_eq!(first[col("fix_quality")], "4");
    assert_eq!(first[col("heading_deg")], "90.000000");
    assert_eq!(first[col("baseline_m")], "0.860");
    let last = &samples[299];
    assert_eq!(last[col("lat_deg")], "-35.280885050", "299 steps of 50 nano-degrees"); // -35.2809 + 299*50e-9... see script
    assert_eq!(last[col("heading_deg")], "92.990000");
    for r in &samples {
        assert_eq!(r[col("gnss_valid")], "1");
        assert_eq!(r[col("heading_valid")], "1");
        assert_eq!(r[col("imu_valid")], "1");
        assert_eq!(r[col("utc_valid")], "1", "the PPS clock labels time from the very first epoch");
        assert_eq!(r[col("pps_locked")], "1");
        assert_eq!(r[col("can2_alive")], "1");
    }
    // IMU window summarised 41 samples each time with the right mean
    assert_eq!(first[col("imu_count")], "41");
    assert_eq!(first[col("accel_mean_x_g")], "1.000020"); // mean of 1_000_000 + 0..40 = 1_000_020 ug
    assert_eq!(first[col("accel_peak_x_g")], "1.000040");
    // UTC: every sample's time equals the truth for its epoch plus its own build delay, within 1 ms
    for (k, r) in samples.iter().enumerate() {
        let utc = r[col("utc")].clone();
        // parse HH:MM:SS.mmm from the ISO string and compare with the script's epoch + delay
        let ms_of_day = {
            let hh: i64 = utc[11..13].parse().unwrap();
            let mm: i64 = utc[14..16].parse().unwrap();
            let ss: i64 = utc[17..19].parse().unwrap();
            let mmm: i64 = utc[20..23].parse().unwrap();
            ((hh * 60 + mm) * 60 + ss) * 1000 + mmm
        };
        let epoch_ms_of_day = (T0_UNIX_MS + k as i64 * 100).rem_euclid(86_400_000);
        let delay_ms = 20 + (k as i64 * 37 % 3000) / 1000; // arrival - epoch, in ms (truncated), 20..23
        let diff = ms_of_day - epoch_ms_of_day - delay_ms;
        assert!(diff.abs() <= 1, "sample {k}: utc {utc} is {diff} ms off");
    }

    // the corrupted GGA was detected and counted, and the next sample still has a valid position
    assert_eq!(last[col("nmea_errors")], injected_errors.to_string());
    // with the GGA lost for epoch 120 the RMC alone supplied the position that time
    let s120 = &samples[120];
    assert_eq!(s120[col("gnss_valid")], "1");
    assert_eq!(s120[col("alt_m")], "0.000", "no GGA that epoch, so no altitude (and no stale altitude either)");

    // ---- decoded CAN signals on bus 2
    let (_, sigs) = rows(&mut b, 0xBBBB_0002, What::Signals);
    let roll: Vec<&Vec<String>> = sigs.iter().filter(|r| r[4] == "ROLL").collect();
    assert_eq!(roll.len(), 300);
    let last_roll: f64 = roll[299][6].parse().unwrap();
    // last epoch k = 299: the newest frame written before the sample is f = 9 -> raw 2999
    assert!((last_roll - 2999.0 * 1e-4).abs() < 1e-9, "{last_roll}");
    // ---- raw CAN lane
    let (_, raw) = rows(&mut b, 0xBBBB_0002, What::CanRaw);
    assert_eq!(raw.len(), 300);
    assert_eq!(raw[0][3], "0x132");

    // ---- the failure was recorded as an event on the surviving card
    let (_, events) = rows(&mut b, 0xBBBB_0002, What::Events);
    assert!(events.iter().any(|e| e[3] == "card_fault" && e[4] == "1"), "{events:?}");

    // ---- card A: stopped when it died, but what it holds is intact and readable
    let ra = scan(&mut a.mem, geo(), 256).unwrap();
    assert_eq!(ra.invalid_blocks, 0);
    assert!(!ra.runs[0].clean_close, "the card died, so the session was never closed");
    let (_, a_samples) = rows(&mut a, 0xAAAA_0001, What::Samples);
    assert!(a_samples.len() >= 100 && a_samples.len() < 300, "card A kept {} samples", a_samples.len());
    // its records are a clean prefix of card B's
    let a_times: Vec<&String> = a_samples.iter().map(|r| &r[col("t_s")]).collect();
    let b_times: Vec<&String> = samples.iter().map(|r| &r[col("t_s")]).collect();
    assert_eq!(a_times, b_times[..a_times.len()].to_vec());

    // a card read back with a fresh recovery continues the session numbering
    let rec = block_on(recover(&mut b, geo())).unwrap();
    assert_eq!(rec.last_session, 1);
}
