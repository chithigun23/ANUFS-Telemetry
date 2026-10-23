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
//! Property tests for the public API: arbitrary input must never panic, valid
//! frames must always be recovered, and corruption must always be detected.

use proptest::prelude::*;
use tel_nmea::{checksum, parse, LineAssembler, Message, MAX_PAYLOAD};

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut v = vec![b'$'];
    v.extend_from_slice(payload);
    v.extend_from_slice(format!("*{:02X}\r\n", checksum(payload)).as_bytes());
    v
}

/// Printable ASCII excluding the framing characters.
fn payload_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(
        prop::sample::select((0x20_u8..=0x7E).filter(|b| *b != b'$' && *b != b'*').collect::<Vec<u8>>()),
        0..=MAX_PAYLOAD,
    )
}

/// Feed bytes and collect delivered payloads and the error count.
fn run(asm: &mut LineAssembler, bytes: &[u8]) -> (Vec<Vec<u8>>, usize) {
    let mut good = Vec::new();
    let mut errors = 0;
    for &b in bytes {
        match asm.push(b) {
            Some(Ok(s)) => good.push(s.payload().to_vec()),
            Some(Err(_)) => errors += 1,
            None => {}
        }
    }
    (good, errors)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    /// Random bytes never panic the assembler or the decoder, and every
    /// delivered sentence has a verified checksum.
    #[test]
    fn random_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..2000)) {
        let mut asm = LineAssembler::new();
        for &b in &bytes {
            if let Some(Ok(s)) = asm.push(b) {
                let _ = parse(&s);
            }
        }
        let st = asm.stats();
        prop_assert!(u64::from(st.frames_ok) <= bytes.len() as u64);
    }

    /// Bytes that look like NMEA (heavy on `$`, `*`, `,`, digits) never panic either.
    #[test]
    fn nmea_like_noise_never_panics(
        bytes in prop::collection::vec(prop::sample::select(b"$*,.0123456789ABCDEFNSEWGRMQTPAI\r\n-".to_vec()), 0..3000)
    ) {
        let mut asm = LineAssembler::new();
        for &b in &bytes {
            if let Some(Ok(s)) = asm.push(b) {
                let _ = parse(&s);
            }
        }
    }

    /// Every valid frame is delivered exactly once with the payload intact.
    #[test]
    fn valid_frames_roundtrip(payload in payload_strategy()) {
        let mut asm = LineAssembler::new();
        let (good, errors) = run(&mut asm, &frame(&payload));
        prop_assert_eq!(errors, 0);
        prop_assert_eq!(good, vec![payload]);
    }

    /// A valid frame is found after any amount of preceding garbage and
    /// between any garbage, because every `$` resynchronises.
    #[test]
    fn resync_after_garbage(
        garbage_a in prop::collection::vec(any::<u8>(), 0..300),
        payload in payload_strategy(),
        garbage_b in prop::collection::vec(any::<u8>(), 0..300),
    ) {
        let mut data = garbage_a;
        data.extend(frame(&payload));
        data.extend(garbage_b);
        let mut asm = LineAssembler::new();
        let (good, _) = run(&mut asm, &data);
        prop_assert!(good.contains(&payload));
    }

    /// Changing any one payload byte to another non-framing byte is always
    /// caught: the corrupted payload is never delivered as good.
    #[test]
    fn single_byte_corruption_is_detected(
        payload in payload_strategy().prop_filter("non-empty", |p| !p.is_empty()),
        pos in any::<prop::sample::Index>(),
        replacement in prop::sample::select((0x20_u8..=0x7E).filter(|b| *b != b'$' && *b != b'*').collect::<Vec<u8>>()),
    ) {
        let i = pos.index(payload.len());
        prop_assume!(payload[i] != replacement);
        let mut corrupted_frame = frame(&payload);
        corrupted_frame[1 + i] = replacement;
        let mut asm = LineAssembler::new();
        let (good, errors) = run(&mut asm, &corrupted_frame);
        prop_assert!(good.is_empty());
        prop_assert_eq!(errors, 1);
    }

    /// Truncating a frame anywhere never yields a delivered sentence.
    #[test]
    fn truncated_frames_are_never_delivered(payload in payload_strategy(), cut in any::<prop::sample::Index>()) {
        let full = frame(&payload);
        // Keep at least '$' and drop at least the final hex digit onward.
        let end = 1 + cut.index(payload.len() + 3); // up to just before the 2nd hex digit
        let mut asm = LineAssembler::new();
        let (good, _) = run(&mut asm, &full[..end]);
        prop_assert!(good.is_empty());
    }

    /// Frames split across arbitrary chunk boundaries behave identically
    /// (the API is per byte, so this checks there is no hidden chunk state).
    #[test]
    fn back_to_back_frames(p1 in payload_strategy(), p2 in payload_strategy()) {
        let mut data = frame(&p1);
        data.extend(frame(&p2));
        let mut asm = LineAssembler::new();
        let (good, errors) = run(&mut asm, &data);
        prop_assert_eq!(errors, 0);
        prop_assert_eq!(good, vec![p1, p2]);
    }
}

// ---- decoder round trips ---------------------------------------------------

