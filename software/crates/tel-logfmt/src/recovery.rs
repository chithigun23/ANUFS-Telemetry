//! Start-up recovery: find where the previous run stopped.
//!
//! Sans-IO. Loop on [`Recovery::next_read`], perform each read and report it
//! with [`Recovery::provide`], then take the result from
//! [`Recovery::outcome`]:
//!
//! 1. Read the region header (block 0). A card that was never prepared, or
//!    whose size changed, is reported rather than guessed at.
//! 2. Read both hint blocks and use the newest valid one as a starting point.
//! 3. Scan forward from the hint's tail, accepting a block only if its CRC is
//!    good, its `card_id` matches, and it continues the session (same session,
//!    next `seq`) or starts the next one (`FIRST`, `seq` 0, session + 1).
//!
//! The scan stops at the first block that fails any of those tests, which is
//! exactly where a new session may safely begin. A read *error* (as opposed to
//! a block that reads fine but is invalid) aborts recovery, because treating a
//! transient error as end-of-data could overwrite valid blocks.

use crate::block::{
    parse_data, parse_hint, parse_region, Block, Hint, RegionHeader, DATA_START, FLAG_FIRST, HEADER_LBA, HINT_LBA,
};

/// Where a region lives on the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    /// Absolute block number of region block 0.
    pub start_lba: u32,
    /// Region size in blocks (header, two hints, then data).
    pub blocks: u32,
}

impl Geometry {
    /// Absolute block number of a region-relative block, if it is inside the region.
    pub fn abs(&self, rel: u32) -> Option<u32> {
        if rel >= self.blocks {
            return None;
        }
        self.start_lba.checked_add(rel)
    }
}

/// A read could not be completed (bus error, card removed, timeout).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadFailed;

/// Why recovery could not produce a result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryError {
    /// The region header is missing or damaged: the card needs preparing.
    NotPrepared,
    /// The header describes a region of a different size than expected.
    GeometryMismatch {
        /// Region size recorded in the header.
        found: u32,
    },
    /// A block could not be read.
    ReadFailed {
        /// Absolute block number.
        lba: u32,
    },
    /// The geometry is too small to hold a header, hints and one data block.
    RegionTooSmall,
}

/// The state to resume from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recovered {
    /// Card identifier from the region header.
    pub card_id: u32,
    /// Highest session number found (0 if none).
    pub last_session: u32,
    /// Region-relative block number where the next block goes.
    pub next_rel: u32,
    /// Highest hint sequence number found (0 if none).
    pub hint_seq: u32,
    /// Data blocks verified during the scan (not counting those before the hint).
    pub blocks_scanned: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Header,
    Hint(usize),
    Scan,
    Done(Result<Recovered, RecoveryError>),
}

/// The recovery state machine.
#[derive(Clone, Debug)]
pub struct Recovery {
    geo: Geometry,
    state: State,
    header: Option<RegionHeader>,
    hints: [Option<Hint>; 2],
    scan_rel: u32,
    session: u32,
    next_seq: u32,
    hint_seq: u32,
    scanned: u32,
}

impl Recovery {
    /// Begin recovery for a region.
    pub fn new(geo: Geometry) -> Self {
        let state =
            if geo.blocks <= DATA_START { State::Done(Err(RecoveryError::RegionTooSmall)) } else { State::Header };
        Self {
            geo,
            state,
            header: None,
            hints: [None, None],
            scan_rel: DATA_START,
            session: 0,
            next_seq: 0,
            hint_seq: 0,
            scanned: 0,
        }
    }

    fn current_rel(&self) -> Option<u32> {
        match self.state {
            State::Header => Some(HEADER_LBA),
            State::Hint(i) => HINT_LBA.get(i).copied(),
            State::Scan => Some(self.scan_rel),
            State::Done(_) => None,
        }
    }

