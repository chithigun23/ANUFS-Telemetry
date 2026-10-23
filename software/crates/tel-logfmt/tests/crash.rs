#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    missing_docs,
    clippy::type_complexity,
    clippy::useless_vec,
    clippy::needless_range_loop
)]
//! Crash, corruption and fault tests for the block log, against a simulated
//! card whose unwritten blocks contain random garbage.

use proptest::prelude::*;
use std::collections::{BTreeMap, HashSet};
use tel_logfmt::block::{
    encode_hint, encode_region, parse_data, Block, Hint, RegionHeader, BLOCK_SIZE, DATA_START, FLAG_FIRST, FLAG_LAST,
    HINT_LBA, PAYLOAD_MAX,
};
use tel_logfmt::records::{Kind, Records};
use tel_logfmt::recovery::{Geometry, ReadFailed, Recovered, Recovery, RecoveryError};
use tel_logfmt::writer::{LogWriter, PushError, State, WriteKind, HINT_INTERVAL};

// ---------------------------------------------------------------- simulated card

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

struct Card {
    geo: Geometry,
    blocks: Vec<Block>,
    writes_done: usize,
    crash_at: Option<usize>,
    torn_bytes: usize,
    crashed: bool,
    read_fail: HashSet<u32>,
    write_fail: HashSet<u32>,
    /// Every completed write, in order: (absolute lba, image).
    completed: Vec<(u32, Block)>,
}

impl Card {
    fn new(start_lba: u32, blocks: u32, seed: u64) -> Self {
        let mut rng = Rng(seed | 1);
        let mut v = Vec::new();
        for _ in 0..blocks {
            let mut b = [0_u8; BLOCK_SIZE];
            for chunk in b.chunks_mut(8) {
                chunk.copy_from_slice(&rng.next().to_le_bytes()[..chunk.len()]);
            }
            v.push(b);
        }
        Self {
            geo: Geometry { start_lba, blocks },
            blocks: v,
            writes_done: 0,
            crash_at: None,
            torn_bytes: 0,
            crashed: false,
            read_fail: HashSet::new(),
            write_fail: HashSet::new(),
            completed: Vec::new(),
        }
    }

    fn rel(&self, lba: u32) -> usize {
        (lba - self.geo.start_lba) as usize
    }

    fn write(&mut self, lba: u32, data: &Block) -> Result<(), ()> {
        assert!(lba >= self.geo.start_lba && lba < self.geo.start_lba + self.geo.blocks, "write outside region: {lba}");
        if self.crashed || self.write_fail.contains(&lba) {
            return Err(());
        }
        let idx = self.rel(lba);
        if self.crash_at == Some(self.writes_done) {
            // power cut mid-write: only the first `torn_bytes` reach the media
            let n = self.torn_bytes.min(BLOCK_SIZE);
            self.blocks[idx][..n].copy_from_slice(&data[..n]);
            self.crashed = true;
            if n == BLOCK_SIZE {
                self.completed.push((lba, *data));
            }
            return Err(());
        }
        self.blocks[idx] = *data;
        self.writes_done += 1;
        self.completed.push((lba, *data));
        Ok(())
    }

    fn read(&self, lba: u32) -> Result<Block, ReadFailed> {
        if self.read_fail.contains(&lba) {
            return Err(ReadFailed);
        }
        Ok(self.blocks[self.rel(lba)])
    }

    /// Power cycle: the card keeps its contents, faults are cleared.
    fn power_cycle(&mut self) {
        self.crashed = false;
        self.crash_at = None;
        self.read_fail.clear();
        self.write_fail.clear();
    }

    fn prepare(&mut self, card_id: u32) {
        let mut b = [0_u8; BLOCK_SIZE];
        encode_region(&mut b, &RegionHeader { card_id, region_blocks: self.geo.blocks }).unwrap();
        self.write(self.geo.start_lba, &b).unwrap();
        for h in HINT_LBA {
            self.write(self.geo.start_lba + h, &[0; BLOCK_SIZE]).unwrap();
        }
        self.completed.clear();
        self.writes_done = 0;
    }
}

fn recover(card: &Card) -> Result<Recovered, RecoveryError> {
    let mut r = Recovery::new(card.geo);
    while let Some(lba) = r.next_read() {
        match card.read(lba) {
            Ok(b) => r.provide(Ok(&b)),
            Err(e) => r.provide(Err(e)),
        }
    }
    r.outcome().unwrap()
}

