//! Sans-IO append-only block writer.
//!
//! The writer packs records into 512 byte blocks and queues finished blocks in
//! a small ring. The caller owns the I/O loop:
//!
//! ```text
//! while let Some(req) = writer.pending() {
//!     match card.write(req.lba, req.data) {
//!         Ok(()) => writer.ack_write(),
//!         Err(_) => writer.write_failed(),
//!     }
//! }
//! ```
//!
//! Rules that make this crash-safe:
//!
//! * A data block is written once and never modified, so a power cut can only
//!   damage the block being written, and the CRC reveals that.
//! * A block is acknowledged (and hints advanced) only after the caller says
//!   the write completed.
//! * Hint blocks alternate between two slots, so one torn hint never loses the
//!   other.
//! * Records are never dropped silently: every drop is counted in [`Stats`].

use crate::block::{
    encode_data, encode_hint, Block, DataHeader, Hint, BLOCK_SIZE, DATA_START, FLAG_FIRST, FLAG_LAST, HINT_LBA,
    PAYLOAD_MAX,
};
use crate::records::{append_record, Kind, MAX_BODY};
use crate::recovery::{Geometry, Recovered};

/// Write a hint after this many acknowledged data blocks.
pub const HINT_INTERVAL: u32 = 256;

/// Writer state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Accepting records.
    Open,
    /// `close` completed; no more records.
    Closed,
    /// The region is full.
    Full,
    /// A data block write failed; the card must be replaced or re-prepared.
    Faulted,
}

/// Why a record was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushError {
    /// The ring of finished blocks is full because writes are not keeping up.
    Overrun,
    /// The region is full.
    Full,
    /// A previous write failed.
    Faulted,
    /// The writer was closed.
    Closed,
    /// The record body is larger than 255 bytes.
    TooLarge,
}

/// Counters. All saturate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Records accepted.
    pub records: u32,
    /// Data blocks acknowledged as written.
    pub blocks_written: u32,
    /// Records refused because the ring was full.
    pub dropped_overrun: u32,
    /// Records refused because the region was full.
    pub dropped_full: u32,
    /// Records refused because the writer was faulted or closed.
    pub dropped_state: u32,
}

/// What kind of block a [`WriteRequest`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteKind {
    /// A data block.
    Data,
    /// A checkpoint hint (its failure is not fatal).
    Hint,
}

/// A block the caller must write.
#[derive(Clone, Copy, Debug)]
pub struct WriteRequest<'a> {
    /// Absolute block number.
    pub lba: u32,
    /// The block image.
    pub data: &'a Block,
    /// Data or hint.
    pub kind: WriteKind,
}

/// `ack_write` or `write_failed` was called with nothing outstanding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NothingPending;

/// Why [`LogWriter::start`] failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartError {
    /// `DEPTH` must be at least 1.
    ZeroDepth,
    /// The session counter would overflow.
    SessionOverflow,
}

/// The writer. `DEPTH` is the number of finished blocks that may be queued.
#[derive(Clone, Debug)]
pub struct LogWriter<const DEPTH: usize> {
    geo: Geometry,
    card_id: u32,
    session: u32,
    next_rel: u32,
    next_seq: u32,
    slots: [Block; DEPTH],
    rels: [u32; DEPTH],
    seqs: [u32; DEPTH],
    head: usize,
    queued: usize,
    active: [u8; PAYLOAD_MAX],
    active_used: usize,
    state: State,
    hint: Option<(u32, Block)>,
    hint_seq: u32,
    hint_slot: usize,
    last_hint_tail: u32,
    pending_hint_tail: u32,
    stats: Stats,
}

