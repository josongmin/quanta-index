//! The integrity scrub as quota'd maintenance (QI-BB-017).
//!
//! Every serving door proves a sealed generation by its layout; the bytes
//! are proven here, one bounded scrub step at most every policy interval,
//! over the adapters' [`IntegrityScrubPort`]s. The composition root's
//! maintenance timer drives the steps ([`PacedIntegrityScrubV1`]); there is
//! no second maintenance thread. The scheduler keeps a cursor so a large
//! generation is walked across steps, picks the generation whose completed
//! scrub is the oldest (never-scrubbed first), and on a corruption — which
//! the adapter has already quarantined durably — retires the generation's
//! resident handle on that track from the snapshot registry (fencing an
//! open in flight) so the next query meets the adapter's typed refusal
//! instead of a cached searcher. What it did is scraped as
//! `scrub_runs_total`, `scrub_bytes_total`, `scrub_files_total`,
//! `scrub_corruptions_total`, `scrub_errors_total` and
//! `scrub_last_completed_unix`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use quanta_index_contract::{GenerationSnapshot, SearchPlaneTrackKind};
use quanta_index_core::{
    CoreError, IntegrityScrubCandidateV1, IntegrityScrubCursorV1, IntegrityScrubOutcomeV1,
    IntegrityScrubPolicyV1, IntegrityScrubPort, MetricPointV1, MetricSourcePort,
};
use quanta_index_search_plane::{SnapshotKey, SnapshotRegistries};

/// What the scrub task did since the process started.
#[derive(Debug, Default)]
pub struct ScrubTalliesV1 {
    runs: AtomicU64,
    bytes: AtomicU64,
    files: AtomicU64,
    corruptions: AtomicU64,
    errors: AtomicU64,
    last_completed_unix: AtomicU64,
}

impl ScrubTalliesV1 {
    fn record_step(&self, files: u64, bytes: u64) {
        let _previous = self.runs.fetch_add(1, Ordering::Relaxed);
        let _previous = self.bytes.fetch_add(bytes, Ordering::Relaxed);
        let _previous = self.files.fetch_add(files, Ordering::Relaxed);
    }

    fn record_completed(&self, unix: u64) {
        let _previous = self.last_completed_unix.fetch_max(unix, Ordering::Relaxed);
    }

    fn record_corruption(&self) {
        let _previous = self.corruptions.fetch_add(1, Ordering::Relaxed);
    }

    fn record_error(&self) {
        let _previous = self.errors.fetch_add(1, Ordering::Relaxed);
    }
}

impl MetricSourcePort for ScrubTalliesV1 {
    fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
        Ok(vec![
            MetricPointV1::counter("scrub_runs_total", self.runs.load(Ordering::Relaxed)),
            MetricPointV1::counter("scrub_bytes_total", self.bytes.load(Ordering::Relaxed)),
            MetricPointV1::counter("scrub_files_total", self.files.load(Ordering::Relaxed)),
            MetricPointV1::counter(
                "scrub_corruptions_total",
                self.corruptions.load(Ordering::Relaxed),
            ),
            MetricPointV1::counter("scrub_errors_total", self.errors.load(Ordering::Relaxed)),
            MetricPointV1::gauge_count(
                "scrub_last_completed_unix",
                self.last_completed_unix.load(Ordering::Relaxed),
            ),
        ])
    }
}

/// A scrub in progress: which port, which generation, where to resume.
#[derive(Clone, Debug)]
struct InFlightScrubV1 {
    port: usize,
    generation: GenerationSnapshot,
    cursor: Option<IntegrityScrubCursorV1>,
}

/// The pure scheduling half of the task: what to scrub next and what to
/// do with a report. Owns no thread, so it is tested directly.
pub struct ScrubSchedulerV1 {
    ports: Vec<Arc<dyn IntegrityScrubPort + Send + Sync>>,
    policy: IntegrityScrubPolicyV1,
    snapshots: SnapshotRegistries,
    tallies: Arc<ScrubTalliesV1>,
    in_flight: Option<InFlightScrubV1>,
}

/// What one tick of the scheduler did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScrubTickV1 {
    /// No port names a candidate right now.
    Idle,
    /// One bounded step ran over `generation` and ended as `outcome`.
    Stepped {
        generation: GenerationSnapshot,
        outcome: IntegrityScrubOutcomeV1,
    },
    /// A port could not list its candidates; nothing ran.
    ListingFailed { error: String },
    /// The port refused or failed the step; the generation is dropped
    /// from the schedule until it is listed again.
    Failed {
        generation: GenerationSnapshot,
        error: String,
    },
}

