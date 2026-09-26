//! Purpose-specific measurement payloads.
//!
//! One envelope, purpose-specific payloads. The payload is the *structure* of what
//! was measured; the registry family is the *question*. A payload kind is
//! never reinterpreted: a retrieval payload cannot be read as a latency row,
//! an instruction count cannot be read as wall latency, and an agent outcome
//! cannot be inferred from a retrieval score.

use crate::codec::Wire as _;
use crate::error::ProtocolError;

/// Unit vocabulary accepted by the typed payloads.
pub const UNIT_MS: &str = "ms";
/// Ratio in `[0, 1]` (NDCG, MRR, recall, win-rate).
pub const UNIT_RATIO: &str = "ratio";
/// Dimensionless count.
pub const UNIT_COUNT: &str = "count";
/// Nanoseconds (Criterion wall statistic).
pub const UNIT_NS: &str = "ns";
/// Retired-instruction count (Iai-Callgrind); never wall-clock.
pub const UNIT_INSTRUCTIONS: &str = "instructions";
/// Queries per second.
pub const UNIT_QPS: &str = "qps";
/// Bytes.
pub const UNIT_BYTES: &str = "bytes";

/// A single scalar metric with explicit unit and provenance counts.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricValue {
    /// Metric name, e.g. `ndcg@10`.
    pub name: String,
    /// Unit from the contract vocabulary.
    pub unit: String,
    /// Observed value.
    pub value: f64,
    /// Numerator, when the metric is a rate.
    pub numerator: Option<u64>,
    /// Denominator, when the metric is a rate.
    pub denominator: Option<u64>,
}
crate::impl_wire!(MetricValue {
    name,
    unit,
    value,
    numerator,
    denominator,
});

impl MetricValue {
    /// Validate the unit vocabulary and numerator/denominator consistency.
    pub fn validate(&self, where_: &str) -> Result<(), ProtocolError> {
        check_unit(&self.unit, where_)?;
        match (self.numerator, self.denominator) {
            (Some(numerator), Some(denominator)) => {
                if numerator > denominator {
                    return Err(ProtocolError::semantic(format!(
                        "{where_}: numerator {numerator} exceeds denominator {denominator}"
                    )));
                }
                Ok(())
            }
            (None, None) => Ok(()),
            _ => Err(ProtocolError::semantic(format!(
                "{where_}: numerator and denominator must be present together"
            ))),
        }
    }
}

/// Criterion / microbenchmark statistic.
#[derive(Debug, Clone, PartialEq)]
pub struct MicroPayload {
    /// Criterion benchmark id.
    pub bench_id: String,
    /// Metric name, e.g. `mean`.
    pub metric: String,
    /// `ns` for wall timing, `instructions` for retired-instruction counts.
    pub unit: String,
    /// `wall` or `instructions`; must agree with `unit`.
    pub instrumentation: String,
    /// `mean`, `median` or `min`.
    pub statistic: String,
    /// Observed value.
    pub value: f64,
    /// Criterion iteration count.
    pub iterations: u64,
    /// Number of independent measurement samples.
    pub samples: u64,
}
crate::impl_wire!(MicroPayload tag "micro" {
    bench_id, metric, unit, instrumentation, statistic, value, iterations, samples,
});

impl MicroPayload {
    fn validate(&self) -> Result<(), ProtocolError> {
        let expected_unit = match self.instrumentation.as_str() {
            "wall" => UNIT_NS,
            "instructions" => UNIT_INSTRUCTIONS,
            other => {
                return Err(ProtocolError::semantic(format!(
                    "micro.instrumentation {other:?} is not wall or instructions"
                )));
            }
        };
        if self.unit != expected_unit {
            return Err(ProtocolError::semantic(format!(
                "micro unit {:?} contradicts instrumentation {:?}; expected {expected_unit:?}",
                self.unit, self.instrumentation
            )));
        }
        if !matches!(self.statistic.as_str(), "mean" | "median" | "min") {
            return Err(ProtocolError::semantic(format!(
                "micro.statistic {:?} is not mean/median/min",
                self.statistic
            )));
        }
        if self.samples == 0 {
            return Err(ProtocolError::semantic(
                "micro.samples must be at least 1".to_owned(),
            ));
        }
        Ok(())
    }
}

