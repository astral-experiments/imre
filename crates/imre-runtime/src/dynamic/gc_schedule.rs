//! Collection thresholds shared by an interpreter and allocation counters for one boxed heap.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

const DEFAULT_THRESHOLDS: [i32; 3] = [2000, 10, 10];

pub(super) struct GcSettings {
    pub(super) enabled: AtomicBool,
    thresholds: Mutex<[i32; 3]>,
    // Most allocations only need to compare the young count.
    young: AtomicI32,
}

impl Default for GcSettings {
    fn default() -> Self {
        Self {
            enabled: AtomicBool::new(true),
            thresholds: Mutex::new(DEFAULT_THRESHOLDS),
            young: AtomicI32::new(DEFAULT_THRESHOLDS[0]),
        }
    }
}

impl GcSettings {
    pub(super) fn thresholds(&self) -> [i32; 3] {
        *self.thresholds.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(super) fn set_thresholds(&self, values: &[i32]) {
        let mut thresholds = self.thresholds.lock().unwrap_or_else(|p| p.into_inner());
        thresholds[..values.len()].copy_from_slice(values);
        self.young.store(thresholds[0], Ordering::Relaxed);
    }

    pub(super) fn young(&self) -> i32 {
        self.young.load(Ordering::Relaxed)
    }
}

#[derive(Default)]
pub(super) struct GcSchedule {
    counts: [i32; 3],
    requested: bool,
    settling: bool,
    long_lived_pending: usize,
    long_lived_total: usize,
}

impl GcSchedule {
    pub(super) fn allocated(&mut self, settings: &GcSettings, collecting: bool) {
        self.counts[0] = self.counts[0].saturating_add(1);
        let young = settings.young();
        if self.counts[0] > young
            && young != 0
            && !collecting
            && !self.settling
            && settings.enabled.load(Ordering::Relaxed)
        {
            self.requested = true;
        }
    }

    pub(super) fn freed(&mut self) {
        if self.counts[0] > 0 {
            self.counts[0] -= 1;
        }
    }

    pub(super) fn requested(&self) -> bool {
        self.requested
    }

    pub(super) fn begin_settling(&mut self) {
        self.requested = false;
        self.settling = true;
    }

    pub(super) fn end_settling(&mut self) {
        self.settling = false;
    }

    /// Choose the oldest eligible generation after deferred acyclic releases have settled.
    pub(super) fn generation_if_due(&self, settings: &GcSettings) -> Option<u8> {
        let [young, middle, old] = settings.thresholds();
        if !settings.enabled.load(Ordering::Relaxed) || young == 0 || self.counts[0] <= young {
            return None;
        }
        if self.counts[2] > old && self.long_lived_pending >= self.long_lived_total / 4 {
            Some(2)
        } else if self.counts[1] > middle {
            Some(1)
        } else {
            Some(0)
        }
    }

    pub(super) fn start_collection(&mut self, generation: u8) {
        let generation = usize::from(generation);
        self.counts[..=generation].fill(0);
        if let Some(older) = self.counts.get_mut(generation + 1) {
            *older = older.saturating_add(1);
        }
        self.requested = false;
    }

    /// Count promotions into the oldest generation, or replace its population after a full pass.
    pub(super) fn record_survivors(&mut self, generation: u8, promoted_or_old_count: usize) {
        match generation {
            0 => {}
            1 => {
                self.long_lived_pending = self
                    .long_lived_pending
                    .saturating_add(promoted_or_old_count);
            }
            2 => {
                self.long_lived_pending = 0;
                self.long_lived_total = promoted_or_old_count;
            }
            _ => unreachable!("invalid generation"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settle_net_allocations_before_collecting() {
        let settings = GcSettings::default();
        settings.set_thresholds(&[1]);
        let mut schedule = GcSchedule::default();
        schedule.freed();
        schedule.allocated(&settings, false);
        assert!(!schedule.requested());
        schedule.allocated(&settings, false);
        assert!(schedule.requested());
        schedule.begin_settling();
        schedule.allocated(&settings, false);
        assert!(!schedule.requested());
        schedule.freed();
        schedule.freed();
        schedule.end_settling();
        assert_eq!(schedule.generation_if_due(&settings), None);
        schedule.allocated(&settings, false);
        assert_eq!(schedule.generation_if_due(&settings), Some(0));
    }

    #[test]
    fn full_collection_requires_enough_promotions() {
        let settings = GcSettings::default();
        settings.set_thresholds(&[1, 0, 0]);
        let mut schedule = GcSchedule::default();
        schedule.start_collection(2);
        schedule.record_survivors(2, 11);
        for generation in [0, 1, 0, 1, 2] {
            schedule.allocated(&settings, false);
            schedule.allocated(&settings, false);
            assert_eq!(schedule.generation_if_due(&settings), Some(generation));
            schedule.start_collection(generation);
            assert!(!schedule.requested());
            schedule.record_survivors(generation, 1);
        }
    }
}