/// Run the writer's I/O loop until nothing is pending or a write fails.
fn pump<const D: usize>(w: &mut LogWriter<D>, card: &mut Card) -> usize {
    let mut n = 0;
    while let Some(req) = w.pending() {
        let (lba, data) = (req.lba, *req.data);
        match card.write(lba, &data) {
            Ok(()) => {
                w.ack_write().unwrap();
                n += 1;
            }
            Err(()) => {
                w.write_failed().unwrap();
                break;
            }
        }
        if card.crashed {
            break;
        }
    }
    n
}

/// Read every valid data block of the region as the host tool would and return
/// records per session, checking sequence continuity inside each session.
fn read_all(card: &Card, card_id: u32) -> BTreeMap<u32, Vec<(u8, Vec<u8>)>> {
    let mut per: BTreeMap<u32, Vec<(u32, Vec<(u8, Vec<u8>)>)>> = BTreeMap::new();
    for rel in DATA_START..card.geo.blocks {
        let b = card.blocks[rel as usize];
        if let Ok(d) = parse_data(&b) {
            if d.header.card_id != card_id {
                continue;
            }
            let recs: Vec<(u8, Vec<u8>)> = Records::new(d.payload)
                .map(|r| {
                    let r = r.expect("a CRC-valid block never has torn framing");
                    (r.kind.code(), r.body.to_vec())
                })
                .collect();
            per.entry(d.header.session).or_default().push((d.header.seq, recs));
        }
    }
    let mut out = BTreeMap::new();
    for (s, mut blocks) in per {
        blocks.sort_by_key(|b| b.0);
        for (i, b) in blocks.iter().enumerate() {
            assert_eq!(b.0 as usize, i, "session {s}: sequence gap or duplicate");
        }
        out.insert(s, blocks.into_iter().flat_map(|b| b.1).collect());
    }
    out
}

/// Records contained in completed (fully written) data blocks, by session.
fn completed_records(card: &Card, card_id: u32) -> BTreeMap<u32, Vec<(u8, Vec<u8>)>> {
    let mut per: BTreeMap<u32, Vec<(u32, Vec<(u8, Vec<u8>)>)>> = BTreeMap::new();
    for (_, img) in &card.completed {
        if let Ok(d) = parse_data(img) {
            if d.header.card_id == card_id {
                let recs =
                    Records::new(d.payload).map(|r| r.unwrap()).map(|r| (r.kind.code(), r.body.to_vec())).collect();
                per.entry(d.header.session).or_default().push((d.header.seq, recs));
            }
        }
    }
    per.into_iter()
        .map(|(s, mut b)| {
            b.sort_by_key(|x| x.0);
            (s, b.into_iter().flat_map(|x| x.1).collect())
        })
        .collect()
}

fn rec_body(id: u32, len: usize) -> Vec<u8> {
    let mut v = id.to_le_bytes().to_vec();
    v.extend((0..len).map(|i| (id as usize + i) as u8));
    v
}

const CARD_ID: u32 = 0xC0FF_EE01;
const START: u32 = 1000;

fn fresh_card(blocks: u32, seed: u64) -> Card {
    let mut c = Card::new(START, blocks, seed);
    c.prepare(CARD_ID);
    c
}

// ---------------------------------------------------------------- basic behaviour

#[test]
fn fresh_card_starts_at_session_one() {
    let card = fresh_card(1000, 1);
    let r = recover(&card).unwrap();
    assert_eq!(r.card_id, CARD_ID);
    assert_eq!(r.last_session, 0);
    assert_eq!(r.next_rel, DATA_START);
    assert_eq!(r.blocks_scanned, 0);
}

