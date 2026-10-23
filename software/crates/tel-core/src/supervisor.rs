//! The supervisor: a pure state machine that decides what the system should do
//! as cards come and go, the GNSS module configures and finds a fix, and tasks
//! stay alive or stall.
//!
//! It performs no I/O. Feed it [`Event`]s; it returns [`Actions`] for the
//! firmware to carry out, and it exposes the LED colours and whether the
//! watchdog may be kicked.
//!
//! Policy:
//!
//! * Both SD cards are logged to when both are healthy (mirroring). Losing one
//!   does not interrupt logging on the other.
//! * Logging starts when the GNSS module is ready and has a fix, or after
//!   [`Config::fix_wait_ms`] without one (the samples are then flagged).
//! * With no usable card the system keeps the other subsystems running, shows
//!   a fault, and retries mounting.
//! * The watchdog is kicked only while every required task has reported
//!   within its deadline, so a stalled loop resets the MCU instead of hiding.

use tel_logfmt::records::event;

/// One of the two SD cards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Card {
    /// SD1 (SDMMC1).
    Sd1,
    /// SD2 (SPI2).
    Sd2,
}

impl Card {
    const fn index(self) -> usize {
        match self {
            Self::Sd1 => 0,
            Self::Sd2 => 1,
        }
    }

    /// The card number used in event arguments (1 or 2).
    pub const fn number(self) -> u32 {
        match self {
            Self::Sd1 => 1,
            Self::Sd2 => 2,
        }
    }
}

/// State of a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardState {
    /// No card detected.
    Absent,
    /// A card is present but has not been mounted successfully.
    Present,
    /// Mounted and recovered; logging may use it.
    Ready,
    /// A write or mount failed.
    Faulted,
}

/// Tasks that report liveness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Task {
    /// GNSS byte parser.
    Gnss,
    /// IMU reader.
    Imu,
    /// Record builder and log writer.
    Logger,
    /// The supervisor's own tick.
    Supervisor,
}

impl Task {
    const COUNT: usize = 4;
    const fn index(self) -> usize {
        match self {
            Self::Gnss => 0,
            Self::Imu => 1,
            Self::Logger => 2,
            Self::Supervisor => 3,
        }
    }
    const fn bit(self) -> u8 {
        1 << self.index()
    }
}

/// Overall phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Waiting for the first mount results.
    Booting,
    /// No usable card; retrying.
    NoCards,
    /// A card is usable; the GNSS module is being configured.
    GnssSetup,
    /// GNSS ready; waiting for a fix.
    WaitFix,
    /// Logging.
    Running,
}

/// What the GNSS side currently reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GnssHealth {
    /// The module is configured as required.
    pub configured: bool,
    /// A valid position fix.
    pub fix: bool,
    /// RTK fixed solution.
    pub rtk_fixed: bool,
    /// A valid dual-antenna heading.
    pub heading: bool,
}

/// An input to the supervisor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Periodic tick with the current time in milliseconds.
    Tick {
        /// Monotonic milliseconds.
        now_ms: u64,
    },
    /// A mount attempt finished.
    CardMounted {
        /// Which card.
        card: Card,
        /// Whether it succeeded (card present and log region recovered).
        ok: bool,
    },
    /// Card-detect says a card was inserted.
    CardInserted {
        /// Which card.
        card: Card,
    },
    /// Card-detect says a card was removed.
    CardRemoved {
        /// Which card.
        card: Card,
    },
    /// A data write to the card failed.
    WriteFailed {
        /// Which card.
        card: Card,
    },
    /// New GNSS status.
    Gnss(GnssHealth),
    /// GNSS configuration failed permanently (it will not become ready).
    GnssFailed,
    /// A task is alive.
    Heartbeat {
        /// Which task.
        task: Task,
        /// Monotonic milliseconds.
        now_ms: u64,
    },
    /// CAN bus activity changed.
    CanAlive {
        /// 1 or 2.
        bus: u8,
        /// Whether frames are being received.
        alive: bool,
    },
    /// A CAN controller went bus-off.
    CanBusOff {
        /// 1 or 2.
        bus: u8,
    },
    /// IMU health changed.
    ImuOk {
        /// Whether the IMU is delivering data.
        ok: bool,
    },
    /// A queue overran.
    QueueOverrun {
        /// Queue identifier.
        queue: u8,
    },
}

