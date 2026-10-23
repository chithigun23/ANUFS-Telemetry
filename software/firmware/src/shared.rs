//! State and queues shared between the logger tasks.
//!
//! Everything here is plain data behind a critical-section mutex or a channel,
//! so tasks on different executors can use it without `unsafe`. The critical
//! sections are a few instructions long.

use core::cell::RefCell;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use tel_can_sbg::Latest as SbgLatest;
use tel_core::clock::PpsClock;
use tel_core::imu_window::ImuWindow;
use tel_core::supervisor::{Card, Event};
use tel_logfmt::records::{Kind, MAX_BODY};

/// One record waiting to be written.
#[derive(Clone, Copy)]
pub struct Rec {
    pub kind: Kind,
    len: u8,
    body: [u8; MAX_BODY],
}

impl Rec {
    /// Copy `bytes` into a record. `None` if they do not fit.
    pub fn new(kind: Kind, bytes: &[u8]) -> Option<Self> {
        let mut body = [0_u8; MAX_BODY];
        body.get_mut(..bytes.len())?.copy_from_slice(bytes);
        Some(Self { kind, len: u8::try_from(bytes.len()).ok()?, body })
    }

    /// The record body.
    pub fn bytes(&self) -> &[u8] {
        self.body.get(..usize::from(self.len)).unwrap_or(&[])
    }
}

/// Records queued per card.
pub const REC_QUEUE: usize = 24;

/// What the supervisor tells a card task to do.
#[derive(Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum CardCmd {
    /// Initialise the card and recover its log region.
    Mount,
    /// Start a log session.
    Open,
}

/// Records for each card (index 0 = SD1, 1 = SD2).
pub static CARD_RECS: [Channel<CriticalSectionRawMutex, Rec, REC_QUEUE>; 2] = [Channel::new(), Channel::new()];

/// Commands for each card task.
pub static CARD_CMD: [Signal<CriticalSectionRawMutex, CardCmd>; 2] = [Signal::new(), Signal::new()];

/// Events for the supervisor.
pub static SUP_EVENTS: Channel<CriticalSectionRawMutex, Event, 32> = Channel::new();

/// Raised when the supervisor wants the GNSS module configured.
pub static GNSS_START: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Index of a card in the arrays above.
pub const fn card_index(card: Card) -> usize {
    match card {
        Card::Sd1 => 0,
        Card::Sd2 => 1,
    }
}

/// Mutable state used by more than one task.
pub struct Shared {
    /// UTC clock disciplined by the pulse per second.
    pub clock: PpsClock,
    /// IMU samples since the last record.
    pub imu: ImuWindow,
    /// Newest decoded SBG signals.
    pub sbg: SbgLatest,
    /// Frames received on CAN1 and CAN2.
    pub can_frames: [u32; 2],
    /// Records dropped because a card queue was full, per card.
    pub overruns: [u32; 2],
    /// Flag bits the supervisor wants in every sample.
    pub ext_flags: u32,
    /// Bits for `CAN1_ALIVE` / `CAN2_ALIVE`.
    pub can_alive: [bool; 2],
    /// A session is open on the card, so records for it are being drained.
    pub recording: [bool; 2],
    /// Supervisor events dropped because the event queue was full.
    pub event_drops: u32,
    /// When a queue overrun was last reported for each card, in milliseconds.
    last_overrun_event_ms: [u64; 2],
}

impl Shared {
    const fn new() -> Self {
        Self {
            clock: PpsClock::new(tel_core::clock::Config::DEFAULT),
            imu: ImuWindow::new(),
            sbg: SbgLatest::new(),
            can_frames: [0; 2],
            overruns: [0; 2],
            ext_flags: 0,
            can_alive: [false; 2],
            recording: [false; 2],
            event_drops: 0,
            last_overrun_event_ms: [0; 2],
        }
    }
}

static SHARED: Mutex<CriticalSectionRawMutex, RefCell<Shared>> = Mutex::new(RefCell::new(Shared::new()));

/// Run `f` with exclusive access to the shared state.
pub fn with<R>(f: impl FnOnce(&mut Shared) -> R) -> R {
    SHARED.lock(|cell| f(&mut cell.borrow_mut()))
}

/// Offer an event to the supervisor. A full queue drops the event; the
/// supervisor tick re-derives most state, and the drop is counted.
pub fn send_event(ev: Event) {
    if SUP_EVENTS.try_send(ev).is_err() {
        with(|s| s.event_drops = s.event_drops.saturating_add(1));
    }
}

/// Queue a record for every card that has a session open. A card whose queue is
/// full loses the record; the loss is counted, and reported to the supervisor
/// at most twice a second so a stalled card cannot flood it.
pub fn publish(kind: Kind, bytes: &[u8]) {
    let Some(rec) = Rec::new(kind, bytes) else { return };
    let now_ms = embassy_time::Instant::now().as_millis();
    for (i, ch) in CARD_RECS.iter().enumerate() {
        if !with(|s| s.recording.get(i).copied().unwrap_or(false)) {
            continue;
        }
        if ch.try_send(rec).is_err() {
            let report = with(|s| {
                if let Some(c) = s.overruns.get_mut(i) {
                    *c = c.saturating_add(1);
                }
                match s.last_overrun_event_ms.get_mut(i) {
                    Some(last) if now_ms.saturating_sub(*last) >= 500 => {
                        *last = now_ms;
                        true
                    }
                    _ => false,
                }
            });
            if report {
                let queue = u8::try_from(i).unwrap_or(u8::MAX);
                send_event(Event::QueueOverrun { queue });
            }
        }
    }
}