impl<const DEPTH: usize> LogWriter<DEPTH> {
    /// Start a new session after recovery. The new session number is one more
    /// than the highest found. A hint announcing it is queued immediately.
    pub fn start(geo: Geometry, rec: &Recovered) -> Result<Self, StartError> {
        if DEPTH == 0 {
            return Err(StartError::ZeroDepth);
        }
        let session = rec.last_session.checked_add(1).ok_or(StartError::SessionOverflow)?;
        let mut w = Self {
            geo,
            card_id: rec.card_id,
            session,
            next_rel: rec.next_rel.max(DATA_START),
            next_seq: 0,
            slots: [[0; BLOCK_SIZE]; DEPTH],
            rels: [0; DEPTH],
            seqs: [0; DEPTH],
            head: 0,
            queued: 0,
            active: [0; PAYLOAD_MAX],
            active_used: 0,
            state: State::Open,
            hint: None,
            hint_seq: rec.hint_seq,
            hint_slot: 0,
            last_hint_tail: 0,
            pending_hint_tail: 0,
            stats: Stats::default(),
        };
        if w.next_rel >= geo.blocks {
            w.state = State::Full;
        }
        w.make_hint(w.next_rel, 0);
        Ok(w)
    }

    /// Session number of this writer.
    pub fn session(&self) -> u32 {
        self.session
    }

    /// Current state.
    pub fn state(&self) -> State {
        self.state
    }

    /// Counters.
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Region-relative block number the next sealed block will occupy.
    pub fn next_rel(&self) -> u32 {
        self.next_rel
    }

    /// Finished blocks waiting to be written.
    pub fn queued(&self) -> usize {
        self.queued
    }

    /// Bytes of the open block that are in use.
    pub fn active_bytes(&self) -> usize {
        self.active_used
    }

    fn make_hint(&mut self, tail: u32, next_seq: u32) {
        if self.hint.is_some() {
            return;
        }
        self.hint_seq = self.hint_seq.saturating_add(1);
        self.pending_hint_tail = tail;
        let h = Hint { card_id: self.card_id, hint_seq: self.hint_seq, last_session: self.session, tail, next_seq };
        let mut b = [0_u8; BLOCK_SIZE];
        if encode_hint(&mut b, &h).is_ok() {
            let lba = HINT_LBA.get(self.hint_slot).copied().unwrap_or(1);
            self.hint = Some((lba, b));
            self.hint_slot ^= 1;
        }
    }

    fn slot_index(&self, offset: usize) -> usize {
        self.head.checked_add(offset).and_then(|v| v.checked_rem(DEPTH)).unwrap_or(0)
    }

    fn seal(&mut self, last: bool) -> Result<(), PushError> {
        if self.queued >= DEPTH {
            return Err(PushError::Overrun);
        }
        if self.next_rel >= self.geo.blocks {
            self.state = State::Full;
            return Err(PushError::Full);
        }
        let idx = self.slot_index(self.queued);
        let mut flags = 0;
        if self.next_seq == 0 {
            flags |= FLAG_FIRST;
        }
        if last {
            flags |= FLAG_LAST;
        }
        let used = u16::try_from(self.active_used).map_err(|_| PushError::TooLarge)?;
        let header = DataHeader { card_id: self.card_id, session: self.session, seq: self.next_seq, used, flags };
        let payload = self.active.get(..self.active_used).ok_or(PushError::TooLarge)?;
        let slot = self.slots.get_mut(idx).ok_or(PushError::Overrun)?;
        encode_data(slot, &header, payload).map_err(|_| PushError::TooLarge)?;
        if let Some(r) = self.rels.get_mut(idx) {
            *r = self.next_rel;
        }
        if let Some(s) = self.seqs.get_mut(idx) {
            *s = self.next_seq;
        }
        self.queued = self.queued.saturating_add(1);
        self.next_rel = self.next_rel.saturating_add(1);
        self.next_seq = self.next_seq.saturating_add(1);
        self.active = [0; PAYLOAD_MAX];
        self.active_used = 0;
        Ok(())
    }

    fn refuse(&mut self, e: PushError) -> PushError {
        match e {
            PushError::Overrun => self.stats.dropped_overrun = self.stats.dropped_overrun.saturating_add(1),
            PushError::Full => self.stats.dropped_full = self.stats.dropped_full.saturating_add(1),
            PushError::Faulted | PushError::Closed => {
                self.stats.dropped_state = self.stats.dropped_state.saturating_add(1)
            }
            PushError::TooLarge => {}
        }
        e
    }