#[test]
fn write_close_reopen_roundtrip_across_sessions() {
    let mut card = fresh_card(1000, 2);
    let mut expected: BTreeMap<u32, Vec<(u8, Vec<u8>)>> = BTreeMap::new();
    for session in 1..=3_u32 {
        let rec = recover(&card).unwrap();
        assert_eq!(rec.last_session, session - 1);
        let mut w: LogWriter<4> = LogWriter::start(card.geo, &rec).unwrap();
        assert_eq!(w.session(), session);
        for i in 0..200_u32 {
            let body = rec_body(session * 1000 + i, (i % 90) as usize);
            while w.push(Kind::Event, &body) == Err(PushError::Overrun) {
                pump(&mut w, &mut card);
            }
            expected.entry(session).or_default().push((Kind::Event.code(), body));
            if i % 7 == 0 {
                pump(&mut w, &mut card);
            }
        }
        while w.close() == Err(PushError::Overrun) {
            pump(&mut w, &mut card);
        }
        pump(&mut w, &mut card);
        assert_eq!(w.state(), State::Closed);
        assert_eq!(w.queued(), 0);
        assert_eq!(w.stats().dropped_overrun, 0, "overrun was handled by pumping");
    }
    assert_eq!(read_all(&card, CARD_ID), expected);
    // the closing block of the last session carries LAST
    let last = (DATA_START..card.geo.blocks)
        .filter_map(|r| parse_data(&card.blocks[r as usize]).ok())
        .filter(|d| d.header.card_id == CARD_ID && d.header.session == 3)
        .max_by_key(|d| d.header.seq)
        .unwrap();
    assert!(last.header.flags & FLAG_LAST != 0);
}

#[test]
fn blocks_are_contiguous_with_first_flag_only_on_seq_zero() {
    let mut card = fresh_card(500, 3);
    let rec = recover(&card).unwrap();
    let mut w: LogWriter<8> = LogWriter::start(card.geo, &rec).unwrap();
    for i in 0..300_u32 {
        let _ = w.push(Kind::Sample, &rec_body(i, 100));
        pump(&mut w, &mut card);
    }
    w.close().unwrap();
    pump(&mut w, &mut card);
    let mut prev_rel = None;
    for rel in DATA_START..card.geo.blocks {
        if let Ok(d) = parse_data(&card.blocks[rel as usize]) {
            if d.header.card_id != CARD_ID {
                continue;
            }
            assert_eq!(d.header.flags & FLAG_FIRST != 0, d.header.seq == 0);
            assert_eq!(d.header.seq, rel - DATA_START, "first session fills from the start");
            if let Some(p) = prev_rel {
                assert_eq!(rel, p + 1);
            }
            prev_rel = Some(rel);
        }
    }
    assert!(prev_rel.is_some());
}

#[test]
fn flush_publishes_a_partial_block_and_never_rewrites_it() {
    let mut card = fresh_card(100, 4);
    let rec = recover(&card).unwrap();
    let mut w: LogWriter<4> = LogWriter::start(card.geo, &rec).unwrap();
    w.push(Kind::Event, &[1, 2, 3]).unwrap();
    assert_eq!(w.active_bytes(), 5);
    w.flush().unwrap();
    assert_eq!(w.active_bytes(), 0);
    pump(&mut w, &mut card);
    let first_rel = DATA_START;
    let image_before = card.blocks[first_rel as usize];
    w.push(Kind::Event, &[4, 5]).unwrap();
    w.flush().unwrap();
    pump(&mut w, &mut card);
    assert_eq!(card.blocks[first_rel as usize], image_before, "an already written block is never modified");
    assert_eq!(read_all(&card, CARD_ID)[&1], vec![(5, vec![1, 2, 3]), (5, vec![4, 5])]);
    // flushing an empty block is a no-op
    let q = w.queued();
    w.flush().unwrap();
    assert_eq!(w.queued(), q);
}

#[test]
fn record_that_exactly_fills_a_block_and_the_next_that_does_not_fit() {
    let mut card = fresh_card(100, 5);
    let rec = recover(&card).unwrap();
    let mut w: LogWriter<4> = LogWriter::start(card.geo, &rec).unwrap();
    // 486-byte body + 2 byte framing = 488 = PAYLOAD_MAX... bodies are capped at 255, so use two records
    let a = vec![7_u8; 255]; // 257 bytes
    let b = vec![8_u8; 229]; // 231 bytes: 257 + 231 = 488 exactly
    w.push(Kind::Event, &a).unwrap();
    w.push(Kind::Event, &b).unwrap();
    assert_eq!(w.active_bytes(), PAYLOAD_MAX);
    assert_eq!(w.queued(), 0, "an exactly full block is not sealed until needed");
    w.push(Kind::Event, &[9]).unwrap();
    assert_eq!(w.queued(), 1);
    w.close().unwrap();
    pump(&mut w, &mut card);
    let got = &read_all(&card, CARD_ID)[&1];
    assert_eq!(got, &vec![(5, a), (5, b), (5, vec![9])]);
}

