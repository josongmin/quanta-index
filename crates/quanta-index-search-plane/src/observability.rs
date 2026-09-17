//! The query plane's metric store and the scrape that serves it
//! (QI-BB-015).
//!
//! Every sample the dispatcher emits is folded into an aggregate keyed by
//! metric name — an exact counter sum, the last gauge value, or a bucketed
//! distribution — so a scrape reads totals that no ring capacity truncates.
//! Beside the aggregates two bounded rings keep the newest samples and
//! errors as a diagnostic tail, each counting what it let go. Sources
//! outside the query plane (socket servers, caches, registries, the boot
//! inventory) join the scrape through [`MetricSourcePort`]; the scrape
//! merges everything into one [`MetricsSnapshotV1`] and refuses, typed, a
//! source whose points would make that snapshot invalid on the wire.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use quanta_index_contract::{
    MetricBucketV1, MetricCounterV1, MetricGaugeV1, MetricHistogramV1, MetricsDiagnosticsV1,
    MetricsSnapshotV1, is_metric_name_v1,
};
use quanta_index_core::{CoreError, MetricPointV1, MetricSourcePort, MetricValueV1};
use quanta_index_lq_obs::{
    CardinalityGuard, MetricKind, MetricSample, OBS_OVERFLOW_LABEL, ObsError, ObsErrorCode,
    validate_dimensions,
};

/// Where the query dispatcher sends every metric sample.
pub trait QueryObsSink {
    fn emit(&self, sample: MetricSample);
}

/// A sink that keeps nothing, for benchmarks and test doubles.
pub(crate) struct NoopQueryObsSink;

impl QueryObsSink for NoopQueryObsSink {
    fn emit(&self, _sample: MetricSample) {}
}

/// Samples the diagnostic tail keeps; older ones are let go and counted.
pub const MAX_OBS_SAMPLES: usize = 4_096;

/// Errors the diagnostic tail keeps; older ones are let go and counted.
pub const MAX_OBS_ERRORS: usize = 256;

/// Upper bounds of the histogram buckets every distribution shares.
///
/// A log-ish ladder that resolves milliseconds and small counts alike; the
/// wire histogram adds the `+Inf` bucket after these.
pub const HISTOGRAM_BUCKET_BOUNDS: [f64; 14] = [
    1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1_000.0, 2_500.0, 5_000.0, 10_000.0,
    30_000.0,
];

const ERR_METRICS_SOURCE_DEFECT: &str = "METRICS_SOURCE_DEFECT";

/// The newest `capacity` items, and how many were recorded and let go.
struct BoundedRing<T> {
    items: VecDeque<T>,
    capacity: usize,
    recorded: u64,
    dropped: u64,
}

impl<T: Clone> BoundedRing<T> {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            items: VecDeque::with_capacity(capacity),
            capacity,
            recorded: 0,
            dropped: 0,
        }
    }

    fn push(&mut self, item: T) {
        self.recorded = self.recorded.saturating_add(1);
        if self.items.len() >= self.capacity {
            let _evicted = self.items.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.items.push_back(item);
    }

    fn tail(&self) -> Vec<T> {
        self.items.iter().cloned().collect()
    }
}

/// One distribution aggregated since process start.
#[derive(Clone, Debug, PartialEq)]
struct HistogramAggregate {
    count: u64,
    sum: f64,
    min: f64,
    max: f64,
    /// Observations in each bucket of [`HISTOGRAM_BUCKET_BOUNDS`]
    /// (at or below that bound and above the previous), non-cumulative;
    /// what none of them holds is the `+Inf` remainder.
    below: [u64; HISTOGRAM_BUCKET_BOUNDS.len()],
}

impl HistogramAggregate {
    const fn new() -> Self {
        Self {
            count: 0,
            sum: 0.0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            below: [0; HISTOGRAM_BUCKET_BOUNDS.len()],
        }
    }

    fn observe(&mut self, value: f64) {
        self.count = self.count.saturating_add(1);
        self.sum += value;
        self.min = self.min.min(value);
        self.max = self.max.max(value);
        if let Some(slot) = HISTOGRAM_BUCKET_BOUNDS
            .iter()
            .position(|bound| value <= *bound)
            .and_then(|position| self.below.get_mut(position))
        {
            *slot = slot.saturating_add(1);
        }
    }

