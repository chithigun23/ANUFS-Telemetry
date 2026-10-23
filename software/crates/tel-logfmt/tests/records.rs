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
    clippy::type_complexity,
    clippy::useless_vec,
    clippy::needless_range_loop
)]
//! Record codec tests.

use proptest::prelude::*;
use tel_logfmt::records::{
    append_record, CanFrame, CanRawBatch, CanRawView, DecodeError, EncodeError, Event, Kind, Record, Records, Sample,
    SessionInfo, SignalsBatch, SignalsView, Truncated, MAX_BODY, MAX_SIGNALS,
};

fn arb_sample() -> impl Strategy<Value = Sample> {
    let a3 = || prop::array::uniform3(any::<i32>());
    (
        (any::<u64>(), any::<i64>(), any::<u32>(), any::<i64>(), any::<i64>(), any::<i32>(), any::<u32>()),
        (any::<u16>(), any::<u8>(), any::<u8>(), any::<u16>(), any::<u16>(), any::<u32>(), any::<i32>()),
        (any::<u32>(), any::<u32>(), any::<u8>(), any::<u8>()),
        (a3(), a3(), a3(), a3()),
        (any::<i16>(), any::<u16>(), any::<u32>(), any::<u32>(), any::<u32>(), any::<u32>()),
    )
        .prop_map(|(a, b, c, d, e)| Sample {
            t_us: a.0,
            utc_unix_ms: a.1,
            flags: a.2,
            lat_e9: a.3,
            lon_e9: a.4,
            alt_mm: a.5,
            sog_mm_s: a.6,
            cog_cdeg: b.0,
            fix_quality: b.1,
            num_sv: b.2,
            hdop_e2: b.3,
            diff_age_ds: b.4,
            heading_e6: b.5,
            pitch_e6: b.6,
            baseline_mm: c.0,
            acc_heading_e6: c.1,
            heading_quality: c.2,
            heading_sv: c.3,
            accel_mean_ug: d.0,
            accel_peak_ug: d.1,
            gyro_mean_mdps: d.2,
            gyro_peak_mdps: d.3,
            imu_temp_centi_c: e.0,
            imu_count: e.1,
            nmea_errors: e.2,
            overruns: e.3,
            can1_frames: e.4,
            can2_frames: e.5,
        })
}

proptest! {
    #[test]
    fn sample_roundtrip(s in arb_sample()) {
        let mut buf = [0_u8; 256];
        let n = s.encode(&mut buf).unwrap();
        prop_assert_eq!(n, Sample::LEN);
        prop_assert_eq!(Sample::decode(&buf[..n]).unwrap(), s);
        // trailing bytes (a newer schema) are ignored
        prop_assert_eq!(Sample::decode(&buf[..n + 50]).unwrap(), s);
    }

    #[test]
    fn event_roundtrip(t in any::<u64>(), code in any::<u16>(), arg in any::<u32>()) {
        let e = Event { t_us: t, code, arg };
        let mut b = [0_u8; 32];
        let n = e.encode(&mut b).unwrap();
        prop_assert_eq!(n, Event::LEN);
        prop_assert_eq!(Event::decode(&b[..n]).unwrap(), e);
    }

    #[test]
    fn session_info_roundtrip(
        fw in prop::array::uniform16(any::<u8>()), fix in any::<u16>(), c1 in any::<u32>(), c2 in any::<u32>(),
        odr in any::<u16>(), t in any::<u64>(), utc in any::<i64>(),
    ) {
        let s = SessionInfo { schema: 1, firmware: fw, fix_interval_ms: fix, can1_bitrate: c1, can2_bitrate: c2, imu_odr_hz: odr, t_us: t, utc_unix_ms: utc };
        let mut b = [0_u8; 64];
        let n = s.encode(&mut b).unwrap();
        prop_assert_eq!(n, SessionInfo::LEN);
        prop_assert_eq!(SessionInfo::decode(&b[..n]).unwrap(), s);
    }

    /// Decoders are total: any bytes give Ok or Err, never a panic.
    #[test]
    fn decoders_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..400)) {
        let _ = Sample::decode(&bytes);
        let _ = Event::decode(&bytes);
        let _ = SessionInfo::decode(&bytes);
        if let Ok(v) = CanRawView::parse(&bytes) {
            for f in v.frames() {
                let _ = f;
            }
        }
        if let Ok(v) = SignalsView::parse(&bytes) {
            let _ = v.entries().count();
        }
        let _ = Records::new(&bytes).count();
    }
}