#[test]
fn oversized_records_and_wrong_states_are_refused() {
    let card = fresh_card(100, 6);
    let rec = recover(&card).unwrap();
    let mut w: LogWriter<2> = LogWriter::start(card.geo, &rec).unwrap();
    assert_eq!(w.push(Kind::Event, &[0; 256]), Err(PushError::TooLarge));
    assert!(w.push(Kind::Event, &[0; 255]).is_ok());
    w.close().unwrap();
    assert_eq!(w.push(Kind::Event, &[1]), Err(PushError::Closed));
    assert_eq!(w.stats().dropped_state, 1);
    assert!(w.close().is_ok(), "closing twice is harmless");
}

#[test]
fn zero_depth_is_rejected() {
    let card = fresh_card(100, 7);
    let rec = recover(&card).unwrap();
    assert!(LogWriter::<0>::start(card.geo, &rec).is_err());
}

// ---------------------------------------------------------------- overrun and full

#[test]
fn ring_overrun_drops_are_counted_and_nothing_is_corrupted() {
    let mut card = fresh_card(1000, 8);
    let rec = recover(&card).unwrap();
    let mut w: LogWriter<2> = LogWriter::start(card.geo, &rec).unwrap();
    let mut accepted = Vec::new();
    let mut refused = 0;
    for i in 0..60_u32 {
        let body = rec_body(i, 200);
        match w.push(Kind::Event, &body) {
            Ok(()) => accepted.push((Kind::Event.code(), body)),
            Err(PushError::Overrun) => refused += 1,
            Err(e) => panic!("{e:?}"),
        }
    }
    assert!(refused > 0, "with nothing draining, the ring must overrun");
    assert_eq!(w.stats().dropped_overrun, refused);
    assert_eq!(w.queued(), 2);
    // Now drain and keep going: accepted again.
    pump(&mut w, &mut card);
    assert!(w.push(Kind::Event, &rec_body(999, 10)).is_ok());
    accepted.push((Kind::Event.code(), rec_body(999, 10)));
    w.close().unwrap();
    pump(&mut w, &mut card);
    assert_eq!(read_all(&card, CARD_ID)[&1], accepted);
}

#[test]
fn flush_and_close_report_overrun_and_can_be_retried() {
    let mut card = fresh_card(100, 9);
    let rec = recover(&card).unwrap();
    let mut w: LogWriter<1> = LogWriter::start(card.geo, &rec).unwrap();
    w.push(Kind::Event, &[1]).unwrap();
    w.flush().unwrap();
    assert_eq!(w.queued(), 1);
    w.push(Kind::Event, &[2]).unwrap();
    assert_eq!(w.flush(), Err(PushError::Overrun));
    assert_eq!(w.close(), Err(PushError::Overrun));
    assert_eq!(w.state(), State::Open);
    pump(&mut w, &mut card);
    assert_eq!(w.close(), Ok(()));
    pump(&mut w, &mut card);
    assert_eq!(read_all(&card, CARD_ID)[&1], vec![(5, vec![1]), (5, vec![2])]);
}

#[test]
fn full_region_never_writes_outside_and_recovers_as_full() {
    let blocks = DATA_START + 6;
    let mut card = fresh_card(blocks, 10);
    let rec = recover(&card).unwrap();
    let mut w: LogWriter<2> = LogWriter::start(card.geo, &rec).unwrap();
    let mut full_seen = false;
    for i in 0..200_u32 {
        match w.push(Kind::Event, &rec_body(i, 150)) {
            Ok(()) => {}
            Err(PushError::Overrun) => {
                pump(&mut w, &mut card);
            }
            Err(PushError::Full) => {
                full_seen = true;
                break;
            }
            Err(e) => panic!("{e:?}"),
        }
        pump(&mut w, &mut card); // Card::write asserts every lba is inside the region
    }
    assert!(full_seen);
    assert_eq!(w.state(), State::Full);
    pump(&mut w, &mut card);
    assert!(w.stats().dropped_full >= 1);
    let r = recover(&card).unwrap();
    assert_eq!(r.next_rel, blocks, "no room left");
    let w2: LogWriter<2> = LogWriter::start(card.geo, &r).unwrap();
    assert_eq!(w2.state(), State::Full);
}

// ---------------------------------------------------------------- write faults

