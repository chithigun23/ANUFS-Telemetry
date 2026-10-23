//! PPS-disciplined clock: maps the MCU's monotonic microsecond clock to UTC.
//!
//! * The GNSS module emits one pulse per second. A timer input-capture stamps
//!   each pulse with the monotonic clock (`on_pps`), with no software latency.
//! * The pulse carries no date or time. The NMEA message for that second
//!   arrives shortly afterwards; when its time has zero milliseconds it names
//!   the pulse that just happened (`on_time_message`), which labels the pulse
//!   with an exact UTC second.
//! * Every later pulse must arrive an exact whole number of seconds after the
//!   previous one, within a tolerance. Pulses that do not are rejected and the
//!   lock is dropped, so a glitch or an extra edge can never shift the time.
//!
//! If no pulse is available, the first time message can still seed a coarse
//! reference from its arrival time (accuracy tens to hundreds of milliseconds),
//! which a later pulse replaces with a precise one.

/// How the current time reference was obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// No reference yet.
    Unsynced,
    /// Seeded from message arrival time, no pulse.
    Coarse,
    /// Locked to the pulse per second.
    Pps,
}

/// What `on_pps` decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PpsOutcome {
    /// Remembered; no UTC label yet.
    Stored,
    /// Accepted and used to refresh the reference.
    Locked,
    /// Rejected: not a whole number of seconds after the previous pulse. The
    /// lock was dropped.
    Rejected,
}

/// Tunable limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Allowed deviation from a whole number of seconds between pulses.
    pub tolerance_us: u64,
    /// A reference older than this (no accepted pulse) no longer counts as locked.
    pub lock_timeout_us: u64,
    /// A time message labels a pulse only if it arrives within this long after it.
    pub label_window_us: u64,
    /// Pulses further apart than this many seconds are not trusted to be
    /// consecutive (the second count could be ambiguous).
    pub max_gap_s: u64,
}

impl Config {
    /// The default limits, usable in `const` and `static` initialisers.
    pub const DEFAULT: Self =
        Self { tolerance_us: 2_000, lock_timeout_us: 3_000_000, label_window_us: 900_000, max_gap_s: 10 };
}

impl Default for Config {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Clone, Copy, Debug)]
struct Reference {
    mono_us: u64,
    utc_ms: i64,
    quality: Quality,
}

/// The clock.
#[derive(Clone, Copy, Debug)]
pub struct PpsClock {
    cfg: Config,
    reference: Option<Reference>,
    last_pps_us: Option<u64>,
    last_accepted_us: Option<u64>,
    rejected: u32,
}

impl Default for PpsClock {
    fn default() -> Self {
        Self::new(Config::default())
    }
}

impl PpsClock {
    /// A new, unsynchronised clock.
    pub const fn new(cfg: Config) -> Self {
        Self { cfg, reference: None, last_pps_us: None, last_accepted_us: None, rejected: 0 }
    }

    /// Pulses rejected so far.
    pub const fn rejected(&self) -> u32 {
        self.rejected
    }

    /// Handle a pulse captured at `capture_us` on the monotonic clock.
    pub fn on_pps(&mut self, capture_us: u64) -> PpsOutcome {
        self.last_pps_us = Some(capture_us);
        let Some(r) = self.reference else { return PpsOutcome::Stored };
        if r.quality != Quality::Pps {
            // A coarse reference cannot validate a pulse; wait for a label.
            return PpsOutcome::Stored;
        }
        let Some(dt) = capture_us.checked_sub(r.mono_us) else {
            return self.reject();
        };
        let secs = dt.saturating_add(500_000).checked_div(1_000_000).unwrap_or(0);
        let expected = secs.saturating_mul(1_000_000);
        let err = dt.abs_diff(expected);
        if secs == 0 || secs > self.cfg.max_gap_s || err > self.cfg.tolerance_us {
            return self.reject();
        }
        let advance = i64::try_from(secs.saturating_mul(1000)).unwrap_or(0);
        self.reference =
            Some(Reference { mono_us: capture_us, utc_ms: r.utc_ms.saturating_add(advance), quality: Quality::Pps });
        self.last_accepted_us = Some(capture_us);
        PpsOutcome::Locked
    }

    fn reject(&mut self) -> PpsOutcome {
        self.rejected = self.rejected.saturating_add(1);
        self.reference = None;
        self.last_accepted_us = None;
        PpsOutcome::Rejected
    }

