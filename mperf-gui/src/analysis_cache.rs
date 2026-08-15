use std::{cell::RefCell, collections::BTreeSet, rc::Rc};

use flamelens::flame::StackIdentifier;

use crate::{
    MperfGui,
    profile::TimeRange,
    profile_analysis::{
        CallTree, CounterTracks, CpuUtilizationHeatmap, FlameScopeHeatmap, FunctionAnalysis,
        TimeOrderTimeline,
    },
    views::flame_canvas::FlameChart,
};

/// The global sample filter inputs that analyses depend on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FilterKey {
    pub range: Option<TimeRange>,
    pub frame_ids: Option<BTreeSet<usize>>,
}

struct Slot<K, V>(RefCell<Option<(K, V)>>);

impl<K: PartialEq, V: Clone> Slot<K, V> {
    fn get_or_insert(&self, key: K, build: impl FnOnce() -> V) -> V {
        if let Some((cached_key, value)) = self.0.borrow().as_ref()
            && *cached_key == key
        {
            return value.clone();
        }
        let value = build();
        *self.0.borrow_mut() = Some((key, value.clone()));
        value
    }
}

impl<K, V> Default for Slot<K, V> {
    fn default() -> Self {
        Self(RefCell::new(None))
    }
}

/// Memoized per-recording analyses. Rebuilding these on every render frame is
/// what made large recordings unusable, so views must go through these getters.
#[derive(Default)]
pub(crate) struct AnalysisCaches {
    call_tree: Slot<FilterKey, Rc<CallTree>>,
    function_analysis: Slot<FilterKey, Rc<FunctionAnalysis>>,
    flamescope: Slot<FilterKey, Option<Rc<FlameScopeHeatmap>>>,
    timeline: Slot<FilterKey, Option<Rc<TimeOrderTimeline>>>,
    cpu_heatmap: Slot<(FilterKey, Option<u64>), Option<Rc<CpuUtilizationHeatmap>>>,
    counter_tracks: Slot<FilterKey, Option<Rc<CounterTracks>>>,
    flame_chart: Slot<(bool, StackIdentifier), Option<Rc<FlameChart>>>,
    filtered_flame_chart: Slot<FilterKey, Rc<FlameChart>>,
    icicle_chart: Slot<(FilterKey, Vec<usize>), Rc<FlameChart>>,
}

impl AnalysisCaches {
    pub fn clear(&self) {
        *self.call_tree.0.borrow_mut() = None;
        *self.function_analysis.0.borrow_mut() = None;
        *self.flamescope.0.borrow_mut() = None;
        *self.timeline.0.borrow_mut() = None;
        *self.cpu_heatmap.0.borrow_mut() = None;
        *self.counter_tracks.0.borrow_mut() = None;
        *self.flame_chart.0.borrow_mut() = None;
        *self.filtered_flame_chart.0.borrow_mut() = None;
        *self.icicle_chart.0.borrow_mut() = None;
    }
}

impl MperfGui {
    fn filter_key(&self) -> FilterKey {
        let filter = self.profile_sample_filter();
        FilterKey {
            range: filter.range,
            frame_ids: filter.frame_ids,
        }
    }

    pub(crate) fn cached_call_tree(&self) -> Option<Rc<CallTree>> {
        let profile = &self.model.as_ref()?.profile;
        Some(self.caches.call_tree.get_or_insert(self.filter_key(), || {
            Rc::new(CallTree::build(profile, &self.profile_sample_filter()))
        }))
    }

    pub(crate) fn cached_function_analysis(&self) -> Option<Rc<FunctionAnalysis>> {
        let profile = &self.model.as_ref()?.profile;
        Some(
            self.caches
                .function_analysis
                .get_or_insert(self.filter_key(), || {
                    Rc::new(FunctionAnalysis::build(
                        profile,
                        &self.profile_sample_filter(),
                    ))
                }),
        )
    }

    /// FlameScope keeps the folded time domain visible, so its filter drops
    /// the selected time range.
    pub(crate) fn cached_flamescope(&self, max_columns: usize) -> Option<Rc<FlameScopeHeatmap>> {
        let profile = &self.model.as_ref()?.profile;
        let mut filter = self.profile_sample_filter();
        filter.range = None;
        let key = FilterKey {
            range: None,
            frame_ids: filter.frame_ids.clone(),
        };
        self.caches.flamescope.get_or_insert(key, || {
            FlameScopeHeatmap::build(profile, &filter, max_columns).map(Rc::new)
        })
    }

    pub(crate) fn cached_timeline(&self, max_bins: usize) -> Option<Rc<TimeOrderTimeline>> {
        let profile = &self.model.as_ref()?.profile;
        self.caches.timeline.get_or_insert(self.filter_key(), || {
            TimeOrderTimeline::build(profile, &self.profile_sample_filter(), max_bins).map(Rc::new)
        })
    }

    pub(crate) fn cached_cpu_heatmap(
        &self,
        max_buckets: usize,
        bucket_duration_ns: Option<u64>,
    ) -> Option<Rc<CpuUtilizationHeatmap>> {
        let profile = &self.model.as_ref()?.profile;
        self.caches
            .cpu_heatmap
            .get_or_insert((self.filter_key(), bucket_duration_ns), || {
                CpuUtilizationHeatmap::build_with_bucket_duration(
                    profile,
                    &self.profile_sample_filter(),
                    max_buckets,
                    bucket_duration_ns,
                )
                .map(Rc::new)
            })
    }

    pub(crate) fn cached_counter_tracks(&self, max_bins: usize) -> Option<Rc<CounterTracks>> {
        let profile = &self.model.as_ref()?.profile;
        self.caches
            .counter_tracks
            .get_or_insert(self.filter_key(), || {
                CounterTracks::build(profile, &self.profile_sample_filter(), max_bins).map(Rc::new)
            })
    }

    pub(crate) fn cached_flame_chart(&self) -> Option<Rc<FlameChart>> {
        let flamegraph = self.flamegraph_data()?;
        self.caches.flame_chart.get_or_insert(
            (self.flamegraph_instructions, self.flamegraph_zoom),
            || {
                flamegraph
                    .layout(self.flamegraph_instructions, self.flamegraph_zoom)
                    .map(|layout| Rc::new(FlameChart::from_flame_layout(&layout)))
            },
        )
    }

    pub(crate) fn cached_filtered_flame_chart(&self) -> Option<Rc<FlameChart>> {
        let tree = self.cached_call_tree()?;
        Some(
            self.caches
                .filtered_flame_chart
                .get_or_insert(self.filter_key(), || {
                    Rc::new(FlameChart::from_icicle_layout(
                        &tree.icicle_layout(None),
                        &tree,
                        true,
                    ))
                }),
        )
    }

    pub(crate) fn cached_icicle_chart(&self) -> Option<Rc<FlameChart>> {
        let tree = self.cached_call_tree()?;
        Some(self.caches.icicle_chart.get_or_insert(
            (self.filter_key(), self.icicle_focus_path.clone()),
            || {
                let focus =
                    crate::views::flame_canvas::find_focus_node(&tree, &self.icicle_focus_path);
                Rc::new(FlameChart::from_icicle_layout(
                    &tree.icicle_layout(focus),
                    &tree,
                    false,
                ))
            },
        ))
    }
}