#[test]
fn sample_byte_layout_is_pinned() {
    let s = Sample { t_us: 0x0102_0304_0506_0708, utc_unix_ms: -2, flags: 0xA1B2_C3D4, lat_e9: 1, ..Sample::default() };
    let mut b = [0_u8; 256];
    let n = s.encode(&mut b).unwrap();
    assert_eq!(n, 138);
    assert_eq!(&b[0..8], &[8, 7, 6, 5, 4, 3, 2, 1]);
    assert_eq!(&b[8..16], &(-2_i64).to_le_bytes());
    assert_eq!(&b[16..20], &[0xD4, 0xC3, 0xB2, 0xA1]);
    assert_eq!(&b[20..28], &1_i64.to_le_bytes());
}

#[test]
fn every_truncation_of_every_record_is_an_error() {
    let mut b = [0_u8; 256];
    let n = Sample::default().encode(&mut b).unwrap();
    for cut in 0..n {
        assert_eq!(Sample::decode(&b[..cut]), Err(DecodeError::Truncated), "sample cut {cut}");
    }
    let n = Event { t_us: 1, code: 2, arg: 3 }.encode(&mut b).unwrap();
    for cut in 0..n {
        assert_eq!(Event::decode(&b[..cut]), Err(DecodeError::Truncated));
    }
    let info = SessionInfo {
        schema: 1,
        firmware: [0; 16],
        fix_interval_ms: 100,
        can1_bitrate: 0,
        can2_bitrate: 0,
        imu_odr_hz: 416,
        t_us: 0,
        utc_unix_ms: 0,
    };
    let n = info.encode(&mut b).unwrap();
    for cut in 0..n {
        assert_eq!(SessionInfo::decode(&b[..cut]), Err(DecodeError::Truncated));
    }
}

#[test]
fn encoders_reject_every_too_small_buffer() {
    let s = Sample::default();
    for cap in 0..Sample::LEN {
        let mut b = vec![0_u8; cap];
        assert_eq!(s.encode(&mut b), Err(EncodeError::BufferTooSmall), "cap {cap}");
    }
    assert_eq!(Sample::default().encode(&mut vec![0_u8; Sample::LEN]), Ok(Sample::LEN));
    for cap in 0..Event::LEN {
        assert_eq!(Event { t_us: 0, code: 0, arg: 0 }.encode(&mut vec![0_u8; cap]), Err(EncodeError::BufferTooSmall));
    }
}

fn frame(t: u64, id: u32, ext: bool, data: &[u8]) -> CanFrame {
    let mut d = [0_u8; 8];
    d[..data.len()].copy_from_slice(data);
    CanFrame { t_us: t, id, extended: ext, dlc: data.len() as u8, data: d }
}

#[test]
fn can_raw_batch_roundtrip_and_layout() {
    let mut b = CanRawBatch::new(2);
    assert!(b.is_empty());
    let f1 = frame(1_000_000, 0x640, false, &[1, 2, 3, 4, 5, 6, 7, 8]);
    let f2 = frame(1_000_250, 0x18FF_0001, true, &[9]);
    let f3 = frame(1_000_900, 0x7FF, false, &[]);
    assert!(b.try_push(&f1) && b.try_push(&f2) && b.try_push(&f3));
    assert_eq!(b.count(), 3);
    let mut out = [0_u8; 255];
    let n = b.encode(&mut out).unwrap();
    assert!(n <= MAX_BODY);
    assert_eq!(out[0], 2, "bus");
    assert_eq!(out[1], 3, "count");
    assert_eq!(&out[2..10], &1_000_000_u64.to_le_bytes());
    assert_eq!(&out[10..14], &0_u32.to_le_bytes(), "first dt");
    let view = CanRawView::parse(&out[..n]).unwrap();
    assert_eq!(view.bus, 2);
    let got: Vec<CanFrame> = view.frames().map(Result::unwrap).collect();
    assert_eq!(got, vec![f1, f2, f3]);
}

