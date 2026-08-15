use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

use crate::profile::{
    CounterMetric, CpuObservation, CpuObservationSource, ProfileData, ProfileFrame, ProfileSample,
    TimeRange,
};

const NANOS_PER_SECOND: u64 = 1_000_000_000;
/// Never fold into fewer rows than this, however sparse the recording is.
const MIN_FOLD_BINS: u64 = 12;

/// A common sample filter used by every profile analysis.
///
/// Time ranges are half-open. `required_counter` filters by counter presence
/// and a non-zero finite value, but every accepted row still contributes one
/// sample. Counter deltas are never treated as sample weights.
#[derive(Debug, Default)]
pub(crate) struct SampleFilter {
    pub range: Option<TimeRange>,
    pub process_id: Option<u32>,
    pub thread_id: Option<u32>,
    /// Accept only samples from these threads; `None` accepts every thread.
    pub threads: Option<BTreeSet<u32>>,
    pub cpu: Option<u32>,
    pub required_counter: Option<usize>,
    /// Accept a sample when its stack contains at least one selected frame.
    pub frame_ids: Option<BTreeSet<usize>>,
    /// Accept a sample when its leaf frame is in this set (module scoping).
    pub leaf_frames: Option<BTreeSet<usize>>,
}

impl SampleFilter {
    pub fn matches(&self, sample: &ProfileSample) -> bool {
        if let Some(range) = &self.range
            && (sample.timestamp_ns < range.start_ns || sample.timestamp_ns >= range.end_ns)
        {
            return false;
        }
        self.matches_without_time(sample)
    }

    fn matches_without_time(&self, sample: &ProfileSample) -> bool {
        if self
            .process_id
            .is_some_and(|process_id| sample.process_id != process_id)
        {
            return false;
        }
        if self
            .thread_id
            .is_some_and(|thread_id| sample.thread_id != thread_id)
        {
            return false;
        }
        if self
            .threads
            .as_ref()
            .is_some_and(|threads| !threads.contains(&sample.thread_id))
        {
            return false;
        }
        if self.cpu.is_some_and(|cpu| sample.cpu != Some(cpu)) {
            return false;
        }
        if self.frame_ids.as_ref().is_some_and(|frame_ids| {
            !sample
                .stack
                .iter()
                .any(|frame_id| frame_ids.contains(frame_id))
        }) {
            return false;
        }
        if self.leaf_frames.as_ref().is_some_and(|leaf_frames| {
            !sample
                .stack
                .last()
                .is_some_and(|frame_id| leaf_frames.contains(frame_id))
        }) {
            return false;
        }
        if let Some(counter) = self.required_counter {
            return sample
                .counters
                .get(counter)
                .and_then(|value| *value)
                .is_some_and(|value| value.is_finite() && value != 0.0);
        }
        true
    }

    fn matches_cpu_observation_without_time(&self, observation: &CpuObservation) -> bool {
        if self
            .process_id
            .is_some_and(|process_id| observation.process_id != process_id)
        {
            return false;
        }
        if self
            .thread_id
            .is_some_and(|thread_id| observation.thread_id != thread_id)
        {
            return false;
        }
        if self
            .threads
            .as_ref()
            .is_some_and(|threads| !threads.contains(&observation.thread_id))
        {
            return false;
        }
        if self.cpu.is_some_and(|cpu| observation.cpu != Some(cpu)) {
            return false;
        }
        if self.frame_ids.as_ref().is_some_and(|frame_ids| {
            !observation
                .stack
                .iter()
                .any(|frame_id| frame_ids.contains(frame_id))
        }) {
            return false;
        }
        if self.leaf_frames.as_ref().is_some_and(|leaf_frames| {
            !observation
                .stack
                .last()
                .is_some_and(|frame_id| leaf_frames.contains(frame_id))
        }) {
            return false;
        }
        true
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FlameScopeBin {
    /// Number of accepted sampling observations in this cell.
    pub samples: u64,
}

/// A FlameScope-style folded heatmap.
///
/// Rows are consecutive seconds of the selected recording range and columns
/// are adaptive sub-second buckets. The grid stores sample density, not PMU
/// counter deltas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FlameScopeHeatmap {
    pub range: TimeRange,
    /// Time folded into one row (classically one second, shortened for short
    /// recordings so the grid keeps a usable number of columns).
    pub fold_ns: u64,
    pub bin_width_ns: u64,
    pub rows: usize,
    pub columns: usize,
    pub bins: Vec<FlameScopeBin>,
    pub total_samples: u64,
    pub max_samples: u64,
}

impl FlameScopeHeatmap {
    pub fn build(profile: &ProfileData, filter: &SampleFilter, max_bins: usize) -> Option<Self> {
        let range = analysis_range(profile, filter)?;
        let duration = range.end_ns.saturating_sub(range.start_ns);
        if duration == 0 {
            return None;
        }

        let fold_ns = fold_period(duration);
        let rows = usize::try_from(div_ceil(duration, fold_ns))
            .unwrap_or(usize::MAX)
            .max(1);

        // Bin height follows sample density, not just time: a grid finer than
        // the samples can fill reads as noise, not as a heatmap.
        let accepted = profile
            .samples
            .iter()
            .filter(|sample| filter.matches(sample))
            .filter(|sample| {
                sample.timestamp_ns >= range.start_ns && sample.timestamp_ns < range.end_ns
            })
            .count() as u64;
        let cap = max_bins.max(1) as u64;
        let target_bins = (accepted / rows as u64 / 2).clamp(MIN_FOLD_BINS.min(cap), cap);
        let bin_width_ns = nice_bin_width(div_ceil(fold_ns, target_bins)).min(fold_ns).max(1);
        let columns = usize::try_from(div_ceil(fold_ns, bin_width_ns))
            .unwrap_or(usize::MAX)
            .max(1);
        let cell_count = rows.checked_mul(columns)?;
        let mut bins = vec![FlameScopeBin::default(); cell_count];
        let mut total_samples = 0u64;

        for sample in profile
            .samples
            .iter()
            .filter(|sample| filter.matches(sample))
        {
            if sample.timestamp_ns < range.start_ns || sample.timestamp_ns >= range.end_ns {
                continue;
            }
            let relative = sample.timestamp_ns - range.start_ns;
            let row = usize::try_from(relative / fold_ns).ok()?;
            let within_fold = relative % fold_ns;
            let column = usize::try_from(within_fold / bin_width_ns).ok()?;
            let Some(bin) = row
                .checked_mul(columns)
                .and_then(|index| index.checked_add(column))
                .and_then(|index| bins.get_mut(index))
            else {
                continue;
            };
            bin.samples = bin.samples.saturating_add(1);
            total_samples = total_samples.saturating_add(1);
        }

        let max_samples = bins.iter().map(|bin| bin.samples).max().unwrap_or(0);
        Some(Self {
            range,
            fold_ns,
            bin_width_ns,
            rows,
            columns,
            bins,
            total_samples,
            max_samples,
        })
    }

    pub fn bin(&self, row: usize, column: usize) -> Option<&FlameScopeBin> {
        row.checked_mul(self.columns)
            .and_then(|index| index.checked_add(column))
            .and_then(|index| self.bins.get(index))
    }

