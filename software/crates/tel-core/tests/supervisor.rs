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
//! Supervisor scenarios and invariants.

use proptest::prelude::*;
use std::collections::HashSet;
use tel_core::supervisor::{
    Action, Actions, Card, CardState, Config, Event, GnssHealth, LedColor, Phase, Supervisor, Task,
};
use tel_logfmt::records::event as ev;

fn tick(s: &mut Supervisor, ms: u64) -> Actions {
    s.step(Event::Tick { now_ms: ms })
}

fn boot(s: &mut Supervisor) {
    tick(s, 0);
    s.step(Event::CardMounted { card: Card::Sd1, ok: true });
    s.step(Event::CardMounted { card: Card::Sd2, ok: true });
}

const CONFIGURED: GnssHealth = GnssHealth { configured: true, fix: false, rtk_fixed: false, heading: false };
const FIX: GnssHealth = GnssHealth { configured: true, fix: true, rtk_fixed: false, heading: false };

fn running(s: &mut Supervisor) {
    boot(s);
    s.step(Event::Gnss(CONFIGURED));
    s.step(Event::Gnss(FIX));
    assert_eq!(s.phase(), Phase::Running);
}

fn active(s: &Supervisor) -> Vec<Card> {
    s.active_cards().collect()
}

#[test]
fn boot_waits_for_both_mount_results_then_starts_gnss_setup() {
    let mut s = Supervisor::new(Config::default());
    tick(&mut s, 0);
    assert_eq!(s.phase(), Phase::Booting);
    let a = s.step(Event::CardMounted { card: Card::Sd1, ok: true });
    assert!(a.is_empty());
    assert_eq!(s.phase(), Phase::Booting, "still waiting for the other card's result");
    let a = s.step(Event::CardMounted { card: Card::Sd2, ok: false });
    assert_eq!(s.phase(), Phase::GnssSetup);
    assert!(a.contains(Action::StartGnssSetup));
    assert_eq!(a.iter().filter(|x| *x == Action::StartGnssSetup).count(), 1);
    assert_eq!(s.card_state(Card::Sd1), CardState::Ready);
    assert_eq!(s.card_state(Card::Sd2), CardState::Absent);
}

#[test]
fn normal_start_waits_for_a_fix_then_logs_on_both_cards() {
    let mut s = Supervisor::new(Config::default());
    boot(&mut s);
    let a = s.step(Event::Gnss(CONFIGURED));
    assert_eq!(s.phase(), Phase::WaitFix);
    assert!(a.is_empty());
    assert!(active(&s).is_empty(), "no logging before a fix");
    let a = s.step(Event::Gnss(FIX));
    assert_eq!(s.phase(), Phase::Running);
    assert!(a.contains(Action::LogEvent { code: ev::FIX_ACQUIRED, arg: 0 }));
    assert!(a.contains(Action::OpenSession(Card::Sd1)));
    assert!(a.contains(Action::OpenSession(Card::Sd2)));
    assert_eq!(active(&s), vec![Card::Sd1, Card::Sd2]);
    assert!(!s.no_fix_start());
    assert_eq!(a.dropped, 0);
}

#[test]
fn starts_without_a_fix_after_the_wait_and_says_so() {
    let mut s = Supervisor::new(Config::default());
    boot(&mut s);
    s.step(Event::Gnss(CONFIGURED));
    assert!(!tick(&mut s, 119_999).contains(Action::OpenSession(Card::Sd1)));
    assert_eq!(s.phase(), Phase::WaitFix);
    let a = tick(&mut s, 120_000);
    assert_eq!(s.phase(), Phase::Running);
    assert!(s.no_fix_start());
    assert!(a.contains(Action::OpenSession(Card::Sd1)));
    assert!(!a.contains(Action::LogEvent { code: ev::FIX_ACQUIRED, arg: 0 }));
}

#[test]
fn gnss_configuration_failure_still_logs() {
    let mut s = Supervisor::new(Config::default());
    boot(&mut s);
    let a = s.step(Event::GnssFailed);
    assert_eq!(s.phase(), Phase::Running);
    assert!(s.no_fix_start());
    assert!(a.contains(Action::OpenSession(Card::Sd2)));
}