#[test]
fn can_raw_batch_fills_then_refuses_without_corrupting() {
    let mut b = CanRawBatch::new(1);
    let mut pushed = Vec::new();
    for i in 0..100_u32 {
        let f = frame(u64::from(i) * 100, 0x100 + i, false, &[i as u8; 8]);
        if b.try_push(&f) {
            pushed.push(f);
        } else {
            break;
        }
    }
    assert!(pushed.len() >= 10, "an 8 byte frame costs 17 bytes, so about 14 fit");
    let mut out = [0_u8; 255];
    let n = b.encode(&mut out).unwrap();
    assert!(n <= MAX_BODY);
    let got: Vec<CanFrame> = CanRawView::parse(&out[..n]).unwrap().frames().map(Result::unwrap).collect();
    assert_eq!(got, pushed);
    // After clear it accepts again.
    b.clear();
    assert!(b.is_empty());
    assert!(b.try_push(&pushed[0]));
}

#[test]
fn can_raw_batch_rejects_invalid_frames_and_big_time_gaps() {
    let mut b = CanRawBatch::new(1);
    assert!(
        !b.try_push(&CanFrame { t_us: 0, id: 0x800, extended: false, dlc: 1, data: [0; 8] }),
        "11-bit id too large"
    );
    assert!(
        !b.try_push(&CanFrame { t_us: 0, id: 0x2000_0000, extended: true, dlc: 1, data: [0; 8] }),
        "29-bit id too large"
    );
    assert!(!b.try_push(&CanFrame { t_us: 0, id: 1, extended: false, dlc: 9, data: [0; 8] }), "dlc > 8");
    assert!(b.is_empty());
    assert!(b.try_push(&frame(10, 1, false, &[1])));
    assert!(!b.try_push(&frame(9, 1, false, &[1])), "time going backwards");
    assert!(!b.try_push(&frame(10 + (1 << 32), 1, false, &[1])), "dt does not fit in u32");
    assert!(b.try_push(&frame(10 + u64::from(u32::MAX), 1, false, &[1])), "largest representable dt");
    assert_eq!(b.count(), 2);
}

#[test]
fn can_raw_view_stops_at_first_error_and_never_overreads() {
    let mut b = CanRawBatch::new(1);
    for i in 0..5_u32 {
        assert!(b.try_push(&frame(u64::from(i), i + 1, false, &[1, 2, 3])));
    }
    let mut out = [0_u8; 255];
    let n = b.encode(&mut out).unwrap();
    for cut in 10..n {
        let v = CanRawView::parse(&out[..cut]).unwrap();
        let results: Vec<_> = v.frames().collect();
        let oks = results.iter().take_while(|r| r.is_ok()).count();
        assert!(oks <= 5);
        // anything after the first error is nothing
        if let Some(pos) = results.iter().position(Result::is_err) {
            assert_eq!(results.len(), pos + 1);
        }
    }
    // bad dlc
    let mut bad = out;
    bad[10 + 8] = 9;
    let v = CanRawView::parse(&bad[..n]).unwrap();
    assert!(matches!(v.frames().next(), Some(Err(DecodeError::BadValue))));
}