#[test]
fn data_write_failure_faults_the_writer_and_stops_output() {
    let mut card = fresh_card(100, 11);
    let rec = recover(&card).unwrap();
    let mut w: LogWriter<4> = LogWriter::start(card.geo, &rec).unwrap();
    pump(&mut w, &mut card); // initial hint
    for i in 0..10_u32 {
        let _ = w.push(Kind::Event, &rec_body(i, 200));
    }
    let target = card.geo.start_lba + DATA_START + 1;
    card.write_fail.insert(target);
    pump(&mut w, &mut card);
    assert_eq!(w.state(), State::Faulted);
    assert!(w.pending().is_none(), "no more writes are offered after a data write fault");
    assert_eq!(w.push(Kind::Event, &[1]), Err(PushError::Faulted));
    assert_eq!(w.ack_write(), Err(tel_logfmt::writer::NothingPending));
    // the block before the failure is intact
    assert_eq!(read_all(&card, CARD_ID)[&1].len(), 2);
}

#[test]
fn hint_write_failure_is_not_fatal() {
    let mut card = fresh_card(2000, 12);
    for h in HINT_LBA {
        card.write_fail.insert(card.geo.start_lba + h);
    }
    let rec = recover(&card).unwrap();
    let mut w: LogWriter<4> = LogWriter::start(card.geo, &rec).unwrap();
    let mut pushed = Vec::new();
    for i in 0..1200_u32 {
        let body = rec_body(i, 200);
        while w.push(Kind::Event, &body) == Err(PushError::Overrun) {
            pump(&mut w, &mut card);
        }
        pushed.push((Kind::Event.code(), body));
        pump(&mut w, &mut card);
    }
    assert_ne!(w.state(), State::Faulted);
    w.close().unwrap();
    pump(&mut w, &mut card);
    assert_eq!(read_all(&card, CARD_ID)[&1], pushed);
    // With no hint ever written, recovery falls back to scanning every block.
    card.power_cycle();
    let r = recover(&card).unwrap();
    assert_eq!(r.last_session, 1);
    assert_eq!(
        r.next_rel,
        DATA_START
            + card.completed.iter().filter(|(l, _)| parse_data(&card.blocks[(*l - START) as usize]).is_ok()).count()
                as u32
    );
}

// ---------------------------------------------------------------- hints

#[test]
fn hints_alternate_slots_and_bound_the_recovery_scan() {
    let mut card = fresh_card(5000, 13);
    let rec = recover(&card).unwrap();
    let mut w: LogWriter<4> = LogWriter::start(card.geo, &rec).unwrap();
    let mut hint_lbas = Vec::new();
    for i in 0..3000_u32 {
        while w.push(Kind::Event, &rec_body(i, 220)) == Err(PushError::Overrun) {
            drain(&mut w, &mut card, &mut hint_lbas);
        }
        drain(&mut w, &mut card, &mut hint_lbas);
    }
    assert!(hint_lbas.len() >= 5, "several hints must have been written");
    // One hint per HINT_INTERVAL data blocks plus the initial one: never a hint per block.
    let data_blocks = w.stats().blocks_written;
    assert!(data_blocks > 1000);
    assert!(
        hint_lbas.len() as u32 <= data_blocks / HINT_INTERVAL + 2,
        "{} hints for {} blocks",
        hint_lbas.len(),
        data_blocks
    );
    for pair in hint_lbas.windows(2) {
        assert_ne!(pair[0], pair[1], "consecutive hints must use different slots");
    }
    assert!(hint_lbas.iter().all(|l| *l == START + HINT_LBA[0] || *l == START + HINT_LBA[1]));
    card.power_cycle();
    let r = recover(&card).unwrap();
    assert!(r.blocks_scanned <= HINT_INTERVAL + 2, "scan covered {} blocks", r.blocks_scanned);
    assert_eq!(r.last_session, 1);
}

fn drain<const D: usize>(w: &mut LogWriter<D>, card: &mut Card, hints: &mut Vec<u32>) {
    while let Some(req) = w.pending() {
        let (lba, data, kind) = (req.lba, *req.data, req.kind);
        card.write(lba, &data).unwrap();
        w.ack_write().unwrap();
        if kind == WriteKind::Hint {
            hints.push(lba);
        }
    }
}

