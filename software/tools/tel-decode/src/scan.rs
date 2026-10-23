//! Scanning a card or image for log data.
//!
//! The reader is deliberately more tolerant than the firmware's recovery: it
//! walks the data area block by block, keeps every block whose CRC and card id
//! check out, and tolerates holes (torn blocks, blocks lost to write
//! reordering) and reused session numbers. Consecutive blocks that continue
//! the same session form a *run*; anything else starts a new run. It stops
//! after a long stretch of unusable blocks, which is how the end of the data
//! is recognised on a card that is mostly empty.

use crate::source::BlockSource;
use std::io;
use tel_logfmt::block::{parse_data, parse_region, DataBlock, RegionHeader, DATA_START, FLAG_LAST, HEADER_LBA};
use tel_logfmt::recovery::Geometry;

/// Consecutive unusable blocks after which the scan assumes the data has ended.
pub const DEFAULT_STOP_AFTER_INVALID: u32 = 256;

/// Why a card cannot be read as a telemetry log.
#[derive(Debug)]
pub enum ScanError {
    /// I/O failure reading the header.
    Io(io::Error),
    /// The region header is missing or damaged.
    NotPrepared,
    /// The header describes a region of a different size than requested.
    GeometryMismatch {
        /// Size recorded in the header.
        found: u32,
    },
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::NotPrepared => write!(f, "no valid region header found (card not prepared, or wrong --start-lba)"),
            Self::GeometryMismatch { found } => write!(f, "header says the region has {found} blocks"),
        }
    }
}

impl std::error::Error for ScanError {}

impl From<io::Error> for ScanError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Read and validate the region header.
pub fn read_header<S: BlockSource>(src: &mut S, geo: Geometry) -> Result<RegionHeader, ScanError> {
    let b = src.read_block(geo.start_lba)?;
    let h = parse_region(&b).map_err(|_| ScanError::NotPrepared)?;
    if h.region_blocks != geo.blocks {
        return Err(ScanError::GeometryMismatch { found: h.region_blocks });
    }
    Ok(h)
}

/// A stretch of consecutive blocks continuing one session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    /// Session number.
    pub session: u32,
    /// Region-relative block number of the first block.
    pub first_rel: u32,
    /// Number of blocks.
    pub blocks: u32,
    /// `seq` of the first block.
    pub first_seq: u32,
    /// `seq` of the last block.
    pub last_seq: u32,
    /// The last block carries the clean-close flag.
    pub clean_close: bool,
    /// Unusable blocks between this run and the one before it.
    pub gap_before: u32,
}

/// Why the scan ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    /// A long stretch of unusable blocks (end of the data).
    NoMoreData,
    /// The end of the region was reached.
    RegionEnd,
    /// The medium ended before the region did.
    MediaEnd,
}

/// Result of a scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// Card identifier.
    pub card_id: u32,
    /// Region size in blocks.
    pub region_blocks: u32,
    /// Runs in disk order.
    pub runs: Vec<Run>,
    /// Blocks that passed every check.
    pub valid_blocks: u32,
    /// Damaged blocks inside the data (failed CRC or magic), not counting the
    /// empty stretch that ended the scan.
    pub invalid_blocks: u32,
    /// Valid blocks that belong to a different card id.
    pub foreign_blocks: u32,
    /// Blocks that could not be read, not counting the final stretch.
    pub read_errors: u32,
    /// Unusable blocks at the very end that ended the scan (normally the empty
    /// part of the card).
    pub trailing_unusable: u32,
    /// First region-relative block that was not scanned.
    pub scanned_to_rel: u32,
    /// Why scanning stopped.
    pub ended: Ended,
}