    /// Handle a time message. `utc_unix_ms` is the UTC time the message names
    /// and `arrival_us` is when it was received on the monotonic clock.
    pub fn on_time_message(&mut self, utc_unix_ms: i64, arrival_us: u64) {
        let on_second = utc_unix_ms.rem_euclid(1000) == 0;
        if on_second {
            if let Some(p) = self.last_pps_us {
                let age = arrival_us.checked_sub(p);
                if age.is_some_and(|a| a <= self.cfg.label_window_us) {
                    // Already locked to this very pulse (it was accepted before the
                    // message arrived)? Then the label must agree; disagreement drops the lock.
                    if let (Some(r), Some(acc)) = (self.reference, self.last_accepted_us) {
                        if r.quality == Quality::Pps && acc == p {
                            if r.utc_ms != utc_unix_ms {
                                self.reject();
                            }
                            return;
                        }
                    }
                    self.reference = Some(Reference { mono_us: p, utc_ms: utc_unix_ms, quality: Quality::Pps });
                    self.last_accepted_us = Some(p);
                    return;
                }
            }
        }
        if self.reference.is_none() {
            self.reference = Some(Reference { mono_us: arrival_us, utc_ms: utc_unix_ms, quality: Quality::Coarse });
        }
    }

    /// Current quality given the time `now_us`.
    pub fn quality(&self, now_us: u64) -> Quality {
        match self.reference {
            None => Quality::Unsynced,
            Some(r) if r.quality == Quality::Pps => {
                let fresh = self.last_accepted_us.is_some_and(|a| now_us.saturating_sub(a) <= self.cfg.lock_timeout_us);
                if fresh {
                    Quality::Pps
                } else {
                    Quality::Coarse
                }
            }
            Some(_) => Quality::Coarse,
        }
    }

    /// True when locked to the pulse and the lock is fresh.
    pub fn is_locked(&self, now_us: u64) -> bool {
        self.quality(now_us) == Quality::Pps
    }