    /// Append one record.
    pub fn push(&mut self, kind: Kind, body: &[u8]) -> Result<(), PushError> {
        match self.state {
            State::Open => {}
            State::Full => return Err(self.refuse(PushError::Full)),
            State::Faulted => return Err(self.refuse(PushError::Faulted)),
            State::Closed => return Err(self.refuse(PushError::Closed)),
        }
        let need = body.len().saturating_add(2);
        if body.len() > MAX_BODY || need > PAYLOAD_MAX {
            return Err(PushError::TooLarge);
        }
        if self.active_used.saturating_add(need) > PAYLOAD_MAX {
            if let Err(e) = self.seal(false) {
                return Err(self.refuse(e));
            }
        }
        if append_record(&mut self.active, &mut self.active_used, kind, body) {
            self.stats.records = self.stats.records.saturating_add(1);
            Ok(())
        } else {
            Err(PushError::TooLarge)
        }
    }

    /// Finish the open block so it can be written (no effect if it is empty).
    /// A refused flush (ring full) can simply be retried later.
    pub fn flush(&mut self) -> Result<(), PushError> {
        if self.state != State::Open || self.active_used == 0 {
            return Ok(());
        }
        self.seal(false)
    }

    /// Finish the session with a block flagged `LAST` (even if it is empty).
    /// Retry if it returns `Overrun`.
    pub fn close(&mut self) -> Result<(), PushError> {
        if self.state != State::Open {
            return Ok(());
        }
        self.seal(true)?;
        self.state = State::Closed;
        Ok(())
    }

    /// The next block to write, if any. A pending hint is returned first.
    /// Returns `None` after a data write has failed.
    pub fn pending(&self) -> Option<WriteRequest<'_>> {
        if let Some((rel, data)) = &self.hint {
            return Some(WriteRequest { lba: self.geo.start_lba.saturating_add(*rel), data, kind: WriteKind::Hint });
        }
        if self.state == State::Faulted || self.queued == 0 {
            return None;
        }
        let idx = self.slot_index(0);
        let data = self.slots.get(idx)?;
        let rel = *self.rels.get(idx)?;
        Some(WriteRequest { lba: self.geo.start_lba.saturating_add(rel), data, kind: WriteKind::Data })
    }

    /// The block returned by [`LogWriter::pending`] was written.
    pub fn ack_write(&mut self) -> Result<WriteKind, NothingPending> {
        if self.hint.take().is_some() {
            // The hint's tail is what it recorded; remember it as the new baseline.
            self.last_hint_tail = self.pending_hint_tail;
            return Ok(WriteKind::Hint);
        }
        if self.state == State::Faulted || self.queued == 0 {
            return Err(NothingPending);
        }
        let idx = self.slot_index(0);
        let rel = self.rels.get(idx).copied().unwrap_or(0);
        let seq = self.seqs.get(idx).copied().unwrap_or(0);
        self.head = self.slot_index(1);
        self.queued = self.queued.saturating_sub(1);
        self.stats.blocks_written = self.stats.blocks_written.saturating_add(1);
        let tail = rel.saturating_add(1);
        if tail.saturating_sub(self.last_hint_tail) >= HINT_INTERVAL && self.hint.is_none() {
            self.make_hint(tail, seq.saturating_add(1));
        }
        Ok(WriteKind::Data)
    }

    /// The write of the block returned by [`LogWriter::pending`] failed.
    ///
    /// A failed hint is dropped (the next one is generated later). A failed
    /// data block puts the writer in [`State::Faulted`].
    pub fn write_failed(&mut self) -> Result<WriteKind, NothingPending> {
        if self.hint.take().is_some() {
            return Ok(WriteKind::Hint);
        }
        if self.state == State::Faulted || self.queued == 0 {
            return Err(NothingPending);
        }
        self.state = State::Faulted;
        Ok(WriteKind::Data)
    }
}