/// One measured case in a latency payload.
#[derive(Debug, Clone, PartialEq)]
pub struct LatencyRow {
    /// Stable scenario/case id.
    pub case_id: String,
    /// Metric name, e.g. `p50`.
    pub metric: String,
    /// `ms` for wall latency; quality rails use `ratio`/`count`.
    pub unit: String,
    /// Number of pooled samples.
    pub samples: u64,
    /// p50, absent when the row was not measured.
    pub p50: Option<f64>,
    /// p95, absent when the row was not measured.
    pub p95: Option<f64>,
    /// p99, absent when the row was not measured.
    pub p99: Option<f64>,
    /// Failed requests, never silently dropped from the sample set.
    pub error_count: u64,
    /// Timed-out requests.
    pub timeout_count: u64,
    /// Set when the row was *not* measured; percentiles are then absent.
    pub early_stop_reason: Option<String>,
}
crate::impl_wire!(LatencyRow {
    case_id,
    metric,
    unit,
    samples,
    p50,
    p95,
    p99,
    error_count,
    timeout_count,
    early_stop_reason,
});

impl LatencyRow {
    fn validate(&self) -> Result<(), ProtocolError> {
        check_unit(&self.unit, "latency.rows[].unit")?;
        if let Some(reason) = &self.early_stop_reason {
            if self.p50.is_some() || self.p95.is_some() || self.p99.is_some() {
                return Err(ProtocolError::semantic(format!(
                    "latency row {:?} is marked unmeasured ({reason}) but carries percentiles",
                    self.case_id
                )));
            }
            return Ok(());
        }
        if self.samples == 0 {
            return Err(ProtocolError::semantic(format!(
                "latency row {:?} has zero samples and no early_stop_reason",
                self.case_id
            )));
        }
        if self.p50.is_none() {
            return Err(ProtocolError::semantic(format!(
                "latency row {:?} has samples but no measured p50",
                self.case_id
            )));
        }
        Ok(())
    }
}

/// Wall-latency (and quality-scenario latency) rows.
#[derive(Debug, Clone, PartialEq)]
pub struct LatencyPayload {
    /// Measured rows.
    pub rows: Vec<LatencyRow>,
    /// Failed operations recorded during the capture.
    pub errors: u64,
    /// Timed-out operations recorded during the capture.
    pub timeouts: u64,
    /// Dropped operations recorded during the capture.
    pub drops: u64,
}
crate::impl_wire!(LatencyPayload tag "latency" {
    rows, errors, timeouts, drops,
});

impl LatencyPayload {
    fn validate(&self) -> Result<(), ProtocolError> {
        if self.rows.is_empty() {
            return Err(ProtocolError::semantic(
                "latency.rows must not be empty".to_owned(),
            ));
        }
        let mut seen: Vec<&str> = Vec::with_capacity(self.rows.len());
        for row in &self.rows {
            row.validate()?;
            if seen.contains(&row.case_id.as_str()) {
                return Err(ProtocolError::semantic(format!(
                    "latency.rows repeats case_id {:?}",
                    row.case_id
                )));
            }
            seen.push(&row.case_id);
        }
        Ok(())
    }
}

/// One offered-load operating point.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadPoint {
    /// Operating-point label, e.g. `rate-200`.
    pub label: String,
    /// Offered arrival rate; required for open-loop points.
    pub offered_rate: Option<f64>,
    /// Completed rate observed by the generator.
    pub completed_rate: f64,
    /// Requests the generator could not admit; capacity loss, not a sample.
    pub dropped: u64,
    /// Requests that did not complete before the deadline.
    pub timeouts: u64,
}
crate::impl_wire!(LoadPoint {
    label,
    offered_rate,
    completed_rate,
    dropped,
    timeouts,
});

