//! Per-identifier summary of the raw CAN frames of one session: how many frames
//! each id sent, when, and what the last one carried. Frames are folded as they
//! are read, so a half-hour log with millions of frames costs a few kilobytes.

use crate::export::{catalog_for_bus, ExportError};
use crate::scan::{walk, Report};
use crate::source::BlockSource;
use std::collections::BTreeMap;
use tel_logfmt::records::{CanRawView, Kind, Records};
use tel_logfmt::recovery::Geometry;

/// Everything known about one (bus, id).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdStat {
    /// Bus number, 1 or 2.
    pub bus: u8,
    /// CAN identifier.
    pub id: u32,
    /// 29-bit identifier.
    pub extended: bool,
    /// Message name when the bus's database knows the id.
    pub name: Option<&'static str>,
    /// Frames seen.
    pub count: u64,
    /// Monotonic time of the first frame, microseconds.
    pub first_us: u64,
    /// Monotonic time of the last frame, microseconds.
    pub last_us: u64,
    /// Data length of the last frame.
    pub dlc: u8,
    /// Data of the last frame.
    pub last: [u8; 8],
}

/// Fold the raw CAN frames of `session` into per-id statistics, ordered by bus then id.
pub fn can_summary<S: BlockSource>(
    src: &mut S,
    geo: Geometry,
    card_id: u32,
    session: u32,
    stop_after_invalid: u32,
) -> Result<(Vec<IdStat>, Report), ExportError> {
    let mut map: BTreeMap<(u8, bool, u32), IdStat> = BTreeMap::new();
    let report = walk(src, geo, card_id, stop_after_invalid, |_, d| {
        if d.header.session != session {
            return;
        }
        for rec in Records::new(d.payload).flatten() {
            if rec.kind != Kind::CanRaw {
                continue;
            }
            let Ok(v) = CanRawView::parse(rec.body) else { continue };
            for f in v.frames().flatten() {
                let e = map.entry((v.bus, f.extended, f.id)).or_insert_with(|| IdStat {
                    bus: v.bus,
                    id: f.id,
                    extended: f.extended,
                    name: catalog_for_bus(v.bus).and_then(|c| c.find(f.id, f.extended)).map(|m| m.name),
                    count: 0,
                    first_us: f.t_us,
                    last_us: f.t_us,
                    dlc: f.dlc,
                    last: [0; 8],
                });
                e.count = e.count.saturating_add(1);
                e.first_us = e.first_us.min(f.t_us);
                if f.t_us >= e.last_us {
                    e.last_us = f.t_us;
                    e.dlc = f.dlc;
                    e.last = f.data;
                }
            }
        }
    })?;
    Ok((map.into_values().collect(), report))
}