    fn to_wire(&self, name: &str) -> MetricHistogramV1 {
        let mut cumulative = 0_u64;
        let mut buckets = Vec::with_capacity(HISTOGRAM_BUCKET_BOUNDS.len().saturating_add(1));
        for (bound, below) in HISTOGRAM_BUCKET_BOUNDS.iter().zip(self.below.iter()) {
            cumulative = cumulative.saturating_add(*below);
            buckets.push(MetricBucketV1 {
                le: *bound,
                count: cumulative,
            });
        }
        buckets.push(MetricBucketV1 {
            le: f64::INFINITY,
            count: self.count,
        });
        MetricHistogramV1 {
            name: name.to_string(),
            count: self.count,
            sum: self.sum,
            min: if self.count == 0 { 0.0 } else { self.min },
            max: if self.count == 0 { 0.0 } else { self.max },
            buckets,
        }
    }
}

/// One metric's aggregate; a name keeps the kind of its first sample.
enum MetricAggregate {
    Counter(u64),
    Gauge(f64),
    Histogram(HistogramAggregate),
}

impl MetricAggregate {
    const fn kind(&self) -> MetricKind {
        match self {
            Self::Counter(_) => MetricKind::Counter,
            Self::Gauge(_) => MetricKind::Gauge,
            Self::Histogram(_) => MetricKind::Histogram,
        }
    }

    const fn empty(kind: MetricKind) -> Self {
        match kind {
            MetricKind::Counter => Self::Counter(0),
            MetricKind::Gauge => Self::Gauge(0.0),
            MetricKind::Histogram => Self::Histogram(HistogramAggregate::new()),
        }
    }

    /// Fold one validated value in.
    fn observe(&mut self, value: f64) {
        match self {
            Self::Counter(total) => *total = total.saturating_add(counter_increment(value)),
            Self::Gauge(last) => *last = value,
            Self::Histogram(histogram) => histogram.observe(value),
        }
    }
}

/// The integer increment a validated counter sample carries.
///
/// [`validate_sample`] admits only finite, non-negative, integral counter
/// values, so the cast neither truncates nor loses sign; a value past
/// `u64::MAX` saturates, which is the intended ceiling.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "validate_sample admits only finite, non-negative, integral counter values, so the f64-to-u64 cast (f64 has no TryFrom for u64) is exact below u64::MAX and saturates above it"
)]
fn counter_increment(value: f64) -> u64 {
    value as u64
}

/// Every metric aggregated since process start, by name.
#[derive(Default)]
struct MetricAggregates {
    by_name: BTreeMap<Box<str>, MetricAggregate>,
}

impl MetricAggregates {
    /// Fold one validated sample in; a sample whose kind disagrees with the
    /// name's first sample is a producer defect and is refused.
    fn observe(&mut self, sample: &MetricSample) -> Result<(), ObsError> {
        let aggregate = self
            .by_name
            .entry(sample.name.clone())
            .or_insert_with(|| MetricAggregate::empty(sample.kind));
        if aggregate.kind() != sample.kind {
            return Err(ObsError::new(
                ObsErrorCode::ObsInvalidMetric,
                format!(
                    "metric `{}` is a {:?} but a {:?} sample was emitted under its name",
                    sample.name,
                    aggregate.kind(),
                    sample.kind
                ),
            ));
        }
        aggregate.observe(sample.value);
        Ok(())
    }
}

/// What a sample must satisfy to be aggregated: a wire-valid name and a
/// finite value, integral and non-negative when it increments a counter.
fn validate_sample(sample: &MetricSample) -> Result<(), ObsError> {
    if !is_metric_name_v1(&sample.name) {
        return Err(ObsError::new(
            ObsErrorCode::ObsInvalidMetric,
            format!("metric name `{}` is not [a-z][a-z0-9_]*", sample.name),
        ));
    }
    if !sample.value.is_finite() {
        return Err(ObsError::new(
            ObsErrorCode::ObsInvalidMetric,
            format!("metric `{}` sample value is not finite", sample.name),
        ));
    }
    if sample.kind == MetricKind::Counter && (sample.value < 0.0 || sample.value.fract() != 0.0) {
        return Err(ObsError::new(
            ObsErrorCode::ObsInvalidMetric,
            format!(
                "counter `{}` increment {} is not a non-negative integer",
                sample.name, sample.value
            ),
        ));
    }
    Ok(())
}