/// Offered-load (open-loop capacity) or closed-loop throughput.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadPayload {
    /// `open_loop` or `closed_loop`.
    pub arrival: String,
    /// True when the generator could not keep up with the offered rate.
    pub generator_saturated: bool,
    /// Operating points.
    pub points: Vec<LoadPoint>,
    /// Transport errors observed while measuring.
    pub errors: u64,
}
crate::impl_wire!(LoadPayload tag "load" {
    arrival, generator_saturated, points, errors,
});

impl LoadPayload {
    fn validate(&self) -> Result<(), ProtocolError> {
        if self.points.is_empty() {
            return Err(ProtocolError::semantic(
                "load.points must not be empty".to_owned(),
            ));
        }
        match self.arrival.as_str() {
            "open_loop" => {
                for point in &self.points {
                    if point.offered_rate.is_none() {
                        return Err(ProtocolError::semantic(format!(
                            "open-loop point {:?} has no offered_rate; closed-loop throughput \
                             cannot be reported as offered capacity",
                            point.label
                        )));
                    }
                }
            }
            "closed_loop" => {
                for point in &self.points {
                    if point.offered_rate.is_some() {
                        return Err(ProtocolError::semantic(format!(
                            "closed-loop point {:?} claims an offered_rate",
                            point.label
                        )));
                    }
                }
            }
            other => {
                return Err(ProtocolError::semantic(format!(
                    "load.arrival {other:?} is not open_loop or closed_loop"
                )));
            }
        }
        Ok(())
    }
}

/// One ingest/freshness phase.
#[derive(Debug, Clone, PartialEq)]
pub struct FreshnessPhase {
    /// `full_build`, `time_to_searchable`, `edit`, `add`, `delete`, `rename`,
    /// `reopen`, `restart` or `replay`.
    pub name: String,
    /// Duration in milliseconds.
    pub ms: f64,
    /// Independent samples behind `ms`.
    pub samples: u64,
}
crate::impl_wire!(FreshnessPhase { name, ms, samples });

/// Ingest freshness / time-to-searchable evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct FreshnessPayload {
    /// Distinct phases; build, update, reopen and replay stay separate.
    pub phases: Vec<FreshnessPhase>,
    /// Queries answered from a stale generation.
    pub stale_hits: u64,
    /// Observable generation identifier after activation.
    pub generation: Option<String>,
}
crate::impl_wire!(FreshnessPayload tag "freshness" {
    phases, stale_hits, generation,
});

impl FreshnessPayload {
    fn validate(&self) -> Result<(), ProtocolError> {
        if self.phases.is_empty() {
            return Err(ProtocolError::semantic(
                "freshness.phases must not be empty".to_owned(),
            ));
        }
        let mut seen: Vec<&str> = Vec::with_capacity(self.phases.len());
        for phase in &self.phases {
            if seen.contains(&phase.name.as_str()) {
                return Err(ProtocolError::semantic(format!(
                    "freshness phase {:?} is duplicated; phases must stay distinct",
                    phase.name
                )));
            }
            seen.push(&phase.name);
        }
        Ok(())
    }
}

/// One per-query retrieval outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalRow {
    /// Stable query id.
    pub query_id: String,
    /// Metric name, e.g. `recall@20`.
    pub metric: String,
    /// Unit from the contract vocabulary.
    pub unit: String,
    /// Value, absent for `unjudged`, `timeout` and `unsupported`.
    pub value: Option<f64>,
    /// `judged`, `irrelevant`, `unjudged`, `no_answer`, `timeout` or
    /// `unsupported`.
    pub state: String,
}
crate::impl_wire!(RetrievalRow {
    query_id,
    metric,
    unit,
    value,
    state,
});

/// Retrieval relevance evidence in an explicit metric space.
#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalPayload {
    /// `native_default` or `controlled_mechanism`; never averaged together.
    pub lane: String,
    /// `file`, `line`, `span` or `context`.
    pub metric_space: String,
    /// `judged`, `pooled` or `mechanically_labeled`.
    pub judgments: String,
    /// Queries with no judgment; distinct from irrelevant.
    pub unjudged: u64,
    /// Per-query rows, in the frozen query-pack order.
    pub rows: Vec<RetrievalRow>,
    /// True only when the product's searchable universe was independently proven.
    pub universe_attested: bool,
    /// Digest of the exact corpus view used by every compared product.
    pub corpus_digest: String,
    /// Digest of the frozen query pack.
    pub query_pack_digest: String,
}
crate::impl_wire!(RetrievalPayload tag "retrieval" {
    lane, metric_space, judgments, unjudged, rows, universe_attested, corpus_digest, query_pack_digest,
});

