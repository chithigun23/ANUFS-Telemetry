//! Builds the periodic [`Sample`] from the newest GNSS, heading and IMU data.
//!
//! Rules that keep the log honest:
//!
//! * Every input is stamped with its arrival time. Anything older than the
//!   staleness limit is treated as absent and its validity flag stays clear;
//!   old data is never presented as current.
//! * RMC and GGA describe the same epoch only when their UTC times match.
//!   Position, speed and course are never mixed across epochs.
//! * Invalid fields are zero and the matching flag bit is clear, so a reader
//!   can always tell "zero" from "not available".

use crate::imu_window::ImuSummary;
use tel_logfmt::records::{flags, Sample};
use tel_nmea::{FixQuality, Gga, ModeIndicator, Rmc, Tar};

#[derive(Clone, Copy, Debug)]
struct Stamped<T> {
    value: T,
    at_us: u64,
}

/// Everything that is not a GNSS message.
#[derive(Clone, Copy, Debug, Default)]
pub struct Context {
    /// Monotonic microseconds when the sample is built.
    pub now_us: u64,
    /// UTC (Unix milliseconds) at `now_us`, if the clock knows it.
    pub utc_unix_ms: Option<i64>,
    /// IMU window summary, if any samples arrived.
    pub imu: Option<ImuSummary>,
    /// Extra [`flags`] set by the caller (`CAN*_ALIVE`, `CARD_FAULT`, ...).
    pub flags: u32,
    /// Cumulative NMEA checksum errors.
    pub nmea_errors: u32,
    /// Cumulative queue overruns.
    pub overruns: u32,
    /// Cumulative CAN1 frames.
    pub can1_frames: u32,
    /// Cumulative CAN2 frames.
    pub can2_frames: u32,
}

/// Holds the newest messages.
#[derive(Clone, Copy, Debug)]
pub struct Fusion {
    stale_us: u64,
    rmc: Option<Stamped<Rmc>>,
    gga: Option<Stamped<Gga>>,
    tar: Option<Stamped<Tar>>,
}

impl Fusion {
    /// Create for a GNSS fix interval. Data older than twice the interval plus
    /// 100 ms is considered stale.
    pub const fn new(fix_interval_ms: u16) -> Self {
        let stale_ms = (fix_interval_ms as u64).saturating_mul(2).saturating_add(100);
        Self { stale_us: stale_ms.saturating_mul(1000), rmc: None, gga: None, tar: None }
    }

    /// Record an RMC message received at `at_us`.
    pub fn on_rmc(&mut self, rmc: Rmc, at_us: u64) {
        self.rmc = Some(Stamped { value: rmc, at_us });
    }

    /// Record a GGA message received at `at_us`.
    pub fn on_gga(&mut self, gga: Gga, at_us: u64) {
        self.gga = Some(Stamped { value: gga, at_us });
    }

    /// Record a `$PQTMTAR` message received at `at_us`.
    pub fn on_tar(&mut self, tar: Tar, at_us: u64) {
        self.tar = Some(Stamped { value: tar, at_us });
    }

    fn fresh<T: Copy>(&self, s: Option<Stamped<T>>, now_us: u64) -> Option<T> {
        let s = s?;
        // A stamp in the future (clock hiccup) counts as fresh rather than stale.
        if now_us.saturating_sub(s.at_us) <= self.stale_us {
            Some(s.value)
        } else {
            None
        }
    }

    /// Build the sample for `ctx`.
    pub fn build(&self, ctx: &Context) -> Sample {
        let rmc = self.fresh(self.rmc, ctx.now_us);
        let gga = self.fresh(self.gga, ctx.now_us);
        let tar = self.fresh(self.tar, ctx.now_us);
        let mut s = Sample {
            t_us: ctx.now_us,
            flags: ctx.flags,
            nmea_errors: ctx.nmea_errors,
            overruns: ctx.overruns,
            can1_frames: ctx.can1_frames,
            can2_frames: ctx.can2_frames,
            diff_age_ds: u16::MAX,
            ..Sample::default()
        };

        // Same epoch only when the UTC time fields agree.
        let same_epoch = match (&rmc, &gga) {
            (Some(r), Some(g)) => r.time.is_some() && r.time == g.time,
            _ => false,
        };
        // When the two are from different epochs, use only the newer one.
        let rmc_newer = self.rmc.zip(self.gga).is_some_and(|(r, g)| r.at_us >= g.at_us);
        let use_gga = gga.is_some() && (same_epoch || rmc.is_none() || !rmc_newer);
        let use_rmc = rmc.is_some() && (same_epoch || gga.is_none() || rmc_newer);

        let mut have_position = false;
        if let (true, Some(g)) = (use_gga, gga) {
            if g.quality != FixQuality::Invalid {
                if let (Some(lat), Some(lon)) = (g.lat_e9, g.lon_e9) {
                    s.lat_e9 = lat;
                    s.lon_e9 = lon;
                    s.alt_mm = g.alt_mm.unwrap_or(0);
                    s.fix_quality = g.quality.as_u8();
                    s.num_sv = g.num_sv;
                    s.hdop_e2 = g.hdop_e2.unwrap_or(0);
                    s.diff_age_ds = g.diff_age_ds.map_or(u16::MAX, |d| u16::try_from(d).unwrap_or(u16::MAX - 1));
                    have_position = true;
                }
            }
        }
        if let (true, Some(r)) = (use_rmc, rmc) {
            if r.valid {
                s.sog_mm_s = r.sog_mm_s.unwrap_or(0);
                s.cog_cdeg = r.cog_cdeg.unwrap_or(0);
                if !have_position {
                    if let (Some(lat), Some(lon)) = (r.lat_e9, r.lon_e9) {
                        s.lat_e9 = lat;
                        s.lon_e9 = lon;
                        s.fix_quality = match r.mode {
                            Some(ModeIndicator::RtkFixed) => 4,
                            Some(ModeIndicator::FloatRtk) => 5,
                            Some(ModeIndicator::Differential) => 2,
                            _ => 1,
                        };
                        have_position = true;
                    }
                }
            }
        }
        if have_position {
            s.flags |= flags::GNSS_VALID;
        }

        if let Some(t) = tar {
            if t.quality != FixQuality::Invalid {
                if let Some(h) = t.heading_e6 {
                    s.heading_e6 = h;
                    s.pitch_e6 = t.pitch_e6.unwrap_or(0);
                    s.baseline_mm = t.baseline_mm.unwrap_or(0);
                    s.acc_heading_e6 = t.acc_heading_e6.unwrap_or(0);
                    s.heading_quality = t.quality.as_u8();
                    s.heading_sv = t.used_sv.unwrap_or(0);
                    s.flags |= flags::HEADING_VALID;
                }
            }
        }

        if let Some(ms) = ctx.utc_unix_ms {
            s.utc_unix_ms = ms;
            s.flags |= flags::UTC_VALID;
        }

        if let Some(i) = ctx.imu {
            s.accel_mean_ug = i.accel_mean_ug;
            s.accel_peak_ug = i.accel_peak_ug;
            s.gyro_mean_mdps = i.gyro_mean_mdps;
            s.gyro_peak_mdps = i.gyro_peak_mdps;
            s.imu_temp_centi_c = i.temp_centi_c;
            s.imu_count = i.count;
            s.flags |= flags::IMU_VALID;
        }
        s
    }
}
