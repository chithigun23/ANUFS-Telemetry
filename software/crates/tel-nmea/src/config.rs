//! Sans-IO sequencer that verifies, and if necessary applies, the LC29H(EA)
//! settings this project depends on: fix interval, UART baud and `$PQTMTAR`
//! output.
//!
//! The caller owns all I/O and timing. Loop on [`Configurator::poll`]; send the
//! command it returns, then feed every decoded [`Message`] to
//! [`Configurator::on_message`] and call [`Configurator::on_timeout`] when the
//! module does not answer in time.
//!
//! The apply order follows Quectel's protocol specification V1.5 (power the
//! GNSS system down, change the fix interval, save, power it up again) with the
//! message-rate change issued while the module is running. This has not yet
//! been exercised against real hardware.

use crate::cmd::{Command, EncodeError};
use crate::msg::{Message, PqtmCommand, PqtmResult};

/// Attempts per command before the whole sequence fails.
pub const MAX_ATTEMPTS: u8 = 3;

const TAR_NAME: &str = "PQTMTAR";
const TAR_VERSION: u8 = 1;
const MAX_PLAN: usize = 8;

/// Settings the project needs the module to have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    /// Position fix interval in milliseconds (100 = 10 Hz).
    pub fix_interval_ms: u16,
    /// UART1 baud rate.
    pub baud: u32,
    /// `$PQTMTAR` output rate: 1 = every fix, 0 = off.
    pub tar_rate: u8,
}

/// How a successful run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The module already matched the target; nothing was changed.
    AlreadyConfigured,
    /// Settings were written. Reset the module, reopen the UART at `new_baud`
    /// and run the configurator again to verify.
    Applied {
        /// Baud rate the UART must use after the module restarts.
        new_baud: u32,
    },
}

/// Why a run failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// The target contains a value the module cannot accept.
    InvalidTarget,
    /// A command was not acknowledged after [`MAX_ATTEMPTS`] attempts.
    CommandFailed {
        /// Zero-based step number: 0 and 1 are the queries, 2 onward the plan.
        step: u8,
    },
}

