//! Shared-LLC model (write-back, write-allocate) used to derive modeled DRAM
//! traffic from architectural accesses.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MemoryTraffic {
    pub bytes_load: u64,
    pub bytes_store: u64,
}

#[derive(Clone, Copy, Debug)]
struct CacheLine {
    tag: u64,
    last_used: u64,
    dirty: bool,
}

/// Tag marking an unoccupied way. Real tags are `line_address / set_count`,
/// which cannot reach `u64::MAX`.
const TAG_INVALID: u64 = u64::MAX;

pub struct CacheModel {
    line_size: u64,
    line_shift: u32,
    /// Set when the set count is a power of two (the common case), replacing
    /// the division/modulo in set-index/tag computation with shifts.
    set_shift: Option<u32>,
    capacity: u64,
    associativity: usize,
    set_count: usize,
    /// Flat `set_count * associativity` array; set `i` occupies
    /// `[i * associativity, (i + 1) * associativity)`. Unoccupied ways carry
    /// `TAG_INVALID`. A flat layout keeps neighboring sets on the same cache
    /// lines of the host, which matters for streaming workloads.
    lines: Vec<CacheLine>,
    clock: u64,
    /// Small MRU filter over recently hit lines: (line address, flat index
    /// into `lines`). Skips set lookup and tag scan for the few lines hot
    /// loops cycle through (e.g. two or three interleaved streams). Stored
    /// indexes stay valid as long as no line is replaced, so the whole filter
    /// is invalidated on every miss (misses are comparatively rare).
    mru: [(u64, u32); MRU_WAYS],
    mru_cursor: usize,
}

const MRU_WAYS: usize = 4;
const MRU_INVALID: (u64, u32) = (u64::MAX, 0);

impl CacheModel {
    pub fn new(line_size: u64, capacity: u64, associativity: usize) -> Option<Self> {
        if !line_size.is_power_of_two() || line_size == 0 || associativity == 0 {
            return None;
        }
        let lines = capacity / line_size;
        let set_count = lines / associativity as u64;
        if set_count == 0 {
            return None;
        }
        Some(Self {
            line_size,
            line_shift: line_size.trailing_zeros(),
            set_shift: set_count
                .is_power_of_two()
                .then(|| set_count.trailing_zeros()),
            capacity: set_count * associativity as u64 * line_size,
            associativity,
            set_count: set_count as usize,
            lines: vec![
                CacheLine {
                    tag: TAG_INVALID,
                    last_used: 0,
                    dirty: false,
                };
                set_count as usize * associativity
            ],
            clock: 0,
            mru: [MRU_INVALID; MRU_WAYS],
            mru_cursor: 0,
        })
    }

    pub fn line_size(&self) -> u64 {
        self.line_size
    }

    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    pub fn associativity(&self) -> usize {
        self.associativity
    }

    /// Inlined into the batch-processing loop: at billions of calls per run,
    /// call overhead alone is measurable.
    #[inline(always)]
    pub fn access(&mut self, address: u64, size: u64, store: bool) -> MemoryTraffic {
        let mut traffic = MemoryTraffic::default();
        if size == 0 {
            return traffic;
        }
        let first_line = address >> self.line_shift;
        let last_address = address.saturating_add(size - 1);
        let last_line = last_address >> self.line_shift;

        'lines: for line_address in first_line..=last_line {
            self.clock = self.clock.wrapping_add(1);
            for &(hit_address, index) in &self.mru {
                if hit_address == line_address {
                    let line = &mut self.lines[index as usize];
                    line.last_used = self.clock;
                    if store {
                        line.dirty = true;
                    }
                    continue 'lines;
                }
            }
            let (set_index, tag) = match self.set_shift {
                Some(shift) => (
                    (line_address & ((1_u64 << shift) - 1)) as usize,
                    line_address >> shift,
                ),
                None => (
                    (line_address % self.set_count as u64) as usize,
                    line_address / self.set_count as u64,
                ),
            };
            let base = set_index * self.associativity;
            let set = &mut self.lines[base..base + self.associativity];
            if let Some(way) = set.iter().position(|line| line.tag == tag) {
                let line = &mut set[way];
                line.last_used = self.clock;
                if store {
                    line.dirty = true;
                }
                self.mru[self.mru_cursor] = (line_address, (base + way) as u32);
                self.mru_cursor = (self.mru_cursor + 1) % MRU_WAYS;
                continue;
            }

            self.mru = [MRU_INVALID; MRU_WAYS];
            // Miss: fill an empty way or evict the least recently used one.
            let victim = set
                .iter()
                .enumerate()
                .find(|(_, line)| line.tag == TAG_INVALID)
                .map(|(way, _)| way)
                .unwrap_or_else(|| {
                    set.iter()
                        .enumerate()
                        .min_by_key(|(_, line)| line.last_used)
                        .map(|(way, _)| way)
                        .unwrap_or(0)
                });
            if set[victim].tag != TAG_INVALID && set[victim].dirty {
                traffic.bytes_store = traffic.bytes_store.saturating_add(self.line_size);
            }
            set[victim] = CacheLine {
                tag,
                last_used: self.clock,
                dirty: store,
            };
            // A cold write uses ordinary write allocation. The instrumentation
            // layers do not currently expose enough instruction semantics to
            // identify every non-temporal store, so those are conservatively
            // modeled here.
            traffic.bytes_load = traffic.bytes_load.saturating_add(self.line_size);
        }
        traffic
    }

    pub fn flush(&mut self) -> MemoryTraffic {
        self.mru = [MRU_INVALID; MRU_WAYS];
        let mut dirty = 0_u64;
        for line in &mut self.lines {
            if line.tag != TAG_INVALID && line.dirty {
                dirty += 1;
            }
            line.tag = TAG_INVALID;
            line.last_used = 0;
            line.dirty = false;
        }
        MemoryTraffic {
            bytes_load: 0,
            bytes_store: dirty.saturating_mul(self.line_size),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_model_counts_write_allocate_dirty_eviction_and_final_flush() {
        let mut cache = CacheModel::new(64, 128, 1).unwrap();
        assert_eq!(
            cache.access(0, 8, false),
            MemoryTraffic {
                bytes_load: 64,
                bytes_store: 0
            }
        );
        assert_eq!(cache.access(8, 8, false), MemoryTraffic::default());
        assert_eq!(cache.access(8, 8, true), MemoryTraffic::default());
        assert_eq!(cache.access(16, 8, true), MemoryTraffic::default());
        assert_eq!(
            cache.access(128, 8, false),
            MemoryTraffic {
                bytes_load: 64,
                bytes_store: 64
            }
        );
        assert_eq!(cache.access(0, 8, false).bytes_load, 64);
        assert_eq!(cache.flush(), MemoryTraffic::default());
    }

    #[test]
    fn cache_model_splits_cross_line_accesses() {
        let mut cache = CacheModel::new(64, 256, 1).unwrap();
        assert_eq!(cache.access(60, 8, false).bytes_load, 128);
        assert!(CacheModel::new(48, 256, 1).is_none());
        assert!(CacheModel::new(64, 64, 2).is_none());
    }
}
