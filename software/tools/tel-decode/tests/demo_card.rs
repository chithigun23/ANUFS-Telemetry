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
//! Writes a demo card image with a realistic three-lap drive, for developing the
//! Android viewer without hardware. It does nothing unless `TEL_DEMO_IMAGE` names
//! the file to write:
//!
//! ```text
//! TEL_DEMO_IMAGE=sample-data/demo_laps.img cargo test -p tel-decode --test demo_card
//! ```
//!
//! The drive is made up: a closed circuit, speeds from a grip-limited profile, an
//! onboard LC29H that wanders by about a metre, and an SBG Ellipse on CAN2 that
//! tracks the truth to a few centimetres. Bus 1 carries invented frames at ids the
//! MoTeC configuration will eventually claim, so the raw-frame table has something
//! to show. Values in the Ellipse status words are assumptions, not SBG's tables.

use futures_executor::block_on;
use tel_can::{ingest, insert, ByteOrder, Latest};
use tel_can_sbg::{SbgKeys, CATALOG, SIGNAL_COUNT};
use tel_decode::source::{BlockSource, MemSource};
use tel_logfmt::block::{Block, BLOCK_SIZE};
use tel_logfmt::records::{
    flags, CanFrame, CanRawBatch, Kind, Sample, SessionInfo, SignalsBatch, SCHEMA_VERSION,
};
use tel_logfmt::recovery::Geometry;
use tel_logfmt::writer::PushError;
use tel_store::{prepare, recover, BlockDevice, CardLog};

struct SimCard {
    mem: MemSource,
}

impl BlockDevice for SimCard {
    type Error = ();
    async fn read_block(&mut self, lba: u32, buf: &mut Block) -> Result<(), ()> {
        *buf = self.mem.read_block(lba).map_err(|_| ())?;
        Ok(())
    }
    async fn write_block(&mut self, lba: u32, data: &Block) -> Result<(), ()> {
        use tel_decode::source::BlockSink;
        self.mem.write_block(lba, data).map_err(|_| ())
    }
}

const BLOCKS: u32 = 12_000;
const G: f64 = 9.80665;
const UTC0_MS: i64 = 1_761_229_500_000; // 2025-10-23T14:25:00Z
const BOOT_US: u64 = 5_000_000;
const LAT0: f64 = -35.2809;
const LON0: f64 = 149.1300;

fn geo() -> Geometry {
    Geometry { start_lba: 0, blocks: BLOCKS }
}

// ---------------------------------------------------------------- the circuit

struct Track {
    s: Vec<f64>,
    x: Vec<f64>,
    y: Vec<f64>,
    h: Vec<f64>,
    k: Vec<f64>,
    v: Vec<f64>,
    len: f64,
}

fn track() -> Track {
    let ctrl: [[f64; 2]; 13] = [
        [0., 0.], [220., 10.], [330., 80.], [300., 170.], [200., 200.], [150., 260.], [200., 330.],
        [120., 380.], [20., 340.], [-60., 260.], [-40., 150.], [-120., 90.], [-60., 20.],
    ];
    let n = ctrl.len();
    let per = 40;
    let mut pts: Vec<[f64; 2]> = Vec::new();
    for i in 0..n {
        let p0 = ctrl[(i + n - 1) % n];
        let p1 = ctrl[i];
        let p2 = ctrl[(i + 1) % n];
        let p3 = ctrl[(i + 2) % n];
        for kk in 0..per {
            let t = kk as f64 / per as f64;
            let (t2, t3) = (t * t, t * t * t);
            let mut p = [0.; 2];
            for d in 0..2 {
                p[d] = 0.5
                    * ((2. * p1[d])
                        + (-p0[d] + p2[d]) * t
                        + (2. * p0[d] - 5. * p1[d] + 4. * p2[d] - p3[d]) * t2
                        + (-p0[d] + 3. * p1[d] - 3. * p2[d] + p3[d]) * t3);
            }
            pts.push(p);
        }
    }
    let m = pts.len();
    let mut s = vec![0.0];
    let mut h = Vec::new();
    for i in 0..m {
        let (a, b) = (pts[i], pts[(i + 1) % m]);
        h.push((b[1] - a[1]).atan2(b[0] - a[0]));
        s.push(s[i] + (b[0] - a[0]).hypot(b[1] - a[1]));
    }
    let len = s[m];
    let wrap = |mut d: f64| {
        while d > std::f64::consts::PI {
            d -= 2. * std::f64::consts::PI;
        }
        while d < -std::f64::consts::PI {
            d += 2. * std::f64::consts::PI;
        }
        d
    };
    let mut k: Vec<f64> = (0..m).map(|i| wrap(h[(i + 1) % m] - h[i]) / (s[i + 1] - s[i])).collect();
    for _ in 0..4 {
        let old = k.clone();
        k = (0..m)
            .map(|i| (old[(i + m - 2) % m] + old[(i + m - 1) % m] + old[i] + old[(i + 1) % m] + old[(i + 2) % m]) / 5.)
            .collect();
    }
    let (vmax, ay, a_acc, a_brk) = (21.0_f64, 9.5_f64, 4.5_f64, 9.0_f64);
    let mut v: Vec<f64> = k.iter().map(|k| vmax.min((ay / k.abs().max(1e-6)).sqrt())).collect();
    for _ in 0..2 {
        for i in 0..m {
            let j = (i + 1) % m;
            let ds = s[i + 1] - s[i];
            v[j] = v[j].min((v[i] * v[i] + 2. * a_acc * ds).sqrt());
        }
        for i in (0..m).rev() {
            let j = (i + 1) % m;
            let ds = s[i + 1] - s[i];
            v[i] = v[i].min((v[j] * v[j] + 2. * a_brk * ds).sqrt());
        }
    }
    Track { x: pts.iter().map(|p| p[0]).collect(), y: pts.iter().map(|p| p[1]).collect(), s, h, k, v, len }
}

