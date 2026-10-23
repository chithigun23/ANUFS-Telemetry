//! Host tool for telemetry cards: scan, report, export CSV, prepare.
//!
//! The card (or an image of it) is read as a region of 512 byte blocks that
//! starts at `start_lba` and is `blocks` long; see `tel-logfmt` for the layout.

pub mod export;
pub mod scan;
pub mod source;

use source::{BlockSink, BlockSource};
use std::io;
use tel_logfmt::block::{encode_region, RegionHeader, BLOCK_SIZE, HEADER_LBA, HINT_LBA};
use tel_logfmt::recovery::Geometry;

/// Format a region: blank hints first, then the header (so a crash while
/// preparing never leaves a header pointing at stale hints). Existing data in
/// the data area is *not* erased; the new `card_id` makes it unrecognisable.
pub fn prepare<S: BlockSource + BlockSink>(dev: &mut S, geo: Geometry, card_id: u32) -> io::Result<()> {
    let need = u64::from(geo.start_lba).saturating_add(u64::from(geo.blocks));
    let have = dev.len_blocks()?;
    if have < need {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("the device has {have} blocks but the region needs {need}"),
        ));
    }
    for rel in HINT_LBA {
        if let Some(lba) = geo.abs(rel) {
            dev.write_block(lba, &[0; BLOCK_SIZE])?;
        }
    }
    let mut header = [0_u8; BLOCK_SIZE];
    encode_region(&mut header, &RegionHeader { card_id, region_blocks: geo.blocks })
        .map_err(|_| io::Error::other("could not encode the header"))?;
    if let Some(lba) = geo.abs(HEADER_LBA) {
        dev.write_block(lba, &header)?;
    }
    Ok(())
}