#[test]
fn fix_transitions_are_logged_while_running() {
    let mut s = Supervisor::new(Config::default());
    running(&mut s);
    let a = s.step(Event::Gnss(CONFIGURED)); // fix lost
    assert!(a.contains(Action::LogEvent { code: ev::FIX_LOST, arg: 0 }));
    let a = s.step(Event::Gnss(CONFIGURED));
    assert!(a.is_empty(), "no repeat while nothing changes");
    let a = s.step(Event::Gnss(FIX));
    assert!(a.contains(Action::LogEvent { code: ev::FIX_ACQUIRED, arg: 0 }));
}

#[test]
fn no_cards_at_boot_retries_and_recovers() {
    let mut s = Supervisor::new(Config::default());
    tick(&mut s, 0);
    s.step(Event::CardMounted { card: Card::Sd1, ok: false });
    s.step(Event::CardMounted { card: Card::Sd2, ok: false });
    assert_eq!(s.phase(), Phase::NoCards);
    // retries every second for both (empty) slots
    assert!(tick(&mut s, 500).is_empty());
    let a = tick(&mut s, 1000);
    assert!(a.contains(Action::RetryMount(Card::Sd1)) && a.contains(Action::RetryMount(Card::Sd2)));
    assert!(tick(&mut s, 1500).is_empty());
    assert!(tick(&mut s, 2000).contains(Action::RetryMount(Card::Sd1)));
    // a card appears
    let a = s.step(Event::CardInserted { card: Card::Sd2 });
    assert!(a.contains(Action::RetryMount(Card::Sd2)));
    assert_eq!(s.card_state(Card::Sd2), CardState::Present);
    let a = s.step(Event::CardMounted { card: Card::Sd2, ok: true });
    assert_eq!(s.phase(), Phase::GnssSetup);
    assert!(a.contains(Action::StartGnssSetup));
}

#[test]
fn losing_one_card_keeps_logging_on_the_other() {
    let mut s = Supervisor::new(Config::default());
    running(&mut s);
    let a = s.step(Event::WriteFailed { card: Card::Sd1 });
    assert!(a.contains(Action::LogEvent { code: ev::CARD_FAULT, arg: 1 }));
    assert!(a.contains(Action::LogEvent { code: ev::CARD_SWITCH, arg: 2 }));
    assert_eq!(s.phase(), Phase::Running);
    assert_eq!(active(&s), vec![Card::Sd2]);
    assert_eq!(s.card_state(Card::Sd1), CardState::Faulted);
    assert_eq!(s.leds(0)[2], LedColor::Red);
    assert_eq!(s.leds(0)[3], LedColor::Green);
}

#[test]
fn losing_both_cards_stops_logging_and_recovery_resumes_it() {
    let mut s = Supervisor::new(Config::default());
    running(&mut s);
    tick(&mut s, 50_000);
    s.step(Event::WriteFailed { card: Card::Sd1 });
    let a = s.step(Event::WriteFailed { card: Card::Sd2 });
    assert_eq!(s.phase(), Phase::NoCards);
    assert!(active(&s).is_empty());
    assert!(!a.contains(Action::LogEvent { code: ev::CARD_SWITCH, arg: 1 }), "nothing to switch to");
    // faulted cards are retried every 10 s
    assert!(tick(&mut s, 55_000).is_empty());
    let a = tick(&mut s, 60_000);
    assert!(a.contains(Action::RetryMount(Card::Sd1)) && a.contains(Action::RetryMount(Card::Sd2)));
    // one comes back
    let a = s.step(Event::CardMounted { card: Card::Sd1, ok: true });
    assert_eq!(s.phase(), Phase::Running, "GNSS was already configured, so logging resumes");
    assert!(a.contains(Action::OpenSession(Card::Sd1)));
    assert_eq!(active(&s), vec![Card::Sd1]);
}

#[test]
fn removing_an_active_card_is_a_fault_removing_an_idle_one_is_not() {
    let mut s = Supervisor::new(Config::default());
    boot(&mut s);
    s.step(Event::Gnss(CONFIGURED)); // WaitFix: sessions not open yet
    let a = s.step(Event::CardRemoved { card: Card::Sd1 });
    assert!(a.is_empty(), "no session was open on that card");
    s.step(Event::Gnss(FIX));
    assert_eq!(active(&s), vec![Card::Sd2]);
    let a = s.step(Event::CardRemoved { card: Card::Sd2 });
    assert!(a.contains(Action::LogEvent { code: ev::CARD_FAULT, arg: 2 }));
    assert_eq!(s.phase(), Phase::NoCards);
}

