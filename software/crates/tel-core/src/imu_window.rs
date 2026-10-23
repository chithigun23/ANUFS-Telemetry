//! Accumulates IMU samples between log records and summarises them as a mean
//! and a peak per axis, so a 10 Hz record still reflects a 416 Hz sensor.

use tel_asm330::Scaled;

/// Summary of one window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImuSummary {
    /// Mean acceleration per axis, micro-g (rounded toward zero).
    pub accel_mean_ug: [i32; 3],
    /// Largest absolute acceleration per axis, micro-g.
    pub accel_peak_ug: [i32; 3],
    /// Mean angular rate per axis, milli-degrees per second.
    pub gyro_mean_mdps: [i32; 3],
    /// Largest absolute angular rate per axis, milli-degrees per second.
    pub gyro_peak_mdps: [i32; 3],
    /// Mean temperature, hundredths of a degree C.
    pub temp_centi_c: i16,
    /// Samples in the window (saturates at `u16::MAX`).
    pub count: u16,
}

/// The accumulator. Sums are 64-bit, so a window of billions of samples cannot overflow.
#[derive(Clone, Copy, Debug, Default)]
pub struct ImuWindow {
    sum_a: [i64; 3],
    sum_g: [i64; 3],
    peak_a: [u32; 3],
    peak_g: [u32; 3],
    sum_t: i64,
    count: u32,
}

impl ImuWindow {
    /// An empty window.
    pub const fn new() -> Self {
        Self { sum_a: [0; 3], sum_g: [0; 3], peak_a: [0; 3], peak_g: [0; 3], sum_t: 0, count: 0 }
    }

    /// Number of samples added since the last [`ImuWindow::take`].
    pub const fn count(&self) -> u32 {
        self.count
    }

    /// Add one sample.
    pub fn add(&mut self, s: &Scaled) {
        for (i, (&a, &g)) in s.accel_micro_g.iter().zip(&s.gyro_mdps).enumerate() {
            if let Some(x) = self.sum_a.get_mut(i) {
                *x = x.saturating_add(i64::from(a));
            }
            if let Some(x) = self.sum_g.get_mut(i) {
                *x = x.saturating_add(i64::from(g));
            }
            if let Some(x) = self.peak_a.get_mut(i) {
                *x = (*x).max(a.unsigned_abs());
            }
            if let Some(x) = self.peak_g.get_mut(i) {
                *x = (*x).max(g.unsigned_abs());
            }
        }
        self.sum_t = self.sum_t.saturating_add(i64::from(s.temp_centi_c));
        self.count = self.count.saturating_add(1);
    }

    /// Summarise and reset. Returns `None` if the window is empty.
    pub fn take(&mut self) -> Option<ImuSummary> {
        let n = i64::from(self.count);
        if n == 0 {
            return None;
        }
        let mean = |sum: i64| -> i32 { i32::try_from(sum.checked_div(n).unwrap_or(0)).unwrap_or(0) };
        let peak = |p: u32| -> i32 { i32::try_from(p).unwrap_or(i32::MAX) };
        let summary = ImuSummary {
            accel_mean_ug: [mean(self.sum_a[0]), mean(self.sum_a[1]), mean(self.sum_a[2])],
            accel_peak_ug: [peak(self.peak_a[0]), peak(self.peak_a[1]), peak(self.peak_a[2])],
            gyro_mean_mdps: [mean(self.sum_g[0]), mean(self.sum_g[1]), mean(self.sum_g[2])],
            gyro_peak_mdps: [peak(self.peak_g[0]), peak(self.peak_g[1]), peak(self.peak_g[2])],
            temp_centi_c: i16::try_from(self.sum_t.checked_div(n).unwrap_or(0)).unwrap_or(0),
            count: u16::try_from(self.count).unwrap_or(u16::MAX),
        };
        *self = Self::new();
        Some(summary)
    }
}
