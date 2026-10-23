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
//! Storage driver tests against simulated cards.

use futures_executor::block_on;
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use tel_logfmt::block::{parse_data, Block, BLOCK_SIZE, DATA_START};
use tel_logfmt::records::{Kind, Records};
use tel_logfmt::recovery::{Geometry, RecoveryError};
use tel_logfmt::writer::{PushError, State};
use tel_store::{prepare, recover, BlockDevice, CardLog, Service, StoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DevError;

struct SimCard {
    start: u32,
    blocks: Vec<Block>,
    write_fail: HashSet<u32>,
    read_fail: HashSet<u32>,
    /// Fail every write once this many writes have succeeded.
    fail_after: Option<usize>,
    writes: usize,
    /// Make the next write return Pending once before completing.
    stall_next_write: bool,
    log: Vec<u32>,
}

impl SimCard {
    fn new(start: u32, n: u32) -> Self {
        let mut x = 0x1234_5678_9ABC_DEF1_u64;
        let blocks = (0..n)
            .map(|_| {
                let mut b = [0_u8; BLOCK_SIZE];
                for c in b.chunks_mut(8) {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    c.copy_from_slice(&x.to_le_bytes());
                }
                b
            })
            .collect();
        Self {
            start,
            blocks,
            write_fail: HashSet::new(),
            read_fail: HashSet::new(),
            fail_after: None,
            writes: 0,
            stall_next_write: false,
            log: Vec::new(),
        }
    }
    fn geo(&self) -> Geometry {
        Geometry { start_lba: self.start, blocks: self.blocks.len() as u32 }
    }
}

struct StallOnce(bool);
impl Future for StallOnce {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

impl BlockDevice for SimCard {
    type Error = DevError;

    async fn read_block(&mut self, lba: u32, buf: &mut Block) -> Result<(), DevError> {
        if self.read_fail.contains(&lba) {
            return Err(DevError);
        }
        *buf = self.blocks[(lba - self.start) as usize];
        Ok(())
    }

    async fn write_block(&mut self, lba: u32, data: &Block) -> Result<(), DevError> {
        assert!(lba >= self.start && lba < self.start + self.blocks.len() as u32, "write outside the region");
        if self.stall_next_write {
            self.stall_next_write = false;
            StallOnce(false).await;
        }
        if self.write_fail.contains(&lba) || self.fail_after.is_some_and(|n| self.writes >= n) {
            return Err(DevError);
        }
        self.blocks[(lba - self.start) as usize] = *data;
        self.writes += 1;
        self.log.push(lba);
        Ok(())
    }
}

fn records(card: &SimCard, card_id: u32, session: u32) -> Vec<(u8, Vec<u8>)> {
    let mut blocks = Vec::new();
    for rel in DATA_START..card.blocks.len() as u32 {
        if let Ok(d) = parse_data(&card.blocks[rel as usize]) {
            if d.header.card_id == card_id && d.header.session == session {
                let recs: Vec<_> =
                    Records::new(d.payload).map(|r| r.unwrap()).map(|r| (r.kind.code(), r.body.to_vec())).collect();
                blocks.push((d.header.seq, recs));
            }
        }
    }
    blocks.sort_by_key(|b| b.0);
    blocks.into_iter().flat_map(|b| b.1).collect()
}

fn body(i: u32) -> Vec<u8> {
    let mut v = i.to_le_bytes().to_vec();
    v.extend((0..(i % 100)).map(|x| x as u8));
    v
}

const ID: u32 = 0x5EED_0001;

#[test]
fn prepare_recover_write_reopen() {
    let mut card = SimCard::new(500, 800);
    let geo = card.geo();
    block_on(prepare(&mut card, geo, ID)).unwrap();
    let rec = block_on(recover(&mut card, geo)).unwrap();
    assert_eq!((rec.card_id, rec.last_session, rec.next_rel), (ID, 0, DATA_START));

    let mut log: CardLog<4> = CardLog::open(geo, &rec).unwrap();
    assert_eq!(log.session(), 1);
    let mut pushed = Vec::new();
    for i in 0..300_u32 {
        let b = body(i);
        while log.push(Kind::Event, &b) == Err(PushError::Overrun) {
            assert!(matches!(block_on(log.service(&mut card)), Service::Wrote(_)));
        }
        pushed.push((Kind::Event.code(), b));
        if i % 5 == 0 {
            block_on(log.service(&mut card));
        }
    }
    while log.close() == Err(PushError::Overrun) {
        block_on(log.service(&mut card));
    }
    assert!(matches!(block_on(log.service(&mut card)), Service::Wrote(_) | Service::Idle));
    assert_eq!(log.state(), State::Closed);
    assert_eq!(log.queued(), 0);
    assert_eq!(records(&card, ID, 1), pushed);

    // reopen: next session continues after the first
    let rec2 = block_on(recover(&mut card, geo)).unwrap();
    assert_eq!(rec2.last_session, 1);
    let log2: CardLog<4> = CardLog::open(geo, &rec2).unwrap();
    assert_eq!(log2.session(), 2);
}

#[test]
fn service_reports_idle_wrote_and_faulted() {
    let mut card = SimCard::new(0, 200);
    let geo = card.geo();
    block_on(prepare(&mut card, geo, ID)).unwrap();
    let rec = block_on(recover(&mut card, geo)).unwrap();
    let mut log: CardLog<4> = CardLog::open(geo, &rec).unwrap();
    // first service writes the initial hint
    assert_eq!(block_on(log.service(&mut card)), Service::Wrote(1));
    assert_eq!(block_on(log.service(&mut card)), Service::Idle);
    log.push(Kind::Event, &[1, 2, 3]).unwrap();
    log.flush().unwrap();
    assert_eq!(block_on(log.service(&mut card)), Service::Wrote(1));
    // now the card dies
    card.fail_after = Some(card.writes);
    log.push(Kind::Event, &[4]).unwrap();
    log.flush().unwrap();
    assert_eq!(block_on(log.service(&mut card)), Service::Faulted);
    assert!(log.is_faulted());
    assert_eq!(log.push(Kind::Event, &[5]), Err(PushError::Faulted));
    // once faulted it stays faulted, and no further writes are attempted
    let w = card.writes;
    card.fail_after = None;
    assert_eq!(block_on(log.service(&mut card)), Service::Faulted);
    assert_eq!(card.writes, w);
}

#[test]
fn a_failing_hint_does_not_stop_logging() {
    let mut card = SimCard::new(0, 400);
    let geo = card.geo();
    block_on(prepare(&mut card, geo, ID)).unwrap();
    card.write_fail.insert(1);
    card.write_fail.insert(2);
    let rec = block_on(recover(&mut card, geo)).unwrap();
    let mut log: CardLog<4> = CardLog::open(geo, &rec).unwrap();
    for i in 0..100_u32 {
        let b = body(i);
        while log.push(Kind::Event, &b) == Err(PushError::Overrun) {
            block_on(log.service(&mut card));
        }
    }
    while log.close() == Err(PushError::Overrun) {
        block_on(log.service(&mut card));
    }
    while log.queued() > 0 {
        assert_ne!(block_on(log.service(&mut card)), Service::Faulted);
    }
    assert!(!log.is_faulted());
    assert_eq!(records(&card, ID, 1).len(), 100);
}

#[test]
fn recover_maps_errors() {
    let mut blank = SimCard::new(0, 100);
    let geo = blank.geo();
    assert_eq!(block_on(recover(&mut blank, geo)), Err(StoreError::Recovery(RecoveryError::NotPrepared)));
    let mut card = SimCard::new(0, 100);
    block_on(prepare(&mut card, geo, ID)).unwrap();
    card.read_fail.insert(0);
    assert_eq!(block_on(recover(&mut card, geo)), Err(StoreError::Device(DevError)));
    card.read_fail.clear();
    assert_eq!(
        block_on(recover(&mut card, Geometry { start_lba: 0, blocks: 99 })),
        Err(StoreError::Recovery(RecoveryError::GeometryMismatch { found: 100 }))
    );
}

#[test]
fn a_read_error_during_the_scan_is_a_device_error_not_end_of_data() {
    let mut card = SimCard::new(0, 600);
    let geo = card.geo();
    block_on(prepare(&mut card, geo, ID)).unwrap();
    let rec = block_on(recover(&mut card, geo)).unwrap();
    let mut log: CardLog<4> = CardLog::open(geo, &rec).unwrap();
    for i in 0..200_u32 {
        let b = body(i);
        while log.push(Kind::Event, &b) == Err(PushError::Overrun) {
            block_on(log.service(&mut card));
        }
    }
    while log.close() == Err(PushError::Overrun) {
        block_on(log.service(&mut card));
    }
    while log.queued() > 0 {
        block_on(log.service(&mut card));
    }
    block_on(log.service(&mut card));
    // wipe the hints so recovery must scan, then make a block in the middle unreadable
    card.blocks[1] = [0; BLOCK_SIZE];
    card.blocks[2] = [0; BLOCK_SIZE];
    card.read_fail.insert(DATA_START + 10);
    assert_eq!(block_on(recover(&mut card, geo)), Err(StoreError::Device(DevError)));
}

#[test]
fn dropping_the_service_future_mid_write_is_safe() {
    let mut card = SimCard::new(0, 300);
    let geo = card.geo();
    block_on(prepare(&mut card, geo, ID)).unwrap();
    let rec = block_on(recover(&mut card, geo)).unwrap();
    let mut log: CardLog<4> = CardLog::open(geo, &rec).unwrap();
    block_on(log.service(&mut card));
    let mut pushed = Vec::new();
    for i in 0..40_u32 {
        let b = body(i);
        while log.push(Kind::Event, &b) == Err(PushError::Overrun) {
            block_on(log.service(&mut card));
        }
        pushed.push((Kind::Event.code(), b));
    }
    log.flush().unwrap();
    // Start a service call, let it stall inside the first write, then drop it.
    card.stall_next_write = true;
    {
        let mut fut = std::pin::pin!(log.service(&mut card));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(fut.as_mut().poll(&mut cx).is_pending());
        // dropped here, before the write completed
    }
    assert!(log.queued() > 0, "the unacknowledged block is still queued");
    while log.queued() > 0 {
        assert!(matches!(block_on(log.service(&mut card)), Service::Wrote(_)));
    }
    while log.close() == Err(PushError::Overrun) {
        block_on(log.service(&mut card));
    }
    block_on(log.service(&mut card));
    assert_eq!(records(&card, ID, 1), pushed, "nothing lost, nothing duplicated");
    // no data block was written twice (the dropped write never reached the media)
    let data_writes: Vec<u32> = card.log.iter().copied().filter(|l| *l >= DATA_START).collect();
    let unique: HashSet<u32> = data_writes.iter().copied().collect();
    assert_eq!(unique.len(), data_writes.len());
}

#[test]
fn mirroring_continues_on_the_surviving_card() {
    let mut a = SimCard::new(100, 600);
    let mut b = SimCard::new(7_000, 600);
    let (ga, gb) = (a.geo(), b.geo());
    block_on(prepare(&mut a, ga, 0xA)).unwrap();
    block_on(prepare(&mut b, gb, 0xB)).unwrap();
    let ra = block_on(recover(&mut a, ga)).unwrap();
    let rb = block_on(recover(&mut b, gb)).unwrap();
    let mut la: CardLog<4> = CardLog::open(ga, &ra).unwrap();
    let mut lb: CardLog<4> = CardLog::open(gb, &rb).unwrap();
    let mut pushed = Vec::new();
    for i in 0..400_u32 {
        let body = body(i);
        // the application writes each record to every card that is still healthy
        for (log, dev) in [(&mut la, &mut a), (&mut lb, &mut b)] {
            if !log.is_faulted() {
                while log.push(Kind::Event, &body) == Err(PushError::Overrun) {
                    block_on(log.service(dev));
                }
            }
        }
        pushed.push((Kind::Event.code(), body));
        if i == 150 {
            a.fail_after = Some(a.writes); // card A dies here
        }
        block_on(la.service(&mut a));
        block_on(lb.service(&mut b));
    }
    lb.close().unwrap();
    block_on(lb.service(&mut b));
    assert!(la.is_faulted());
    assert!(!lb.is_faulted());
    assert_eq!(records(&b, 0xB, 1), pushed, "the surviving card has every record");
    assert!(records(&a, 0xA, 1).len() < pushed.len());
}