    /// UTC (Unix milliseconds) at monotonic time `mono_us`, extrapolating from
    /// the latest reference. `None` before any reference exists.
    pub fn utc_unix_ms_at(&self, mono_us: u64) -> Option<i64> {
        let r = self.reference?;
        let delta_us = i128::from(mono_us).checked_sub(i128::from(r.mono_us))?;
        let delta_ms = i64::try_from(delta_us.div_euclid(1000)).ok()?;
        r.utc_ms.checked_add(delta_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_700_000_000_000; // an exact UTC second in Unix ms

    /// Drive a clean 1 Hz feed: pulse at `base + n s`, message 150 ms later.
    fn feed(c: &mut PpsClock, base: u64, n: u64) {
        let cap = base + n * 1_000_000;
        c.on_pps(cap);
        c.on_time_message(T0 + (n as i64) * 1000, cap + 150_000);
    }

    #[test]
    fn locks_after_first_labelled_pulse() {
        let mut c = PpsClock::default();
        assert_eq!(c.quality(0), Quality::Unsynced);
        assert_eq!(c.utc_unix_ms_at(5), None);
        feed(&mut c, 10_000_000, 0);
        assert_eq!(c.quality(10_200_000), Quality::Pps);
        // exactly at the pulse
        assert_eq!(c.utc_unix_ms_at(10_000_000), Some(T0));
        // 250 ms later
        assert_eq!(c.utc_unix_ms_at(10_250_000), Some(T0 + 250));
        // before the pulse (a sample taken just before it)
        assert_eq!(c.utc_unix_ms_at(9_999_000), Some(T0 - 1));
    }

    #[test]
    fn stays_locked_through_a_clean_feed_and_tracks_the_second_count() {
        let mut c = PpsClock::default();
        for n in 0..100 {
            feed(&mut c, 1_000_000, n);
            assert!(c.is_locked(1_000_000 + n * 1_000_000 + 500_000));
            assert_eq!(c.utc_unix_ms_at(1_000_000 + n * 1_000_000 + 400_000), Some(T0 + n as i64 * 1000 + 400));
        }
        assert_eq!(c.rejected(), 0);
    }

    #[test]
    fn tolerates_crystal_drift() {
        // +40 ppm crystal: each "second" is 1_000_040 us
        let mut c = PpsClock::default();
        c.on_pps(1_000_000);
        c.on_time_message(T0, 1_150_000);
        for n in 1..200_u64 {
            assert_eq!(c.on_pps(1_000_000 + n * 1_000_040), PpsOutcome::Locked, "n = {n}");
        }
        assert_eq!(c.rejected(), 0);
    }

    #[test]
    fn rejects_an_extra_edge_and_drops_lock() {
        let mut c = PpsClock::default();
        feed(&mut c, 0, 0);
        feed(&mut c, 0, 1);
        // glitch 300 ms after the last pulse
        assert_eq!(c.on_pps(1_300_000), PpsOutcome::Rejected);
        assert_eq!(c.rejected(), 1);
        assert_eq!(c.quality(1_400_000), Quality::Unsynced);
        assert_eq!(c.utc_unix_ms_at(1_400_000), None, "no time is better than wrong time");
        // The genuine next pulse cannot validate itself: it waits for a label.
        assert_eq!(c.on_pps(2_000_000), PpsOutcome::Stored);
        c.on_time_message(T0 + 2000, 2_150_000);
        assert!(c.is_locked(2_200_000));
        assert_eq!(c.utc_unix_ms_at(2_000_000), Some(T0 + 2000));
    }

    #[test]
    fn missed_pulses_are_bridged_by_counting_seconds() {
        let mut c = PpsClock::default();
        feed(&mut c, 0, 0);
        // pulses 1 and 2 are missing; pulse 3 arrives
        assert_eq!(c.on_pps(3_000_500), PpsOutcome::Locked);
        assert_eq!(c.utc_unix_ms_at(3_000_500), Some(T0 + 3000));
    }

    #[test]
    fn pulse_spacing_tolerance_boundaries() {
        // default tolerance is 2000 us either side of a whole number of seconds
        let cases: [(u64, PpsOutcome); 8] = [
            (1_000_000, PpsOutcome::Locked),
            (1_002_000, PpsOutcome::Locked),   // exactly at the limit
            (1_002_001, PpsOutcome::Rejected), // one microsecond beyond
            (998_000, PpsOutcome::Locked),
            (997_999, PpsOutcome::Rejected),
            (1_400_000, PpsOutcome::Rejected), // 0.4 s off a whole second
            (1_500_000, PpsOutcome::Rejected), // exactly half way: ambiguous second count
            (2_003_000, PpsOutcome::Rejected), // right second, 3 ms off
        ];
        for (dt, want) in cases {
            let mut c = PpsClock::default();
            feed(&mut c, 0, 0);
            assert_eq!(c.on_pps(dt), want, "dt = {dt}");
        }
    }

    #[test]
    fn too_long_a_gap_is_not_trusted() {
        let mut c = PpsClock::default();
        feed(&mut c, 0, 0);
        assert_eq!(c.on_pps(11_000_000), PpsOutcome::Rejected);
    }

    #[test]
    fn lock_expires_without_pulses_but_time_still_extrapolates_as_coarse() {
        let mut c = PpsClock::default();
        feed(&mut c, 0, 0);
        assert!(c.is_locked(2_900_000));
        assert!(!c.is_locked(3_100_000));
        assert_eq!(c.quality(3_100_000), Quality::Coarse);
        assert_eq!(c.utc_unix_ms_at(3_100_000), Some(T0 + 3100));
    }

    #[test]
    fn a_message_far_from_any_pulse_does_not_label_it() {
        let mut c = PpsClock::default();
        c.on_pps(1_000_000);
        // message names an exact second but arrives 950 ms after the pulse: too late to be that pulse's label
        c.on_time_message(T0, 1_950_000);
        assert_eq!(c.quality(1_960_000), Quality::Coarse, "only a coarse reference from arrival time");
    }

    #[test]
    fn fractional_second_messages_never_label_pulses() {
        let mut c = PpsClock::default();
        c.on_pps(1_000_000);
        c.on_time_message(T0 + 100, 1_100_000); // 10 Hz fix at .1 s
        assert_ne!(c.quality(1_200_000), Quality::Pps);
    }

    #[test]
    fn coarse_reference_is_upgraded_by_a_pulse_label() {
        let mut c = PpsClock::default();
        c.on_time_message(T0 + 300, 5_000_000); // no pulse yet: coarse
        assert_eq!(c.quality(5_000_001), Quality::Coarse);
        assert_eq!(c.utc_unix_ms_at(5_100_000), Some(T0 + 400));
        c.on_pps(6_000_000);
        c.on_time_message(T0 + 1000, 6_120_000);
        assert_eq!(c.quality(6_130_000), Quality::Pps);
        assert_eq!(c.utc_unix_ms_at(6_000_000), Some(T0 + 1000));
    }

    #[test]
    fn a_wrong_second_label_for_an_already_locked_pulse_drops_the_lock() {
        let mut c = PpsClock::default();
        feed(&mut c, 0, 0);
        c.on_pps(1_000_000); // accepted by continuity: reference says T0 + 1000
        c.on_time_message(T0 + 5000, 1_150_000); // message claims a different second
        assert_eq!(c.quality(1_200_000), Quality::Unsynced);
        assert_eq!(c.rejected(), 1);
    }

    #[test]
    fn a_consistent_late_message_for_a_locked_pulse_changes_nothing() {
        let mut c = PpsClock::default();
        feed(&mut c, 0, 0);
        c.on_pps(1_000_000);
        c.on_time_message(T0 + 1000, 1_150_000);
        assert!(c.is_locked(1_200_000));
        assert_eq!(c.rejected(), 0);
    }

    #[test]
    fn pulses_arriving_before_any_reference_are_just_stored() {
        let mut c = PpsClock::default();
        assert_eq!(c.on_pps(100), PpsOutcome::Stored);
        assert_eq!(c.on_pps(1_000_100), PpsOutcome::Stored);
    }

    #[test]
    fn a_pulse_time_going_backwards_is_rejected() {
        let mut c = PpsClock::default();
        feed(&mut c, 10_000_000, 0);
        assert_eq!(c.on_pps(9_000_000), PpsOutcome::Rejected);
    }
}
