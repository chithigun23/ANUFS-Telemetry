//! Bridge between the Android app and the log reader.
//!
//! The app hands over the path of a card image (or a `/proc/self/fd/N` path of
//! a file the user picked) and gets JSON back. Every function here is safe Rust
//! that returns a `String`, so it is tested on the PC; the `Java_*` functions at
//! the bottom only move strings across the JNI boundary.
//!
//! Errors never cross the boundary as exceptions: they come back as
//! `{"error": "..."}` and the Kotlin side turns them into exceptions.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{BufWriter, Write};
use tel_decode::cansummary::can_summary;
use tel_decode::export::{export, session_info, What};
use tel_decode::gpx::export_gpx;
use tel_decode::scan::{read_header, scan, ScanError, DEFAULT_STOP_AFTER_INVALID};
use tel_decode::source::{BlockSource, FileSource};
use tel_logfmt::recovery::Geometry;

/// Append `s` as a JSON string literal.
fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn error_json(msg: &str) -> String {
    let mut o = String::from("{\"error\":");
    json_str(&mut o, msg);
    o.push('}');
    o
}

/// Open `path` and work out the log region: the whole medium, or the size the
/// region header claims when that differs.
fn open(path: &str) -> Result<(FileSource, Geometry, u32), String> {
    let mut src = FileSource::open(path).map_err(|e| format!("{path}: {e}"))?;
    let total = src.len_blocks().map_err(|e| e.to_string())?;
    let blocks = u32::try_from(total).unwrap_or(u32::MAX);
    let mut geo = Geometry { start_lba: 0, blocks };
    let header = match read_header(&mut src, geo) {
        Err(ScanError::GeometryMismatch { found }) => {
            geo = Geometry { start_lba: 0, blocks: found };
            read_header(&mut src, geo).map_err(|e| e.to_string())?
        }
        other => other.map_err(|e| e.to_string())?,
    };
    Ok((src, geo, header.card_id))
}

/// List the sessions on a card image as JSON.
pub fn scan_json(path: &str) -> String {
    match scan_inner(path) {
        Ok(s) => s,
        Err(e) => error_json(&e),
    }
}

fn scan_inner(path: &str) -> Result<String, String> {
    let (mut src, geo, card_id) = open(path)?;
    let report = scan(&mut src, geo, DEFAULT_STOP_AFTER_INVALID).map_err(|e| e.to_string())?;
    let mut infos = HashMap::new();
    for run in &report.runs {
        if let std::collections::hash_map::Entry::Vacant(slot) = infos.entry(run.session) {
            if let Ok(Some(i)) = session_info(&mut src, geo, card_id, run.session, DEFAULT_STOP_AFTER_INVALID) {
                slot.insert(i);
            }
        }
    }
    let mut o = String::new();
    let _ = write!(
        o,
        "{{\"cardId\":{},\"regionBlocks\":{},\"validBlocks\":{},\"invalidBlocks\":{},\"foreignBlocks\":{},\"readErrors\":{},\"runs\":[",
        report.card_id,
        report.region_blocks,
        report.valid_blocks,
        report.invalid_blocks,
        report.foreign_blocks,
        report.read_errors
    );
    for (n, r) in report.runs.iter().enumerate() {
        if n > 0 {
            o.push(',');
        }
        let _ = write!(
            o,
            "{{\"session\":{},\"blocks\":{},\"firstSeq\":{},\"lastSeq\":{},\"cleanClose\":{},\"gapBefore\":{}",
            r.session, r.blocks, r.first_seq, r.last_seq, r.clean_close, r.gap_before
        );
        if let Some(i) = infos.get(&r.session) {
            let fw = String::from_utf8_lossy(&i.firmware);
            o.push_str(",\"firmware\":");
            json_str(&mut o, fw.trim_end_matches('\0'));
            let _ = write!(
                o,
                ",\"utcUnixMs\":{},\"fixIntervalMs\":{},\"can1Bitrate\":{},\"can2Bitrate\":{},\"imuOdrHz\":{}",
                i.utc_unix_ms, i.fix_interval_ms, i.can1_bitrate, i.can2_bitrate, i.imu_odr_hz
            );
        }
        o.push('}');
    }
    o.push_str("]}");
    Ok(o)
}

/// Export one kind of data for one session to `out_path`.
///
/// `kind` is `samples`, `signals`, `events`, `can-raw`, `can-summary` (JSON) or `gpx`. Returns
/// `{"rows":N}` (points for GPX) or `{"error":"..."}`.
pub fn export_json(path: &str, session: u32, kind: &str, out_path: &str) -> String {
    match export_inner(path, session, kind, out_path) {
        Ok(rows) => format!("{{\"rows\":{rows}}}"),
        Err(e) => error_json(&e),
    }
}