#[test]
fn a_corrupted_hint_slot_falls_back_to_the_other_or_to_a_full_scan() {
    let mut card = fresh_card(3000, 14);
    let rec = recover(&card).unwrap();
    let mut w: LogWriter<4> = LogWriter::start(card.geo, &rec).unwrap();
    let mut hints = Vec::new();
    for i in 0..2000_u32 {
        while w.push(Kind::Event, &rec_body(i, 220)) == Err(PushError::Overrun) {
            drain(&mut w, &mut card, &mut hints);
        }
        drain(&mut w, &mut card, &mut hints);
    }
    w.close().unwrap();
    drain(&mut w, &mut card, &mut hints);
    let good = recover(&card).unwrap();
    for slot in 0..2 {
        let mut c2 = Card::new(START, 3000, 14);
        c2.blocks = card.blocks.clone();
        c2.blocks[HINT_LBA[slot] as usize][77] ^= 0xFF;
        assert_eq!(recover(&c2).unwrap().next_rel, good.next_rel, "slot {slot} corrupted");
    }
    let mut c3 = Card::new(START, 3000, 14);
    c3.blocks = card.blocks.clone();
    for h in HINT_LBA {
        c3.blocks[h as usize] = [0; BLOCK_SIZE];
    }
    let r = recover(&c3).unwrap();
    assert_eq!(r.next_rel, good.next_rel, "no hints: full scan finds the same end");
    assert_eq!(r.last_session, 1);
    assert!(r.blocks_scanned > HINT_INTERVAL, "had to scan everything");
}

#[test]
fn hint_from_another_card_id_is_ignored() {
    let mut card = fresh_card(500, 15);
    let mut b = [0_u8; BLOCK_SIZE];
    // a hint with the wrong card id claiming the data is far along
    encode_hint(&mut b, &Hint { card_id: 0x1111, hint_seq: 9999, last_session: 50, tail: 400, next_seq: 7 }).unwrap();
    card.write(START + HINT_LBA[0], &b).unwrap();
    let r = recover(&card).unwrap();
    assert_eq!((r.last_session, r.next_rel, r.hint_seq), (0, DATA_START, 0));
    // a hint with the right card id but a tail outside the region is ignored too
    encode_hint(&mut b, &Hint { card_id: CARD_ID, hint_seq: 5, last_session: 1, tail: 100_000, next_seq: 1 }).unwrap();
    card.write(START + HINT_LBA[1], &b).unwrap();
    let r = recover(&card).unwrap();
    assert_eq!((r.last_session, r.next_rel), (0, DATA_START));
}

// ---------------------------------------------------------------- recovery errors

#[test]
fn unprepared_cards_and_wrong_sizes_are_reported() {
    let card = Card::new(START, 500, 16); // garbage everywhere
    assert_eq!(recover(&card), Err(RecoveryError::NotPrepared));
    let mut card = fresh_card(500, 17);
    card.geo.blocks = 500;
    let mut r = Recovery::new(Geometry { start_lba: START, blocks: 400 });
    // The expected size differs from the header: refuse instead of guessing.
    while let Some(lba) = r.next_read() {
        match card.read(lba) {
            Ok(b) => r.provide(Ok(&b)),
            Err(e) => r.provide(Err(e)),
        }
        if lba >= START + 500 {
            break;
        }
    }
    assert_eq!(r.outcome(), Some(Err(RecoveryError::GeometryMismatch { found: 500 })));
    assert_eq!(recover(&Card::new(START, 3, 1)), Err(RecoveryError::RegionTooSmall));
    assert_eq!(recover(&Card::new(START, DATA_START, 1)), Err(RecoveryError::RegionTooSmall));
}

#[test]
fn read_errors_abort_instead_of_being_taken_for_end_of_data() {
    let mut card = fresh_card(2000, 18);
    let rec = recover(&card).unwrap();
    let mut w: LogWriter<4> = LogWriter::start(card.geo, &rec).unwrap();
    for i in 0..400_u32 {
        while w.push(Kind::Event, &rec_body(i, 220)) == Err(PushError::Overrun) {
            pump(&mut w, &mut card);
        }
        pump(&mut w, &mut card);
    }
    w.flush().unwrap();
    pump(&mut w, &mut card);
    let good = recover(&card).unwrap();
    // an I/O error in the middle of the (hint-less) scan must not look like the end
    let mut c2 = Card::new(START, 2000, 18);
    c2.blocks = card.blocks.clone();
    for h in HINT_LBA {
        c2.blocks[h as usize] = [0; BLOCK_SIZE];
    }
    c2.read_fail.insert(START + DATA_START + 50);
    assert_eq!(recover(&c2), Err(RecoveryError::ReadFailed { lba: START + DATA_START + 50 }));
    // header read error aborts
    let mut c3 = Card::new(START, 2000, 18);
    c3.blocks = card.blocks.clone();
    c3.read_fail.insert(START);
    assert_eq!(recover(&c3), Err(RecoveryError::ReadFailed { lba: START }));
    // an unreadable hint is merely skipped
    let mut c4 = Card::new(START, 2000, 18);
    c4.blocks = card.blocks.clone();
    c4.read_fail.insert(START + HINT_LBA[0]);
    c4.read_fail.insert(START + HINT_LBA[1]);
    assert_eq!(recover(&c4).unwrap().next_rel, good.next_rel);
}

