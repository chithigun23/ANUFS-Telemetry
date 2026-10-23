//! Async storage driver for the block log.
//!
//! [`tel_logfmt`] is sans-IO; this crate supplies the I/O loop around it for
//! any card that implements [`BlockDevice`] (SDMMC, SPI-mode SD, a test
//! double, a disk image). Hardware crates only have to implement the two
//! block operations.
//!
//! * [`prepare`] formats a region (header plus blank hint blocks).
//! * [`recover`] finds where the last run stopped.
//! * [`CardLog`] owns a [`LogWriter`] and its [`CardLog::service`] loop writes
//!   whatever the writer has pending. `service` is cancel-safe: a block is
//!   acknowledged only after its write completes, so dropping the future
//!   mid-write just causes that block to be written again later.
//!
//! `no_std`, no allocation, no `unsafe`.

#![no_std]
#![allow(async_fn_in_trait)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_possible_wrap
    )
)]

#[cfg(test)]
extern crate std;

use tel_logfmt::block::{encode_region, Block, RegionHeader, BLOCK_SIZE, HEADER_LBA, HINT_LBA};
use tel_logfmt::records::Kind;
use tel_logfmt::recovery::{Geometry, ReadFailed, Recovered, Recovery, RecoveryError};
use tel_logfmt::writer::{LogWriter, PushError, StartError, State, Stats, WriteKind};

/// Block-level access to a card.
pub trait BlockDevice {
    /// Error type of the underlying driver.
    type Error: Copy + core::fmt::Debug;

    /// Read one 512 byte block at absolute block number `lba`.
    async fn read_block(&mut self, lba: u32, buf: &mut Block) -> Result<(), Self::Error>;

    /// Write one 512 byte block at absolute block number `lba`.
    async fn write_block(&mut self, lba: u32, data: &Block) -> Result<(), Self::Error>;
}

/// Why a storage operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreError<E> {
    /// The device reported an error.
    Device(E),
    /// Recovery could not determine the state of the card.
    Recovery(RecoveryError),
    /// The writer could not be started.
    Start(StartError),
}

/// Format a region: write the header and clear both hint blocks.
///
/// Choose a fresh random `card_id` each time; it makes blocks from any earlier
/// use of the same media unrecognisable.
pub async fn prepare<D: BlockDevice>(dev: &mut D, geo: Geometry, card_id: u32) -> Result<(), D::Error> {
    let mut header = [0_u8; BLOCK_SIZE];
    // Encoding a header into a full block cannot fail.
    let _ = encode_region(&mut header, &RegionHeader { card_id, region_blocks: geo.blocks });
    // Blank the hints first so a crash during preparation never leaves a header
    // that points at stale hints.
    for rel in HINT_LBA {
        if let Some(lba) = geo.abs(rel) {
            dev.write_block(lba, &[0; BLOCK_SIZE]).await?;
        }
    }
    if let Some(lba) = geo.abs(HEADER_LBA) {
        dev.write_block(lba, &header).await?;
    }
    Ok(())
}

/// Find where the previous run stopped.
pub async fn recover<D: BlockDevice>(dev: &mut D, geo: Geometry) -> Result<Recovered, StoreError<D::Error>> {
    let mut rec = Recovery::new(geo);
    let mut buf = [0_u8; BLOCK_SIZE];
    let mut last_device_error = None;
    while let Some(lba) = rec.next_read() {
        match dev.read_block(lba, &mut buf).await {
            Ok(()) => rec.provide(Ok(&buf)),
            Err(e) => {
                last_device_error = Some(e);
                rec.provide(Err(ReadFailed));
            }
        }
    }
    match rec.outcome() {
        Some(Ok(r)) => Ok(r),
        Some(Err(RecoveryError::ReadFailed { .. })) => match last_device_error {
            Some(e) => Err(StoreError::Device(e)),
            None => Err(StoreError::Recovery(RecoveryError::NotPrepared)),
        },
        Some(Err(e)) => Err(StoreError::Recovery(e)),
        // `next_read` returned None, so an outcome always exists.
        None => Err(StoreError::Recovery(RecoveryError::NotPrepared)),
    }
}

/// What one [`CardLog::service`] call accomplished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    /// Nothing was pending.
    Idle,
    /// This many blocks (data and hints) were written.
    Wrote(u32),
    /// A data write failed; the log on this card has stopped.
    Faulted,
}

/// A log on one card: the writer plus its service loop.
#[derive(Clone, Debug)]
pub struct CardLog<const DEPTH: usize> {
    writer: LogWriter<DEPTH>,
}

impl<const DEPTH: usize> CardLog<DEPTH> {
    /// Start a new session after [`recover`].
    pub fn open(geo: Geometry, rec: &Recovered) -> Result<Self, StartError> {
        Ok(Self { writer: LogWriter::start(geo, rec)? })
    }

    /// Session number.
    pub fn session(&self) -> u32 {
        self.writer.session()
    }

    /// Writer state.
    pub fn state(&self) -> State {
        self.writer.state()
    }

    /// Writer counters.
    pub fn stats(&self) -> Stats {
        self.writer.stats()
    }

    /// Finished blocks waiting to be written.
    pub fn queued(&self) -> usize {
        self.writer.queued()
    }

    /// True once a data write has failed.
    pub fn is_faulted(&self) -> bool {
        self.writer.state() == State::Faulted
    }

    /// Append a record (see [`LogWriter::push`]).
    pub fn push(&mut self, kind: Kind, body: &[u8]) -> Result<(), PushError> {
        self.writer.push(kind, body)
    }

    /// Finish the open block so it will be written.
    pub fn flush(&mut self) -> Result<(), PushError> {
        self.writer.flush()
    }

    /// End the session cleanly.
    pub fn close(&mut self) -> Result<(), PushError> {
        self.writer.close()
    }

    /// Write everything the writer has pending to `dev`.
    ///
    /// A failed hint write is tolerated. A failed data write stops the log and
    /// returns [`Service::Faulted`]; the caller should report the fault to the
    /// supervisor and stop using this card for the session.
    pub async fn service<D: BlockDevice>(&mut self, dev: &mut D) -> Service {
        let mut wrote: u32 = 0;
        while let Some(req) = self.writer.pending() {
            let kind = req.kind;
            let result = dev.write_block(req.lba, req.data).await;
            match result {
                Ok(()) => {
                    let _ = self.writer.ack_write();
                    wrote = wrote.saturating_add(1);
                }
                Err(_) => {
                    let _ = self.writer.write_failed();
                    if kind == WriteKind::Data {
                        return Service::Faulted;
                    }
                    // A hint failure was dropped by the writer; carry on.
                }
            }
        }
        if self.is_faulted() {
            Service::Faulted
        } else if wrote == 0 {
            Service::Idle
        } else {
            Service::Wrote(wrote)
        }
    }
}