/// What the caller should do next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Send this command and then wait for responses.
    Send(Command<'static>),
    /// A command is outstanding; keep feeding messages or report a timeout.
    Wait,
    /// Finished successfully.
    Done(Outcome),
    /// Finished with an error.
    Failed(ConfigError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    QueryFixRate,
    QueryTarRate,
    Apply,
    Finished(Result<Outcome, ConfigError>),
}

enum Verdict {
    Ignore,
    Pending,
    Complete,
    Failed,
}

/// The sequencer.
#[derive(Clone, Debug)]
pub struct Configurator {
    target: Target,
    current_baud: u32,
    phase: Phase,
    plan: [Option<Command<'static>>; MAX_PLAN],
    plan_len: usize,
    idx: usize,
    sent: bool,
    attempts: u8,
    mismatch: bool,
}

impl Configurator {
    /// Create a sequencer. `current_baud` is the rate the UART is using now.
    pub fn new(target: Target, current_baud: u32) -> Result<Self, ConfigError> {
        // Reuse the encoder's argument validation so the plan can never contain
        // a command the module would reject on range grounds.
        let mut scratch = [0_u8; 96];
        let checks = [
            Command::Pair050 { interval_ms: target.fix_interval_ms },
            Command::Pair864 { baud: target.baud },
            Command::PqtmCfgMsgRateSet { name: TAR_NAME, rate: target.tar_rate, version: TAR_VERSION },
        ];
        for c in checks {
            match c.encode(&mut scratch) {
                Ok(_) => {}
                Err(EncodeError::InvalidArgument | EncodeError::BufferTooSmall) => {
                    return Err(ConfigError::InvalidTarget)
                }
            }
        }
        Ok(Self {
            target,
            current_baud,
            phase: Phase::QueryFixRate,
            plan: [None; MAX_PLAN],
            plan_len: 0,
            idx: 0,
            sent: false,
            attempts: 0,
            mismatch: current_baud != target.baud,
        })
    }

    fn step_number(&self) -> u8 {
        match self.phase {
            Phase::QueryFixRate => 0,
            Phase::QueryTarRate => 1,
            Phase::Apply => u8::try_from(self.idx.saturating_add(2)).unwrap_or(u8::MAX),
            Phase::Finished(_) => u8::MAX,
        }
    }

    fn current_command(&self) -> Option<Command<'static>> {
        match self.phase {
            Phase::QueryFixRate => Some(Command::Pair051),
            Phase::QueryTarRate => Some(Command::PqtmCfgMsgRateGet { name: TAR_NAME, version: TAR_VERSION }),
            Phase::Apply => self.plan.get(self.idx).copied().flatten(),
            Phase::Finished(_) => None,
        }
    }

    fn build_plan(&mut self) {
        let t = self.target;
        let mut steps: [Option<Command<'static>>; MAX_PLAN] = [None; MAX_PLAN];
        let mut n = 0_usize;
        let mut push = |c: Command<'static>| {
            if let Some(slot) = steps.get_mut(n) {
                *slot = Some(c);
                n = n.saturating_add(1);
            }
        };
        push(Command::PqtmCfgMsgRateSet { name: TAR_NAME, rate: t.tar_rate, version: TAR_VERSION });
        push(Command::Pair382 { value: 1 });
        push(Command::Pair003);
        push(Command::Pair050 { interval_ms: t.fix_interval_ms });
        if self.current_baud != t.baud {
            push(Command::Pair864 { baud: t.baud });
        }
        push(Command::PqtmSavePar);
        push(Command::Pair002);
        self.plan = steps;
        self.plan_len = n;
        self.idx = 0;
    }

    /// Ask what to do next.
    pub fn poll(&mut self) -> Action {
        if let Phase::Finished(result) = self.phase {
            return match result {
                Ok(o) => Action::Done(o),
                Err(e) => Action::Failed(e),
            };
        }
        if self.sent {
            return Action::Wait;
        }
        match self.current_command() {
            Some(c) => {
                self.sent = true;
                self.attempts = self.attempts.saturating_add(1);
                Action::Send(c)
            }
            None => {
                // Cannot happen: every non-final phase has a command. Fail closed.
                self.phase = Phase::Finished(Err(ConfigError::CommandFailed { step: self.step_number() }));
                Action::Failed(ConfigError::CommandFailed { step: self.step_number() })
            }
        }
    }

    fn classify(&self, cmd: &Command<'static>, msg: &Message) -> Verdict {
        match (cmd, msg) {
            // Query results arrive after a PAIR001 ack of the same id.
            (Command::Pair051, Message::FixRate { .. }) => Verdict::Complete,
            (Command::PqtmCfgMsgRateGet { .. }, Message::Pqtm { command: PqtmCommand::CfgMsgRate, result }) => {
                match result {
                    PqtmResult::OkMsgRate { name, .. } if name.as_bytes() == TAR_NAME.as_bytes() => Verdict::Complete,
                    PqtmResult::Error(_) => Verdict::Failed,
                    _ => Verdict::Ignore,
                }
            }
            (Command::PqtmCfgMsgRateSet { .. }, Message::Pqtm { command: PqtmCommand::CfgMsgRate, result }) => {
                match result {
                    PqtmResult::Ok => Verdict::Complete,
                    PqtmResult::Error(_) => Verdict::Failed,
                    PqtmResult::OkMsgRate { .. } => Verdict::Ignore,
                }
            }
            (Command::PqtmSavePar, Message::Pqtm { command: PqtmCommand::SavePar, result }) => match result {
                PqtmResult::Ok => Verdict::Complete,
                PqtmResult::Error(_) => Verdict::Failed,
                PqtmResult::OkMsgRate { .. } => Verdict::Ignore,
            },
            (_, Message::PairAck { command_id, result }) if cmd.pair_id() == Some(*command_id) => {
                let is_query = matches!(cmd, Command::Pair051 | Command::Pair865);
                match *result {
                    0 if is_query => Verdict::Pending,
                    0 => Verdict::Complete,
                    1 => Verdict::Pending,
                    _ => Verdict::Failed,
                }
            }
            _ => Verdict::Ignore,
        }
    }

    fn retry_or_fail(&mut self) {
        if self.attempts >= MAX_ATTEMPTS {
            let step = self.step_number();
            self.phase = Phase::Finished(Err(ConfigError::CommandFailed { step }));
        }
        self.sent = false;
    }

    fn advance(&mut self, msg: &Message) {
        self.sent = false;
        self.attempts = 0;
        match self.phase {
            Phase::QueryFixRate => {
                if let Message::FixRate { interval_ms } = msg {
                    if *interval_ms != self.target.fix_interval_ms {
                        self.mismatch = true;
                    }
                }
                self.phase = Phase::QueryTarRate;
            }
            Phase::QueryTarRate => {
                if let Message::Pqtm { result: PqtmResult::OkMsgRate { rate, version, .. }, .. } = msg {
                    if *rate != self.target.tar_rate || *version != TAR_VERSION {
                        self.mismatch = true;
                    }
                }
                if self.mismatch {
                    self.build_plan();
                    self.phase = Phase::Apply;
                } else {
                    self.phase = Phase::Finished(Ok(Outcome::AlreadyConfigured));
                }
            }
            Phase::Apply => {
                self.idx = self.idx.saturating_add(1);
                if self.idx >= self.plan_len {
                    self.phase = Phase::Finished(Ok(Outcome::Applied { new_baud: self.target.baud }));
                }
            }
            Phase::Finished(_) => {}
        }
    }

    /// Feed a decoded message. Unrelated messages are ignored.
    pub fn on_message(&mut self, msg: &Message) {
        if !self.sent {
            return;
        }
        let Some(cmd) = self.current_command() else { return };
        match self.classify(&cmd, msg) {
            Verdict::Ignore | Verdict::Pending => {}
            Verdict::Complete => self.advance(msg),
            Verdict::Failed => self.retry_or_fail(),
        }
    }

    /// Report that the outstanding command timed out.
    pub fn on_timeout(&mut self) {
        if self.sent {
            self.retry_or_fail();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msg::MsgName;
    use proptest::prelude::*;
    use std::vec::Vec;

    /// A tiny model of the module's configuration behaviour.
    #[derive(Clone, Debug)]
    struct Sim {
        interval_ms: u16,
        tar_rate: u8,
        baud: u32,
        pending_baud: Option<u32>,
        powered_down: bool,
        saved: bool,
        log: Vec<Command<'static>>,
        /// Number of upcoming commands to leave unanswered.
        drop_next: u32,
        /// Number of upcoming commands to refuse with a parameter-error ack.
        nack_next: u32,
    }

    impl Sim {
        fn defaults() -> Self {
            Self {
                interval_ms: 100,
                tar_rate: 0,
                baud: 115_200,
                pending_baud: None,
                powered_down: false,
                saved: false,
                log: Vec::new(),
                drop_next: 0,
                nack_next: 0,
            }
        }

        fn respond(&mut self, c: &Command<'static>) -> Vec<Message> {
            self.log.push(*c);
            if self.drop_next > 0 {
                self.drop_next -= 1;
                return Vec::new();
            }
            let ack = |id: u16, result: u8| Message::PairAck { command_id: id, result };
            if self.nack_next > 0 {
                self.nack_next -= 1;
                return match c.pair_id() {
                    Some(id) => std::vec![ack(id, 4)],
                    None => std::vec![Message::Pqtm { command: PqtmCommand::CfgMsgRate, result: PqtmResult::Error(1) }],
                };
            }
            match *c {
                Command::Pair051 => std::vec![ack(51, 0), Message::FixRate { interval_ms: self.interval_ms }],
                Command::PqtmCfgMsgRateGet { .. } => std::vec![Message::Pqtm {
                    command: PqtmCommand::CfgMsgRate,
                    result: PqtmResult::OkMsgRate {
                        name: MsgName::from_slice(b"PQTMTAR").unwrap(),
                        rate: self.tar_rate,
                        version: 1,
                    },
                }],
                Command::PqtmCfgMsgRateSet { rate, .. } => {
                    self.tar_rate = rate;
                    std::vec![Message::Pqtm { command: PqtmCommand::CfgMsgRate, result: PqtmResult::Ok }]
                }
                Command::Pair382 { .. } => {
                    self.powered_down = true;
                    std::vec![ack(382, 0)]
                }
                Command::Pair003 => std::vec![ack(3, 0)],
                Command::Pair050 { interval_ms } => {
                    if !self.powered_down {
                        return std::vec![ack(50, 4)]; // module insists on being powered down first
                    }
                    self.interval_ms = interval_ms;
                    std::vec![ack(50, 0)]
                }
                Command::Pair864 { baud } => {
                    self.pending_baud = Some(baud);
                    std::vec![ack(864, 0)]
                }
                Command::PqtmSavePar => {
                    self.saved = true;
                    std::vec![Message::Pqtm { command: PqtmCommand::SavePar, result: PqtmResult::Ok }]
                }
                Command::Pair002 => {
                    self.powered_down = false;
                    std::vec![ack(2, 0)]
                }
                _ => Vec::new(),
            }
        }

        /// Model the module restart that follows a successful apply.
        fn restart(&mut self) {
            if let Some(b) = self.pending_baud.take() {
                self.baud = b;
            }
            self.powered_down = false;
        }
    }

    const TARGET: Target = Target { fix_interval_ms: 100, baud: 460_800, tar_rate: 1 };

    /// Run to completion with a bounded number of iterations.
    fn run(c: &mut Configurator, sim: &mut Sim) -> Action {
        for _ in 0..200 {
            match c.poll() {
                Action::Send(cmd) => {
                    let replies = sim.respond(&cmd);
                    if replies.is_empty() {
                        c.on_timeout();
                    }
                    for m in replies {
                        c.on_message(&m);
                    }
                }
                Action::Wait => c.on_timeout(),
                done @ (Action::Done(_) | Action::Failed(_)) => return done,
            }
        }
        panic!("configurator did not terminate");
    }

    #[test]
    fn already_configured_only_queries() {
        let mut sim = Sim { tar_rate: 1, baud: 460_800, ..Sim::defaults() };
        let mut c = Configurator::new(TARGET, 460_800).unwrap();
        assert_eq!(run(&mut c, &mut sim), Action::Done(Outcome::AlreadyConfigured));
        assert_eq!(sim.log, std::vec![Command::Pair051, Command::PqtmCfgMsgRateGet { name: "PQTMTAR", version: 1 }]);
    }

    #[test]
    fn factory_state_runs_full_plan_in_order() {
        let mut sim = Sim::defaults();
        let mut c = Configurator::new(TARGET, 115_200).unwrap();
        assert_eq!(run(&mut c, &mut sim), Action::Done(Outcome::Applied { new_baud: 460_800 }));
        assert_eq!(
            sim.log[2..].to_vec(),
            std::vec![
                Command::PqtmCfgMsgRateSet { name: "PQTMTAR", rate: 1, version: 1 },
                Command::Pair382 { value: 1 },
                Command::Pair003,
                Command::Pair050 { interval_ms: 100 },
                Command::Pair864 { baud: 460_800 },
                Command::PqtmSavePar,
                Command::Pair002,
            ]
        );
        assert!(sim.saved);
        assert!(!sim.powered_down, "module must be powered back up");
        assert_eq!(sim.tar_rate, 1);
        // Restart and verify: second run must find everything in place.
        sim.restart();
        sim.log.clear();
        let mut c2 = Configurator::new(TARGET, 460_800).unwrap();
        assert_eq!(run(&mut c2, &mut sim), Action::Done(Outcome::AlreadyConfigured));
        assert_eq!(sim.log.len(), 2);
    }

    #[test]
    fn wrong_interval_alone_triggers_apply_without_baud_change() {
        let mut sim = Sim { interval_ms: 1000, tar_rate: 1, baud: 460_800, ..Sim::defaults() };
        let mut c = Configurator::new(TARGET, 460_800).unwrap();
        assert_eq!(run(&mut c, &mut sim), Action::Done(Outcome::Applied { new_baud: 460_800 }));
        assert!(!sim.log.iter().any(|c| matches!(c, Command::Pair864 { .. })));
        assert_eq!(sim.interval_ms, 100);
    }

    #[test]
    fn tar_disabled_triggers_apply() {
        let mut sim = Sim { tar_rate: 0, baud: 460_800, ..Sim::defaults() };
        let mut c = Configurator::new(TARGET, 460_800).unwrap();
        assert!(matches!(run(&mut c, &mut sim), Action::Done(Outcome::Applied { .. })));
        assert_eq!(sim.tar_rate, 1);
    }

    #[test]
    fn recovers_from_dropped_responses() {
        let mut sim = Sim { drop_next: 2, tar_rate: 1, baud: 460_800, ..Sim::defaults() };
        let mut c = Configurator::new(TARGET, 460_800).unwrap();
        assert_eq!(run(&mut c, &mut sim), Action::Done(Outcome::AlreadyConfigured));
        // The first query was sent three times (2 dropped + 1 answered).
        assert_eq!(sim.log.iter().filter(|c| **c == Command::Pair051).count(), 3);
    }

    #[test]
    fn gives_up_after_max_attempts() {
        let mut sim = Sim { drop_next: 3, ..Sim::defaults() };
        let mut c = Configurator::new(TARGET, 460_800).unwrap();
        assert_eq!(run(&mut c, &mut sim), Action::Failed(ConfigError::CommandFailed { step: 0 }));
        assert_eq!(sim.log.len(), 3);
        // The failure is sticky.
        assert_eq!(c.poll(), Action::Failed(ConfigError::CommandFailed { step: 0 }));
    }

    #[test]
    fn nack_is_retried_then_fails() {
        let mut sim = Sim { nack_next: 100, ..Sim::defaults() };
        let mut c = Configurator::new(TARGET, 460_800).unwrap();
        assert_eq!(run(&mut c, &mut sim), Action::Failed(ConfigError::CommandFailed { step: 0 }));
    }

    #[test]
    fn failure_mid_plan_reports_plan_step() {
        // Let the queries and first two plan commands succeed, then refuse everything.
        let mut sim = Sim::defaults();
        let mut c = Configurator::new(TARGET, 115_200).unwrap();
        let mut sent = 0;
        let result = loop {
            match c.poll() {
                Action::Send(cmd) => {
                    sent += 1;
                    if sent > 4 {
                        sim.nack_next = 100;
                    }
                    for m in sim.respond(&cmd) {
                        c.on_message(&m);
                    }
                }
                Action::Wait => c.on_timeout(),
                other => break other,
            }
        };
        // Steps: 0,1 queries; 2 = msg rate, 3 = PAIR382; the 5th command (PAIR003) fails.
        assert_eq!(result, Action::Failed(ConfigError::CommandFailed { step: 4 }));
    }

    #[test]
    fn unrelated_and_stale_messages_are_ignored() {
        let mut c = Configurator::new(TARGET, 460_800).unwrap();
        assert!(matches!(c.poll(), Action::Send(Command::Pair051)));
        for m in [
            Message::Unknown,
            Message::PairAck { command_id: 3, result: 0 },
            Message::PairAck { command_id: 51, result: 1 }, // still processing
            Message::PairAck { command_id: 51, result: 0 }, // accepted, result still to come
            Message::Baud { baud: 1 },
        ] {
            c.on_message(&m);
            assert_eq!(c.poll(), Action::Wait);
        }
        c.on_message(&Message::FixRate { interval_ms: 100 });
        assert!(matches!(c.poll(), Action::Send(Command::PqtmCfgMsgRateGet { .. })));
    }

    #[test]
    fn query_for_a_different_message_name_is_ignored() {
        let mut c = Configurator::new(TARGET, 460_800).unwrap();
        c.poll();
        c.on_message(&Message::FixRate { interval_ms: 100 });
        assert!(matches!(c.poll(), Action::Send(Command::PqtmCfgMsgRateGet { .. })));
        let other = Message::Pqtm {
            command: PqtmCommand::CfgMsgRate,
            result: PqtmResult::OkMsgRate { name: MsgName::from_slice(b"PQTMGGA").unwrap(), rate: 1, version: 1 },
        };
        c.on_message(&other);
        assert_eq!(c.poll(), Action::Wait);
    }

    #[test]
    fn messages_before_a_command_is_sent_are_ignored() {
        let mut c = Configurator::new(TARGET, 460_800).unwrap();
        c.on_message(&Message::FixRate { interval_ms: 100 });
        c.on_timeout();
        assert!(matches!(c.poll(), Action::Send(Command::Pair051)));
    }

    #[test]
    fn invalid_targets_rejected() {
        for t in [
            Target { fix_interval_ms: 50, ..TARGET },
            Target { fix_interval_ms: 2000, ..TARGET },
            Target { baud: 12_345, ..TARGET },
            Target { tar_rate: 2, ..TARGET },
        ] {
            assert_eq!(Configurator::new(t, 115_200).err(), Some(ConfigError::InvalidTarget), "{t:?}");
        }
    }

    fn arb_message() -> impl Strategy<Value = Message> {
        prop_oneof![
            Just(Message::Unknown),
            (0_u16..1100).prop_map(|interval_ms| Message::FixRate { interval_ms }),
            (any::<u32>()).prop_map(|baud| Message::Baud { baud }),
            (prop::sample::select(std::vec![2_u16, 3, 50, 51, 382, 513, 864, 865, 999]), 0_u8..7)
                .prop_map(|(command_id, result)| Message::PairAck { command_id, result }),
            (0_u8..3, 0_u8..3).prop_map(|(rate, version)| Message::Pqtm {
                command: PqtmCommand::CfgMsgRate,
                result: PqtmResult::OkMsgRate { name: MsgName::from_slice(b"PQTMTAR").unwrap(), rate, version },
            }),
            Just(Message::Pqtm { command: PqtmCommand::SavePar, result: PqtmResult::Ok }),
            Just(Message::Pqtm { command: PqtmCommand::CfgMsgRate, result: PqtmResult::Ok }),
            (0_u8..4).prop_map(|c| Message::Pqtm { command: PqtmCommand::SavePar, result: PqtmResult::Error(c) }),
        ]
    }

    proptest! {
        /// Any interleaving of arbitrary messages and timeouts terminates in a
        /// bounded number of commands and never panics.
        #[test]
        fn arbitrary_input_terminates(events in prop::collection::vec((arb_message(), any::<bool>()), 0..300)) {
            let mut c = Configurator::new(TARGET, 115_200).unwrap();
            let mut sends = 0_u32;
            let mut it = events.into_iter();
            let mut finished = false;
            for _ in 0..2000 {
                match c.poll() {
                    Action::Send(_) => sends += 1,
                    Action::Wait => match it.next() {
                        Some((m, timeout)) => { if timeout { c.on_timeout(); } else { c.on_message(&m); } }
                        None => c.on_timeout(),
                    },
                    Action::Done(_) | Action::Failed(_) => { finished = true; break; }
                }
            }
            prop_assert!(finished);
            // 2 queries + at most 7 plan commands, each tried at most MAX_ATTEMPTS times.
            prop_assert!(sends <= 9 * u32::from(MAX_ATTEMPTS));
        }
    }
}