/// Something for the firmware to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Begin GNSS verification and configuration.
    StartGnssSetup,
    /// Try mounting this card again.
    RetryMount(Card),
    /// Open a new log session on this card.
    OpenSession(Card),
    /// Append an event record to the log.
    LogEvent {
        /// Event code (see `tel_logfmt::records::event`).
        code: u16,
        /// Event argument.
        arg: u32,
    },
}

/// Fixed-capacity list of actions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Actions {
    items: [Option<Action>; Self::CAP],
    len: usize,
    /// Actions that did not fit (should stay zero).
    pub dropped: u32,
}

impl Actions {
    /// Capacity.
    pub const CAP: usize = 8;

    fn push(&mut self, a: Action) {
        match self.items.get_mut(self.len) {
            Some(slot) => {
                *slot = Some(a);
                self.len = self.len.saturating_add(1);
            }
            None => self.dropped = self.dropped.saturating_add(1),
        }
    }

    /// Number of actions.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// True if there are none.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Iterate over the actions.
    pub fn iter(&self) -> impl Iterator<Item = Action> + '_ {
        self.items.iter().take(self.len).flatten().copied()
    }

    /// True if `a` is among the actions.
    pub fn contains(&self, a: Action) -> bool {
        self.iter().any(|x| x == a)
    }
}

/// Tunable timing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Give up waiting for a GNSS fix after this long and start logging anyway.
    pub fix_wait_ms: u64,
    /// Retry mounting an absent or present card this often while no card works.
    pub mount_retry_ms: u64,
    /// Retry a faulted card this often.
    pub fault_retry_ms: u64,
    /// A required task must report within this long.
    pub heartbeat_deadline_ms: u64,
    /// Tasks may stay silent this long after start before the deadline applies.
    pub boot_grace_ms: u64,
    /// Bit mask of tasks the watchdog depends on (`Task` bits).
    pub required_tasks: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            fix_wait_ms: 120_000,
            mount_retry_ms: 1_000,
            fault_retry_ms: 10_000,
            heartbeat_deadline_ms: 500,
            boot_grace_ms: 5_000,
            required_tasks: Task::Gnss.bit() | Task::Imu.bit() | Task::Logger.bit() | Task::Supervisor.bit(),
        }
    }
}

/// LED colour (the firmware maps this to the LED driver).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedColor {
    /// Dark.
    Off,
    /// Red.
    Red,
    /// Green.
    Green,
    /// Amber (both dies).
    Amber,
}

/// Number of status LEDs: system, GNSS, SD1, SD2, sensors.
pub const LED_COUNT: usize = 5;

/// The supervisor.
#[derive(Clone, Copy, Debug)]
pub struct Supervisor {
    cfg: Config,
    phase: Phase,
    cards: [CardState; 2],
    mount_reported: [bool; 2],
    session_open: [bool; 2],
    gnss: GnssHealth,
    gnss_failed: bool,
    wait_fix_since_ms: u64,
    no_fix_start: bool,
    last_retry_ms: [u64; 2],
    can_alive: [bool; 2],
    imu_ok: bool,
    beats: [Option<u64>; Task::COUNT],
    started_ms: Option<u64>,
    now_ms: u64,
    overruns: u32,
}

impl Supervisor {
    /// A new supervisor in [`Phase::Booting`].
    pub const fn new(cfg: Config) -> Self {
        Self {
            cfg,
            phase: Phase::Booting,
            cards: [CardState::Absent; 2],
            mount_reported: [false; 2],
            session_open: [false; 2],
            gnss: GnssHealth { configured: false, fix: false, rtk_fixed: false, heading: false },
            gnss_failed: false,
            wait_fix_since_ms: 0,
            no_fix_start: false,
            last_retry_ms: [0; 2],
            can_alive: [false; 2],
            imu_ok: false,
            beats: [None; Task::COUNT],
            started_ms: None,
            now_ms: 0,
            overruns: 0,
        }
    }