#[test]
fn a_card_inserted_while_running_is_mounted_and_joins_the_log() {
    let mut s = Supervisor::new(Config::default());
    tick(&mut s, 0);
    s.step(Event::CardMounted { card: Card::Sd1, ok: true });
    s.step(Event::CardMounted { card: Card::Sd2, ok: false });
    s.step(Event::Gnss(CONFIGURED));
    s.step(Event::Gnss(FIX));
    assert_eq!(active(&s), vec![Card::Sd1]);
    let a = s.step(Event::CardInserted { card: Card::Sd2 });
    assert!(a.contains(Action::RetryMount(Card::Sd2)));
    // a failed first mount is retried every second even though SD1 works
    s.step(Event::CardMounted { card: Card::Sd2, ok: false });
    assert!(tick(&mut s, 5_000).contains(Action::RetryMount(Card::Sd2)));
    let a = s.step(Event::CardMounted { card: Card::Sd2, ok: true });
    assert!(a.contains(Action::OpenSession(Card::Sd2)));
    assert_eq!(active(&s), vec![Card::Sd1, Card::Sd2]);
}

#[test]
fn an_open_session_is_never_opened_twice() {
    let mut s = Supervisor::new(Config::default());
    running(&mut s);
    let a = s.step(Event::CardMounted { card: Card::Sd1, ok: true }); // duplicate report
    assert!(!a.contains(Action::OpenSession(Card::Sd1)));
    let a = tick(&mut s, 10_000);
    assert!(!a.iter().any(|x| matches!(x, Action::OpenSession(_))));
}

#[test]
fn time_never_moves_backwards() {
    let mut s = Supervisor::new(Config::default());
    boot(&mut s);
    tick(&mut s, 10_000);
    s.step(Event::Gnss(CONFIGURED));
    // an out-of-order tick must not make the fix wait restart or underflow
    tick(&mut s, 5_000);
    assert!(tick(&mut s, 10_000 + 120_000).contains(Action::OpenSession(Card::Sd1)));
}

#[test]
fn diagnostic_events_are_logged_once() {
    let mut s = Supervisor::new(Config::default());
    let a = s.step(Event::QueueOverrun { queue: 3 });
    assert!(a.contains(Action::LogEvent { code: ev::QUEUE_OVERRUN, arg: 3 }));
    assert_eq!(s.overruns(), 1);
    let a = s.step(Event::CanBusOff { bus: 2 });
    assert!(a.contains(Action::LogEvent { code: ev::CAN_BUS_OFF, arg: 2 }));
    s.step(Event::ImuOk { ok: true });
    assert!(s.step(Event::ImuOk { ok: true }).is_empty());
    let a = s.step(Event::ImuOk { ok: false });
    assert!(a.contains(Action::LogEvent { code: ev::IMU_FAULT, arg: 0 }));
    assert!(s.step(Event::ImuOk { ok: false }).is_empty());
    // out-of-range bus numbers are ignored without panicking
    s.step(Event::CanAlive { bus: 0, alive: true });
    s.step(Event::CanAlive { bus: 9, alive: true });
    s.step(Event::CanBusOff { bus: 77 });
}

// ---------------------------------------------------------------- watchdog

fn beat_all(s: &mut Supervisor, ms: u64) {
    for task in [Task::Gnss, Task::Imu, Task::Logger, Task::Supervisor] {
        s.step(Event::Heartbeat { task, now_ms: ms });
    }
}

#[test]
fn watchdog_is_kicked_while_everything_beats() {
    let mut s = Supervisor::new(Config::default());
    for ms in (0..20_000).step_by(100) {
        beat_all(&mut s, ms);
        assert!(s.should_kick_watchdog(ms + 50), "t = {ms}");
    }
}

#[test]
fn watchdog_has_a_start_up_grace_then_requires_every_task() {
    let mut s = Supervisor::new(Config::default());
    // Only the supervisor's own tick runs; the other tasks have not started yet.
    for ms in (0..5_000).step_by(100) {
        s.step(Event::Heartbeat { task: Task::Supervisor, now_ms: ms });
        assert!(s.should_kick_watchdog(ms + 50), "tasks may still be starting at {ms}");
    }
    s.step(Event::Heartbeat { task: Task::Supervisor, now_ms: 5_000 });
    assert!(!s.should_kick_watchdog(5_050), "a task that never started is a failure after the grace period");
    // and a supervisor that itself stops is caught inside the grace period too
    let mut s2 = Supervisor::new(Config::default());
    s2.step(Event::Heartbeat { task: Task::Supervisor, now_ms: 0 });
    assert!(!s2.should_kick_watchdog(1_000), "the supervisor's own heartbeat is always required");
}