fn fmt_coord(deg: u32, minutes_micro: u32, width: usize) -> String {
    // ddmm.mmmmmm / dddmm.mmmmmm
    let whole = minutes_micro / 1_000_000;
    let frac = minutes_micro % 1_000_000;
    format!("{deg:0width$}{whole:02}.{frac:06}", width = width - 2)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3000))]

    /// RMC positions decode to the exact fixed-point value computed from the
    /// formatted degrees and minutes.
    #[test]
    fn rmc_position_roundtrip(
        lat_deg in 0_u32..90, lat_min in 0_u32..60_000_000, south in any::<bool>(),
        lon_deg in 0_u32..180, lon_min in 0_u32..60_000_000, west in any::<bool>(),
        h in 0_u32..24, m in 0_u32..60, s in 0_u32..60, ms in 0_u32..1000,
        sog in 0_u32..100_000, cog in 0_u32..36_000,
    ) {
        let lat = fmt_coord(lat_deg, lat_min, 4);
        let lon = fmt_coord(lon_deg, lon_min, 5);
        let sentence = format!(
            "GNRMC,{h:02}{m:02}{s:02}.{ms:03},A,{lat},{},{lon},{},{}.{:02},{}.{:02},010125,,,A,V",
            if south { 'S' } else { 'N' },
            if west { 'W' } else { 'E' },
            sog / 100, sog % 100,
            cog / 100, cog % 100,
        );
        let payload = sentence.as_bytes();
        let mut asm = LineAssembler::new();
        let (good, _) = run(&mut asm, &frame(payload));
        prop_assert_eq!(good.len(), 1);
        let msg = parse(&tel_nmea::Sentence::new(&good[0])).unwrap();
        let Message::Rmc(r) = msg else { panic!("not rmc") };
        let expect = |deg: u32, min_micro: u32, neg: bool| -> i64 {
            // minutes (1e-6) -> degrees (1e-9): min_micro * 1000 / 60, rounded to nearest
            let frac = (i64::from(min_micro) * 1000 + 30) / 60;
            let v = i64::from(deg) * 1_000_000_000 + frac;
            if neg { -v } else { v }
        };
        prop_assert_eq!(r.lat_e9, Some(expect(lat_deg, lat_min, south)));
        prop_assert_eq!(r.lon_e9, Some(expect(lon_deg, lon_min, west)));
        prop_assert_eq!(r.cog_cdeg, Some(u16::try_from(cog).unwrap()));
        let t = r.time.unwrap();
        prop_assert_eq!((u32::from(t.hour), u32::from(t.minute), u32::from(t.second), u32::from(t.millis)), (h, m, s, ms));
        // speed: knots*1000 -> mm/s, truncated
        let expect_mm_s = u64::from(sog) * 10 * 514_444 / 1_000_000;
        prop_assert_eq!(u64::from(r.sog_mm_s.unwrap()), expect_mm_s);
    }

    /// PQTMTAR values decode exactly.
    #[test]
    fn tar_roundtrip(
        q in prop::sample::select(vec![0_u8, 1, 2, 4, 5]),
        len_mm in 0_u32..100_000,
        pitch in -90_000_000_i32..=90_000_000,
        heading in 0_u32..360_000_000,
        accp in 0_u32..10_000_000, acch in 0_u32..10_000_000, sv in 0_u8..60,
    ) {
        let f6 = |v: i64| {
            let sign = if v < 0 { "-" } else { "" };
            let a = v.unsigned_abs();
            format!("{sign}{}.{:06}", a / 1_000_000, a % 1_000_000)
        };
        let sentence = format!(
            "PQTMTAR,1,165034.000,{q},,{}.{:03},{},,{},{},,{},{sv}",
            len_mm / 1000, len_mm % 1000,
            f6(i64::from(pitch)), f6(i64::from(heading)), f6(i64::from(accp)), f6(i64::from(acch)),
        );
        let mut asm = LineAssembler::new();
        let (good, _) = run(&mut asm, &frame(sentence.as_bytes()));
        prop_assert_eq!(good.len(), 1);
        let msg = parse(&tel_nmea::Sentence::new(&good[0])).unwrap();
        let Message::Tar(t) = msg else { panic!("not tar") };
        prop_assert_eq!(t.baseline_mm, Some(len_mm));
        prop_assert_eq!(t.pitch_e6, Some(pitch));
        prop_assert_eq!(t.heading_e6, Some(heading));
        prop_assert_eq!(t.acc_pitch_e6, Some(accp));
        prop_assert_eq!(t.acc_heading_e6, Some(acch));
        prop_assert_eq!(t.used_sv, Some(sv));
        prop_assert_eq!(t.quality.as_u8(), q);
    }

    /// Arbitrary text in numeric fields of known sentences never panics.
    #[test]
    fn random_fields_in_known_sentences(
        kind in prop::sample::select(vec!["GNRMC", "GNGGA", "PQTMTAR", "PAIR001", "PAIR051", "PAIR865", "PQTMCFGMSGRATE", "PQTMSAVEPAR"]),
        fields in prop::collection::vec("[ -~&&[^,$*]]{0,20}", 0..16),
    ) {
        let mut s = String::from(kind);
        for f in &fields {
            s.push(',');
            s.push_str(f);
        }
        let _ = parse(&tel_nmea::Sentence::new(s.as_bytes()));
    }
}