impl ScrubSchedulerV1 {
    #[must_use]
    pub fn new(
        ports: Vec<Arc<dyn IntegrityScrubPort + Send + Sync>>,
        policy: IntegrityScrubPolicyV1,
        snapshots: SnapshotRegistries,
        tallies: Arc<ScrubTalliesV1>,
    ) -> Self {
        Self {
            ports,
            policy,
            snapshots,
            tallies,
            in_flight: None,
        }
    }

    /// The generation to scrub next: a paused one first, else across every
    /// port the candidate never scrubbed to completion, else the one whose
    /// completion is oldest; ties break on the identity so the order is
    /// stable across ticks.
    fn next(&mut self) -> Result<Option<InFlightScrubV1>, CoreError> {
        if let Some(in_flight) = self.in_flight.take() {
            return Ok(Some(in_flight));
        }
        let mut best: Option<(usize, IntegrityScrubCandidateV1)> = None;
        for (port_index, port) in self.ports.iter().enumerate() {
            for candidate in port.scrub_candidates()? {
                let older = best.as_ref().is_none_or(|(_, current)| {
                    match (candidate.last_completed_unix, current.last_completed_unix) {
                        (None, Some(_)) => true,
                        (Some(_), None) => false,
                        (mine, theirs) => {
                            (mine, identity_key(&candidate.identity))
                                < (theirs, identity_key(&current.identity))
                        }
                    }
                });
                if older {
                    best = Some((port_index, candidate));
                }
            }
        }
        Ok(best.map(|(port, candidate)| InFlightScrubV1 {
            port,
            generation: candidate.identity,
            cursor: None,
        }))
    }

    /// Run one bounded step.
    pub fn tick(&mut self) -> ScrubTickV1 {
        let in_flight = match self.next() {
            Ok(Some(in_flight)) => in_flight,
            Ok(None) => return ScrubTickV1::Idle,
            Err(error) => {
                self.tallies.record_error();
                return ScrubTickV1::ListingFailed {
                    error: error.to_string(),
                };
            }
        };
        let Some(port) = self.ports.get(in_flight.port) else {
            self.tallies.record_error();
            return ScrubTickV1::Failed {
                generation: in_flight.generation,
                error: "scrub port index out of range".to_string(),
            };
        };
        let report = match port.scrub(&in_flight.generation, in_flight.cursor, self.policy.budget())
        {
            Ok(report) => report,
            Err(error) => {
                self.tallies.record_error();
                return ScrubTickV1::Failed {
                    generation: in_flight.generation,
                    error: error.to_string(),
                };
            }
        };
        self.tallies
            .record_step(report.files_verified, report.bytes_read);
        match &report.outcome {
            IntegrityScrubOutcomeV1::Completed => {
                // A clock before the epoch cannot stamp the completion; it
                // is counted as an error rather than recorded as time zero.
                match SystemTime::now().duration_since(UNIX_EPOCH) {
                    Ok(since_epoch) => self.tallies.record_completed(since_epoch.as_secs()),
                    Err(_before_epoch) => self.tallies.record_error(),
                }
            }
            IntegrityScrubOutcomeV1::Paused { cursor } => {
                self.in_flight = Some(InFlightScrubV1 {
                    port: in_flight.port,
                    generation: in_flight.generation,
                    cursor: Some(*cursor),
                });
            }
            IntegrityScrubOutcomeV1::Corrupt { .. } => {
                self.tallies.record_corruption();
                // The adapter quarantined the generation durably; a resident
                // handle must not keep serving it. A registry that cannot be
                // fenced is counted, not ignored: the next query would still
                // reopen through the adapter once the handle is evicted.
                let key = SnapshotKey::new(
                    &in_flight.generation.repo_id,
                    &in_flight.generation.revision_id,
                    in_flight.generation.manifest_generation,
                );
                let retired = match in_flight.generation.track {
                    SearchPlaneTrackKind::Lexical => self.snapshots.lexical.retire(&key),
                    SearchPlaneTrackKind::Semantic => self.snapshots.semantic.retire(&key),
                    SearchPlaneTrackKind::Structural => Err(CoreError::InvalidContract(
                        "integrity scrub: structural is not a scrubbed track".to_string(),
                    )),
                };
                if retired.is_err() {
                    self.tallies.record_error();
                }
            }
        }
        ScrubTickV1::Stepped {
            generation: report.generation,
            outcome: report.outcome,
        }
    }
}

fn identity_key(identity: &GenerationSnapshot) -> (String, String, u64) {
    (
        identity.repo_id.as_str().to_string(),
        identity.revision_id.as_str().to_string(),
        identity.manifest_generation.get(),
    )
}

