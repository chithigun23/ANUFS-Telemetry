//! Fixed-size store of the newest raw value of every signal.

use crate::catalog::{Catalog, Decoded, SignalId};

/// Newest sample of one signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    /// Raw bits as extracted from the frame.
    pub raw: u64,
    /// Receive time in microseconds on the caller's monotonic clock.
    pub stamp_us: u64,
    /// How many times this signal has been updated (wraps never, saturates).
    pub count: u32,
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    sample: Sample,
    valid: bool,
}

impl Slot {
    const EMPTY: Self = Self { sample: Sample { raw: 0, stamp_us: 0, count: 0 }, valid: false };
}

/// Newest value per signal for a catalog with `N` signals.
#[derive(Clone, Debug)]
pub struct Latest<const N: usize> {
    slots: [Slot; N],
    rejected: u32,
}

impl<const N: usize> Default for Latest<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Latest<N> {
    /// An empty store.
    pub const fn new() -> Self {
        Self { slots: [Slot::EMPTY; N], rejected: 0 }
    }

    /// Record a value. Returns `false` (and counts it) if `id` is out of range.
    pub fn update(&mut self, id: SignalId, raw: u64, stamp_us: u64) -> bool {
        match self.slots.get_mut(usize::from(id.0)) {
            Some(slot) => {
                slot.sample = Sample { raw, stamp_us, count: slot.sample.count.saturating_add(1) };
                slot.valid = true;
                true
            }
            None => {
                self.rejected = self.rejected.saturating_add(1);
                false
            }
        }
    }

    /// Newest sample, if the signal has ever been received.
    pub fn get(&self, id: SignalId) -> Option<Sample> {
        self.slots.get(usize::from(id.0)).filter(|s| s.valid).map(|s| s.sample)
    }

    /// Age of the newest sample in microseconds (0 if the clock went backwards).
    pub fn age_us(&self, id: SignalId, now_us: u64) -> Option<u64> {
        self.get(id).map(|s| now_us.saturating_sub(s.stamp_us))
    }

    /// Updates refused because the signal id was out of range.
    pub fn rejected(&self) -> u32 {
        self.rejected
    }

    /// Forget every sample (for example after a bus-off recovery).
    pub fn clear(&mut self) {
        self.slots = [Slot::EMPTY; N];
    }
}

/// Decode a frame with `catalog` and store every signal in `latest`.
pub fn ingest<const N: usize>(
    catalog: &Catalog,
    latest: &mut Latest<N>,
    id: u32,
    extended: bool,
    data: &[u8],
    stamp_us: u64,
) -> Decoded {
    catalog.decode(id, extended, data, |sid, _signal, raw| {
        latest.update(sid, raw, stamp_us);
    })
}