/// The query plane's metric store: aggregates that a scrape reads, and two
/// bounded diagnostic tails.
pub struct BoundedQueryObsStore {
    guard: Mutex<CardinalityGuard>,
    aggregates: Mutex<MetricAggregates>,
    samples: Mutex<BoundedRing<MetricSample>>,
    errors: Mutex<BoundedRing<ObsError>>,
}

impl Default for BoundedQueryObsStore {
    fn default() -> Self {
        Self {
            guard: Mutex::new(CardinalityGuard::default()),
            aggregates: Mutex::new(MetricAggregates::default()),
            samples: Mutex::new(BoundedRing::with_capacity(MAX_OBS_SAMPLES)),
            errors: Mutex::new(BoundedRing::with_capacity(MAX_OBS_ERRORS)),
        }
    }
}

impl BoundedQueryObsStore {
    fn record_error(&self, err: ObsError) {
        lock_or_recover(&self.errors).push(err);
    }

    /// The diagnostic tail of samples, newest last; bounded, so never a
    /// total.
    #[must_use]
    pub fn snapshot(&self) -> Vec<MetricSample> {
        lock_or_recover(&self.samples).tail()
    }

    /// The diagnostic tail of errors, newest last.
    #[must_use]
    pub fn errors(&self) -> Vec<ObsError> {
        lock_or_recover(&self.errors).tail()
    }

    /// Every counter, gauge and histogram aggregated since process start,
    /// plus what the diagnostic tails kept and dropped.
    #[must_use]
    pub fn metrics_snapshot(&self) -> MetricsSnapshotV1 {
        let mut counters = Vec::new();
        let mut gauges = Vec::new();
        let mut histograms = Vec::new();
        {
            let aggregates = lock_or_recover(&self.aggregates);
            for (name, aggregate) in &aggregates.by_name {
                match aggregate {
                    MetricAggregate::Counter(value) => counters.push(MetricCounterV1 {
                        name: name.to_string(),
                        value: *value,
                    }),
                    MetricAggregate::Gauge(value) => gauges.push(MetricGaugeV1 {
                        name: name.to_string(),
                        value: *value,
                    }),
                    MetricAggregate::Histogram(histogram) => {
                        histograms.push(histogram.to_wire(name));
                    }
                }
            }
        }
        let (samples_recorded, samples_dropped) = {
            let samples = lock_or_recover(&self.samples);
            (samples.recorded, samples.dropped)
        };
        let (errors_recorded, errors_dropped) = {
            let errors = lock_or_recover(&self.errors);
            (errors.recorded, errors.dropped)
        };
        MetricsSnapshotV1 {
            counters,
            gauges,
            histograms,
            diagnostics: MetricsDiagnosticsV1 {
                samples_recorded,
                samples_dropped,
                errors_recorded,
                errors_dropped,
            },
        }
    }
}

impl QueryObsSink for BoundedQueryObsStore {
    /// Validate, guard, aggregate, then keep the sample in the tail; a
    /// sample that fails any check is recorded as an error and nowhere else.
    fn emit(&self, sample: MetricSample) {
        if let Err(err) = validate_sample(&sample) {
            self.record_error(err);
            return;
        }
        if let Err(err) = validate_dimensions(&sample.dimensions) {
            self.record_error(err);
            return;
        }
        let sample = {
            let mut guard = lock_or_recover(&self.guard);
            match guard.observe(&sample.dimensions) {
                Ok(()) => sample,
                Err(err) => {
                    self.record_error(err.clone());
                    overflow_bucket_sample(sample, &err)
                }
            }
        };
        let aggregated = lock_or_recover(&self.aggregates).observe(&sample);
        if let Err(err) = aggregated {
            self.record_error(err);
            return;
        }
        lock_or_recover(&self.samples).push(sample);
    }
}