/// The scheduler behind the maintenance timer: at most one step per
/// policy interval, however often the timer ticks.
///
/// The first step is due one interval after `started`, not at once: a
/// daemon that has just booted is answering its first queries and
/// activations, and the scrub is background proof, not a boot gate.
pub struct PacedIntegrityScrubV1 {
    scheduler: ScrubSchedulerV1,
    interval: Duration,
    last_step: Instant,
}

impl PacedIntegrityScrubV1 {
    #[must_use]
    pub fn new(
        scheduler: ScrubSchedulerV1,
        policy: IntegrityScrubPolicyV1,
        started: Instant,
    ) -> Self {
        Self {
            scheduler,
            interval: Duration::from_millis(policy.interval_millis),
            last_step: started,
        }
    }

    /// Run one scheduler step if the interval has passed since the last
    /// one (or since the start); `None` when it was not due at `now`.
    pub fn step_if_due(&mut self, now: Instant) -> Option<ScrubTickV1> {
        if now.saturating_duration_since(self.last_step) < self.interval {
            return None;
        }
        self.last_step = now;
        Some(self.scheduler.tick())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use quanta_index_contract::{
        ClusterMembershipBatchReadRequestV1, ClusterMembershipBatchReadResponseV1,
        LexicalCandidate, ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
        SemanticCorpusKindV1,
    };
    use quanta_index_core::{
        DenseIndexV1, DenseLaneAttestationV1, DenseLaneContractV1, GenerationQuarantineReasonV1,
        IntegrityScrubBudgetV1, IntegrityScrubReportV1, QuarantinedGenerationV1, RequestBudgetV1,
        SemanticSearchHitV1, SemanticSearcher,
    };
    use quanta_index_search_plane::{OpenedSnapshot, SnapshotRegistryPolicy};

    use super::*;

    /// A resident semantic handle that answers nothing: only its residency
    /// in the registry matters to these tests.
    struct ResidentHandle;

    fn never() -> CoreError {
        CoreError::NotImplemented("resident handle double".to_string())
    }

    impl SemanticSearcher for ResidentHandle {
        fn resident_bytes_estimate(&self) -> u64 {
            1
        }

        fn score_candidate(
            &self,
            _candidate_id: &str,
            _query_vector: &[f32],
            _budget: &RequestBudgetV1,
        ) -> Result<Option<f32>, CoreError> {
            Err(never())
        }

        fn manifest_digest(&self) -> &'static str {
            "manifest-resident"
        }

        fn cluster_membership_batch_read(
            &self,
            _request: &ClusterMembershipBatchReadRequestV1,
        ) -> Result<ClusterMembershipBatchReadResponseV1, CoreError> {
            Err(never())
        }

        fn search(
            &self,
            _query_vector: &[f32],
            _top_k: u32,
            _budget: &RequestBudgetV1,
        ) -> Result<Vec<LexicalCandidate>, CoreError> {
            Err(never())
        }

        fn search_hits(
            &self,
            _query_vector: &[f32],
            _top_k: u32,
            _budget: &RequestBudgetV1,
        ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
            Err(never())
        }

        fn search_hits_for_corpus(
            &self,
            _query_vector: &[f32],
            _corpus_kind: SemanticCorpusKindV1,
            _top_k: u32,
            _budget: &RequestBudgetV1,
        ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
            Err(never())
        }

        fn search_scoped(
            &self,
            _query_vector: &[f32],
            _allowed_ids: &std::collections::BTreeSet<String>,
            _top_k: u32,
            _budget: &RequestBudgetV1,
        ) -> Result<Vec<LexicalCandidate>, CoreError> {
            Err(never())
        }

        fn index_model_id(&self) -> &'static str {
            "double"
        }

        fn index_model_revision(&self) -> Option<&str> {
            None
        }