impl RetrievalPayload {
    fn validate(&self) -> Result<(), ProtocolError> {
        if !matches!(
            self.lane.as_str(),
            "native_default" | "controlled_mechanism"
        ) {
            return Err(ProtocolError::semantic(format!(
                "retrieval.lane {:?} is not native_default or controlled_mechanism",
                self.lane
            )));
        }
        if !matches!(
            self.metric_space.as_str(),
            "file" | "line" | "span" | "context"
        ) {
            return Err(ProtocolError::semantic(format!(
                "retrieval.metric_space {:?} is not file/line/span/context",
                self.metric_space
            )));
        }
        if !matches!(
            self.judgments.as_str(),
            "judged" | "pooled" | "mechanically_labeled"
        ) {
            return Err(ProtocolError::semantic(format!(
                "retrieval.judgments {:?} is not judged/pooled/mechanically_labeled",
                self.judgments
            )));
        }
        if self.metric_space == "span" && self.judgments == "mechanically_labeled" {
            return Err(ProtocolError::semantic(
                "file-only mechanical labels cannot become span judgments".to_owned(),
            ));
        }
        crate::wire::require_digest("retrieval.corpus_digest", &self.corpus_digest)?;
        crate::wire::require_digest("retrieval.query_pack_digest", &self.query_pack_digest)?;
        if self.rows.is_empty() {
            return Err(ProtocolError::semantic(
                "retrieval.rows must not be empty".to_owned(),
            ));
        }
        let mut seen: Vec<&str> = Vec::with_capacity(self.rows.len());
        for row in &self.rows {
            check_unit(&row.unit, "retrieval.rows[].unit")?;
            match row.state.as_str() {
                "judged" | "irrelevant" | "no_answer" => {
                    if row.value.is_none() {
                        return Err(ProtocolError::semantic(format!(
                            "retrieval row {:?} is {:?} but carries no value",
                            row.query_id, row.state
                        )));
                    }
                }
                "unjudged" | "timeout" | "unsupported" => {
                    if row.value.is_some() {
                        return Err(ProtocolError::semantic(format!(
                            "retrieval row {:?} is {:?} and must not carry a score",
                            row.query_id, row.state
                        )));
                    }
                }
                other => {
                    return Err(ProtocolError::semantic(format!(
                        "retrieval row {:?} has unknown state {other:?}",
                        row.query_id
                    )));
                }
            }
            if seen.contains(&row.query_id.as_str()) {
                return Err(ProtocolError::semantic(format!(
                    "retrieval.rows repeats query_id {:?}",
                    row.query_id
                )));
            }
            seen.push(&row.query_id);
        }
        Ok(())
    }
}

/// Recorded A/B/C agent outcome evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentOutcomePayload {
    /// Distinct tasks represented.
    pub task_count: u64,
    /// Complete A/B/C triples.
    pub pair_count: u64,
    /// Arm labels; exactly `A`, `B` and `C`.
    pub arms: Vec<String>,
    /// Pairs excluded from the aggregate, with a reason recorded upstream.
    pub excluded_pairs: u64,
    /// Pairs with unknown verification.
    pub unknown_pairs: u64,
    /// Domain metrics; the evaluator keeps numerator/denominator.
    pub metrics: Vec<MetricValue>,
    /// `recorded_unauthenticated` or `authenticated`; never inferred.
    pub capture: String,
    /// Digest of the exact input JSONL bytes.
    pub input_digest: String,
}
crate::impl_wire!(AgentOutcomePayload tag "agent_outcome" {
    task_count, pair_count, arms, excluded_pairs, unknown_pairs, metrics, capture, input_digest,
});