/// Truth at distance `d` along the track.
struct Truth {
    x: f64,
    y: f64,
    v: f64,
    k: f64,
    h: f64,
}

impl Track {
    fn at(&self, d: f64, idx: &mut usize) -> Truth {
        let d = d % self.len;
        if d < self.s[*idx] {
            *idx = 0;
        }
        while *idx + 2 < self.s.len() && self.s[*idx + 1] <= d {
            *idx += 1;
        }
        let i = *idx;
        let m = self.x.len();
        let j = (i + 1) % m;
        let f = (d - self.s[i]) / (self.s[i + 1] - self.s[i]);
        let lerp = |a: f64, b: f64| a + f * (b - a);
        Truth { x: lerp(self.x[i], self.x[j]), y: lerp(self.y[i], self.y[j]), v: lerp(self.v[i], self.v[j]), k: lerp(self.k[i], self.k[j]), h: self.h[i] }
    }
}

// ---------------------------------------------------------------- noise

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f64) / (1u64 << 31) as f64 // 0..1
    }
    fn sym(&mut self, a: f64) -> f64 {
        (self.next() - 0.5) * 2. * a
    }
}

fn raw(v: f64, scale: f64, bits: u32) -> u64 {
    ((v / scale).round() as i64 as u64) & ((1u64 << bits) - 1)
}

fn frame(t_us: u64, id: u32, dlc: u8, data: &[u8]) -> CanFrame {
    let mut d = [0_u8; 8];
    d[..data.len()].copy_from_slice(data);
    CanFrame { t_us, id, extended: false, dlc, data: d }
}

fn push_record(log: &mut CardLog<4>, dev: &mut SimCard, kind: Kind, body: &[u8]) {
    loop {
        match log.push(kind, body) {
            Ok(()) => return,
            Err(PushError::Overrun) => {
                block_on(log.service(dev));
            }
            Err(e) => panic!("{e:?}"),
        }
    }
}

fn flush_raw(batch: &mut CanRawBatch, log: &mut CardLog<4>, dev: &mut SimCard) {
    let mut body = [0_u8; 255];
    if let Ok(n) = batch.encode(&mut body) {
        if !batch.is_empty() {
            push_record(log, dev, Kind::CanRaw, &body[..n]);
        }
    }
    batch.clear();
}

fn add_raw(batch: &mut CanRawBatch, f: CanFrame, log: &mut CardLog<4>, dev: &mut SimCard) {
    if !batch.try_push(&f) {
        flush_raw(batch, log, dev);
        assert!(batch.try_push(&f));
    }
}

