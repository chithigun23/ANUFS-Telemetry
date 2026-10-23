//! CSV export. Numbers are formatted with exact integer arithmetic (no floats)
//! wherever the log holds integers, so the output never carries rounding noise.

use crate::scan::{walk, Report, ScanError};
use crate::source::BlockSource;
use std::fmt::Write as _;
use std::io::{self, Write};
use tel_can::{Catalog, SignalId};
use tel_logfmt::records::{event, flags, CanRawView, Event, Kind, Records, Sample, SessionInfo, SignalsView};
use tel_logfmt::recovery::Geometry;

/// Format `v / 10^decimals` exactly, for example `fmt_fixed(-1500, 3)` is `-1.500`.
// Every operation is on an `i128`/`u128` made from an `i64`, so none can overflow
// (checked by a property test over all `i64`); `decimals` is at most 9 here.
#[allow(clippy::arithmetic_side_effects)]
pub fn fmt_fixed(v: i64, decimals: u32) -> String {
    let neg = v < 0;
    let a = i128::from(v).unsigned_abs();
    let scale = 10_u128.pow(decimals);
    let int = a / scale;
    let frac = a % scale;
    let sign = if neg { "-" } else { "" };
    if decimals == 0 {
        format!("{sign}{int}")
    } else {
        format!("{sign}{int}.{frac:0width$}", width = decimals as usize)
    }
}

/// Format `unix_ms` as `YYYY-MM-DDTHH:MM:SS.mmmZ`.
// Day counts derived from an `i64` of milliseconds are at most ~1e11, so the calendar
// arithmetic cannot overflow (checked by a property test over all `i64`).
#[allow(clippy::arithmetic_side_effects)]
pub fn iso8601(unix_ms: i64) -> String {
    let days = unix_ms.div_euclid(86_400_000);
    let ms_of_day = unix_ms.rem_euclid(86_400_000);
    // civil_from_days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let h = ms_of_day / 3_600_000;
    let mi = (ms_of_day / 60_000) % 60;
    let s = (ms_of_day / 1000) % 60;
    let ms = ms_of_day % 1000;
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{ms:03}Z")
}