fn overflow_bucket_sample(mut sample: MetricSample, err: &ObsError) -> MetricSample {
    match err.dim_overflow.as_deref() {
        Some("tenant_id") => {
            sample.dimensions.tenant_id = OBS_OVERFLOW_LABEL.into();
        }
        Some("repo_id") => {
            sample.dimensions.repo_id = OBS_OVERFLOW_LABEL.into();
        }
        Some("ticket_id") => {
            sample.dimensions.ticket_id = OBS_OVERFLOW_LABEL.into();
        }
        Some("wave_id") => {
            sample.dimensions.wave_id = OBS_OVERFLOW_LABEL.into();
        }
        Some(_) | None => {}
    }
    sample
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(err) => err.into_inner(),
    }
}

/// The scrape: the query plane's aggregates and every registered source,
/// merged into one snapshot.
pub struct ObservabilityScrape {
    store: Arc<BoundedQueryObsStore>,
    sources: Vec<Arc<dyn MetricSourcePort>>,
}

impl ObservabilityScrape {
    #[must_use]
    pub fn new(store: Arc<BoundedQueryObsStore>, sources: Vec<Arc<dyn MetricSourcePort>>) -> Self {
        Self { store, sources }
    }

    /// One snapshot of everything, sorted by name within each kind.
    ///
    /// A source point with a name that is not wire-valid, or that another
    /// point or the store already uses, is a defect in this process and is
    /// answered typed as `METRICS_SOURCE_DEFECT` rather than by a snapshot
    /// with that point silently missing.
    pub fn scrape(&self) -> Result<MetricsSnapshotV1, CoreError> {
        let mut snapshot = self.store.metrics_snapshot();
        let mut names: BTreeSet<String> = snapshot
            .counters
            .iter()
            .map(|counter| counter.name.clone())
            .chain(snapshot.gauges.iter().map(|gauge| gauge.name.clone()))
            .chain(
                snapshot
                    .histograms
                    .iter()
                    .map(|histogram| histogram.name.clone()),
            )
            .collect();
        for source in &self.sources {
            for point in source.scrape()? {
                admit_source_point(&mut snapshot, &mut names, point)?;
            }
        }
        snapshot
            .counters
            .sort_by(|left, right| left.name.cmp(&right.name));
        snapshot
            .gauges
            .sort_by(|left, right| left.name.cmp(&right.name));
        Ok(snapshot)
    }
}

fn admit_source_point(
    snapshot: &mut MetricsSnapshotV1,
    names: &mut BTreeSet<String>,
    point: MetricPointV1,
) -> Result<(), CoreError> {
    if !is_metric_name_v1(&point.name) {
        return Err(source_defect(&format!(
            "source point name `{}` is not [a-z][a-z0-9_]*",
            point.name
        )));
    }
    if let MetricValueV1::Gauge(value) = point.value
        && !value.is_finite()
    {
        return Err(source_defect(&format!(
            "source gauge `{}` is not finite",
            point.name
        )));
    }
    if !names.insert(point.name.clone()) {
        return Err(source_defect(&format!(
            "metric `{}` is reported by more than one source",
            point.name
        )));
    }
    match point.value {
        MetricValueV1::Counter(value) => snapshot.counters.push(MetricCounterV1 {
            name: point.name,
            value,
        }),
        MetricValueV1::Gauge(value) => snapshot.gauges.push(MetricGaugeV1 {
            name: point.name,
            value,
        }),
    }
    Ok(())
}