fn export_inner(path: &str, session: u32, kind: &str, out_path: &str) -> Result<u64, String> {
    let (mut src, geo, card_id) = open(path)?;
    let file = std::fs::File::create(out_path).map_err(|e| format!("{out_path}: {e}"))?;
    let mut out = BufWriter::new(file);
    let rows = if kind == "can-summary" {
        let (ids, _) = can_summary(&mut src, geo, card_id, session, DEFAULT_STOP_AFTER_INVALID)
            .map_err(|e| e.to_string())?;
        out.write_all(can_summary_json(&ids).as_bytes()).map_err(|e| e.to_string())?;
        ids.len() as u64
    } else if kind == "gpx" {
        export_gpx(&mut src, geo, card_id, session, DEFAULT_STOP_AFTER_INVALID, &mut out)
            .map_err(|e| e.to_string())?
            .0
            .points
    } else {
        let what = match kind {
            "samples" => What::Samples,
            "signals" => What::Signals,
            "events" => What::Events,
            "can-raw" => What::CanRaw,
            other => return Err(format!("unknown export kind {other}")),
        };
        export(&mut src, geo, card_id, Some(session), what, DEFAULT_STOP_AFTER_INVALID, &mut out)
            .map_err(|e| e.to_string())?
            .0
            .rows
    };
    out.flush().map_err(|e| e.to_string())?;
    Ok(rows)
}

/// The per-id CAN statistics as a JSON array.
pub fn can_summary_json(ids: &[tel_decode::cansummary::IdStat]) -> String {
    let mut o = String::from("[");
    for (n, i) in ids.iter().enumerate() {
        if n > 0 {
            o.push(',');
        }
        let mut hex = String::new();
        for b in i.last.iter().take(usize::from(i.dlc)) {
            let _ = write!(hex, "{b:02X}");
        }
        let _ = write!(
            o,
            "{{\"bus\":{},\"id\":{},\"extended\":{},\"count\":{},\"firstUs\":{},\"lastUs\":{},\"dlc\":{},\"last\":\"{hex}\"",
            i.bus, i.id, i.extended, i.count, i.first_us, i.last_us, i.dlc
        );
        if let Some(name) = i.name {
            o.push_str(",\"name\":");
            json_str(&mut o, name);
        }
        o.push('}');
    }
    o.push(']');
    o
}

// ---------------------------------------------------------------- JNI entry points

use jni::objects::{JClass, JString};
use jni::EnvUnowned;

/// `au.anufs.telemetry.core.Native.scan(path: String): String`
#[unsafe(no_mangle)]
pub extern "system" fn Java_au_anufs_telemetry_core_Native_scan<'caller>(
    mut unowned_env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    path: JString<'caller>,
) -> JString<'caller> {
    let outcome = unowned_env.with_env(|env| -> Result<_, jni::errors::Error> {
        let path = path.to_string();
        JString::from_str(env, scan_json(&path))
    });
    outcome.resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

/// `au.anufs.telemetry.core.Native.export(path: String, session: Int, kind: String, outPath: String): String`
#[unsafe(no_mangle)]
pub extern "system" fn Java_au_anufs_telemetry_core_Native_export<'caller>(
    mut unowned_env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    path: JString<'caller>,
    session: jni::sys::jint,
    kind: JString<'caller>,
    out_path: JString<'caller>,
) -> JString<'caller> {
    let outcome = unowned_env.with_env(|env| -> Result<_, jni::errors::Error> {
        let (path, kind, out_path) = (path.to_string(), kind.to_string(), out_path.to_string());
        let session = u32::try_from(session).unwrap_or(u32::MAX);
        JString::from_str(env, export_json(&path, session, &kind, &out_path))
    });
    outcome.resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_strings_are_escaped() {
        let mut o = String::new();
        json_str(&mut o, "a\"b\\c\nd\u{1}");
        assert_eq!(o, "\"a\\\"b\\\\c\\nd\\u0001\"");
    }

    #[test]
    fn a_missing_file_is_an_error_object() {
        let j = scan_json("/definitely/not/here.img");
        assert!(j.starts_with("{\"error\":\""), "{j}");
    }

    #[test]
    fn unknown_export_kind_is_an_error_object() {
        let j = export_json("/definitely/not/here.img", 1, "nope", "/tmp/x");
        assert!(j.starts_with("{\"error\":\""), "{j}");
    }

    /// Needs the images written by `TEL_DUMP_IMAGES=sample-data cargo test -p tel-decode --test pipeline`.
    fn sample(name: &str) -> Option<String> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../sample-data").join(name);
        p.exists().then(|| p.to_string_lossy().into_owned())
    }

    #[test]
    fn sample_images_scan_and_export() {
        let Some(b) = sample("card_b.img") else { return };
        let j = scan_json(&b);
        assert!(j.contains("\"cleanClose\":true"), "{j}");
        assert!(j.contains("\"session\":1"), "{j}");
        // the simulated drive writes no SessionInfo record, so the optional fields are absent
        assert!(!j.contains("firmware"), "{j}");
        let dir = std::env::temp_dir();
        let gpx = dir.join("tel_jni_test.gpx");
        let r = export_json(&b, 1, "gpx", &gpx.to_string_lossy());
        assert_eq!(r, "{\"rows\":300}");
        let csv = dir.join("tel_jni_test.csv");
        let r = export_json(&b, 1, "samples", &csv.to_string_lossy());
        assert_eq!(r, "{\"rows\":300}");
        let sum = dir.join("tel_jni_test_summary.json");
        let r = export_json(&b, 1, "can-summary", &sum.to_string_lossy());
        assert_eq!(r, "{\"rows\":1}");
        let text = std::fs::read_to_string(&sum).unwrap();
        assert!(text.contains("\"name\":\"SBG_ECAN_MSG_EKF_EULER\"") && text.contains("\"count\":300"), "{text}");
        let Some(a) = sample("card_a.img") else { return };
        assert!(scan_json(&a).contains("\"cleanClose\":false"));
    }
}