/// Quote a CSV field if needed.
pub fn csv_escape(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[allow(clippy::arithmetic_side_effects)] // division by non-zero constants
fn t_s(t_us: u64) -> String {
    format!("{}.{:06}", t_us / 1_000_000, t_us % 1_000_000)
}

/// Header row for [`sample_row`].
pub const SAMPLE_HEADER: &str = "t_s,utc,flags,gnss_valid,heading_valid,utc_valid,imu_valid,can1_alive,can2_alive,no_fix_start,card_fault,overrun,pps_locked,\
lat_deg,lon_deg,alt_m,sog_mps,cog_deg,fix_quality,num_sv,hdop,diff_age_s,\
heading_deg,pitch_deg,baseline_m,heading_acc_deg,heading_quality,heading_sv,\
accel_mean_x_g,accel_mean_y_g,accel_mean_z_g,accel_peak_x_g,accel_peak_y_g,accel_peak_z_g,\
gyro_mean_x_dps,gyro_mean_y_dps,gyro_mean_z_dps,gyro_peak_x_dps,gyro_peak_y_dps,gyro_peak_z_dps,imu_temp_c,imu_count,\
nmea_errors,overruns,can1_frames,can2_frames";

/// One CSV row for a sample. Fields whose validity flag is clear are left empty.
pub fn sample_row(s: &Sample) -> String {
    let has = |bit: u32| s.flags & bit != 0;
    let mut o = String::new();
    let mut put = |v: String| {
        if !o.is_empty() {
            o.push(',');
        }
        o.push_str(&v);
    };
    let gate = |on: bool, v: String| if on { v } else { String::new() };
    put(t_s(s.t_us));
    put(gate(has(flags::UTC_VALID), iso8601(s.utc_unix_ms)));
    put(format!("{:#010x}", s.flags));
    for bit in [
        flags::GNSS_VALID,
        flags::HEADING_VALID,
        flags::UTC_VALID,
        flags::IMU_VALID,
        flags::CAN1_ALIVE,
        flags::CAN2_ALIVE,
        flags::NO_FIX_START,
        flags::CARD_FAULT,
        flags::OVERRUN,
        flags::PPS_LOCKED,
    ] {
        put(u8::from(has(bit)).to_string());
    }
    let g = has(flags::GNSS_VALID);
    put(gate(g, fmt_fixed(s.lat_e9, 9)));
    put(gate(g, fmt_fixed(s.lon_e9, 9)));
    put(gate(g, fmt_fixed(i64::from(s.alt_mm), 3)));
    put(gate(g, fmt_fixed(i64::from(s.sog_mm_s), 3)));
    put(gate(g, fmt_fixed(i64::from(s.cog_cdeg), 2)));
    put(gate(g, s.fix_quality.to_string()));
    put(gate(g, s.num_sv.to_string()));
    put(gate(g, fmt_fixed(i64::from(s.hdop_e2), 2)));
    put(gate(g && s.diff_age_ds != u16::MAX, fmt_fixed(i64::from(s.diff_age_ds), 1)));
    let h = has(flags::HEADING_VALID);
    put(gate(h, fmt_fixed(i64::from(s.heading_e6), 6)));
    put(gate(h, fmt_fixed(i64::from(s.pitch_e6), 6)));
    put(gate(h, fmt_fixed(i64::from(s.baseline_mm), 3)));
    put(gate(h, fmt_fixed(i64::from(s.acc_heading_e6), 6)));
    put(gate(h, s.heading_quality.to_string()));
    put(gate(h, s.heading_sv.to_string()));
    let i = has(flags::IMU_VALID);
    for v in s.accel_mean_ug.iter().chain(&s.accel_peak_ug) {
        put(gate(i, fmt_fixed(i64::from(*v), 6)));
    }
    for v in s.gyro_mean_mdps.iter().chain(&s.gyro_peak_mdps) {
        put(gate(i, fmt_fixed(i64::from(*v), 3)));
    }
    put(gate(i, fmt_fixed(i64::from(s.imu_temp_centi_c), 2)));
    put(gate(i, s.imu_count.to_string()));
    put(s.nmea_errors.to_string());
    put(s.overruns.to_string());
    put(s.can1_frames.to_string());
    put(s.can2_frames.to_string());
    o
}

/// Human name of an event code.
pub fn event_name(code: u16) -> &'static str {
    match code {
        event::BOOT => "boot",
        event::GNSS_RECONFIGURED => "gnss_reconfigured",
        event::CARD_FAULT => "card_fault",
        event::CARD_SWITCH => "card_switch",
        event::QUEUE_OVERRUN => "queue_overrun",
        event::CAN_BUS_OFF => "can_bus_off",
        event::FIX_ACQUIRED => "fix_acquired",
        event::FIX_LOST => "fix_lost",
        event::IMU_FAULT => "imu_fault",
        event::LED_REINIT => "led_reinit",
        _ => "unknown",
    }
}

/// Which catalog decodes the signals of a CAN bus: bus 1 is the MoTeC network,
/// bus 2 the SBG Ellipse.
pub fn catalog_for_bus(bus: u8) -> Option<&'static Catalog> {
    static MOTEC: Catalog = tel_can_motec::CATALOG;
    static SBG: Catalog = tel_can_sbg::CATALOG;
    match bus {
        1 => Some(&MOTEC),
        2 => Some(&SBG),
        _ => None,
    }
}

/// Find a signal by its catalog-wide id.
pub fn find_signal(cat: &Catalog, id: SignalId) -> Option<(&'static str, &'static tel_can::Signal)> {
    for m in cat.messages() {
        let first = usize::from(m.first_signal);
        if let Some(idx) = usize::from(id.0).checked_sub(first) {
            if let Some(sig) = m.signals.get(idx) {
                return Some((m.name, sig));
            }
        }
    }
    None
}

/// What an export produced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExportStats {
    /// Rows written.
    pub rows: u64,
    /// Records that could not be decoded (counted, not fatal).
    pub undecodable: u64,
}

/// Which export to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum What {
    /// Periodic samples.
    Samples,
    /// Event records.
    Events,
    /// Raw CAN frames.
    CanRaw,
    /// Decoded CAN signals.
    Signals,
}