fn source_defect(message: &str) -> CoreError {
    CoreError::Typed {
        code: ERR_METRICS_SOURCE_DEFECT.to_string(),
        message: format!("metrics scrape: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use quanta_index_core::{CoreError, MetricPointV1, MetricSourcePort};
    use quanta_index_lq_obs::{Dimensions, MetricKind, MetricSample, ObsErrorCode};

    use super::{
        BoundedQueryObsStore, HISTOGRAM_BUCKET_BOUNDS, MAX_OBS_ERRORS, MAX_OBS_SAMPLES,
        ObservabilityScrape, QueryObsSink,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn dimensions() -> Dimensions {
        Dimensions::new("LXE-10", "8", "local", "repo-obs", 4)
    }

    fn sample(name: &str, kind: MetricKind, value: f64) -> MetricSample {
        MetricSample::new(name, kind, value, dimensions())
    }

    fn counter_value(store: &BoundedQueryObsStore, name: &str) -> Option<u64> {
        store
            .metrics_snapshot()
            .counters
            .into_iter()
            .find(|counter| counter.name == name)
            .map(|counter| counter.value)
    }

    /// A counter total is exact past the sample ring's capacity; the ring
    /// keeps only the newest samples and says how many it let go.
    #[test]
    fn counter_totals_survive_the_sample_ring_being_full() -> TestResult {
        let store = BoundedQueryObsStore::default();
        let emitted = MAX_OBS_SAMPLES.saturating_add(1_000);
        for _ in 0..emitted {
            store.emit(sample("lq_query_intake_total", MetricKind::Counter, 1.0));
        }
        let emitted_u64 = u64::try_from(emitted)?;
        if counter_value(&store, "lq_query_intake_total") != Some(emitted_u64) {
            return Err(format!(
                "the counter total is every sample, got {:?}",
                counter_value(&store, "lq_query_intake_total")
            )
            .into());
        }
        let snapshot = store.metrics_snapshot();
        if snapshot.diagnostics.samples_recorded != emitted_u64
            || snapshot.diagnostics.samples_dropped != 1_000
            || store.snapshot().len() != MAX_OBS_SAMPLES
        {
            return Err(format!(
                "the ring keeps {MAX_OBS_SAMPLES} and counts the rest: {:?}, tail {}",
                snapshot.diagnostics,
                store.snapshot().len()
            )
            .into());
        }
        if !store.errors().is_empty() || snapshot.diagnostics.errors_recorded != 0 {
            return Err("valid samples record no errors".into());
        }
        Ok(())
    }

    /// A gauge keeps its last value; a histogram keeps count, sum,
    /// extremes and cumulative buckets that end in `+Inf`.
    #[test]
    fn gauges_keep_the_last_value_and_histograms_bucket_cumulatively() -> TestResult {
        let store = BoundedQueryObsStore::default();
        for value in [3.0, 9.0, 4.0] {
            store.emit(sample("lexical_writers_open", MetricKind::Gauge, value));
        }
        // One observation per band: below the first bound, on a bound,
        // between bounds, past the last bound.
        for value in [0.5, 1.0, 1.5, 30_001.0] {
            store.emit(sample(
                "lq_route_lexical_latency_ms",
                MetricKind::Histogram,
                value,
            ));
        }
        let snapshot = store.metrics_snapshot();
        let gauge = snapshot
            .gauges
            .iter()
            .find(|gauge| gauge.name == "lexical_writers_open")
            .ok_or("gauge present")?;
        if gauge.value.to_bits() != 4.0_f64.to_bits() {
            return Err(format!("a gauge is its last sample, got {}", gauge.value).into());
        }
        let histogram = snapshot
            .histograms
            .iter()
            .find(|histogram| histogram.name == "lq_route_lexical_latency_ms")
            .ok_or("histogram present")?;
        if (histogram.count, histogram.sum, histogram.min, histogram.max)
            != (4, 30_004.0, 0.5, 30_001.0)
        {
            return Err(format!("histogram summary: {histogram:?}").into());
        }
        let bounds: Vec<f64> = histogram.buckets.iter().map(|bucket| bucket.le).collect();
        let mut expected_bounds = HISTOGRAM_BUCKET_BOUNDS.to_vec();
        expected_bounds.push(f64::INFINITY);
        if bounds != expected_bounds {
            return Err(format!("every bucket bound, then +Inf: {bounds:?}").into());
        }
        let counts: Vec<u64> = histogram
            .buckets
            .iter()
            .map(|bucket| bucket.count)
            .collect();
        // le=1 holds 0.5 and 1.0; le=2 adds 1.5; every later finite bound
        // stays at 3; +Inf holds all four.
        let mut expected_counts = vec![2_u64, 3];
        expected_counts.extend(std::iter::repeat_n(
            3_u64,
            HISTOGRAM_BUCKET_BOUNDS.len() - 2,
        ));
        expected_counts.push(4);
        if counts != expected_counts {
            return Err(format!("cumulative counts: {counts:?} != {expected_counts:?}").into());
        }
        Ok(())
    }

    /// A sample that would make the snapshot lie is refused and recorded.
    ///
    /// That is a bad name, a non-finite value, a fractional or negative
    /// counter increment, or a kind that disagrees with the name's first
    /// sample.
    #[test]
    fn invalid_samples_are_refused_and_recorded_not_aggregated() -> TestResult {
        let store = BoundedQueryObsStore::default();
        store.emit(sample("lq_query_intake_total", MetricKind::Counter, 1.0));
        let refused = [
            sample("Lq-Bad", MetricKind::Counter, 1.0),
            sample("lq_query_intake_total", MetricKind::Counter, f64::NAN),
            sample("lq_query_intake_total", MetricKind::Counter, f64::INFINITY),
            sample("lq_query_intake_total", MetricKind::Counter, 0.5),
            sample("lq_query_intake_total", MetricKind::Counter, -1.0),
            sample("lq_query_intake_total", MetricKind::Gauge, 7.0),
            sample("lq_query_intake_total", MetricKind::Histogram, 7.0),
        ];
        let refused_count = u64::try_from(refused.len())?;
        for bad in refused {
            store.emit(bad);
        }
        let errors = store.errors();
        if errors.len() != usize::try_from(refused_count)?
            || errors
                .iter()
                .any(|error| error.code != ObsErrorCode::ObsInvalidMetric)
        {
            return Err(format!("every refusal is an OBS_INVALID_METRIC error: {errors:?}").into());
        }
        let snapshot = store.metrics_snapshot();
        if counter_value(&store, "lq_query_intake_total") != Some(1)
            || !snapshot.gauges.is_empty()
            || !snapshot.histograms.is_empty()
            || snapshot.counters.len() != 1
        {
            return Err(format!("nothing refused reached an aggregate: {snapshot:?}").into());
        }
        if snapshot.diagnostics.samples_recorded != 1
            || snapshot.diagnostics.errors_recorded != refused_count
            || store.snapshot().len() != 1
        {
            return Err(format!(
                "refused samples are not in the tail: {:?}",
                snapshot.diagnostics
            )
            .into());
        }
        let kind_conflict = errors
            .iter()
            .filter(|error| error.detail.contains("is a Counter but a"))
            .count();
        if kind_conflict != 2 {
            return Err(format!("both kind conflicts name the conflict: {errors:?}").into());
        }
        Ok(())
    }

    /// The error tail is bounded like the sample tail and counts what it
    /// let go.
    #[test]
    fn the_error_ring_is_bounded_and_counts_its_drops() -> TestResult {
        let store = BoundedQueryObsStore::default();
        let emitted = MAX_OBS_ERRORS.saturating_add(40);
        for index in 0..emitted {
            store.emit(sample(&format!("Bad{index}"), MetricKind::Counter, 1.0));
        }
        let errors = store.errors();
        let diagnostics = store.metrics_snapshot().diagnostics;
        if errors.len() != MAX_OBS_ERRORS
            || diagnostics.errors_recorded != u64::try_from(emitted)?
            || diagnostics.errors_dropped != 40
        {
            return Err(format!("{} kept, {:?}", errors.len(), diagnostics).into());
        }
        if !errors
            .first()
            .is_some_and(|error| error.detail.contains("`Bad40`"))
        {
            return Err(format!("the oldest kept error is the 41st: {:?}", errors.first()).into());
        }
        Ok(())
    }

    struct FixedSource(Vec<MetricPointV1>);

    impl MetricSourcePort for FixedSource {
        fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
            Ok(self.0.clone())
        }
    }

    struct FailingSource;

    impl MetricSourcePort for FailingSource {
        fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
            Err(CoreError::Storage("regex cache poisoned".to_string()))
        }
    }

    /// The scrape merges the store with every source, sorted by name within
    /// each kind, and a scrape's own view never carries a NaN gauge.
    #[test]
    fn scrape_merges_sources_into_one_sorted_snapshot() -> TestResult {
        let store = Arc::new(BoundedQueryObsStore::default());
        store.emit(sample("lq_query_intake_total", MetricKind::Counter, 2.0));
        store.emit(sample(
            "lq_route_lexical_latency_ms",
            MetricKind::Histogram,
            3.0,
        ));
        let sources: Vec<Arc<dyn MetricSourcePort>> = vec![
            Arc::new(FixedSource(vec![
                MetricPointV1::counter("snapshot_registry_lexical_hits_total", 5),
                MetricPointV1::gauge_count("snapshot_registry_lexical_entries", 2),
            ])),
            Arc::new(FixedSource(vec![
                MetricPointV1::counter("ipc_query_requests_dispatched_total", 9),
                MetricPointV1::gauge("ipc_query_connections_live", 1.0),
            ])),
        ];
        let snapshot = ObservabilityScrape::new(store, sources).scrape()?;
        let counters: Vec<(&str, u64)> = snapshot
            .counters
            .iter()
            .map(|counter| (counter.name.as_str(), counter.value))
            .collect();
        if counters
            != vec![
                ("ipc_query_requests_dispatched_total", 9),
                ("lq_query_intake_total", 2),
                ("snapshot_registry_lexical_hits_total", 5),
            ]
        {
            return Err(format!("counters merged and sorted: {counters:?}").into());
        }
        let gauges: Vec<(&str, f64)> = snapshot
            .gauges
            .iter()
            .map(|gauge| (gauge.name.as_str(), gauge.value))
            .collect();
        if gauges
            != vec![
                ("ipc_query_connections_live", 1.0),
                ("snapshot_registry_lexical_entries", 2.0),
            ]
        {
            return Err(format!("gauges merged and sorted: {gauges:?}").into());
        }
        if snapshot.histograms.len() != 1 {
            return Err("the store's histogram is the only one".into());
        }
        Ok(())
    }

    /// A source that fails, names an invalid metric, reports a NaN gauge,
    /// or collides with another name fails the scrape typed.
    #[test]
    fn scrape_refuses_defective_sources_typed() -> TestResult {
        let scrape = |sources: Vec<Arc<dyn MetricSourcePort>>| {
            let store = Arc::new(BoundedQueryObsStore::default());
            store.emit(sample("lq_query_intake_total", MetricKind::Counter, 1.0));
            ObservabilityScrape::new(store, sources).scrape()
        };
        match scrape(vec![Arc::new(FailingSource)]) {
            Err(CoreError::Storage(message)) if message.contains("regex cache poisoned") => {}
            other => {
                return Err(
                    format!("a failing source fails the scrape as itself: {other:?}").into(),
                );
            }
        }
        let defects: [(&str, Vec<MetricPointV1>); 4] = [
            ("not [a-z]", vec![MetricPointV1::counter("Bad-Name", 1)]),
            (
                "not finite",
                vec![MetricPointV1::gauge("ipc_query_live", f64::NAN)],
            ),
            (
                "more than one source",
                vec![MetricPointV1::counter("lq_query_intake_total", 1)],
            ),
            (
                "more than one source",
                vec![
                    MetricPointV1::counter("ipc_query_requests_dispatched_total", 1),
                    MetricPointV1::gauge("ipc_query_requests_dispatched_total", 1.0),
                ],
            ),
        ];
        for (expected, points) in defects {
            match scrape(vec![Arc::new(FixedSource(points))]) {
                Err(CoreError::Typed { code, message })
                    if code == "METRICS_SOURCE_DEFECT" && message.contains(expected) => {}
                other => {
                    return Err(format!(
                        "expected METRICS_SOURCE_DEFECT mentioning `{expected}`, got {other:?}"
                    )
                    .into());
                }
            }
        }
        Ok(())
    }
}
