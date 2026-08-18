use std::sync::Arc;

/// A background-computed dataset tagged with the key (filter generation plus
/// view options) it was built for. A stale value keeps rendering until its
/// replacement lands, so filter tweaks never blank a view, and results that
/// arrive after a newer request are dropped.
pub(super) struct Derived<T> {
    value: Option<Arc<T>>,
    key: Option<u64>,
    inflight: Option<u64>,
}

impl<T> Default for Derived<T> {
    fn default() -> Self {
        Self {
            value: None,
            key: None,
            inflight: None,
        }
    }
}

impl<T> Derived<T> {
    /// True when neither the current value nor an in-flight compute is for `key`.
    pub fn needs(&self, key: u64) -> bool {
        self.key != Some(key) && self.inflight != Some(key)
    }

    pub fn begin(&mut self, key: u64) {
        self.inflight = Some(key);
    }

    pub fn install(&mut self, key: u64, value: Arc<T>) -> bool {
        if self.inflight != Some(key) {
            return false;
        }
        self.value = Some(value);
        self.key = Some(key);
        self.inflight = None;
        true
    }

    /// Marks an in-flight compute as finished with no value, so a view can
    /// tell "still computing" from "nothing to show".
    pub fn discard(&mut self, key: u64) {
        if self.inflight == Some(key) {
            self.inflight = None;
        }
    }

    pub fn is_computing(&self) -> bool {
        self.inflight.is_some()
    }

    /// The last computed value, even when a newer key is pending.
    pub fn latest(&self) -> Option<&Arc<T>> {
        self.value.as_ref()
    }

    pub fn stale(&self, key: u64) -> bool {
        self.key != Some(key)
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_late_result_for_an_older_key_is_dropped() {
        let mut derived = Derived::<u32>::default();
        derived.begin(1);
        derived.begin(2);

        assert!(!derived.install(1, Arc::new(10)));
        assert!(derived.install(2, Arc::new(20)));
        assert_eq!(derived.latest().map(|value| **value), Some(20));
        assert!(!derived.stale(2));
    }

    #[test]
    fn a_stale_value_stays_readable_while_the_next_one_computes() {
        let mut derived = Derived::<u32>::default();
        derived.begin(1);
        derived.install(1, Arc::new(10));

        assert!(derived.needs(2));
        derived.begin(2);
        assert!(!derived.needs(2));
        assert!(derived.stale(2));
        assert_eq!(derived.latest().map(|value| **value), Some(10));
    }
}