        fn dense_lane(&self) -> DenseLaneContractV1 {
            DenseLaneContractV1 {
                index: DenseIndexV1::Exact,
                attestation: DenseLaneAttestationV1::Sealed,
            }
        }
    }

    fn identity(generation: u64) -> GenerationSnapshot {
        GenerationSnapshot {
            repo_id: RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev")
                .expect("static fixture ID satisfies canonical policy"),
            track: SearchPlaneTrackKind::Semantic,
            manifest_generation: ManifestGeneration::new(generation),
            manifest_digest: format!("manifest:{generation}"),
        }
    }

    /// A port whose candidates and per-step outcomes are scripted; it
    /// records every call it received.
    struct ScriptedPort {
        candidates: Vec<IntegrityScrubCandidateV1>,
        outcomes: Mutex<Vec<Result<IntegrityScrubOutcomeV1, CoreError>>>,
        calls: Mutex<Vec<(u64, Option<u64>, u64)>>,
    }

    impl IntegrityScrubPort for ScriptedPort {
        fn scrub_candidates(&self) -> Result<Vec<IntegrityScrubCandidateV1>, CoreError> {
            Ok(self.candidates.clone())
        }

        fn scrub(
            &self,
            generation: &GenerationSnapshot,
            cursor: Option<IntegrityScrubCursorV1>,
            budget: IntegrityScrubBudgetV1,
        ) -> Result<IntegrityScrubReportV1, CoreError> {
            self.calls.lock().expect("calls").push((
                generation.manifest_generation.get(),
                cursor.map(|cursor| cursor.next_artifact),
                budget.max_bytes,
            ));
            let outcome = self.outcomes.lock().expect("outcomes").remove(0)?;
            Ok(IntegrityScrubReportV1 {
                generation: generation.clone(),
                files_verified: 1,
                bytes_read: 10,
                outcome,
            })
        }
    }

    fn scheduler(
        port: Arc<ScriptedPort>,
    ) -> (ScrubSchedulerV1, Arc<ScrubTalliesV1>, SnapshotRegistries) {
        let tallies = Arc::new(ScrubTalliesV1::default());
        let policy = IntegrityScrubPolicyV1 {
            interval_millis: 1,
            max_bytes_per_step: 7,
        };
        let snapshots = SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT);
        (
            ScrubSchedulerV1::new(vec![port], policy, snapshots.clone(), Arc::clone(&tallies)),
            tallies,
            snapshots,
        )
    }

    /// Never-scrubbed generations go first, then the oldest completion; a
    /// paused generation is resumed from its cursor before anything else
    /// is picked; the policy's budget reaches the port on every step.
    #[test]
    fn the_scheduler_resumes_a_paused_scrub_and_prefers_the_stalest_candidate() {
        let port = Arc::new(ScriptedPort {
            candidates: vec![
                IntegrityScrubCandidateV1 {
                    identity: identity(1),
                    last_completed_unix: Some(100),
                },
                IntegrityScrubCandidateV1 {
                    identity: identity(2),
                    last_completed_unix: None,
                },
                IntegrityScrubCandidateV1 {
                    identity: identity(3),
                    last_completed_unix: Some(50),
                },
            ],
            outcomes: Mutex::new(vec![
                Ok(IntegrityScrubOutcomeV1::Paused {
                    cursor: IntegrityScrubCursorV1 { next_artifact: 4 },
                }),
                Ok(IntegrityScrubOutcomeV1::Completed),
                Ok(IntegrityScrubOutcomeV1::Completed),
            ]),
            calls: Mutex::new(Vec::new()),
        });
        let (mut scheduler, tallies, _snapshots) = scheduler(Arc::clone(&port));
        let first = scheduler.tick();
        assert!(matches!(
            first,
            ScrubTickV1::Stepped { ref generation, outcome: IntegrityScrubOutcomeV1::Paused { .. } }
                if generation.manifest_generation.get() == 2
        ));
        let second = scheduler.tick();
        assert!(matches!(
            second,
            ScrubTickV1::Stepped { ref generation, outcome: IntegrityScrubOutcomeV1::Completed }
                if generation.manifest_generation.get() == 2
        ));
        // With g2 still listed as never completed by the scripted port, it
        // is picked again: the port's receipts are the authority, not the
        // scheduler's memory.
        let third = scheduler.tick();
        assert!(matches!(third, ScrubTickV1::Stepped { .. }));
        assert_eq!(
            *port.calls.lock().expect("calls"),
            vec![(2, None, 7), (2, Some(4), 7), (2, None, 7)]
        );
        let points = tallies.scrape().expect("scrape");
        let value = |name: &str| {
            points
                .iter()
                .find(|point| point.name == name)
                .map(|point| point.value)
        };
        assert_eq!(value("scrub_runs_total"), Some(quanta_index_core::MetricValueV1::Counter(3)));
        assert_eq!(value("scrub_bytes_total"), Some(quanta_index_core::MetricValueV1::Counter(30)));
        assert!(matches!(
            value("scrub_last_completed_unix"),
            Some(quanta_index_core::MetricValueV1::Gauge(unix)) if unix > 0.0
        ));
    }

    /// A corruption is counted and the resident handle retired.
    ///
    /// The registry no longer holds the generation the scrub quarantined, so
    /// the next query reopens through the adapter's typed refusal; a port
    /// error is counted and the generation dropped from flight.
    #[test]
    fn a_corruption_is_counted_and_fences_the_resident_handle() {
        let port = Arc::new(ScriptedPort {
            candidates: vec![IntegrityScrubCandidateV1 {
                identity: identity(5),
                last_completed_unix: None,
            }],
            outcomes: Mutex::new(vec![
                Ok(IntegrityScrubOutcomeV1::Corrupt {
                    quarantined: QuarantinedGenerationV1 {
                        track: SearchPlaneTrackKind::Semantic,
                        path: std::path::PathBuf::from("/state/g5"),
                        reason: GenerationQuarantineReasonV1::ContentCorrupt,
                        detail: "digest differs".to_string(),
                    },
                }),
                Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationQuarantined,
                    message: "quarantined".to_string(),
                }),
                Ok(IntegrityScrubOutcomeV1::Completed),
            ]),
            calls: Mutex::new(Vec::new()),
        });
        let (mut scheduler, tallies, snapshots) = scheduler(Arc::clone(&port));
        // A handle for g5 is resident, as a query before the scrub leaves it.
        let key = SnapshotKey::new(
            &RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
            &RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
            ManifestGeneration::new(5),
        );
        let acquired = snapshots
            .semantic
            .acquire(&key, &RequestBudgetV1::unbounded(), || {
                Ok(OpenedSnapshot {
                    handle: Arc::new(ResidentHandle),
                    resident_bytes: 1,
                })
            })
            .expect("acquire");
        drop(acquired);
        assert_eq!(snapshots.semantic.stats().expect("stats").entries, 1);
        assert!(matches!(
            scheduler.tick(),
            ScrubTickV1::Stepped {
                outcome: IntegrityScrubOutcomeV1::Corrupt { .. },
                ..
            }
        ));
        let stats = snapshots.semantic.stats().expect("stats");
        assert_eq!(
            (stats.entries, stats.retirements),
            (0, 1),
            "the quarantined generation's handle is retired"
        );
        assert!(matches!(scheduler.tick(), ScrubTickV1::Failed { .. }));
        assert!(matches!(
            scheduler.tick(),
            ScrubTickV1::Stepped {
                outcome: IntegrityScrubOutcomeV1::Completed,
                ..
            }
        ));
        let points = tallies.scrape().expect("scrape");
        let counter = |name: &str| {
            points
                .iter()
                .find(|point| point.name == name)
                .map(|point| point.value)
        };
        assert_eq!(
            counter("scrub_corruptions_total"),
            Some(quanta_index_core::MetricValueV1::Counter(1))
        );
        assert_eq!(
            counter("scrub_errors_total"),
            Some(quanta_index_core::MetricValueV1::Counter(1))
        );
        assert_eq!(counter("scrub_runs_total"), Some(quanta_index_core::MetricValueV1::Counter(2)));
    }

    /// The first step is due one interval after the start, and after that
    /// at most one step runs per interval however often the timer ticks.
    #[test]
    fn the_pacing_waits_one_interval_from_the_start_and_between_steps() {
        let port = Arc::new(ScriptedPort {
            candidates: Vec::new(),
            outcomes: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
        });
        let (scheduler, _tallies, _snapshots) = scheduler(port);
        let policy = IntegrityScrubPolicyV1::new(1_000, 4_096).expect("policy");
        let started = Instant::now();
        let at = |millis: u64| {
            started
                .checked_add(Duration::from_millis(millis))
                .expect("instant in range")
        };
        let mut paced = PacedIntegrityScrubV1::new(scheduler, policy, started);
        assert_eq!(paced.step_if_due(started), None, "not due at boot");
        assert_eq!(paced.step_if_due(at(999)), None);
        assert_eq!(paced.step_if_due(at(1_000)), Some(ScrubTickV1::Idle));
        assert_eq!(paced.step_if_due(at(1_000)), None, "one step per interval");
        assert_eq!(paced.step_if_due(at(1_999)), None);
        assert_eq!(paced.step_if_due(at(2_000)), Some(ScrubTickV1::Idle));
    }

    /// With no candidates the scheduler idles and touches nothing.
    #[test]
    fn an_empty_inventory_idles() {
        let port = Arc::new(ScriptedPort {
            candidates: Vec::new(),
            outcomes: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
        });
        let (mut scheduler, tallies, _snapshots) = scheduler(port);
        assert_eq!(scheduler.tick(), ScrubTickV1::Idle);
        let points = tallies.scrape().expect("scrape");
        assert!(points.iter().all(|point| match point.value {
            quanta_index_core::MetricValueV1::Counter(value) => value == 0,
            quanta_index_core::MetricValueV1::Gauge(value) => value == 0.0,
        }));
    }
}
