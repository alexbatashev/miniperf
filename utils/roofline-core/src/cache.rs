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

pub struct CacheModel {
    line_size: u64,
    capacity: u64,
    associativity: usize,
    sets: Vec<Vec<CacheLine>>,
    clock: u64,
}

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
            capacity: set_count * associativity as u64 * line_size,
            associativity,
            sets: vec![Vec::with_capacity(associativity); set_count as usize],
            clock: 0,
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

    pub fn access(&mut self, address: u64, size: u64, store: bool) -> MemoryTraffic {
        let mut traffic = MemoryTraffic::default();
        if size == 0 {
            return traffic;
        }
        let first_line = address / self.line_size;
        let last_address = address.saturating_add(size - 1);
        let last_line = last_address / self.line_size;

        for line_address in first_line..=last_line {
            self.clock = self.clock.wrapping_add(1);
            let set_index = line_address as usize % self.sets.len();
            let tag = line_address / self.sets.len() as u64;
            let set = &mut self.sets[set_index];
            if let Some(line) = set.iter_mut().find(|line| line.tag == tag) {
                line.last_used = self.clock;
                if store {
                    line.dirty = true;
                }
                continue;
            }

            if set.len() == self.associativity {
                let victim = set
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, line)| line.last_used)
                    .map(|(index, _)| index)
                    .unwrap_or(0);
                let victim = set.swap_remove(victim);
                if victim.dirty {
                    traffic.bytes_store = traffic.bytes_store.saturating_add(self.line_size);
                }
            }
            set.push(CacheLine {
                tag,
                last_used: self.clock,
                dirty: store,
            });
            // A cold write uses ordinary write allocation. The instrumentation
            // layers do not currently expose enough instruction semantics to
            // identify every non-temporal store, so those are conservatively
            // modeled here.
            traffic.bytes_load = traffic.bytes_load.saturating_add(self.line_size);
        }
        traffic
    }

    pub fn flush(&mut self) -> MemoryTraffic {
        let dirty = self
            .sets
            .iter()
            .flat_map(|set| set.iter())
            .filter(|line| line.dirty)
            .count() as u64;
        for set in &mut self.sets {
            set.clear();
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