    /// Converts a rectangular heatmap selection into its disjoint time ranges.
    ///
    /// A folded row is a separate second, so selecting the same sub-second
    /// columns across several rows intentionally returns one range per row.
    pub fn selected_ranges(&self, rows: Range<usize>, columns: Range<usize>) -> Vec<TimeRange> {
        let row_start = rows.start.min(self.rows);
        let row_end = rows.end.min(self.rows);
        let column_start = columns.start.min(self.columns);
        let column_end = columns.end.min(self.columns);
        if row_start >= row_end || column_start >= column_end {
            return Vec::new();
        }

        (row_start..row_end)
            .filter_map(|row| {
                let fold_start = self
                    .range
                    .start_ns
                    .saturating_add((row as u64).saturating_mul(self.fold_ns));
                let start_ns = fold_start
                    .saturating_add((column_start as u64).saturating_mul(self.bin_width_ns));
                let end_ns = fold_start
                    .saturating_add((column_end as u64).saturating_mul(self.bin_width_ns))
                    .min(fold_start.saturating_add(self.fold_ns))
                    .min(self.range.end_ns);
                (start_ns < end_ns).then_some(TimeRange { start_ns, end_ns })
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CallTreeNode {
    pub id: usize,
    pub frame_id: Option<usize>,
    pub label: String,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub depth: usize,
    pub inclusive_samples: u64,
    pub self_samples: u64,
}

/// A deterministic top-down call tree. Each accepted profile row has weight one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CallTree {
    pub nodes: Vec<CallTreeNode>,
    pub root: usize,
    pub total_samples: u64,
}

/// How much one accepted sample contributes to a call-tree node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StackWeight {
    /// Every sample weighs one — the cycles-proportional default.
    Samples,
    /// The sample's value for `counter_metrics[index]`, rounded down.
    Counter(usize),
}

impl StackWeight {
    fn of(self, sample: &ProfileSample) -> u64 {
        match self {
            Self::Samples => 1,
            Self::Counter(index) => sample
                .counters
                .get(index)
                .copied()
                .flatten()
                .filter(|value| value.is_finite() && *value > 0.0)
                .map(|value| value as u64)
                .unwrap_or(0),
        }
    }
}

impl CallTree {
    pub fn build(profile: &ProfileData, filter: &SampleFilter) -> Self {
        Self::build_weighted(profile, filter, false, StackWeight::Samples)
    }

    /// `inverted` reverses every stack, turning the tree bottom-up (leaves at
    /// the root); `weight` scales each sample's contribution.
    pub fn build_weighted(
        profile: &ProfileData,
        filter: &SampleFilter,
        inverted: bool,
        weight: StackWeight,
    ) -> Self {
        let labels = frame_labels(&profile.frames);
        let mut samples = profile
            .samples
            .iter()
            .filter(|sample| filter.matches(sample))
            .collect::<Vec<_>>();
        samples.sort_by(|left, right| {
            (
                left.timestamp_ns,
                left.process_id,
                left.thread_id,
                left.cpu,
                &left.stack,
            )
                .cmp(&(
                    right.timestamp_ns,
                    right.process_id,
                    right.thread_id,
                    right.cpu,
                    &right.stack,
                ))
        });

        let mut nodes = vec![CallTreeNode {
            id: 0,
            frame_id: None,
            label: "All samples".to_owned(),
            parent: None,
            children: Vec::new(),
            depth: 0,
            inclusive_samples: 0,
            self_samples: 0,
        }];
        let mut child_lookup = BTreeMap::<(usize, usize), usize>::new();

        for sample in samples {
            let value = weight.of(sample);
            if value == 0 {
                continue;
            }
            nodes[0].inclusive_samples = nodes[0].inclusive_samples.saturating_add(value);
            let mut parent = 0usize;
            let stack: Vec<usize> = if inverted {
                sample.stack.iter().rev().copied().collect()
            } else {
                sample.stack.clone()
            };
            for (depth, frame_id) in stack.into_iter().enumerate() {
                let node_id = if let Some(node_id) = child_lookup.get(&(parent, frame_id)) {
                    *node_id
                } else {
                    let node_id = nodes.len();
                    nodes.push(CallTreeNode {
                        id: node_id,
                        frame_id: Some(frame_id),
                        label: labels
                            .get(&frame_id)
                            .cloned()
                            .unwrap_or_else(|| format!("Unknown frame {frame_id}")),
                        parent: Some(parent),
                        children: Vec::new(),
                        depth: depth + 1,
                        inclusive_samples: 0,
                        self_samples: 0,
                    });
                    nodes[parent].children.push(node_id);
                    child_lookup.insert((parent, frame_id), node_id);
                    node_id
                };
                nodes[node_id].inclusive_samples =
                    nodes[node_id].inclusive_samples.saturating_add(value);
                parent = node_id;
            }
            nodes[parent].self_samples = nodes[parent].self_samples.saturating_add(value);
        }

        for node_id in 0..nodes.len() {
            let mut children = std::mem::take(&mut nodes[node_id].children);
            children.sort_by(|left, right| {
                Reverse(nodes[*left].inclusive_samples)
                    .cmp(&Reverse(nodes[*right].inclusive_samples))
                    .then_with(|| nodes[*left].label.cmp(&nodes[*right].label))
                    .then_with(|| nodes[*left].frame_id.cmp(&nodes[*right].frame_id))
            });
            nodes[node_id].children = children;
        }

        let total_samples = nodes[0].inclusive_samples;
        Self {
            nodes,
            root: 0,
            total_samples,
        }
    }

    pub fn focus_path(&self, focus_node: usize) -> Vec<usize> {
        let mut path = Vec::new();
        let mut current = self.nodes.get(focus_node).map(|node| node.id);
        while let Some(node_id) = current {
            path.push(node_id);
            current = self.nodes[node_id].parent;
        }
        path.reverse();
        path
    }

    pub fn icicle_layout(&self, focus_node: Option<usize>) -> IcicleLayout {
        let focus_node = focus_node
            .filter(|node| *node < self.nodes.len())
            .unwrap_or(self.root);
        let total_samples = self.nodes[focus_node].inclusive_samples;
        let mut frames = Vec::new();
        let mut max_depth = 0usize;
        if total_samples > 0 {
            self.layout_node(focus_node, 0.0, 1.0, 0, &mut max_depth, &mut frames);
        }
        IcicleLayout {
            focus_node,
            focus_path: self.focus_path(focus_node),
            total_samples,
            max_depth,
            frames,
        }
    }

    fn layout_node(
        &self,
        node_id: usize,
        x: f64,
        width: f64,
        depth: usize,
        max_depth: &mut usize,
        frames: &mut Vec<IcicleFrame>,
    ) {
        let node = &self.nodes[node_id];
        *max_depth = (*max_depth).max(depth);
        frames.push(IcicleFrame {
            node_id,
            frame_id: node.frame_id,
            label: node.label.clone(),
            x,
            width,
            depth,
            inclusive_samples: node.inclusive_samples,
            self_samples: node.self_samples,
        });

        if node.inclusive_samples == 0 {
            return;
        }
        let mut child_x = x;
        for child_id in &node.children {
            let child = &self.nodes[*child_id];
            let child_width =
                width * child.inclusive_samples as f64 / node.inclusive_samples as f64;
            self.layout_node(
                *child_id,
                child_x,
                child_width,
                depth + 1,
                max_depth,
                frames,
            );
            child_x += child_width;
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct IcicleFrame {
    pub node_id: usize,
    pub frame_id: Option<usize>,
    pub label: String,
    pub x: f64,
    pub width: f64,
    pub depth: usize,
    pub inclusive_samples: u64,
    pub self_samples: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct IcicleLayout {
    pub focus_node: usize,
    pub focus_path: Vec<usize>,
    pub total_samples: u64,
    pub max_depth: usize,
    pub frames: Vec<IcicleFrame>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FunctionStat {
    pub frame_id: usize,
    pub label: String,
    pub inclusive_samples: u64,
    pub self_samples: u64,
    pub inclusive_fraction: f64,
    pub self_fraction: f64,
    pub metrics: FunctionMetrics,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FunctionMetrics {
    pub cpu_time_ns: Option<f64>,
    pub cycles: Option<f64>,
    pub instructions: Option<f64>,
    pub ipc: Option<f64>,
    pub llc_miss_rate: Option<f64>,
    pub llc_mpki: Option<f64>,
    pub backend_stall_fraction: Option<f64>,
    pub branch_mpki: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FunctionRelation {
    pub frame_id: usize,
    pub label: String,
    pub samples: u64,
    pub fraction_of_function: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FunctionDetails {
    pub function: FunctionStat,
    pub callers: Vec<FunctionRelation>,
    pub callees: Vec<FunctionRelation>,
}

/// Function-level self/inclusive sample counts and direct call-edge counts.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FunctionAnalysis {
    pub total_samples: u64,
    pub functions: Vec<FunctionStat>,
    labels: BTreeMap<usize, String>,
    edges: BTreeMap<(usize, usize), u64>,
}

impl FunctionAnalysis {
    pub fn build(profile: &ProfileData, filter: &SampleFilter) -> Self {
        let labels = frame_labels(&profile.frames);
        let confidence_scaled = profile
            .counter_metrics
            .iter()
            .map(|metric| confidence_scaled_metric(&metric.key))
            .collect::<Vec<_>>();
        let mut inclusive = BTreeMap::<usize, u64>::new();
        let mut self_counts = BTreeMap::<usize, u64>::new();
        let mut edges = BTreeMap::<(usize, usize), u64>::new();
        let mut counter_sums = BTreeMap::<usize, Vec<Option<f64>>>::new();
        let mut total_samples = 0u64;
        let mut unique_frames = Vec::new();
        let mut unique_edges = Vec::new();

        for sample in profile
            .samples
            .iter()
            .filter(|sample| filter.matches(sample))
        {
            total_samples = total_samples.saturating_add(1);
            unique_frames.clear();
            unique_frames.extend_from_slice(&sample.stack);
            unique_frames.sort_unstable();
            unique_frames.dedup();
            for frame_id in &unique_frames {
                *inclusive.entry(*frame_id).or_default() += 1;
            }
            if let Some(frame_id) = sample.stack.last() {
                *self_counts.entry(*frame_id).or_default() += 1;
                let sums = counter_sums
                    .entry(*frame_id)
                    .or_insert_with(|| vec![None; profile.counter_metrics.len()]);
                for ((sum, value), scaled) in sums
                    .iter_mut()
                    .zip(&sample.counters)
                    .zip(&confidence_scaled)
                {
                    if let Some(value) = value.filter(|value| value.is_finite()) {
                        let value = if *scaled {
                            value / sample.confidence
                        } else {
                            value
                        };
                        *sum = Some(sum.unwrap_or_default() + value);
                    }
                }
            }
            unique_edges.clear();
            unique_edges.extend(sample.stack.windows(2).map(|frames| (frames[0], frames[1])));
            unique_edges.sort_unstable();
            unique_edges.dedup();
            for edge in &unique_edges {
                *edges.entry(*edge).or_default() += 1;
            }
        }

        let metric_indices = MetricIndices::resolve(&profile.counter_metrics);
        let mut functions = inclusive
            .into_iter()
            .map(|(frame_id, inclusive_samples)| {
                let self_samples = self_counts.get(&frame_id).copied().unwrap_or(0);
                FunctionStat {
                    frame_id,
                    label: labels
                        .get(&frame_id)
                        .cloned()
                        .unwrap_or_else(|| format!("Unknown frame {frame_id}")),
                    inclusive_samples,
                    self_samples,
                    inclusive_fraction: fraction(inclusive_samples, total_samples),
                    self_fraction: fraction(self_samples, total_samples),
                    metrics: function_metrics(
                        counter_sums.get(&frame_id).map(Vec::as_slice),
                        metric_indices,
                    ),
                }
            })
            .collect::<Vec<_>>();
        functions.sort_by(|left, right| {
            Reverse(left.inclusive_samples)
                .cmp(&Reverse(right.inclusive_samples))
                .then_with(|| Reverse(left.self_samples).cmp(&Reverse(right.self_samples)))
                .then_with(|| left.label.cmp(&right.label))
                .then_with(|| left.frame_id.cmp(&right.frame_id))
        });

        Self {
            total_samples,
            functions,
            labels,
            edges,
        }
    }

    pub fn details(&self, frame_id: usize) -> Option<FunctionDetails> {
        let function = self
            .functions
            .iter()
            .find(|function| function.frame_id == frame_id)?
            .clone();
        let denominator = function.inclusive_samples;
        let mut callers = self
            .edges
            .iter()
            .filter(|((_, callee), _)| *callee == frame_id)
            .map(|((caller, _), samples)| self.relation(*caller, *samples, denominator))
            .collect::<Vec<_>>();
        let mut callees = self
            .edges
            .iter()
            .filter(|((caller, _), _)| *caller == frame_id)
            .map(|((_, callee), samples)| self.relation(*callee, *samples, denominator))
            .collect::<Vec<_>>();
        sort_relations(&mut callers);
        sort_relations(&mut callees);
        Some(FunctionDetails {
            function,
            callers,
            callees,
        })
    }

    fn relation(&self, frame_id: usize, samples: u64, denominator: u64) -> FunctionRelation {
        FunctionRelation {
            frame_id,
            label: self
                .labels
                .get(&frame_id)
                .cloned()
                .unwrap_or_else(|| format!("Unknown frame {frame_id}")),
            samples,
            fraction_of_function: fraction(samples, denominator),
        }
    }
}

/// Counter column positions, resolved once per analysis instead of per function.
#[derive(Clone, Copy, Debug, Default)]
struct MetricIndices {
    cpu_time: Option<usize>,
    cycles: Option<usize>,
    instructions: Option<usize>,
    llc_misses: Option<usize>,
    llc_references: Option<usize>,
    backend_stalls: Option<usize>,
    branch_misses: Option<usize>,
}

impl MetricIndices {
    fn resolve(metrics: &[CounterMetric]) -> Self {
        Self {
            cpu_time: find_metric(metrics, &["os_cpu_clock", "cpu_clock"]),
            cycles: find_metric(metrics, &["cycles", "pmu_cycles"]),
            instructions: find_metric(metrics, &["instructions", "pmu_instructions"]),
            llc_misses: find_metric(metrics, &["llc_misses", "pmu_llc_misses"]),
            llc_references: find_metric(metrics, &["llc_references", "pmu_llc_references"]),
            backend_stalls: find_metric(
                metrics,
                &["stalled_cycles_backend", "pmu_stalled_cycles_backend"],
            ),
            branch_misses: find_metric(metrics, &["branch_misses", "pmu_branch_misses"]),
        }
    }
}

fn function_metrics(sums: Option<&[Option<f64>]>, indices: MetricIndices) -> FunctionMetrics {
    let Some(sums) = sums else {
        return FunctionMetrics::default();
    };
    let value = |index: Option<usize>| index.and_then(|index| sums.get(index).copied().flatten());
    let cycles = value(indices.cycles);
    let instructions = value(indices.instructions);
    let llc_misses = value(indices.llc_misses);
    let llc_references = value(indices.llc_references);
    let backend_stalls = value(indices.backend_stalls);
    let branch_misses = value(indices.branch_misses);
    let ratio = |numerator: Option<f64>, denominator: Option<f64>| {
        numerator
            .zip(denominator)
            .filter(|(_, denominator)| *denominator > 0.0)
            .map(|(numerator, denominator)| numerator / denominator)
            .filter(|value| value.is_finite())
    };

    FunctionMetrics {
        cpu_time_ns: value(indices.cpu_time),
        cycles,
        instructions,
        ipc: ratio(instructions, cycles),
        llc_miss_rate: ratio(
            llc_misses,
            llc_misses
                .zip(llc_references)
                .map(|(misses, references)| misses + references),
        ),
        llc_mpki: ratio(llc_misses.map(|value| value * 1_000.0), instructions),
        backend_stall_fraction: ratio(backend_stalls, cycles),
        branch_mpki: ratio(branch_misses.map(|value| value * 1_000.0), instructions),
    }
}

fn confidence_scaled_metric(key: &str) -> bool {
    let key = normalized_metric_key(key);
    key.contains("llc")
        || key.contains("branch")
        || key.contains("stalledcycles")
        || key.contains("cachemiss")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum TimelineLaneKey {
    Cpu(Option<u32>),
    Thread(u32),
}

impl TimelineLaneKey {
    pub fn label(self) -> String {
        match self {
            Self::Cpu(Some(cpu)) => format!("CPU {cpu}"),
            Self::Cpu(None) => "CPU unknown".to_owned(),
            Self::Thread(thread_id) => format!("Thread {thread_id}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TimeOrderSegment {
    pub depth: usize,
    pub frame_id: usize,
    pub label: String,
    /// Root-to-this-frame path; prevents unrelated contexts from being merged.
    pub path: Vec<usize>,
    pub start_ns: u64,
    pub end_ns: u64,
    /// All accepted observations in the represented time buckets.
    pub samples: u64,
    /// Observations matching the modal stack chosen for downsampling.
    pub representative_samples: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TimeOrderLane {
    pub key: TimelineLaneKey,
    pub label: String,
    pub segments: Vec<TimeOrderSegment>,
}

/// A downsampled time-order stack timeline.
///
/// Each lane/bin chooses the modal complete stack, then adjacent bins with the
/// same call path are merged. CPU lanes are preferred whenever any accepted
/// sample has CPU attribution; legacy recordings fall back to thread lanes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TimeOrderTimeline {
    pub range: TimeRange,
    pub bin_width_ns: u64,
    pub bins: usize,
    pub total_samples: u64,
    pub max_depth: usize,
    pub uses_cpu_lanes: bool,
    pub lanes: Vec<TimeOrderLane>,
}

impl TimeOrderTimeline {
    pub fn build(profile: &ProfileData, filter: &SampleFilter, max_bins: usize) -> Option<Self> {
        let range = analysis_range(profile, filter)?;
        let duration = range.end_ns.saturating_sub(range.start_ns);
        if duration == 0 {
            return None;
        }
        let bin_width_ns = nice_bin_width(div_ceil(duration, max_bins.max(1) as u64)).max(1);
        let bins = usize::try_from(div_ceil(duration, bin_width_ns))
            .unwrap_or(usize::MAX)
            .max(1);
        let accepted = profile
            .samples
            .iter()
            .filter(|sample| filter.matches(sample))
            .filter(|sample| {
                sample.timestamp_ns >= range.start_ns && sample.timestamp_ns < range.end_ns
            })
            .collect::<Vec<_>>();
        let uses_cpu_lanes = accepted.iter().any(|sample| sample.cpu.is_some());
        let labels = frame_labels(&profile.frames);

        let mut bucket_stacks =
            BTreeMap::<(TimelineLaneKey, usize), BTreeMap<Vec<usize>, u64>>::new();
        for sample in &accepted {
            let key = if uses_cpu_lanes {
                TimelineLaneKey::Cpu(sample.cpu)
            } else {
                TimelineLaneKey::Thread(sample.thread_id)
            };
            let bin = usize::try_from((sample.timestamp_ns - range.start_ns) / bin_width_ns)
                .unwrap_or(usize::MAX)
                .min(bins - 1);
            *bucket_stacks
                .entry((key, bin))
                .or_default()
                .entry(sample.stack.clone())
                .or_default() += 1;
        }

        let lane_keys = bucket_stacks
            .keys()
            .map(|(key, _)| *key)
            .collect::<BTreeSet<_>>();
        let mut max_depth = 0usize;
        let lanes = lane_keys
            .into_iter()
            .map(|key| {
                let mut segments = Vec::<TimeOrderSegment>::new();
                let mut active_segments = Vec::<Option<usize>>::new();
                for bin in 0..bins {
                    let Some(stacks) = bucket_stacks.get(&(key, bin)) else {
                        active_segments.fill(None);
                        continue;
                    };
                    let bucket_samples = stacks.values().copied().sum::<u64>();
                    let Some((stack, representative_samples)) = modal_stack(stacks) else {
                        active_segments.fill(None);
                        continue;
                    };
                    max_depth = max_depth.max(stack.len());
                    let start_ns = range
                        .start_ns
                        .saturating_add((bin as u64).saturating_mul(bin_width_ns));
                    let end_ns = start_ns.saturating_add(bin_width_ns).min(range.end_ns);
                    for depth in 0..stack.len() {
                        if active_segments.len() <= depth {
                            active_segments.resize(depth + 1, None);
                        }
                        let path = stack[..=depth].to_vec();
                        let frame_id = stack[depth];
                        let active = active_segments[depth].filter(|segment_index| {
                            segments.get(*segment_index).is_some_and(|segment| {
                                segment.path == path && segment.end_ns == start_ns
                            })
                        });
                        if let Some(segment_index) = active {
                            let segment = &mut segments[segment_index];
                            segment.end_ns = end_ns;
                            segment.samples = segment.samples.saturating_add(bucket_samples);
                            segment.representative_samples = segment
                                .representative_samples
                                .saturating_add(representative_samples);
                        } else {
                            segments.push(TimeOrderSegment {
                                depth,
                                frame_id,
                                label: labels
                                    .get(&frame_id)
                                    .cloned()
                                    .unwrap_or_else(|| format!("Unknown frame {frame_id}")),
                                path,
                                start_ns,
                                end_ns,
                                samples: bucket_samples,
                                representative_samples,
                            });
                            active_segments[depth] = Some(segments.len() - 1);
                        }
                    }
                    for active in active_segments.iter_mut().skip(stack.len()) {
                        *active = None;
                    }
                }
                segments.sort_by_key(|segment| (segment.depth, segment.start_ns, segment.frame_id));
                TimeOrderLane {
                    key,
                    label: key.label(),
                    segments,
                }
            })
            .collect();

        Some(Self {
            range,
            bin_width_ns,
            bins,
            total_samples: accepted.len() as u64,
            max_depth,
            uses_cpu_lanes,
            lanes,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct CpuUtilizationBucket {
    /// Attributed CPU-clock nanoseconds before utilization is capped.
    pub cpu_time_ns: f64,
    /// `cpu_time_ns / wall_bucket_duration`, capped to the physical [0, 1] range.
    pub utilization: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CpuUtilizationLane {
    pub key: TimelineLaneKey,
    pub label: String,
    pub buckets: Vec<CpuUtilizationBucket>,
}

/// CPU-clock utilization or sampled occupancy aligned to wall-time buckets.
///
/// Counter deltas are split over their preceding wall-time interval. Sampled
/// occupancy observations are point-attributed to the CPU reported by that
/// timer hit, avoiding false back-projection across CPU migrations.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CpuUtilizationHeatmap {
    pub range: TimeRange,
    pub bucket_duration_ns: u64,
    pub buckets: usize,
    pub uses_cpu_lanes: bool,
    pub source: CpuObservationSource,
    pub lanes: Vec<CpuUtilizationLane>,
}

impl CpuUtilizationHeatmap {
    #[cfg(test)]
    pub fn build(profile: &ProfileData, filter: &SampleFilter, max_buckets: usize) -> Option<Self> {
        Self::build_with_bucket_duration(profile, filter, max_buckets, None)
    }

    pub fn build_with_bucket_duration(
        profile: &ProfileData,
        filter: &SampleFilter,
        max_buckets: usize,
        bucket_duration_ns: Option<u64>,
    ) -> Option<Self> {
        let range = analysis_range(profile, filter)?;
        let duration = range.end_ns.saturating_sub(range.start_ns);
        if duration == 0 {
            return None;
        }
        let bucket_duration_ns = match bucket_duration_ns {
            Some(0) => return None,
            Some(duration) => duration,
            None => nice_bin_width(div_ceil(duration, max_buckets.max(1) as u64)).max(1),
        };
        let buckets = usize::try_from(div_ceil(duration, bucket_duration_ns))
            .unwrap_or(usize::MAX)
            .max(1);

        let eligible = profile
            .cpu_observations
            .iter()
            .filter(|observation| filter.matches_cpu_observation_without_time(observation))
            .filter(|observation| {
                if observation.weight_ns == 0 {
                    return false;
                }
                match observation.source {
                    CpuObservationSource::SampledOccupancy => {
                        observation.timestamp_ns >= range.start_ns
                            && observation.timestamp_ns < range.end_ns
                    }
                    CpuObservationSource::CounterDelta | CpuObservationSource::LegacyUnknown => {
                        observation
                            .wall_interval_start_ns()
                            .is_some_and(|interval_start| {
                                interval_start < observation.timestamp_ns
                                    && interval_start < range.end_ns
                                    && observation.timestamp_ns > range.start_ns
                            })
                    }
                }
            })
            .collect::<Vec<_>>();
        if eligible.is_empty() {
            return None;
        }
        let source = eligible
            .iter()
            .map(|observation| observation.source)
            .reduce(|left, right| {
                if left == right {
                    left
                } else {
                    CpuObservationSource::LegacyUnknown
                }
            })
            .unwrap_or(CpuObservationSource::LegacyUnknown);
        let uses_cpu_lanes =
            !eligible.is_empty() && eligible.iter().all(|observation| observation.cpu.is_some());
        let mut lane_buckets = BTreeMap::<TimelineLaneKey, Vec<f64>>::new();
        if uses_cpu_lanes && let Some(logical_cpu_count) = profile.logical_cpu_count {
            for cpu in 0..logical_cpu_count {
                lane_buckets.insert(TimelineLaneKey::Cpu(Some(cpu)), vec![0.0; buckets]);
            }
        }

        for observation in eligible {
            let key = if uses_cpu_lanes {
                TimelineLaneKey::Cpu(observation.cpu)
            } else {
                TimelineLaneKey::Thread(observation.thread_id)
            };
            let attributed = lane_buckets
                .entry(key)
                .or_insert_with(|| vec![0.0; buckets]);

            if observation.source == CpuObservationSource::SampledOccupancy {
                let bucket = usize::try_from(
                    (observation.timestamp_ns - range.start_ns) / bucket_duration_ns,
                )
                .unwrap_or(usize::MAX)
                .min(buckets - 1);
                attributed[bucket] += observation.weight_ns as f64;
                continue;
            }

            let interval_end = observation.timestamp_ns;
            let Some(interval_start) = observation.wall_interval_start_ns() else {
                continue;
            };
            let interval_duration = interval_end.saturating_sub(interval_start);
            if interval_duration == 0 {
                continue;
            }
            let clipped_start = interval_start.max(range.start_ns);
            let clipped_end = interval_end.min(range.end_ns);
            if clipped_start >= clipped_end {
                continue;
            }
            let first_bucket =
                usize::try_from((clipped_start - range.start_ns) / bucket_duration_ns)
                    .unwrap_or(usize::MAX)
                    .min(buckets - 1);
            let last_bucket =
                usize::try_from((clipped_end - 1 - range.start_ns) / bucket_duration_ns)
                    .unwrap_or(usize::MAX)
                    .min(buckets - 1);
            for (bucket, value) in attributed
                .iter_mut()
                .enumerate()
                .take(last_bucket + 1)
                .skip(first_bucket)
            {
                let bucket_start = range
                    .start_ns
                    .saturating_add((bucket as u64).saturating_mul(bucket_duration_ns));
                let bucket_end = bucket_start
                    .saturating_add(bucket_duration_ns)
                    .min(range.end_ns);
                let overlap_start = clipped_start.max(bucket_start);
                let overlap_end = clipped_end.min(bucket_end);
                if overlap_start < overlap_end {
                    let overlap_ns = overlap_end - overlap_start;
                    *value +=
                        observation.weight_ns as f64 * overlap_ns as f64 / interval_duration as f64;
                }
            }
        }

        if lane_buckets.is_empty() {
            return None;
        }
        let lanes = lane_buckets
            .into_iter()
            .map(|(key, attributed)| CpuUtilizationLane {
                key,
                label: key.label(),
                buckets: attributed
                    .into_iter()
                    .enumerate()
                    .map(|(bucket, cpu_time_ns)| {
                        let bucket_start = range
                            .start_ns
                            .saturating_add((bucket as u64).saturating_mul(bucket_duration_ns));
                        let bucket_end = bucket_start
                            .saturating_add(bucket_duration_ns)
                            .min(range.end_ns);
                        let wall_duration = bucket_end.saturating_sub(bucket_start);
                        let utilization = if wall_duration == 0 {
                            0.0
                        } else {
                            (cpu_time_ns / wall_duration as f64).clamp(0.0, 1.0)
                        };
                        CpuUtilizationBucket {
                            cpu_time_ns,
                            utilization,
                        }
                    })
                    .collect(),
            })
            .collect();

        Some(Self {
            range,
            bucket_duration_ns,
            buckets,
            uses_cpu_lanes,
            source,
            lanes,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CounterAggregation {
    Sum,
    Ratio,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CounterTrack {
    pub key: String,
    pub label: String,
    pub aggregation: CounterAggregation,
    /// Values aligned to `CounterTracks::bins`; missing data remains `None`.
    pub values: Vec<Option<f64>>,
}

/// Time-aligned sums for recorded counters plus a sum(instructions)/sum(cycles)
/// IPC track when both source counters exist.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CounterTracks {
    pub range: TimeRange,
    pub bin_width_ns: u64,
    pub bins: usize,
    pub sample_counts: Vec<u64>,
    pub tracks: Vec<CounterTrack>,
}

impl CounterTracks {
    pub fn build(profile: &ProfileData, filter: &SampleFilter, max_bins: usize) -> Option<Self> {
        let range = analysis_range(profile, filter)?;
        let duration = range.end_ns.saturating_sub(range.start_ns);
        if duration == 0 {
            return None;
        }
        let bin_width_ns = nice_bin_width(div_ceil(duration, max_bins.max(1) as u64)).max(1);
        let bins = usize::try_from(div_ceil(duration, bin_width_ns))
            .unwrap_or(usize::MAX)
            .max(1);
        let metric_count = profile.counter_metrics.len();
        let mut sums = vec![vec![0.0f64; bins]; metric_count];
        let mut present = vec![vec![false; bins]; metric_count];
        let mut sample_counts = vec![0u64; bins];

        for sample in profile
            .samples
            .iter()
            .filter(|sample| filter.matches(sample))
        {
            if sample.timestamp_ns < range.start_ns || sample.timestamp_ns >= range.end_ns {
                continue;
            }
            let bin = usize::try_from((sample.timestamp_ns - range.start_ns) / bin_width_ns)
                .unwrap_or(usize::MAX)
                .min(bins - 1);
            sample_counts[bin] = sample_counts[bin].saturating_add(1);
            for (metric_index, value) in sample.counters.iter().enumerate().take(metric_count) {
                let Some(value) = value.filter(|value| value.is_finite()) else {
                    continue;
                };
                sums[metric_index][bin] += value;
                present[metric_index][bin] = true;
            }
        }

        let mut tracks = profile
            .counter_metrics
            .iter()
            .enumerate()
            .map(|(index, metric)| CounterTrack {
                key: metric.key.clone(),
                label: metric.label.clone(),
                aggregation: CounterAggregation::Sum,
                values: (0..bins)
                    .map(|bin| present[index][bin].then_some(sums[index][bin]))
                    .collect(),
            })
            .collect::<Vec<_>>();

        if let (Some(cycles), Some(instructions)) = (
            find_metric(&profile.counter_metrics, &["cycles", "pmu_cycles"]),
            find_metric(
                &profile.counter_metrics,
                &["instructions", "pmu_instructions"],
            ),
        ) {
            tracks.push(CounterTrack {
                key: "derived.ipc".to_owned(),
                label: "IPC (derived)".to_owned(),
                aggregation: CounterAggregation::Ratio,
                values: (0..bins)
                    .map(|bin| {
                        (present[cycles][bin]
                            && present[instructions][bin]
                            && sums[cycles][bin] != 0.0)
                            .then_some(sums[instructions][bin] / sums[cycles][bin])
                            .filter(|value| value.is_finite())
                    })
                    .collect(),
            });
        }

        Some(Self {
            range,
            bin_width_ns,
            bins,
            sample_counts,
            tracks,
        })
    }

    #[cfg(test)]
    pub fn track(&self, key: &str) -> Option<&CounterTrack> {
        self.tracks.iter().find(|track| track.key == key)
    }
}

fn analysis_range(profile: &ProfileData, filter: &SampleFilter) -> Option<TimeRange> {
    let full = profile.full_range()?;
    let range = if let Some(filtered) = &filter.range {
        TimeRange {
            start_ns: full.start_ns.max(filtered.start_ns),
            end_ns: full.end_ns.min(filtered.end_ns),
        }
    } else {
        full
    };
    (range.start_ns < range.end_ns).then_some(range)
}

fn frame_labels(frames: &[ProfileFrame]) -> BTreeMap<usize, String> {
    frames
        .iter()
        .map(|frame| (frame.id, frame.name.clone()))
        .collect()
}

fn modal_stack(stacks: &BTreeMap<Vec<usize>, u64>) -> Option<(&Vec<usize>, u64)> {
    stacks
        .iter()
        .max_by(|(left_stack, left_count), (right_stack, right_count)| {
            left_count
                .cmp(right_count)
                // `max_by` should pick the lexicographically smaller path on a tie.
                .then_with(|| right_stack.cmp(left_stack))
        })
        .map(|(stack, count)| (stack, *count))
}

fn sort_relations(relations: &mut [FunctionRelation]) {
    relations.sort_by(|left, right| {
        Reverse(left.samples)
            .cmp(&Reverse(right.samples))
            .then_with(|| left.label.cmp(&right.label))
            .then_with(|| left.frame_id.cmp(&right.frame_id))
    });
}

/// Column of the retired-instructions counter, when the recording has one.
pub(crate) fn instructions_metric(metrics: &[CounterMetric]) -> Option<usize> {
    find_metric(metrics, &["instructions", "pmu_instructions"])
}

fn find_metric(metrics: &[CounterMetric], candidates: &[&str]) -> Option<usize> {
    metrics.iter().position(|metric| {
        let key = normalized_metric_key(&metric.key);
        candidates
            .iter()
            .any(|candidate| key == normalized_metric_key(candidate))
    })
}

fn normalized_metric_key(key: &str) -> String {
    key.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn fraction(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn div_ceil(value: u64, divisor: u64) -> u64 {
    if divisor == 0 {
        return u64::MAX;
    }
    value / divisor + u64::from(!value.is_multiple_of(divisor))
}

/// Returns the next 1/2/5 × 10ⁿ interval at or above `minimum`.
/// Time folded into one flame-scope row. One second is the classic choice and
/// stays the cap; shorter recordings fold tighter so the grid keeps roughly
/// sixty columns instead of collapsing into two.
fn fold_period(duration_ns: u64) -> u64 {
    nice_bin_width(div_ceil(duration_ns, 60).max(1))
}

fn nice_bin_width(minimum: u64) -> u64 {
    let minimum = minimum.max(1);
    let mut magnitude = 1u64;
    while magnitude <= minimum / 10 {
        let next = magnitude.saturating_mul(10);
        if next == magnitude {
            break;
        }
        magnitude = next;
    }
    for multiplier in [1u64, 2, 5, 10] {
        let candidate = magnitude.saturating_mul(multiplier);
        if candidate >= minimum {
            return candidate;
        }
    }
    u64::MAX
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(id: usize, name: &str) -> ProfileFrame {
        ProfileFrame {
            id,
            name: name.to_owned(),
            module: None,
            file: None,
            line: None,
        }
    }

    fn metric(key: &str) -> CounterMetric {
        CounterMetric {
            key: key.to_owned(),
            label: key.to_owned(),
        }
    }

    fn sample(
        timestamp_ns: u64,
        thread_id: u32,
        cpu: Option<u32>,
        stack: &[usize],
        counters: &[Option<f64>],
    ) -> ProfileSample {
        ProfileSample {
            timestamp_ns,
            process_id: 7,
            thread_id,
            cpu,
            confidence: 1.0,
            stack: stack.to_vec(),
            counters: counters.to_vec(),
        }
    }

    fn profile(
        frames: Vec<ProfileFrame>,
        samples: Vec<ProfileSample>,
        counter_metrics: Vec<CounterMetric>,
    ) -> ProfileData {
        let cpu_clock_index = counter_metrics
            .iter()
            .position(|metric| metric.key.eq_ignore_ascii_case("os_cpu_clock"));
        let cpu_observations = cpu_clock_index
            .map(|index| {
                samples
                    .iter()
                    .filter_map(|sample| {
                        let weight_ns = sample
                            .counters
                            .get(index)
                            .and_then(|value| *value)
                            .filter(|value| value.is_finite() && *value > 0.0)?
                            .round() as u64;
                        Some(CpuObservation {
                            timestamp_ns: sample.timestamp_ns,
                            process_id: sample.process_id,
                            thread_id: sample.thread_id,
                            cpu: sample.cpu,
                            interval_start_ns: Some(sample.timestamp_ns.saturating_sub(weight_ns)),
                            stack: sample.stack.clone(),
                            weight_ns,
                            source: CpuObservationSource::CounterDelta,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        ProfileData {
            frames,
            samples,
            cpu_observations,
            logical_cpu_count: None,
            counter_metrics,
            error: None,
        }
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn flamescope_grid_follows_duration_then_sample_density() {
        let profile = profile(
            vec![frame(0, "root")],
            vec![
                sample(0, 1, None, &[0], &[Some(500.0)]),
                sample(10_000_000, 1, None, &[0], &[Some(9_000.0)]),
                sample(1_010_000_000, 1, None, &[0], &[Some(7_000_000.0)]),
                sample(1_999_999_999, 1, None, &[0], &[Some(1.0)]),
            ],
            vec![metric("pmu_cycles")],
        );

        let heatmap = FlameScopeHeatmap::build(&profile, &SampleFilter::default(), 100).unwrap();

        // A two-second recording folds at 50ms, and four samples cannot fill a
        // fine grid, so the bins collapse to the density floor.
        assert_eq!(heatmap.fold_ns, 50_000_000);
        assert_eq!((heatmap.rows, heatmap.columns), (40, 10));
        assert_eq!(heatmap.bin_width_ns, 5_000_000);
        assert_eq!(heatmap.total_samples, 4);
        assert_eq!(heatmap.max_samples, 1);
        assert_eq!(heatmap.bin(0, 0).unwrap().samples, 1);
        assert_eq!(heatmap.bin(0, 2).unwrap().samples, 1);
        assert_eq!(heatmap.bin(20, 2).unwrap().samples, 1);
        assert_eq!(heatmap.bin(39, 9).unwrap().samples, 1);

        assert_eq!(
            heatmap.selected_ranges(0..2, 2..3),
            vec![
                TimeRange {
                    start_ns: 10_000_000,
                    end_ns: 15_000_000,
                },
                TimeRange {
                    start_ns: 60_000_000,
                    end_ns: 65_000_000,
                },
            ]
        );
    }

    #[test]
    fn dense_recordings_get_the_full_bin_budget() {
        let samples = (0..6_000)
            .map(|ix| sample(ix * 200_000, 1, None, &[0], &[]))
            .collect::<Vec<_>>();
        let profile = profile(vec![frame(0, "root")], samples, vec![]);

        let heatmap = FlameScopeHeatmap::build(&profile, &SampleFilter::default(), 50).unwrap();

        assert_eq!(heatmap.fold_ns, 20_000_000);
        assert_eq!(heatmap.columns, 40);
        assert_eq!(heatmap.bin_width_ns, 500_000);
    }

    #[test]
    fn sample_filter_checks_counter_presence_but_never_uses_delta_as_weight() {
        let profile = profile(
            vec![frame(0, "root")],
            vec![
                sample(0, 1, None, &[0], &[Some(10_000_000.0)]),
                sample(1, 1, None, &[0], &[Some(0.0)]),
                sample(2, 1, None, &[0], &[None]),
            ],
            vec![metric("pmu_cycles")],
        );
        let filter = SampleFilter {
            required_counter: Some(0),
            ..SampleFilter::default()
        };

        let heatmap = FlameScopeHeatmap::build(&profile, &filter, 10).unwrap();
        assert_eq!(heatmap.total_samples, 1);
        assert_eq!(heatmap.max_samples, 1);
    }

    #[test]
    fn frame_filter_is_shared_by_temporal_and_function_analyses() {
        let profile = profile(
            vec![frame(0, "root"), frame(1, "wanted"), frame(2, "other")],
            vec![
                sample(0, 1, None, &[0, 1], &[]),
                sample(1, 1, None, &[0, 2], &[]),
            ],
            vec![],
        );
        let filter = SampleFilter {
            frame_ids: Some(BTreeSet::from([1])),
            ..SampleFilter::default()
        };

        let heatmap = FlameScopeHeatmap::build(&profile, &filter, 10).unwrap();
        let functions = FunctionAnalysis::build(&profile, &filter);

        assert_eq!(heatmap.total_samples, 1);
        assert_eq!(functions.total_samples, 1);
        assert_eq!(
            functions
                .functions
                .iter()
                .map(|function| function.label.as_str())
                .collect::<Vec<_>>(),
            ["wanted", "root"]
        );
    }

    #[test]
    fn thread_set_filter_accepts_only_selected_threads() {
        let profile = profile(
            vec![frame(0, "root")],
            vec![
                sample(0, 1, None, &[0], &[]),
                sample(1, 2, None, &[0], &[]),
                sample(2, 3, None, &[0], &[]),
            ],
            vec![],
        );
        let filter = SampleFilter {
            threads: Some(BTreeSet::from([1, 3])),
            ..SampleFilter::default()
        };

        let functions = FunctionAnalysis::build(&profile, &filter);
        assert_eq!(functions.total_samples, 2);
    }

    #[test]
    fn leaf_frame_filter_scopes_by_sampled_leaf_not_stack_contents() {
        let profile = profile(
            vec![frame(0, "root"), frame(1, "user_fn"), frame(2, "lib_fn")],
            vec![
                sample(0, 1, None, &[0, 1], &[]),
                sample(1, 1, None, &[0, 1, 2], &[]),
                sample(2, 1, None, &[], &[]),
            ],
            vec![],
        );
        let filter = SampleFilter {
            leaf_frames: Some(BTreeSet::from([1])),
            ..SampleFilter::default()
        };

        let functions = FunctionAnalysis::build(&profile, &filter);
        assert_eq!(functions.total_samples, 1);
        assert!(
            functions
                .functions
                .iter()
                .all(|function| function.label != "lib_fn")
        );
    }

    #[test]
    fn flame_scope_folds_short_recordings_tighter_than_a_second() {
        assert_eq!(fold_period(120 * NANOS_PER_SECOND), 2 * NANOS_PER_SECOND);
        assert_eq!(fold_period(60 * NANOS_PER_SECOND), NANOS_PER_SECOND);
        assert_eq!(fold_period(1_160_000_000), 20_000_000);
        assert_eq!(fold_period(100_000_000), 2_000_000);
    }

    #[test]
    fn inverted_call_tree_roots_at_the_leaves() {
        let profile = profile(
            vec![frame(0, "main"), frame(1, "work"), frame(2, "memcpy")],
            vec![
                sample(0, 1, None, &[0, 1, 2], &[]),
                sample(1, 1, None, &[0, 2], &[]),
                sample(2, 1, None, &[0, 1], &[]),
            ],
            vec![],
        );

        let tree = CallTree::build_weighted(
            &profile,
            &SampleFilter::default(),
            true,
            StackWeight::Samples,
        );

        let roots = &tree.nodes[tree.root].children;
        assert_eq!(tree.nodes[roots[0]].label, "memcpy");
        assert_eq!(tree.nodes[roots[0]].inclusive_samples, 2);
        assert_eq!(tree.nodes[roots[1]].label, "work");
        assert_eq!(tree.nodes[roots[1]].inclusive_samples, 1);
    }

    #[test]
    fn counter_weighted_call_tree_uses_counter_values_not_sample_counts() {
        let profile = profile(
            vec![frame(0, "main"), frame(1, "work")],
            vec![
                sample(0, 1, None, &[0, 1], &[Some(100.0)]),
                sample(1, 1, None, &[0], &[Some(25.0)]),
                sample(2, 1, None, &[0, 1], &[None]),
            ],
            vec![metric("instructions")],
        );
        let weight = StackWeight::Counter(instructions_metric(&profile.counter_metrics).unwrap());

        let tree = CallTree::build_weighted(&profile, &SampleFilter::default(), false, weight);

        assert_eq!(tree.total_samples, 125);
        let work = tree.nodes.iter().find(|node| node.label == "work").unwrap();
        assert_eq!(work.inclusive_samples, 100);
        assert_eq!(work.self_samples, 100);
    }

    #[test]
    fn call_tree_is_left_heavy_proportional_and_focusable() {
        let profile = profile(
            vec![frame(0, "A"), frame(1, "B"), frame(2, "C"), frame(3, "D")],
            vec![
                sample(0, 1, None, &[0, 1], &[]),
                sample(1, 1, None, &[0, 1], &[]),
                sample(2, 1, None, &[0, 2], &[]),
                sample(3, 1, None, &[3], &[]),
            ],
            vec![],
        );

        let tree = CallTree::build(&profile, &SampleFilter::default());
        assert_eq!(tree.total_samples, 4);
        let root_children = &tree.nodes[tree.root].children;
        assert_eq!(tree.nodes[root_children[0]].label, "A");
        assert_eq!(tree.nodes[root_children[1]].label, "D");

        let layout = tree.icicle_layout(None);
        let a = layout
            .frames
            .iter()
            .find(|frame| frame.label == "A")
            .unwrap();
        let d = layout
            .frames
            .iter()
            .find(|frame| frame.label == "D")
            .unwrap();
        assert_close(a.width, 0.75);
        assert_close(d.width, 0.25);

        let b_node = tree.nodes.iter().find(|node| node.label == "B").unwrap().id;
        let focused = tree.icicle_layout(Some(b_node));
        assert_eq!(focused.total_samples, 2);
        assert_eq!(
            focused
                .focus_path
                .iter()
                .map(|node| tree.nodes[*node].label.as_str())
                .collect::<Vec<_>>(),
            ["All samples", "A", "B"]
        );
        assert_eq!(focused.frames[0].width, 1.0);
    }

    #[test]
    fn function_analysis_reports_self_inclusive_callers_and_callees() {
        let profile = profile(
            vec![
                frame(0, "A"),
                frame(1, "B"),
                frame(2, "C"),
                frame(3, "D"),
                frame(4, "X"),
            ],
            vec![
                sample(0, 1, None, &[0, 1, 2], &[]),
                sample(1, 1, None, &[0, 1, 2], &[]),
                sample(2, 1, None, &[0, 1, 3], &[]),
                sample(3, 1, None, &[0, 1], &[]),
                sample(4, 1, None, &[4, 1], &[]),
            ],
            vec![],
        );

        let analysis = FunctionAnalysis::build(&profile, &SampleFilter::default());
        let details = analysis.details(1).unwrap();

        assert_eq!(analysis.total_samples, 5);
        assert_eq!(details.function.inclusive_samples, 5);
        assert_eq!(details.function.self_samples, 2);
        assert_eq!(
            details
                .callers
                .iter()
                .map(|relation| (relation.label.as_str(), relation.samples))
                .collect::<Vec<_>>(),
            [("A", 4), ("X", 1)]
        );
        assert_eq!(
            details
                .callees
                .iter()
                .map(|relation| (relation.label.as_str(), relation.samples))
                .collect::<Vec<_>>(),
            [("C", 2), ("D", 1)]
        );
        assert_close(details.callers[0].fraction_of_function, 0.8);
    }

    #[test]
    fn function_analysis_attributes_useful_counter_metrics_to_the_sampled_leaf() {
        let mut profile = profile(
            vec![frame(0, "caller"), frame(1, "leaf")],
            vec![
                sample(
                    0,
                    1,
                    None,
                    &[0, 1],
                    &[
                        Some(10.0),
                        Some(100.0),
                        Some(200.0),
                        Some(4.0),
                        Some(16.0),
                        Some(25.0),
                        Some(2.0),
                    ],
                ),
                sample(
                    1,
                    1,
                    None,
                    &[0, 1],
                    &[
                        Some(20.0),
                        Some(300.0),
                        Some(300.0),
                        Some(6.0),
                        Some(24.0),
                        Some(75.0),
                        Some(3.0),
                    ],
                ),
            ],
            vec![
                metric("os_cpu_clock"),
                metric("pmu_cycles"),
                metric("pmu_instructions"),
                metric("pmu_llc_misses"),
                metric("pmu_llc_references"),
                metric("pmu_stalled_cycles_backend"),
                metric("pmu_branch_misses"),
            ],
        );
        profile.samples[1].confidence = 0.5;

        let analysis = FunctionAnalysis::build(&profile, &SampleFilter::default());
        let caller = analysis
            .functions
            .iter()
            .find(|row| row.frame_id == 0)
            .unwrap();
        let leaf = analysis
            .functions
            .iter()
            .find(|row| row.frame_id == 1)
            .unwrap();

        assert_eq!(caller.metrics, FunctionMetrics::default());
        assert_close(leaf.metrics.cpu_time_ns.unwrap(), 30.0);
        assert_close(leaf.metrics.cycles.unwrap(), 400.0);
        assert_close(leaf.metrics.instructions.unwrap(), 500.0);
        assert_close(leaf.metrics.ipc.unwrap(), 1.25);
        assert_close(leaf.metrics.llc_miss_rate.unwrap(), 0.2);
        assert_close(leaf.metrics.llc_mpki.unwrap(), 32.0);
        assert_close(leaf.metrics.backend_stall_fraction.unwrap(), 0.4375);
        assert_close(leaf.metrics.branch_mpki.unwrap(), 16.0);
    }

    #[test]
    fn recursive_function_counts_once_per_sample_at_function_level() {
        let profile = profile(
            vec![frame(0, "recursive")],
            vec![sample(0, 1, None, &[0, 0], &[])],
            vec![],
        );
        let analysis = FunctionAnalysis::build(&profile, &SampleFilter::default());
        let details = analysis.details(0).unwrap();

        assert_eq!(details.function.inclusive_samples, 1);
        assert_eq!(details.function.self_samples, 1);
        assert_eq!(details.callers[0].samples, 1);
        assert_eq!(details.callees[0].samples, 1);
    }

    #[test]
    fn time_order_prefers_cpu_lanes_and_merges_identical_paths() {
        let profile = profile(
            vec![frame(0, "A"), frame(1, "B"), frame(2, "C")],
            vec![
                sample(0, 10, Some(2), &[0, 1], &[]),
                sample(10, 10, Some(2), &[0, 1], &[]),
                sample(20, 11, Some(3), &[0, 2], &[]),
                sample(30, 12, None, &[0], &[]),
            ],
            vec![],
        );

        let timeline = TimeOrderTimeline::build(&profile, &SampleFilter::default(), 4).unwrap();
        assert!(timeline.uses_cpu_lanes);
        assert_eq!(
            timeline
                .lanes
                .iter()
                .map(|lane| lane.key)
                .collect::<Vec<_>>(),
            [
                TimelineLaneKey::Cpu(None),
                TimelineLaneKey::Cpu(Some(2)),
                TimelineLaneKey::Cpu(Some(3)),
            ]
        );
        let cpu_two = timeline
            .lanes
            .iter()
            .find(|lane| lane.key == TimelineLaneKey::Cpu(Some(2)))
            .unwrap();
        let b = cpu_two
            .segments
            .iter()
            .find(|segment| segment.frame_id == 1)
            .unwrap();
        assert_eq!((b.start_ns, b.end_ns), (0, 20));
        assert_eq!((b.samples, b.representative_samples), (2, 2));
    }

    #[test]
    fn time_order_falls_back_to_thread_lanes_for_legacy_samples() {
        let profile = profile(
            vec![frame(0, "A")],
            vec![sample(0, 8, None, &[0], &[]), sample(1, 7, None, &[0], &[])],
            vec![],
        );

        let timeline = TimeOrderTimeline::build(&profile, &SampleFilter::default(), 10).unwrap();
        assert!(!timeline.uses_cpu_lanes);
        assert_eq!(
            timeline
                .lanes
                .iter()
                .map(|lane| lane.key)
                .collect::<Vec<_>>(),
            [TimelineLaneKey::Thread(7), TimelineLaneKey::Thread(8)]
        );
    }

    #[test]
    fn counter_tracks_align_sums_and_derive_ipc_from_sums() {
        let profile = profile(
            vec![frame(0, "A")],
            vec![
                sample(0, 1, None, &[0], &[Some(100.0), Some(200.0), None, None]),
                sample(
                    10,
                    1,
                    None,
                    &[0],
                    &[Some(300.0), Some(600.0), Some(5.0), None],
                ),
            ],
            vec![
                metric("pmu_cycles"),
                metric("pmu_instructions"),
                metric("cache_misses"),
                metric("unavailable"),
            ],
        );

        let tracks = CounterTracks::build(&profile, &SampleFilter::default(), 1).unwrap();
        assert_eq!(tracks.bins, 1);
        assert_eq!(tracks.sample_counts, [2]);
        assert_eq!(tracks.track("pmu_cycles").unwrap().values, [Some(400.0)]);
        assert_eq!(tracks.track("cache_misses").unwrap().values, [Some(5.0)]);
        assert_eq!(tracks.track("unavailable").unwrap().values, [None]);
        assert_eq!(
            tracks.track("derived.ipc").unwrap().aggregation,
            CounterAggregation::Ratio
        );
        assert_eq!(tracks.track("derived.ipc").unwrap().values, [Some(2.0)]);
    }

    #[test]
    fn cpu_heatmap_splits_cpu_clock_intervals_across_buckets() {
        let profile = profile(
            vec![frame(0, "A")],
            vec![
                sample(0, 1, Some(0), &[0], &[None]),
                sample(10, 1, Some(0), &[0], &[Some(10.0)]),
                sample(20, 1, Some(0), &[0], &[Some(10.0)]),
            ],
            vec![metric("os_cpu_clock")],
        );
        let filter = SampleFilter {
            range: Some(TimeRange {
                start_ns: 0,
                end_ns: 20,
            }),
            ..SampleFilter::default()
        };

        let heatmap = CpuUtilizationHeatmap::build(&profile, &filter, 2).unwrap();
        assert_eq!(heatmap.bucket_duration_ns, 10);
        assert_eq!(heatmap.buckets, 2);
        assert!(heatmap.uses_cpu_lanes);
        assert_eq!(heatmap.lanes[0].key, TimelineLaneKey::Cpu(Some(0)));
        assert_eq!(heatmap.lanes[0].buckets[0].cpu_time_ns, 10.0);
        assert_eq!(heatmap.lanes[0].buckets[1].cpu_time_ns, 10.0);
        assert_eq!(heatmap.lanes[0].buckets[0].utilization, 1.0);
        assert_eq!(heatmap.lanes[0].buckets[1].utilization, 1.0);
    }

    #[test]
    fn counter_delta_uses_explicit_wall_interval_including_first_sample() {
        let mut profile = profile(
            vec![frame(0, "A")],
            vec![sample(10_000_000, 1, Some(2), &[0], &[])],
            vec![],
        );
        profile.cpu_observations = vec![CpuObservation {
            timestamp_ns: 10_000_000,
            process_id: 7,
            thread_id: 1,
            cpu: Some(2),
            interval_start_ns: Some(0),
            stack: vec![0],
            weight_ns: 2_000_000,
            source: CpuObservationSource::CounterDelta,
        }];
        let filter = SampleFilter {
            range: Some(TimeRange {
                start_ns: 0,
                end_ns: 10_000_000,
            }),
            ..SampleFilter::default()
        };

        assert_eq!(
            profile.full_range(),
            Some(TimeRange {
                start_ns: 0,
                end_ns: 10_000_001,
            })
        );
        let heatmap = CpuUtilizationHeatmap::build(&profile, &filter, 1).unwrap();
        assert_eq!(heatmap.lanes[0].buckets[0].cpu_time_ns, 2_000_000.0);
        assert_eq!(heatmap.lanes[0].buckets[0].utilization, 0.2);
    }

    #[test]
    fn current_recordings_preseed_idle_logical_cpu_rows() {
        let mut profile = profile(
            vec![frame(0, "A")],
            vec![sample(10, 1, Some(2), &[0], &[])],
            vec![],
        );
        profile.logical_cpu_count = Some(4);
        profile.cpu_observations = vec![CpuObservation {
            timestamp_ns: 10,
            process_id: 7,
            thread_id: 1,
            cpu: Some(2),
            interval_start_ns: Some(0),
            stack: vec![0],
            weight_ns: 5,
            source: CpuObservationSource::CounterDelta,
        }];
        let filter = SampleFilter {
            range: Some(TimeRange {
                start_ns: 0,
                end_ns: 10,
            }),
            ..SampleFilter::default()
        };

        let heatmap = CpuUtilizationHeatmap::build(&profile, &filter, 1).unwrap();
        assert_eq!(
            heatmap
                .lanes
                .iter()
                .map(|lane| lane.key)
                .collect::<Vec<_>>(),
            [
                TimelineLaneKey::Cpu(Some(0)),
                TimelineLaneKey::Cpu(Some(1)),
                TimelineLaneKey::Cpu(Some(2)),
                TimelineLaneKey::Cpu(Some(3)),
            ]
        );
        assert_eq!(heatmap.lanes[0].buckets[0].utilization, 0.0);
        assert_eq!(heatmap.lanes[1].buckets[0].utilization, 0.0);
        assert_eq!(heatmap.lanes[2].buckets[0].utilization, 0.5);
        assert_eq!(heatmap.lanes[3].buckets[0].utilization, 0.0);
    }

    #[test]
    fn sampled_occupancy_is_point_attributed_after_cpu_migration() {
        let mut profile = profile(
            vec![frame(0, "A")],
            vec![
                sample(0, 1, Some(0), &[0], &[]),
                sample(20, 1, Some(1), &[0], &[]),
            ],
            vec![],
        );
        profile.cpu_observations = vec![
            CpuObservation {
                timestamp_ns: 9,
                process_id: 7,
                thread_id: 1,
                cpu: Some(0),
                interval_start_ns: None,
                stack: vec![0],
                weight_ns: 10,
                source: CpuObservationSource::SampledOccupancy,
            },
            CpuObservation {
                timestamp_ns: 10,
                process_id: 7,
                thread_id: 1,
                cpu: Some(1),
                interval_start_ns: None,
                stack: vec![0],
                weight_ns: 10,
                source: CpuObservationSource::SampledOccupancy,
            },
        ];
        let filter = SampleFilter {
            range: Some(TimeRange {
                start_ns: 0,
                end_ns: 20,
            }),
            ..SampleFilter::default()
        };

        let heatmap = CpuUtilizationHeatmap::build(&profile, &filter, 2).unwrap();
        assert_eq!(heatmap.source, CpuObservationSource::SampledOccupancy);
        let cpu_zero = heatmap
            .lanes
            .iter()
            .find(|lane| lane.key == TimelineLaneKey::Cpu(Some(0)))
            .unwrap();
        let cpu_one = heatmap
            .lanes
            .iter()
            .find(|lane| lane.key == TimelineLaneKey::Cpu(Some(1)))
            .unwrap();
        assert_eq!(cpu_zero.buckets[0].cpu_time_ns, 10.0);
        assert_eq!(cpu_zero.buckets[1].cpu_time_ns, 0.0);
        assert_eq!(cpu_one.buckets[0].cpu_time_ns, 0.0);
        assert_eq!(cpu_one.buckets[1].cpu_time_ns, 10.0);
    }

    #[test]
    fn cpu_heatmap_falls_back_to_thread_lanes_and_caps_overlap() {
        let mut profile = profile(
            vec![frame(0, "A")],
            vec![
                sample(0, 7, None, &[0], &[None]),
                sample(10, 7, Some(3), &[0], &[Some(10.0)]),
                sample(10, 7, None, &[0], &[Some(10.0)]),
                sample(10, 8, None, &[0], &[Some(5.0)]),
            ],
            vec![metric("os_cpu_clock")],
        );
        profile.logical_cpu_count = Some(4);
        let filter = SampleFilter {
            range: Some(TimeRange {
                start_ns: 0,
                end_ns: 10,
            }),
            ..SampleFilter::default()
        };

        let heatmap = CpuUtilizationHeatmap::build(&profile, &filter, 1).unwrap();
        assert!(!heatmap.uses_cpu_lanes);
        assert_eq!(
            heatmap
                .lanes
                .iter()
                .map(|lane| lane.key)
                .collect::<Vec<_>>(),
            [TimelineLaneKey::Thread(7), TimelineLaneKey::Thread(8)]
        );
        assert_eq!(heatmap.lanes[0].buckets[0].cpu_time_ns, 20.0);
        assert_eq!(heatmap.lanes[0].buckets[0].utilization, 1.0);
        assert_eq!(heatmap.lanes[1].buckets[0].utilization, 0.5);
    }

    #[test]
    fn cpu_heatmap_honors_configured_bucket_duration_at_boundaries() {
        let profile = profile(
            vec![frame(0, "A")],
            vec![
                sample(0, 1, Some(0), &[0], &[None]),
                sample(10, 1, Some(0), &[0], &[Some(10.0)]),
                sample(20, 1, Some(0), &[0], &[Some(10.0)]),
            ],
            vec![metric("os_cpu_clock")],
        );
        let filter = SampleFilter {
            range: Some(TimeRange {
                start_ns: 0,
                end_ns: 20,
            }),
            ..SampleFilter::default()
        };

        let heatmap =
            CpuUtilizationHeatmap::build_with_bucket_duration(&profile, &filter, 1, Some(7))
                .unwrap();

        assert_eq!(heatmap.bucket_duration_ns, 7);
        assert_eq!(heatmap.buckets, 3);
        assert_eq!(
            heatmap.lanes[0]
                .buckets
                .iter()
                .map(|bucket| bucket.cpu_time_ns)
                .collect::<Vec<_>>(),
            [7.0, 7.0, 6.0]
        );
        assert!(
            heatmap.lanes[0]
                .buckets
                .iter()
                .all(|bucket| bucket.utilization == 1.0)
        );
        assert!(
            CpuUtilizationHeatmap::build_with_bucket_duration(&profile, &filter, 10, Some(0))
                .is_none()
        );
    }

    #[test]
    fn cpu_heatmap_is_unavailable_without_real_cpu_clock_data() {
        let without_metric = profile(
            vec![frame(0, "A")],
            vec![sample(0, 1, Some(0), &[0], &[Some(1.0)])],
            vec![metric("pmu_cycles")],
        );
        assert!(
            CpuUtilizationHeatmap::build(&without_metric, &SampleFilter::default(), 10).is_none()
        );

        let null_metric = profile(
            vec![frame(0, "A")],
            vec![sample(0, 1, Some(0), &[0], &[None])],
            vec![metric("os_cpu_clock")],
        );
        assert!(CpuUtilizationHeatmap::build(&null_metric, &SampleFilter::default(), 10).is_none());
    }

    #[test]
    fn nice_widths_are_deterministic_one_two_five_steps() {
        assert_eq!(nice_bin_width(1), 1);
        assert_eq!(nice_bin_width(11), 20);
        assert_eq!(nice_bin_width(201), 500);
        assert_eq!(nice_bin_width(1_000), 1_000);
    }
}
