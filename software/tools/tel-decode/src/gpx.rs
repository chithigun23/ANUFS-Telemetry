//! GPX 1.1 track export of one session.
//!
//! Every sample with a valid GNSS fix becomes a track point. A new track
//! segment starts after a sample without a fix or a gap of more than
//! [`SEGMENT_GAP_US`], so a tunnel or a lost fix does not draw a straight line
//! across the map. Speed and course go into an `extensions` block with a
//! private namespace, as the GPX schema has no field for them.

use crate::export::{fmt_fixed, iso8601, ExportError};
use crate::scan::{walk, Report};
use crate::source::BlockSource;
use std::io::{self, Write};
use tel_logfmt::records::{flags, Kind, Records, Sample};
use tel_logfmt::recovery::Geometry;

/// A silence this long (microseconds) between two fixes starts a new segment.
pub const SEGMENT_GAP_US: u64 = 2_000_000;

/// Namespace of the extension elements.
const NS: &str = "https://github.com/chithigun23/ANUFS-Telemetry/gpx/1";

/// What a GPX export produced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpxStats {
    /// Track points written.
    pub points: u64,
    /// Track segments written.
    pub segments: u64,
    /// Samples skipped because they carried no valid position.
    pub skipped: u64,
}

/// The GPX `fix` value for a GGA fix quality indicator.
pub fn gpx_fix(fix_quality: u8) -> &'static str {
    match fix_quality {
        0 => "none",
        2 | 4 | 5 => "dgps",
        _ => "3d",
    }
}

fn point(s: &Sample, out: &mut impl Write) -> io::Result<()> {
    writeln!(out, "      <trkpt lat=\"{}\" lon=\"{}\">", fmt_fixed(s.lat_e9, 9), fmt_fixed(s.lon_e9, 9))?;
    writeln!(out, "        <ele>{}</ele>", fmt_fixed(i64::from(s.alt_mm), 3))?;
    if s.flags & flags::UTC_VALID != 0 {
        writeln!(out, "        <time>{}</time>", iso8601(s.utc_unix_ms))?;
    }
    writeln!(out, "        <fix>{}</fix>", gpx_fix(s.fix_quality))?;
    writeln!(out, "        <sat>{}</sat>", s.num_sv)?;
    writeln!(out, "        <hdop>{}</hdop>", fmt_fixed(i64::from(s.hdop_e2), 2))?;
    if s.diff_age_ds != u16::MAX {
        writeln!(out, "        <ageofdgpsdata>{}</ageofdgpsdata>", fmt_fixed(i64::from(s.diff_age_ds), 1))?;
    }
    writeln!(
        out,
        "        <extensions><tel:speed>{}</tel:speed><tel:course>{}</tel:course></extensions>",
        fmt_fixed(i64::from(s.sog_mm_s), 3),
        fmt_fixed(i64::from(s.cog_cdeg), 2)
    )?;
    writeln!(out, "      </trkpt>")
}

/// Write the track of `session` as GPX 1.1.
pub fn export_gpx<S: BlockSource, W: Write>(
    src: &mut S,
    geo: Geometry,
    card_id: u32,
    session: u32,
    stop_after_invalid: u32,
    out: &mut W,
) -> Result<(GpxStats, Report), ExportError> {
    writeln!(out, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>")?;
    writeln!(
        out,
        "<gpx version=\"1.1\" creator=\"ANUFS telemetry\" xmlns=\"http://www.topografix.com/GPX/1/1\" xmlns:tel=\"{NS}\">"
    )?;
    writeln!(out, "  <trk>")?;
    writeln!(out, "    <name>ANUFS telemetry session {session}</name>")?;

    let mut stats = GpxStats::default();
    let mut io_err: Option<io::Error> = None;
    let mut open = false;
    let mut last_t_us: Option<u64> = None;
    let report = walk(src, geo, card_id, stop_after_invalid, |_, d| {
        if io_err.is_some() || d.header.session != session {
            return;
        }
        for rec in Records::new(d.payload).flatten() {
            if rec.kind != Kind::Sample {
                continue;
            }
            let Ok(s) = Sample::decode(rec.body) else { continue };
            let r: io::Result<()> = (|| {
                if s.flags & flags::GNSS_VALID == 0 {
                    stats.skipped = stats.skipped.saturating_add(1);
                    if open {
                        writeln!(out, "    </trkseg>")?;
                        open = false;
                    }
                    return Ok(());
                }
                let gap = last_t_us.is_some_and(|t| s.t_us.saturating_sub(t) > SEGMENT_GAP_US);
                if open && gap {
                    writeln!(out, "    </trkseg>")?;
                    open = false;
                }
                if !open {
                    writeln!(out, "    <trkseg>")?;
                    open = true;
                    stats.segments = stats.segments.saturating_add(1);
                }
                point(&s, out)?;
                stats.points = stats.points.saturating_add(1);
                last_t_us = Some(s.t_us);
                Ok(())
            })();
            if let Err(e) = r {
                io_err = Some(e);
                return;
            }
        }
    })?;
    if let Some(e) = io_err {
        return Err(ExportError::Io(e));
    }
    if open {
        writeln!(out, "    </trkseg>")?;
    }
    writeln!(out, "  </trk>")?;
    writeln!(out, "</gpx>")?;
    Ok((stats, report))
}