#[test]
fn one_stalled_task_stops_the_kicks() {
    let mut s = Supervisor::new(Config::default());
    beat_all(&mut s, 10_000);
    assert!(s.should_kick_watchdog(10_400));
    // everything but the logger keeps beating
    for task in [Task::Gnss, Task::Imu, Task::Supervisor] {
        s.step(Event::Heartbeat { task, now_ms: 10_600 });
    }
    assert!(!s.should_kick_watchdog(10_700), "logger silent for 700 ms > 500 ms deadline");
    s.step(Event::Heartbeat { task: Task::Logger, now_ms: 10_700 });
    assert!(s.should_kick_watchdog(10_800));
}

#[test]
fn tasks_outside_the_required_mask_are_ignored() {
    let cfg = Config { required_tasks: 0b0100, ..Config::default() }; // only the logger
    let mut s = Supervisor::new(cfg);
    s.step(Event::Heartbeat { task: Task::Logger, now_ms: 10_000 });
    assert!(s.should_kick_watchdog(10_200));
    assert!(s.should_kick_watchdog(10_500));
    assert!(!s.should_kick_watchdog(10_501));
}

#[test]
fn a_clock_behind_the_last_beat_does_not_underflow() {
    let mut s = Supervisor::new(Config::default());
    beat_all(&mut s, 9_000);
    assert!(s.should_kick_watchdog(8_000));
}

// ---------------------------------------------------------------- LEDs

#[test]
fn led_colours_follow_the_state() {
    let mut s = Supervisor::new(Config::default());
    // booting: system red, others dark/red
    let l = s.leds(0);
    assert_eq!(l[0], LedColor::Red);
    assert_eq!(l[1], LedColor::Red, "GNSS not configured");
    assert_eq!(l[2], LedColor::Off);
    running(&mut s);
    s.step(Event::ImuOk { ok: true });
    s.step(Event::CanAlive { bus: 1, alive: true });
    let l = s.leds(0);
    assert_eq!(l[0], LedColor::Green);
    assert_eq!(l[1], LedColor::Amber, "fix but not RTK");
    assert_eq!(l[2], LedColor::Green);
    assert_eq!(l[3], LedColor::Green);
    assert_eq!(l[4], LedColor::Amber, "only one CAN bus alive");
    s.step(Event::CanAlive { bus: 2, alive: true });
    assert_eq!(s.leds(0)[4], LedColor::Green);
    s.step(Event::Gnss(GnssHealth { configured: true, fix: true, rtk_fixed: true, heading: true }));
    assert_eq!(s.leds(0)[1], LedColor::Green);
    s.step(Event::Gnss(GnssHealth { configured: true, fix: true, rtk_fixed: true, heading: false }));
    assert_eq!(s.leds(0)[1], LedColor::Amber, "RTK without heading is not all good");
    s.step(Event::ImuOk { ok: false });
    assert_eq!(s.leds(0)[4], LedColor::Red);
}

#[test]
fn blinking_leds_alternate_with_the_right_period() {
    let mut s = Supervisor::new(Config::default());
    tick(&mut s, 0);
    s.step(Event::CardMounted { card: Card::Sd1, ok: false });
    s.step(Event::CardMounted { card: Card::Sd2, ok: false });
    assert_eq!(s.phase(), Phase::NoCards);
    assert_eq!(s.leds(0)[0], LedColor::Red);
    assert_eq!(s.leds(499)[0], LedColor::Red);
    assert_eq!(s.leds(500)[0], LedColor::Off);
    assert_eq!(s.leds(999)[0], LedColor::Off);
    assert_eq!(s.leds(1000)[0], LedColor::Red);
    // a present-but-unmounted card blinks amber faster
    s.step(Event::CardInserted { card: Card::Sd1 });
    assert_eq!(s.leds(0)[2], LedColor::Amber);
    assert_eq!(s.leds(250)[2], LedColor::Off);
}

#[test]
fn running_without_a_fix_start_shows_amber_until_a_fix() {
    let mut s = Supervisor::new(Config::default());
    boot(&mut s);
    s.step(Event::Gnss(CONFIGURED));
    tick(&mut s, 120_000);
    assert_eq!(s.phase(), Phase::Running);
    assert_eq!(s.leds(0)[0], LedColor::Amber);
    s.step(Event::Gnss(FIX));
    assert_eq!(s.leds(0)[0], LedColor::Green);
}