#[test]
fn write_demo_card() {
    let Some(path) = std::env::var_os("TEL_DEMO_IMAGE") else { return };
    let laps = 3;
    let tr = track();
    let mut dev = SimCard { mem: MemSource { blocks: vec![[0; BLOCK_SIZE]; BLOCKS as usize] } };
    block_on(prepare(&mut dev, geo(), 0xD3_0001)).unwrap();
    let rec = block_on(recover(&mut dev, geo())).unwrap();
    let mut log: CardLog<4> = CardLog::open(geo(), &rec).unwrap();

    let mut fw = [0_u8; 16];
    fw[..7].copy_from_slice(b"demo0.1");
    let info = SessionInfo {
        schema: SCHEMA_VERSION,
        firmware: fw,
        fix_interval_ms: 100,
        can1_bitrate: 1_000_000,
        can2_bitrate: 1_000_000,
        imu_odr_hz: 416,
        t_us: BOOT_US,
        utc_unix_ms: UTC0_MS,
    };
    let mut buf = [0_u8; 255];
    let n = info.encode(&mut buf).unwrap();
    push_record(&mut log, &mut dev, Kind::SessionInfo, &buf[..n]);

    let keys = SbgKeys::resolve(&CATALOG).unwrap();
    let mut latest: Latest<SIGNAL_COUNT> = Latest::new();
    let mut can1 = CanRawBatch::new(1);
    let mut can2 = CanRawBatch::new(2);
    let mut rng = Rng(42);
    let (mut bias_x, mut bias_y) = (0.0_f64, 0.0_f64); // slow LC29H wander, metres
    let (mut frames1, mut frames2) = (0_u32, 0_u32);

    let mut dist = 0.0_f64;
    let mut idx = 0_usize;
    let mut prev_v = tr.at(0.0, &mut idx).v;
    let mut epoch = 0_u64;
    let mono = |ms: u64| BOOT_US + ms * 1000;
    let mut t_ms = 0_u64;
    let mut accel_long_sum;
    let mut accel_lat_sum;
    let mut gyro_z_sum;
    let mut accel_long_peak;
    let mut accel_lat_peak;

    while dist < tr.len * f64::from(laps) {
        accel_long_sum = 0.0;
        accel_lat_sum = 0.0;
        gyro_z_sum = 0.0;
        accel_long_peak = 0.0_f64;
        accel_lat_peak = 0.0_f64;
        let mut truth = tr.at(dist, &mut idx);
        // ten 10 ms steps per 100 ms epoch: the Ellipse streams at 100 Hz
        for sub in 0..10_u64 {
            truth = tr.at(dist, &mut idx);
            let a_long = (truth.v - prev_v) / 0.01;
            prev_v = truth.v;
            let a_lat = truth.v * truth.v * truth.k;
            accel_long_sum += a_long;
            accel_lat_sum += a_lat;
            gyro_z_sum += truth.v * truth.k;
            accel_long_peak = accel_long_peak.max(a_long.abs());
            accel_lat_peak = accel_lat_peak.max(a_lat.abs());
            let t_us = mono(t_ms + sub * 10);
            let yaw = (std::f64::consts::FRAC_PI_2 - truth.h + 2. * std::f64::consts::PI) % (2. * std::f64::consts::PI);
            let yaw = if yaw > std::f64::consts::PI { yaw - 2. * std::f64::consts::PI } else { yaw };
            let roll = -a_lat / G * 0.03;
            let pitch = -a_long / G * 0.02;
            let (ve, vn) = (truth.v * truth.h.cos(), truth.v * truth.h.sin());
            let set = |latest: &mut Latest<SIGNAL_COUNT>, id: u32, bytes: &[u8], t: u64| {
                ingest(&CATALOG, latest, id, false, bytes, t);
            };
            let mut d6 = [0_u8; 6];
            insert(&mut d6, 0, 16, ByteOrder::LittleEndian, raw(roll + rng.sym(0.0003), 1e-4, 16)).unwrap();
            insert(&mut d6, 16, 16, ByteOrder::LittleEndian, raw(pitch + rng.sym(0.0003), 1e-4, 16)).unwrap();
            insert(&mut d6, 32, 16, ByteOrder::LittleEndian, raw(yaw + rng.sym(0.0005), 1e-4, 16)).unwrap();
            set(&mut latest, 306, &d6, t_us);
            add_raw(&mut can2, frame(t_us, 306, 6, &d6), &mut log, &mut dev);
            let mut d6 = [0_u8; 6];
            insert(&mut d6, 0, 16, ByteOrder::LittleEndian, raw(vn + rng.sym(0.02), 0.01, 16)).unwrap();
            insert(&mut d6, 16, 16, ByteOrder::LittleEndian, raw(ve + rng.sym(0.02), 0.01, 16)).unwrap();
            insert(&mut d6, 32, 16, ByteOrder::LittleEndian, raw(rng.sym(0.02), 0.01, 16)).unwrap();
            set(&mut latest, 311, &d6, t_us);
            add_raw(&mut can2, frame(t_us, 311, 6, &d6), &mut log, &mut dev);
            let mut d6 = [0_u8; 6];
            insert(&mut d6, 0, 16, ByteOrder::LittleEndian, raw(a_long + rng.sym(0.08), 0.01, 16)).unwrap();
            insert(&mut d6, 16, 16, ByteOrder::LittleEndian, raw(a_lat + rng.sym(0.08), 0.01, 16)).unwrap();
            insert(&mut d6, 32, 16, ByteOrder::LittleEndian, raw(G + rng.sym(0.08), 0.01, 16)).unwrap();
            set(&mut latest, 289, &d6, t_us);
            add_raw(&mut can2, frame(t_us, 289, 6, &d6), &mut log, &mut dev);
            let mut d6 = [0_u8; 6];
            insert(&mut d6, 0, 16, ByteOrder::LittleEndian, raw(rng.sym(0.004), 0.001, 16)).unwrap();
            insert(&mut d6, 16, 16, ByteOrder::LittleEndian, raw(rng.sym(0.004), 0.001, 16)).unwrap();
            insert(&mut d6, 32, 16, ByteOrder::LittleEndian, raw(truth.v * truth.k + rng.sym(0.004), 0.001, 16)).unwrap();
            set(&mut latest, 290, &d6, t_us);
            add_raw(&mut can2, frame(t_us, 290, 6, &d6), &mut log, &mut dev);
            if sub == 0 {
                // position, altitude and status at 10 Hz
                let lat = LAT0 + (truth.y + rng.sym(0.02)) / 111_320.0;
                let lon = LON0 + (truth.x + rng.sym(0.02)) / (111_320.0 * LAT0.to_radians().cos());
                let mut d8 = [0_u8; 8];
                insert(&mut d8, 0, 32, ByteOrder::LittleEndian, raw(lat, 1e-7, 32)).unwrap();
                insert(&mut d8, 32, 32, ByteOrder::LittleEndian, raw(lon, 1e-7, 32)).unwrap();
                set(&mut latest, 308, &d8, t_us);
                add_raw(&mut can2, frame(t_us, 308, 8, &d8), &mut log, &mut dev);
                let mut d6 = [0_u8; 6];
                insert(&mut d6, 0, 32, ByteOrder::LittleEndian, raw(584.0 + 2.2 * (dist / 120.0).sin(), 0.001, 32)).unwrap();
                set(&mut latest, 309, &d6, t_us);
                add_raw(&mut can2, frame(t_us, 309, 6, &d6), &mut log, &mut dev);
                // EKF solution: AHRS for the first 6 s, then navigation with GPS1 velocity, position and dual-antenna heading
                let sol: u64 = if t_ms < 6_000 { 0x12 } else { 0x2CF4 };
                let mut d6 = [0_u8; 6];
                insert(&mut d6, 0, 32, ByteOrder::LittleEndian, sol).unwrap();
                set(&mut latest, 258, &d6, t_us);
                add_raw(&mut can2, frame(t_us, 258, 6, &d6), &mut log, &mut dev);
                let mut d8 = [0_u8; 8];
                insert(&mut d8, 0, 32, ByteOrder::LittleEndian, t_ms * 1000 + 3_000_000).unwrap();
                insert(&mut d8, 32, 16, ByteOrder::LittleEndian, 0x1F).unwrap();
                set(&mut latest, 256, &d8, t_us);
                add_raw(&mut can2, frame(t_us, 256, 8, &d8), &mut log, &mut dev);
                frames2 += 4;
                // invented MoTeC traffic on bus 1: two 50 Hz ids (every 2nd epoch step) and one 10 Hz id
                let rpm = 3500.0 + 6500.0 * (truth.v / 21.0) + rng.sym(60.0);
                let d = (rpm as u16).to_be_bytes();
                add_raw(&mut can1, frame(t_us, 0x640, 8, &[d[0], d[1], 0x55, 0x10, 0x01, 0x2C, 0, 0]), &mut log, &mut dev);
                let tps = (truth.v / 21.0 * 900.0) as u16;
                add_raw(&mut can1, frame(t_us, 0x660, 4, &[(tps >> 8) as u8, tps as u8, 0x03, 0xE8]), &mut log, &mut dev);
                frames1 += 2;
            }
            if sub % 2 == 0 {
                add_raw(&mut can1, frame(t_us, 0x641, 8, &[0x0B, 0xB8, 0x04, 0x4C, 0, 0, 0xA5, (t_ms / 100) as u8]), &mut log, &mut dev);
                frames1 += 1;
            }
            frames2 += 5;
            dist += truth.v * 0.01;
        }
        let mut sig = SignalsBatch::new(2, mono(t_ms + 20));
        for id in keys.all() {
            if let Some(s) = latest.get(id) {
                sig.try_push(id.0, s.raw);
            }
        }
        let mut sbody = [0_u8; 255];
        let sn = sig.encode(&mut sbody).unwrap();
        push_record(&mut log, &mut dev, Kind::Signals, &sbody[..sn]);

        // the onboard LC29H wanders; its heading comes from the second antenna
        bias_x += rng.sym(0.06);
        bias_y += rng.sym(0.06);
        bias_x *= 0.995;
        bias_y *= 0.995;
        let px = truth.x + bias_x + rng.sym(0.25);
        let py = truth.y + bias_y + rng.sym(0.25);
        let lat = LAT0 + py / 111_320.0;
        let lon = LON0 + px / (111_320.0 * LAT0.to_radians().cos());
        let cog = ((90.0 - truth.h.to_degrees()).rem_euclid(360.0) * 100.0).round() as u16;
        let heading = (90.0 - truth.h.to_degrees() + 1.5 * (epoch as f64 / 9.0).sin() + rng.sym(0.3)).rem_euclid(360.0);
        let ug = |a: f64| (a / G * 1e6).round() as i32;
        let sample = Sample {
            t_us: mono(t_ms + 23),
            utc_unix_ms: UTC0_MS + t_ms as i64 + 23,
            flags: flags::GNSS_VALID | flags::HEADING_VALID | flags::UTC_VALID | flags::IMU_VALID | flags::CAN2_ALIVE | flags::CAN1_ALIVE | flags::PPS_LOCKED,
            lat_e9: (lat * 1e9).round() as i64,
            lon_e9: (lon * 1e9).round() as i64,
            alt_mm: (584_000.0 + 2200.0 * (dist / 120.0).sin() + rng.sym(150.0)) as i32,
            sog_mm_s: ((truth.v + rng.sym(0.05)) * 1000.0).max(0.0) as u32,
            cog_cdeg: cog % 36_000,
            fix_quality: 1,
            num_sv: (24.0 + 2.0 * (epoch as f64 / 45.0).sin() + rng.sym(0.6)).round() as u8,
            hdop_e2: (70.0 + 12.0 * (epoch as f64 / 30.0).sin() + rng.next() * 4.0) as u16,
            diff_age_ds: u16::MAX,
            heading_e6: (heading * 1e6) as u32,
            pitch_e6: (rng.sym(0.4) * 1e6) as i32,
            baseline_mm: 860,
            acc_heading_e6: 50_000,
            heading_quality: 1,
            heading_sv: 19,
            accel_mean_ug: [ug(accel_long_sum / 10.0 + rng.sym(0.15)), ug(accel_lat_sum / 10.0 + rng.sym(0.15)), ug(G + rng.sym(0.15))],
            accel_peak_ug: [ug(accel_long_peak + 1.2), ug(accel_lat_peak + 1.2), ug(G + 1.6)],
            gyro_mean_mdps: [(rng.sym(0.8) * 1000.0) as i32, (rng.sym(0.8) * 1000.0) as i32, ((gyro_z_sum / 10.0).to_degrees() * 1000.0) as i32],
            gyro_peak_mdps: [3000, 3000, ((gyro_z_sum / 10.0).to_degrees().abs() * 1000.0) as i32 + 4000],
            imu_temp_centi_c: (3100.0 + epoch as f64 * 0.12) as i16,
            imu_count: 41,
            nmea_errors: 0,
            overruns: 0,
            can1_frames: frames1,
            can2_frames: frames2,
        };
        let sn = sample.encode(&mut buf).unwrap();
        push_record(&mut log, &mut dev, Kind::Sample, &buf[..sn]);
        epoch += 1;
        t_ms += 100;
    }
    flush_raw(&mut can1, &mut log, &mut dev);
    flush_raw(&mut can2, &mut log, &mut dev);
    while log.close() == Err(PushError::Overrun) {
        block_on(log.service(&mut dev));
    }
    block_on(log.service(&mut dev));
    let bytes: Vec<u8> = dev.mem.blocks.iter().flatten().copied().collect();
    std::fs::write(path, bytes).unwrap();
}