#[test]
fn stale_data_from_a_previous_card_id_is_never_adopted() {
    // Write a full session with an old id, then re-prepare with a new id on the same media.
    let mut card = fresh_card(1500, 19);
    let rec = recover(&card).unwrap();
    let mut w: LogWriter<4> = LogWriter::start(card.geo, &rec).unwrap();
    for i in 0..300_u32 {
        while w.push(Kind::Event, &rec_body(i, 200)) == Err(PushError::Overrun) {
            pump(&mut w, &mut card);
        }
        pump(&mut w, &mut card);
    }
    w.close().unwrap();
    pump(&mut w, &mut card);
    let used = recover(&card).unwrap().next_rel;
    assert!(used > DATA_START + 50);
    card.prepare(0xABCD_0002); // new preparation; old blocks remain on the media
    let r = recover(&card).unwrap();
    assert_eq!((r.card_id, r.last_session, r.next_rel), (0xABCD_0002, 0, DATA_START));
    assert!(read_all(&card, 0xABCD_0002).is_empty());
}

#[test]
fn a_replayed_old_block_after_the_end_is_rejected() {
    let mut card = fresh_card(1000, 20);
    let rec = recover(&card).unwrap();
    let mut w: LogWriter<4> = LogWriter::start(card.geo, &rec).unwrap();
    for i in 0..100_u32 {
        while w.push(Kind::Event, &rec_body(i, 200)) == Err(PushError::Overrun) {
            pump(&mut w, &mut card);
        }
        pump(&mut w, &mut card);
    }
    w.flush().unwrap();
    pump(&mut w, &mut card);
    let end = recover(&card).unwrap().next_rel;
    // copy an earlier valid block (CRC good, right card id, wrong seq) to the end position
    let old = card.blocks[(DATA_START + 3) as usize];
    card.blocks[end as usize] = old;
    assert_eq!(recover(&card).unwrap().next_rel, end, "a block that does not continue the session is not accepted");
}

// ---------------------------------------------------------------- crash property

#[derive(Clone, Debug)]
enum Op {
    Push { len: usize },
    Flush,
    Pump(usize),
}

fn arb_ops() -> impl Strategy<Value = Vec<Op>> {
    prop::collection::vec(
        prop_oneof![
            6 => (0_usize..255).prop_map(|len| Op::Push { len }),
            1 => Just(Op::Flush),
            3 => (0_usize..6).prop_map(Op::Pump),
        ],
        1..400,
    )
}