impl AgentOutcomePayload {
    fn validate(&self) -> Result<(), ProtocolError> {
        if self.arms != ["A", "B", "C"] {
            return Err(ProtocolError::semantic(format!(
                "agent_outcome.arms must be exactly [A, B, C], found {:?}",
                self.arms
            )));
        }
        if !matches!(
            self.capture.as_str(),
            "recorded_unauthenticated" | "authenticated"
        ) {
            return Err(ProtocolError::semantic(format!(
                "agent_outcome.capture {:?} is not recorded_unauthenticated or authenticated",
                self.capture
            )));
        }
        crate::wire::require_digest("agent_outcome.input_digest", &self.input_digest)?;
        if self.pair_count == 0 {
            return Err(ProtocolError::semantic(
                "agent_outcome.pair_count must be at least 1".to_owned(),
            ));
        }
        for metric in &self.metrics {
            metric.validate("agent_outcome.metrics[]")?;
        }
        Ok(())
    }
}

/// One recorded experiment point.
#[derive(Debug, Clone, PartialEq)]
pub struct ExperimentPoint {
    /// Point label, e.g. a corpus size.
    pub label: String,
    /// Metric name.
    pub metric: String,
    /// Unit from the contract vocabulary.
    pub unit: String,
    /// Observed value.
    pub value: f64,
}
crate::impl_wire!(ExperimentPoint {
    label,
    metric,
    unit,
    value,
});

/// Recorded experiment evidence (no producer may fabricate the input).
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedExperimentPayload {
    /// Stable experiment id.
    pub experiment_id: String,
    /// Recorded experiments are diagnostic unless a producer and oracle exist.
    pub diagnostic_only: bool,
    /// Recorded points.
    pub points: Vec<ExperimentPoint>,
    /// Digest of the exact recorded input.
    pub source_digest: String,
}
crate::impl_wire!(RecordedExperimentPayload tag "recorded_experiment" {
    experiment_id, diagnostic_only, points, source_digest,
});

impl RecordedExperimentPayload {
    fn validate(&self) -> Result<(), ProtocolError> {
        crate::wire::require_digest("recorded_experiment.source_digest", &self.source_digest)?;
        if !self.diagnostic_only {
            return Err(ProtocolError::semantic(
                "recorded_experiment.diagnostic_only must be true; a recorded input is not a \
                 qualified measurement"
                    .to_owned(),
            ));
        }
        if self.points.is_empty() {
            return Err(ProtocolError::semantic(
                "recorded_experiment.points must not be empty".to_owned(),
            ));
        }
        for point in &self.points {
            check_unit(&point.unit, "recorded_experiment.points[].unit")?;
        }
        Ok(())
    }
}

/// Terminal test proof counts, never retrieval relevance or timing metrics.
#[derive(Debug, Clone, PartialEq)]
pub struct ProofPayload {
    /// Registered verification rail.
    pub rail: String,
    /// Exact independently collected test inventory count.
    pub selected: u64,
    /// Terminal executed test count.
    pub executed: u64,
    /// Terminal successful test count.
    pub passed: u64,
    /// Terminal failed test count.
    pub failed: u64,
    /// Bound source closure digest.
    pub source_digest: String,
    /// Digest of the raw command/execution context.
    pub execution_context_digest: String,
}
crate::impl_wire!(ProofPayload tag "proof" {
    rail, selected, executed, passed, failed, source_digest, execution_context_digest,
});

impl ProofPayload {
    fn validate(&self) -> Result<(), ProtocolError> {
        if self.rail.is_empty()
            || self.selected == 0
            || self.executed != self.selected
            || self.passed.checked_add(self.failed) != Some(self.executed)
        {
            return Err(ProtocolError::semantic(
                "proof requires a nonempty rail and complete consistent terminal counts".to_owned(),
            ));
        }
        crate::wire::require_digest("proof.source_digest", &self.source_digest)?;
        crate::wire::require_digest(
            "proof.execution_context_digest",
            &self.execution_context_digest,
        )
    }
}