    /// The absolute block to read next, or `None` when finished.
    pub fn next_read(&self) -> Option<u32> {
        self.current_rel().and_then(|rel| self.geo.abs(rel))
    }

    fn finish_scan(&mut self) {
        let card_id = self.header.map(|h| h.card_id).unwrap_or(0);
        self.state = State::Done(Ok(Recovered {
            card_id,
            last_session: self.session,
            next_rel: self.scan_rel,
            hint_seq: self.hint_seq,
            blocks_scanned: self.scanned,
        }));
    }

    fn begin_scan(&mut self) {
        let card_id = self.header.map(|h| h.card_id).unwrap_or(0);
        let best = self
            .hints
            .iter()
            .flatten()
            .filter(|h| h.card_id == card_id && h.tail >= DATA_START && h.tail <= self.geo.blocks)
            .max_by_key(|h| h.hint_seq)
            .copied();
        if let Some(h) = best {
            self.scan_rel = h.tail;
            self.session = h.last_session;
            self.next_seq = h.next_seq;
            self.hint_seq = h.hint_seq;
        }
        self.state = State::Scan;
        if self.scan_rel >= self.geo.blocks {
            self.finish_scan();
        }
    }

    /// Report the result of the read requested by [`Recovery::next_read`].
    pub fn provide(&mut self, result: Result<&Block, ReadFailed>) {
        let Some(lba) = self.next_read() else { return };
        let block = match result {
            Ok(b) => b,
            Err(ReadFailed) => {
                // A failed header or hint read just means that block is unusable
                // as a starting point, but a failed scan read must not be taken
                // for end-of-data.
                match self.state {
                    State::Header => self.state = State::Done(Err(RecoveryError::ReadFailed { lba })),
                    State::Hint(i) => self.advance_hint(i),
                    State::Scan => self.state = State::Done(Err(RecoveryError::ReadFailed { lba })),
                    State::Done(_) => {}
                }
                return;
            }
        };
        match self.state {
            State::Header => match parse_region(block) {
                Ok(h) if h.region_blocks == self.geo.blocks => {
                    self.header = Some(h);
                    self.state = State::Hint(0);
                }
                Ok(h) => self.state = State::Done(Err(RecoveryError::GeometryMismatch { found: h.region_blocks })),
                Err(_) => self.state = State::Done(Err(RecoveryError::NotPrepared)),
            },
            State::Hint(i) => {
                if let Some(slot) = self.hints.get_mut(i) {
                    *slot = parse_hint(block).ok();
                }
                self.advance_hint(i);
            }
            State::Scan => self.scan_step(block),
            State::Done(_) => {}
        }
    }

    fn advance_hint(&mut self, i: usize) {
        if i.saturating_add(1) < HINT_LBA.len() {
            self.state = State::Hint(i.saturating_add(1));
        } else {
            self.begin_scan();
        }
    }

    fn scan_step(&mut self, block: &Block) {
        let card_id = self.header.map(|h| h.card_id).unwrap_or(0);
        let accepted = match parse_data(block) {
            Ok(d) if d.header.card_id == card_id => {
                let h = d.header;
                let continues = h.session == self.session && h.seq == self.next_seq;
                let starts_next =
                    self.session.checked_add(1) == Some(h.session) && h.flags & FLAG_FIRST != 0 && h.seq == 0;
                if continues || starts_next {
                    self.session = h.session;
                    self.next_seq = h.seq.saturating_add(1);
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        if accepted {
            self.scanned = self.scanned.saturating_add(1);
            self.scan_rel = self.scan_rel.saturating_add(1);
            if self.scan_rel >= self.geo.blocks {
                self.finish_scan();
            }
        } else {
            self.finish_scan();
        }
    }

    /// The final result, once [`Recovery::next_read`] returns `None`.
    pub fn outcome(&self) -> Option<Result<Recovered, RecoveryError>> {
        match self.state {
            State::Done(r) => Some(r),
            _ => None,
        }
    }
}