/// Pump at most `n` completed writes.
fn pump_n<const D: usize>(w: &mut LogWriter<D>, card: &mut Card, n: usize) {
    for _ in 0..n {
        let Some(req) = w.pending() else { return };
        let (lba, data) = (req.lba, *req.data);
        match card.write(lba, &data) {
            Ok(()) => {
                w.ack_write().unwrap();
            }
            Err(()) => {
                w.write_failed().unwrap();
                return;
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// After any number of power cuts at arbitrary points (with torn writes),
    /// the recovered log contains exactly the records of every completed write,
    /// in order and uncorrupted, and no valid block is ever overwritten.
    #[test]
    fn survives_repeated_power_loss(
        cycles in prop::collection::vec((arb_ops(), 0_usize..400, 0_usize..BLOCK_SIZE), 1..5),
        seed in any::<u64>(),
    ) {
        let mut card = fresh_card(3000, seed);
        let mut next_id = 0_u32;
        let mut pushed_by_session: BTreeMap<u32, Vec<(u8, Vec<u8>)>> = BTreeMap::new();
        for (ops, crash_after, torn) in cycles {
            card.power_cycle();
            let rec = recover(&card).expect("a prepared card always recovers");
            // A new session must never reuse the number of a session that has durable data.
            let max_durable = card
                .completed
                .iter()
                .filter_map(|(_, img)| parse_data(img).ok())
                .filter(|d| d.header.card_id == CARD_ID)
                .map(|d| d.header.session)
                .max()
                .unwrap_or(0);
            prop_assert!(rec.last_session >= max_durable, "recovery missed durable session {}", max_durable);
            prop_assert!(rec.blocks_scanned <= 2 * HINT_INTERVAL + 4);
            let mut w: LogWriter<4> = LogWriter::start(card.geo, &rec).unwrap();
            prop_assert_eq!(w.session(), rec.last_session + 1);
            prop_assert!(w.session() > max_durable);
            // An earlier incarnation that left no durable data may have used this number.
            pushed_by_session.insert(w.session(), Vec::new());
            card.crash_at = Some(card.writes_done + crash_after);
            card.torn_bytes = torn;
            let writes_before = card.completed.len();
            for op in ops {
                match op {
                    Op::Push { len } => {
                        let body = rec_body(next_id, len.min(240));
                        next_id += 1;
                        if w.push(Kind::Event, &body).is_ok() {
                            pushed_by_session.entry(w.session()).or_default().push((Kind::Event.code(), body));
                        }
                    }
                    Op::Flush => { let _ = w.flush(); }
                    Op::Pump(n) => pump_n(&mut w, &mut card, n),
                }
                if card.crashed { break; }
            }
            // Blocks completed in this cycle must never have replaced earlier valid blocks.
            let mut seen = HashSet::new();
            for (lba, img) in &card.completed[writes_before..] {
                if parse_data(img).is_ok() {
                    prop_assert!(seen.insert(*lba), "a block was written twice");
                    // nothing valid existed there before
                    let before_valid = card.completed[..writes_before].iter().any(|(l, i)| l == lba && parse_data(i).is_ok());
                    prop_assert!(!before_valid, "overwrote a previously valid block at {}", lba);
                }
            }
        }
        // Final power cycle and full read-back.
        card.power_cycle();
        let all = read_all(&card, CARD_ID);
        let truth = completed_records(&card, CARD_ID);
        prop_assert_eq!(&all, &truth, "recovered records must equal the records of completed writes");
        for (s, recs) in &all {
            let pushed = pushed_by_session.get(s).cloned().unwrap_or_default();
            prop_assert!(recs.len() <= pushed.len());
            prop_assert_eq!(recs, &pushed[..recs.len()], "session {} is not a clean prefix of what was pushed", s);
        }
    }

    /// A bit flip anywhere in any block never yields a wrong record: the
    /// block is either intact or dropped.
    #[test]
    fn bit_rot_never_produces_wrong_records(
        flips in prop::collection::vec((0_u32..40, 0_usize..BLOCK_SIZE, 0_u8..8), 1..30),
    ) {
        let mut card = fresh_card(200, 77);
        let rec = recover(&card).unwrap();
        let mut w: LogWriter<4> = LogWriter::start(card.geo, &rec).unwrap();
        let mut pushed = Vec::new();
        for i in 0..120_u32 {
            let body = rec_body(i, 150);
            while w.push(Kind::Event, &body) == Err(PushError::Overrun) { pump(&mut w, &mut card); }
            pushed.push((Kind::Event.code(), body));
            pump(&mut w, &mut card);
        }
        w.close().unwrap();
        pump(&mut w, &mut card);
        for (blk, byte, bit) in flips {
            card.blocks[(DATA_START + blk) as usize][byte] ^= 1 << bit;
        }
        // Whatever survives must be a subsequence of what was written, in order.
        let mut per_block: Vec<Vec<(u8, Vec<u8>)>> = Vec::new();
        for rel in DATA_START..card.geo.blocks {
            if let Ok(d) = parse_data(&card.blocks[rel as usize]) {
                if d.header.card_id == CARD_ID {
                    per_block.push(Records::new(d.payload).map(|r| { let r = r.unwrap(); (r.kind.code(), r.body.to_vec()) }).collect());
                }
            }
        }
        let flat: Vec<_> = per_block.into_iter().flatten().collect();
        let mut it = pushed.iter();
        for r in &flat {
            prop_assert!(it.any(|p| p == r), "record not in the written sequence");
        }
    }
}