/// Export one kind of record to `out` as CSV, for one session or all of them.
pub fn export<S: BlockSource, W: Write>(
    src: &mut S,
    geo: Geometry,
    card_id: u32,
    session: Option<u32>,
    what: What,
    stop_after_invalid: u32,
    out: &mut W,
) -> Result<(ExportStats, Report), ExportError> {
    let header = match what {
        What::Samples => format!("session,{SAMPLE_HEADER}"),
        What::Events => "session,t_s,code,name,arg".to_string(),
        What::CanRaw => "session,t_s,bus,id,extended,dlc,data".to_string(),
        What::Signals => "session,t_s,bus,message,signal,raw,physical,unit".to_string(),
    };
    writeln!(out, "{header}")?;
    let mut stats = ExportStats::default();
    let mut io_err: Option<io::Error> = None;
    let report = walk(src, geo, card_id, stop_after_invalid, |_, d| {
        if io_err.is_some() || session.is_some_and(|s| s != d.header.session) {
            return;
        }
        let sid = d.header.session;
        for rec in Records::new(d.payload) {
            let Ok(rec) = rec else {
                stats.undecodable = stats.undecodable.saturating_add(1);
                break;
            };
            let r: io::Result<()> = (|| {
                match (what, rec.kind) {
                    (What::Samples, Kind::Sample) => match Sample::decode(rec.body) {
                        Ok(s) => {
                            writeln!(out, "{sid},{}", sample_row(&s))?;
                            stats.rows = stats.rows.saturating_add(1);
                        }
                        Err(_) => stats.undecodable = stats.undecodable.saturating_add(1),
                    },
                    (What::Events, Kind::Event) => match Event::decode(rec.body) {
                        Ok(e) => {
                            writeln!(out, "{sid},{},{},{},{}", t_s(e.t_us), e.code, event_name(e.code), e.arg)?;
                            stats.rows = stats.rows.saturating_add(1);
                        }
                        Err(_) => stats.undecodable = stats.undecodable.saturating_add(1),
                    },
                    (What::CanRaw, Kind::CanRaw) => match CanRawView::parse(rec.body) {
                        Ok(v) => {
                            for f in v.frames() {
                                match f {
                                    Ok(f) => {
                                        let mut data = String::new();
                                        for b in f.data.iter().take(usize::from(f.dlc)) {
                                            let _ = write!(data, "{b:02X}");
                                        }
                                        writeln!(
                                            out,
                                            "{sid},{},{},{:#X},{},{},{data}",
                                            t_s(f.t_us),
                                            v.bus,
                                            f.id,
                                            u8::from(f.extended),
                                            f.dlc
                                        )?;
                                        stats.rows = stats.rows.saturating_add(1);
                                    }
                                    Err(_) => {
                                        stats.undecodable = stats.undecodable.saturating_add(1);
                                        break;
                                    }
                                }
                            }
                        }
                        Err(_) => stats.undecodable = stats.undecodable.saturating_add(1),
                    },
                    (What::Signals, Kind::Signals) => match SignalsView::parse(rec.body) {
                        Ok(v) => {
                            let cat = catalog_for_bus(v.bus);
                            for (id, raw) in v.entries() {
                                let found = cat.and_then(|c| find_signal(c, SignalId(id)));
                                let (msg, sig, phys, unit) = match found {
                                    Some((m, s)) => (
                                        m.to_string(),
                                        s.name.to_string(),
                                        s.physical(raw).to_string(),
                                        s.unit.to_string(),
                                    ),
                                    None => (String::new(), format!("signal_{id}"), String::new(), String::new()),
                                };
                                writeln!(
                                    out,
                                    "{sid},{},{},{},{},{raw},{phys},{}",
                                    t_s(v.t_us),
                                    v.bus,
                                    csv_escape(&msg),
                                    csv_escape(&sig),
                                    csv_escape(&unit)
                                )?;
                                stats.rows = stats.rows.saturating_add(1);
                            }
                        }
                        Err(_) => stats.undecodable = stats.undecodable.saturating_add(1),
                    },
                    _ => {}
                }
                Ok(())
            })();
            if let Err(e) = r {
                io_err = Some(e);
                return;
            }
        }
    })?;
    match io_err {
        Some(e) => Err(ExportError::Io(e)),
        None => Ok((stats, report)),
    }
}

/// Find the `SessionInfo` record of a session, if present.
pub fn session_info<S: BlockSource>(
    src: &mut S,
    geo: Geometry,
    card_id: u32,
    session: u32,
    stop_after_invalid: u32,
) -> Result<Option<SessionInfo>, ScanError> {
    let mut found = None;
    walk(src, geo, card_id, stop_after_invalid, |_, d| {
        if found.is_some() || d.header.session != session {
            return;
        }
        for rec in Records::new(d.payload).flatten() {
            if rec.kind == Kind::SessionInfo {
                found = SessionInfo::decode(rec.body).ok();
                return;
            }
        }
    })?;
    Ok(found)
}

/// Export failure.
#[derive(Debug)]
pub enum ExportError {
    /// Writing the output failed.
    Io(io::Error),
    /// Scanning the card failed.
    Scan(ScanError),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Scan(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ExportError {}

impl From<io::Error> for ExportError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<ScanError> for ExportError {
    fn from(e: ScanError) -> Self {
        Self::Scan(e)
    }
}