/// Visit every valid block of this card in disk order. Returns the report.
pub fn walk<S, F>(
    src: &mut S,
    geo: Geometry,
    card_id: u32,
    stop_after_invalid: u32,
    mut visit: F,
) -> Result<Report, ScanError>
where
    S: BlockSource,
    F: FnMut(u32, &DataBlock<'_>),
{
    let mut runs: Vec<Run> = Vec::new();
    let mut valid = 0_u32;
    let mut invalid = 0_u32;
    let mut foreign = 0_u32;
    let mut read_errors = 0_u32;
    let mut consecutive_bad = 0_u32;
    let (mut tail_invalid, mut tail_foreign, mut tail_read) = (0_u32, 0_u32, 0_u32);
    let mut prev_end = DATA_START; // first block after the previous valid block
    let mut ended = Ended::RegionEnd;
    let mut rel = DATA_START;
    while rel < geo.blocks {
        let Some(lba) = geo.abs(rel) else { break };
        let block = match src.read_block(lba) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                ended = Ended::MediaEnd;
                break;
            }
            Err(_) => {
                read_errors = read_errors.saturating_add(1);
                tail_read = tail_read.saturating_add(1);
                consecutive_bad = consecutive_bad.saturating_add(1);
                if consecutive_bad >= stop_after_invalid {
                    ended = Ended::NoMoreData;
                    break;
                }
                rel = rel.saturating_add(1);
                continue;
            }
        };
        match parse_data(&block) {
            Ok(d) if d.header.card_id == card_id => {
                valid = valid.saturating_add(1);
                consecutive_bad = 0;
                (tail_invalid, tail_foreign, tail_read) = (0, 0, 0);
                let h = d.header;
                let extends = runs.last().is_some_and(|r| {
                    r.session == h.session
                        && r.last_seq.checked_add(1) == Some(h.seq)
                        && r.first_rel.saturating_add(r.blocks) == rel
                });
                if extends {
                    if let Some(r) = runs.last_mut() {
                        r.blocks = r.blocks.saturating_add(1);
                        r.last_seq = h.seq;
                        r.clean_close = h.flags & FLAG_LAST != 0;
                    }
                } else {
                    runs.push(Run {
                        session: h.session,
                        first_rel: rel,
                        blocks: 1,
                        first_seq: h.seq,
                        last_seq: h.seq,
                        clean_close: h.flags & FLAG_LAST != 0,
                        gap_before: rel.saturating_sub(prev_end),
                    });
                }
                prev_end = rel.saturating_add(1);
                visit(rel, &d);
            }
            Ok(_) => {
                foreign = foreign.saturating_add(1);
                tail_foreign = tail_foreign.saturating_add(1);
                consecutive_bad = consecutive_bad.saturating_add(1);
            }
            Err(_) => {
                invalid = invalid.saturating_add(1);
                tail_invalid = tail_invalid.saturating_add(1);
                consecutive_bad = consecutive_bad.saturating_add(1);
            }
        }
        if consecutive_bad >= stop_after_invalid {
            ended = Ended::NoMoreData;
            rel = rel.saturating_add(1);
            break;
        }
        rel = rel.saturating_add(1);
    }
    // The unusable stretch at the very end is the empty part of the card (or what ended
    // the scan), not damage inside the data.
    Ok(Report {
        card_id,
        region_blocks: geo.blocks,
        runs,
        valid_blocks: valid,
        invalid_blocks: invalid.saturating_sub(tail_invalid),
        foreign_blocks: foreign.saturating_sub(tail_foreign),
        read_errors: read_errors.saturating_sub(tail_read),
        trailing_unusable: consecutive_bad,
        scanned_to_rel: rel,
        ended,
    })
}

/// Read the header and scan the region.
pub fn scan<S: BlockSource>(src: &mut S, geo: Geometry, stop_after_invalid: u32) -> Result<Report, ScanError> {
    let header = read_header(src, geo)?;
    walk(src, geo, header.card_id, stop_after_invalid, |_, _| {})
}

/// Region-relative block 0 is the header, so the first data block is [`DATA_START`].
pub const fn header_rel() -> u32 {
    HEADER_LBA
}