/// The typed measurement payload of one evidence document.
///
/// The wire form is a JSON object whose `kind` field selects exactly one
/// variant; each variant's own decoder then rejects unknown or missing fields.
#[derive(Debug, Clone, PartialEq)]
pub enum Payload {
    /// Criterion-style microbenchmark statistic.
    Micro(MicroPayload),
    /// Wall-latency or quality-scenario rows.
    Latency(LatencyPayload),
    /// Offered-load or closed-loop throughput.
    Load(LoadPayload),
    /// Ingest freshness.
    Freshness(FreshnessPayload),
    /// Retrieval relevance.
    Retrieval(RetrievalPayload),
    /// Recorded agent outcome.
    AgentOutcome(AgentOutcomePayload),
    /// Recorded experiment.
    RecordedExperiment(RecordedExperimentPayload),
    /// Terminal verification counts, not relevance metrics.
    Proof(ProofPayload),
}

impl Payload {
    /// Wire `kind` discriminator.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Micro(_) => "micro",
            Self::Latency(_) => "latency",
            Self::Load(_) => "load",
            Self::Freshness(_) => "freshness",
            Self::Retrieval(_) => "retrieval",
            Self::AgentOutcome(_) => "agent_outcome",
            Self::RecordedExperiment(_) => "recorded_experiment",
            Self::Proof(_) => "proof",
        }
    }

    /// Decode the variant selected by the `kind` field.
    fn decode_kind(value: &crate::codec::JsonValue) -> Result<Self, ProtocolError> {
        let object = crate::codec::fields(value, "payload")?;
        let kind = crate::codec::field(object, "kind", "payload")?
            .as_str()
            .ok_or_else(|| ProtocolError::semantic("payload.kind must be a string".to_owned()))?;
        match kind {
            "micro" => MicroPayload::decode(value).map(Self::Micro),
            "latency" => LatencyPayload::decode(value).map(Self::Latency),
            "load" => LoadPayload::decode(value).map(Self::Load),
            "freshness" => FreshnessPayload::decode(value).map(Self::Freshness),
            "retrieval" => RetrievalPayload::decode(value).map(Self::Retrieval),
            "agent_outcome" => AgentOutcomePayload::decode(value).map(Self::AgentOutcome),
            "recorded_experiment" => {
                RecordedExperimentPayload::decode(value).map(Self::RecordedExperiment)
            }
            "proof" => ProofPayload::decode(value).map(Self::Proof),
            other => Err(ProtocolError::semantic(format!(
                "payload.kind {other:?} is not registered"
            ))),
        }
    }

    /// Per-kind structural validation. Typed payloads cannot be confused.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        match self {
            Self::Micro(value) => value.validate(),
            Self::Latency(value) => value.validate(),
            Self::Load(value) => value.validate(),
            Self::Freshness(value) => value.validate(),
            Self::Retrieval(value) => value.validate(),
            Self::AgentOutcome(value) => value.validate(),
            Self::RecordedExperiment(value) => value.validate(),
            Self::Proof(value) => value.validate(),
        }
    }
}

impl crate::codec::Wire for Payload {
    fn encode(&self) -> Result<crate::codec::JsonValue, ProtocolError> {
        match self {
            Self::Micro(value) => value.encode(),
            Self::Latency(value) => value.encode(),
            Self::Load(value) => value.encode(),
            Self::Freshness(value) => value.encode(),
            Self::Retrieval(value) => value.encode(),
            Self::AgentOutcome(value) => value.encode(),
            Self::RecordedExperiment(value) => value.encode(),
            Self::Proof(value) => value.encode(),
        }
    }

    fn decode(value: &crate::codec::JsonValue) -> Result<Self, ProtocolError> {
        Self::decode_kind(value)
    }
}

fn check_unit(unit: &str, where_: &str) -> Result<(), ProtocolError> {
    match unit {
        UNIT_MS | UNIT_RATIO | UNIT_COUNT | UNIT_NS | UNIT_INSTRUCTIONS | UNIT_QPS | UNIT_BYTES => {
            Ok(())
        }
        other => Err(ProtocolError::semantic(format!(
            "{where_}: unit {other:?} is not in the contract vocabulary"
        ))),
    }
}