#[test]
fn signals_batch_roundtrip_and_capacity() {
    assert_eq!(MAX_SIGNALS, 24);
    let mut b = SignalsBatch::new(2, 123_456);
    assert!(b.is_empty());
    for i in 0..MAX_SIGNALS {
        assert!(b.try_push(i as u16 * 3, u64::MAX - i as u64));
    }
    assert!(!b.try_push(999, 1), "full");
    assert_eq!(b.len(), MAX_SIGNALS);
    let mut out = [0_u8; 255];
    let n = b.encode(&mut out).unwrap();
    assert!(n <= MAX_BODY);
    let v = SignalsView::parse(&out[..n]).unwrap();
    assert_eq!((v.bus, v.t_us, v.len()), (2, 123_456, 24));
    let got: Vec<(u16, u64)> = v.entries().collect();
    assert_eq!(got.len(), 24);
    assert_eq!(got[5], (15, u64::MAX - 5));
    // truncated bodies are rejected up front
    assert_eq!(SignalsView::parse(&out[..n - 1]).err(), Some(DecodeError::Truncated));
    assert_eq!(SignalsView::parse(&out[..5]).err(), Some(DecodeError::Truncated));
}

#[test]
fn record_framing_roundtrip_and_limits() {
    let mut payload = [0_u8; 100];
    let mut len = 0;
    assert!(append_record(&mut payload, &mut len, Kind::Event, &[1, 2, 3]));
    assert!(append_record(&mut payload, &mut len, Kind::Sample, &[]));
    assert!(append_record(&mut payload, &mut len, Kind::Unknown(77), &[9; 10]));
    assert_eq!(len, 5 + 2 + 12);
    let recs: Vec<_> = Records::new(&payload[..len]).collect();
    assert_eq!(
        recs,
        vec![
            Ok(Record { kind: Kind::Event, body: &[1, 2, 3] }),
            Ok(Record { kind: Kind::Sample, body: &[] }),
            Ok(Record { kind: Kind::Unknown(77), body: &[9; 10] }),
        ]
    );
    // does not fit: unchanged
    let before = (payload, len);
    assert!(!append_record(&mut payload, &mut len, Kind::Event, &[0; 90]));
    assert_eq!((payload, len), before);
    // body larger than 255 is refused even with room
    let mut big = vec![0_u8; 600];
    let mut l = 0;
    assert!(!append_record(&mut big, &mut l, Kind::Event, &[0; 256]));
    assert!(append_record(&mut big, &mut l, Kind::Event, &[0; 255]));
}

#[test]
fn kind_codes_roundtrip_for_every_byte() {
    for c in 0..=255_u8 {
        assert_eq!(Kind::from_code(c).code(), c);
    }
}

#[test]
fn truncated_record_streams_report_once_then_stop() {
    // kind byte with no length byte
    assert_eq!(Records::new(&[2]).collect::<Vec<_>>(), vec![Err(Truncated)]);
    // length says 5 but only 2 bytes follow
    assert_eq!(Records::new(&[2, 5, 1, 2]).collect::<Vec<_>>(), vec![Err(Truncated)]);
    // good record followed by a torn one
    let r: Vec<_> = Records::new(&[5, 1, 0xAA, 2, 9, 1]).collect();
    assert_eq!(r.len(), 2);
    assert!(r[0].is_ok());
    assert_eq!(r[1], Err(Truncated));
    assert_eq!(Records::new(&[]).count(), 0);
}

proptest! {
    /// Records appended with `append_record` are returned unchanged and in order.
    #[test]
    fn framing_roundtrip(recs in prop::collection::vec((any::<u8>(), prop::collection::vec(any::<u8>(), 0..60)), 0..20)) {
        let mut payload = [0_u8; 488];
        let mut len = 0;
        let mut kept = Vec::new();
        for (k, body) in &recs {
            if append_record(&mut payload, &mut len, Kind::from_code(*k), body) {
                kept.push((*k, body.clone()));
            }
        }
        let got: Vec<(u8, Vec<u8>)> = Records::new(&payload[..len]).map(|r| { let r = r.unwrap(); (r.kind.code(), r.body.to_vec()) }).collect();
        prop_assert_eq!(got, kept);
    }
}