    /// Current phase.
    pub const fn phase(&self) -> Phase {
        self.phase
    }

    /// State of a card.
    pub fn card_state(&self, card: Card) -> CardState {
        self.cards.get(card.index()).copied().unwrap_or(CardState::Absent)
    }

    /// Cards that have an open session (the ones to write records to).
    pub fn active_cards(&self) -> impl Iterator<Item = Card> + '_ {
        [Card::Sd1, Card::Sd2].into_iter().filter(|c| self.session_open.get(c.index()).copied().unwrap_or(false))
    }

    /// True when logging started without a fix.
    pub const fn no_fix_start(&self) -> bool {
        self.no_fix_start
    }

    /// Queue overruns reported.
    pub const fn overruns(&self) -> u32 {
        self.overruns
    }

    fn ready_count(&self) -> usize {
        self.cards.iter().filter(|c| **c == CardState::Ready).count()
    }

    fn set_card(&mut self, card: Card, s: CardState) {
        if let Some(c) = self.cards.get_mut(card.index()) {
            *c = s;
        }
    }

    fn mark_reported(&mut self, card: Card) {
        if let Some(r) = self.mount_reported.get_mut(card.index()) {
            *r = true;
        }
    }

    /// Remember when a mount was last attempted or a fault happened, so retries
    /// are spaced from that moment.
    fn note_attempt(&mut self, card: Card, now_ms: u64) {
        if let Some(l) = self.last_retry_ms.get_mut(card.index()) {
            *l = now_ms;
        }
    }

    fn close_session(&mut self, card: Card) {
        if let Some(s) = self.session_open.get_mut(card.index()) {
            *s = false;
        }
    }

    /// Open sessions on every Ready card that does not have one yet (only while Running).
    fn open_sessions(&mut self, out: &mut Actions) {
        if self.phase != Phase::Running {
            return;
        }
        for card in [Card::Sd1, Card::Sd2] {
            let i = card.index();
            if self.cards.get(i) == Some(&CardState::Ready) && self.session_open.get(i) == Some(&false) {
                if let Some(s) = self.session_open.get_mut(i) {
                    *s = true;
                }
                out.push(Action::OpenSession(card));
            }
        }
    }

    /// Re-evaluate the phase after any change.
    fn advance(&mut self, now_ms: u64, out: &mut Actions) {
        loop {
            let before = self.phase;
            match self.phase {
                Phase::Booting => {
                    let all_reported = self.mount_reported.iter().all(|r| *r);
                    if all_reported {
                        self.phase = if self.ready_count() > 0 { Phase::GnssSetup } else { Phase::NoCards };
                        if self.phase == Phase::GnssSetup {
                            out.push(Action::StartGnssSetup);
                        }
                    }
                }
                Phase::NoCards => {
                    if self.ready_count() > 0 {
                        // Cards returned: resume at the right phase.
                        self.phase = if self.gnss.configured { Phase::Running } else { Phase::GnssSetup };
                        if self.phase == Phase::GnssSetup {
                            out.push(Action::StartGnssSetup);
                        } else {
                            self.wait_fix_since_ms = now_ms;
                        }
                    }
                }
                Phase::GnssSetup => {
                    if self.ready_count() == 0 {
                        self.phase = Phase::NoCards;
                    } else if self.gnss.configured {
                        self.phase = Phase::WaitFix;
                        self.wait_fix_since_ms = now_ms;
                    } else if self.gnss_failed {
                        // The module could not be configured: log anyway rather than nothing.
                        self.no_fix_start = true;
                        self.phase = Phase::Running;
                    }
                }
                Phase::WaitFix => {
                    if self.ready_count() == 0 {
                        self.phase = Phase::NoCards;
                    } else if self.gnss.fix {
                        out.push(Action::LogEvent { code: event::FIX_ACQUIRED, arg: 0 });
                        self.phase = Phase::Running;
                    } else if now_ms.saturating_sub(self.wait_fix_since_ms) >= self.cfg.fix_wait_ms {
                        self.no_fix_start = true;
                        self.phase = Phase::Running;
                    }
                }
                Phase::Running => {
                    if self.ready_count() == 0 {
                        self.phase = Phase::NoCards;
                    }
                }
            }
            if self.phase == before {
                break;
            }
        }
        self.open_sessions(out);
    }

    /// Process one event.
    pub fn step(&mut self, ev: Event) -> Actions {
        let mut out = Actions::default();
        // Time only moves forward, and every timed event advances it.
        match ev {
            Event::Tick { now_ms } | Event::Heartbeat { now_ms, .. } => {
                self.now_ms = self.now_ms.max(now_ms);
                self.started_ms.get_or_insert(now_ms);
            }
            _ => {}
        }
        let now_ms = self.now_ms;
        match ev {
            Event::Tick { .. } => {
                // Retry mounting. A card that is present but not mounted is retried in
                // any phase after start-up; a faulted card less often; an empty slot only
                // while no card works at all (in case card-detect is unreliable).
                if self.phase != Phase::Booting {
                    for card in [Card::Sd1, Card::Sd2] {
                        let i = card.index();
                        let st = self.cards.get(i).copied().unwrap_or(CardState::Absent);
                        let interval = match st {
                            CardState::Ready => continue,
                            CardState::Faulted => self.cfg.fault_retry_ms,
                            CardState::Present => self.cfg.mount_retry_ms,
                            CardState::Absent if self.phase == Phase::NoCards => self.cfg.mount_retry_ms,
                            CardState::Absent => continue,
                        };
                        let last = self.last_retry_ms.get(i).copied().unwrap_or(0);
                        if now_ms.saturating_sub(last) >= interval {
                            if let Some(l) = self.last_retry_ms.get_mut(i) {
                                *l = now_ms;
                            }
                            out.push(Action::RetryMount(card));
                        }
                    }
                }
            }
            Event::CardMounted { card, ok } => {
                self.mark_reported(card);
                if ok {
                    self.set_card(card, CardState::Ready);
                } else if self.card_state(card) != CardState::Ready {
                    // A failed mount report for a card that is already ready and logging is
                    // ignored: real write errors arrive as `WriteFailed`.
                    self.note_attempt(card, now_ms);
                    if self.card_state(card) != CardState::Faulted {
                        // A card that is there but cannot be mounted stays Present (retried);
                        // an empty slot stays Absent.
                        let s = if self.card_state(card) == CardState::Absent {
                            CardState::Absent
                        } else {
                            CardState::Present
                        };
                        self.set_card(card, s);
                    }
                }
            }
            Event::CardInserted { card } => {
                if self.card_state(card) != CardState::Ready {
                    if self.card_state(card) == CardState::Absent {
                        self.set_card(card, CardState::Present);
                    }
                    // The firmware should attempt a mount now.
                    self.note_attempt(card, now_ms);
                    out.push(Action::RetryMount(card));
                }
            }
            Event::CardRemoved { card } => {
                self.mark_reported(card);
                let was_active = self.session_open.get(card.index()).copied().unwrap_or(false);
                self.set_card(card, CardState::Absent);
                self.close_session(card);
                if was_active {
                    out.push(Action::LogEvent { code: event::CARD_FAULT, arg: card.number() });
                    self.note_switch(&mut out);
                }
            }
            Event::WriteFailed { card } => {
                self.mark_reported(card);
                self.note_attempt(card, now_ms);
                self.set_card(card, CardState::Faulted);
                self.close_session(card);
                out.push(Action::LogEvent { code: event::CARD_FAULT, arg: card.number() });
                self.note_switch(&mut out);
            }
            Event::Gnss(h) => {
                let had_fix = self.gnss.fix;
                self.gnss = h;
                if self.phase == Phase::Running && had_fix != h.fix {
                    let code = if h.fix { event::FIX_ACQUIRED } else { event::FIX_LOST };
                    out.push(Action::LogEvent { code, arg: 0 });
                }
            }
            Event::GnssFailed => self.gnss_failed = true,
            Event::Heartbeat { task, now_ms: beat_ms } => {
                if let Some(b) = self.beats.get_mut(task.index()) {
                    *b = Some(beat_ms);
                }
            }
            Event::CanAlive { bus, alive } => {
                if let Some(c) = self.can_alive.get_mut(usize::from(bus.saturating_sub(1))) {
                    *c = alive;
                }
            }
            Event::CanBusOff { bus } => {
                if let Some(c) = self.can_alive.get_mut(usize::from(bus.saturating_sub(1))) {
                    *c = false;
                }
                out.push(Action::LogEvent { code: event::CAN_BUS_OFF, arg: u32::from(bus) });
            }
            Event::ImuOk { ok } => {
                if self.imu_ok && !ok {
                    out.push(Action::LogEvent { code: event::IMU_FAULT, arg: 0 });
                }
                self.imu_ok = ok;
            }
            Event::QueueOverrun { queue } => {
                self.overruns = self.overruns.saturating_add(1);
                out.push(Action::LogEvent { code: event::QUEUE_OVERRUN, arg: u32::from(queue) });
            }
        }
        self.advance(now_ms, &mut out);
        out
    }

    fn note_switch(&mut self, out: &mut Actions) {
        // If another card is still being logged to, say so.
        let other = [Card::Sd1, Card::Sd2].into_iter().find(|c| self.session_open.get(c.index()) == Some(&true));
        if let Some(c) = other {
            out.push(Action::LogEvent { code: event::CARD_SWITCH, arg: c.number() });
        }
    }

    /// May the watchdog be kicked at `now_ms`? True only while every required
    /// task has reported within its deadline (after the start-up grace period).
    pub fn should_kick_watchdog(&self, now_ms: u64) -> bool {
        let started = self.started_ms.unwrap_or(now_ms);
        let in_grace = now_ms.saturating_sub(started) < self.cfg.boot_grace_ms;
        for task in [Task::Gnss, Task::Imu, Task::Logger, Task::Supervisor] {
            if self.cfg.required_tasks & task.bit() == 0 {
                continue;
            }
            let ok = match self.beats.get(task.index()).copied().flatten() {
                Some(last) => now_ms.saturating_sub(last) <= self.cfg.heartbeat_deadline_ms,
                None => in_grace,
            };
            if !ok {
                return false;
            }
        }
        true
    }

    fn blink(now_ms: u64, period_ms: u64) -> bool {
        period_ms != 0 && now_ms.checked_rem(period_ms.saturating_mul(2)).is_some_and(|m| m < period_ms)
    }

    /// The colour of each status LED at `now_ms`.
    /// Order: system, GNSS, SD1, SD2, sensors.
    pub fn leds(&self, now_ms: u64) -> [LedColor; LED_COUNT] {
        let fast = Self::blink(now_ms, 250);
        let slow = Self::blink(now_ms, 500);
        let system = match self.phase {
            Phase::Booting => LedColor::Red,
            Phase::NoCards => {
                if slow {
                    LedColor::Red
                } else {
                    LedColor::Off
                }
            }
            Phase::GnssSetup => {
                if slow {
                    LedColor::Amber
                } else {
                    LedColor::Off
                }
            }
            Phase::WaitFix => LedColor::Amber,
            Phase::Running => {
                if self.no_fix_start && !self.gnss.fix {
                    LedColor::Amber
                } else {
                    LedColor::Green
                }
            }
        };
        let gnss = if !self.gnss.configured {
            LedColor::Red
        } else if self.gnss.rtk_fixed && self.gnss.heading {
            LedColor::Green
        } else if self.gnss.fix || slow {
            // steady amber with a fix, blinking amber while searching
            LedColor::Amber
        } else {
            LedColor::Off
        };
        let card = |c: Card| match self.card_state(c) {
            CardState::Absent => LedColor::Off,
            CardState::Present => {
                if fast {
                    LedColor::Amber
                } else {
                    LedColor::Off
                }
            }
            CardState::Ready => LedColor::Green,
            CardState::Faulted => LedColor::Red,
        };
        let sensors = if self.imu_ok && self.can_alive.iter().all(|a| *a) {
            LedColor::Green
        } else if self.imu_ok {
            LedColor::Amber
        } else {
            LedColor::Red
        };
        [system, gnss, card(Card::Sd1), card(Card::Sd2), sensors]
    }
}