// ---------------------------------------------------------------- invariants

#[derive(Clone, Debug)]
enum Ev {
    Tick(u64),
    Mounted(bool, bool),
    Inserted(bool),
    Removed(bool),
    WriteFailed(bool),
    Gnss(bool, bool),
    GnssFailed,
    Overrun,
    Can(u8, bool),
    Imu(bool),
    Beat(u8),
}

fn card(b: bool) -> Card {
    if b {
        Card::Sd2
    } else {
        Card::Sd1
    }
}

fn arb_events() -> impl Strategy<Value = Vec<Ev>> {
    prop::collection::vec(
        prop_oneof![
            4 => (0_u64..30_000).prop_map(Ev::Tick),
            3 => (any::<bool>(), any::<bool>()).prop_map(|(c, ok)| Ev::Mounted(c, ok)),
            2 => any::<bool>().prop_map(Ev::Inserted),
            2 => any::<bool>().prop_map(Ev::Removed),
            2 => any::<bool>().prop_map(Ev::WriteFailed),
            3 => (any::<bool>(), any::<bool>()).prop_map(|(c, f)| Ev::Gnss(c, f)),
            1 => Just(Ev::GnssFailed),
            1 => Just(Ev::Overrun),
            1 => (0_u8..4, any::<bool>()).prop_map(|(b, a)| Ev::Can(b, a)),
            1 => any::<bool>().prop_map(Ev::Imu),
            1 => (0_u8..4).prop_map(Ev::Beat),
        ],
        0..300,
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    /// Over arbitrary event sequences the supervisor never panics, never opens
    /// a session on a card that is not ready, never opens one twice, only runs
    /// with a usable card, and never loses an action.
    #[test]
    fn safety_invariants_hold(events in arb_events()) {
        let mut s = Supervisor::new(Config::default());
        let mut now = 0_u64;
        let mut open: HashSet<u32> = HashSet::new();
        for e in events {
            let event = match e {
                Ev::Tick(dt) => { now += dt; Event::Tick { now_ms: now } }
                Ev::Mounted(c, ok) => Event::CardMounted { card: card(c), ok },
                Ev::Inserted(c) => Event::CardInserted { card: card(c) },
                Ev::Removed(c) => Event::CardRemoved { card: card(c) },
                Ev::WriteFailed(c) => Event::WriteFailed { card: card(c) },
                Ev::Gnss(cfg, fix) => Event::Gnss(GnssHealth { configured: cfg, fix: cfg && fix, rtk_fixed: false, heading: false }),
                Ev::GnssFailed => Event::GnssFailed,
                Ev::Overrun => Event::QueueOverrun { queue: 1 },
                Ev::Can(b, a) => Event::CanAlive { bus: b, alive: a },
                Ev::Imu(ok) => Event::ImuOk { ok },
                Ev::Beat(t) => Event::Heartbeat { task: [Task::Gnss, Task::Imu, Task::Logger, Task::Supervisor][usize::from(t)], now_ms: now },
            };
            // a session closes when its card is removed or faults
            match event {
                Event::CardRemoved { card } | Event::WriteFailed { card } => { open.remove(&card.number()); }
                _ => {}
            }
            let actions = s.step(event);
            prop_assert_eq!(actions.dropped, 0);
            for a in actions.iter() {
                if let Action::OpenSession(c) = a {
                    prop_assert_eq!(s.card_state(c), CardState::Ready, "session opened on a card that is not ready");
                    prop_assert_eq!(s.phase(), Phase::Running);
                    prop_assert!(open.insert(c.number()), "session opened twice on card {}", c.number());
                }
            }
            // every active card is Ready and tracked as open
            for c in s.active_cards() {
                prop_assert_eq!(s.card_state(c), CardState::Ready);
                prop_assert!(open.contains(&c.number()));
            }
            if s.phase() == Phase::Running || s.phase() == Phase::GnssSetup || s.phase() == Phase::WaitFix {
                let ready = [Card::Sd1, Card::Sd2].iter().filter(|c| s.card_state(**c) == CardState::Ready).count();
                prop_assert!(ready > 0, "phase {:?} without a usable card", s.phase());
            }
            let _ = s.leds(now);
            let _ = s.should_kick_watchdog(now);
        }
    }
}
