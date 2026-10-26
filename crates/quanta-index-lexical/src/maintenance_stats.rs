//! Process-local observations of completed lexical seals.
//!
//! These are per-publication logical run/index facts, not an aggregate over
//! serving generations or unique physical pool bytes. Source bodies, pinned
//! or obsolete objects, process RSS, and I/O capacity are outside this scope.

use quanta_index_core::CoreError;
use quanta_index_core::lexical_maintenance::TargetCounters;

use crate::maintenance_merge_facts::{FileRunMergeFacts, NativeRunMergeFacts};

/// Authenticated old and sealed target counters for one publication.
///
/// File index bytes include attributed segment bundles, run-posting bundles,
/// and the outer run map; Native index bytes cover its attributed segment
/// bundles. Debt fields count map rows, not their encoded bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LexicalMaintenanceObservation {
    pub file_before: TargetCounters,
    pub file_after: TargetCounters,
    pub native_before: TargetCounters,
    pub native_after: TargetCounters,
    pub file_debt_entries: u64,
    pub native_debt_entries: u64,
    /// Verified File run merge attribution; zero means no merge was proved.
    pub file_merge: FileRunMergeFacts,
    /// Verified Native input; its optional output marks attribution availability.
    pub native_merge: NativeRunMergeFacts,
}

/// Completed-seal count and the most recent seal's per-authority counters.
///
/// The last observation is absent until this process seals a publication; it
/// must not be presented as a process-wide active-corpus total after restart.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LexicalMaintenanceStats {
    pub sealed_publications: u64,
    pub last_observation: Option<LexicalMaintenanceObservation>,
}

impl LexicalMaintenanceStats {
    /// Record only after the caller has durably sealed and verified both roots.
    pub(crate) fn record_sealed(
        &mut self,
        observation: LexicalMaintenanceObservation,
    ) -> Result<(), CoreError> {
        observation.file_merge.validate()?;
        observation.native_merge.validate()?;
        let sealed_publications = self.sealed_publications.checked_add(1).ok_or_else(|| {
            CoreError::Storage("lexical maintenance sealed publication count overflow".into())
        })?;
        *self = Self {
            sealed_publications,
            last_observation: Some(observation),
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{LexicalMaintenanceObservation, LexicalMaintenanceStats};
    use crate::current_source_fixture;
    use crate::maintenance_merge_facts::{FileRunMergeFacts, NativeRunMergeFacts};
    use crate::publication_fixture::FixtureBatchBuildPort;
    use quanta_index_core::lexical_maintenance::TargetCounters;
    use quanta_index_core::{MetricSourcePort, MetricValueV1};

    fn batch(generation: u64) -> quanta_index_contract::SearchCorpusIngestBatch {
        use quanta_index_contract::lex::LanguageCode;
        use quanta_index_contract::{
            BatchIngestMode, ChunkId, ChunkRecord, ManifestGeneration, RepoId, RepoRelativePath,
            RevisionId, SearchCorpusIngestBatch,
        };
        let language = LanguageCode::new("rust").expect("language");
        let source_repo = RepoId::new("maintenance-source").expect("source repo");
        let body = "alpha beta\n";
        let scope = current_source_fixture::text_scope(
            &source_repo,
            &RevisionId::new("source-revision").expect("source revision"),
            "src/one.rs",
            language.clone(),
            body,
            vec![ChunkRecord {
                chunk_id: ChunkId::new("maintenance-one"),
                repo_relative_path: RepoRelativePath::new("src/one.rs"),
                language,
                start_byte: 0,
                end_byte: u32::try_from(body.len()).expect("body width"),
                start_line: 1,
                end_line: 1,
                text: body.into(),
                structural: None,
                parent_chunk_id: None,
                source_repo_id: Some(source_repo.clone()),
            }],
        )
        .expect("source-bound scope");
        let mut batch = SearchCorpusIngestBatch {
            source_event: current_source_fixture::empty_event(),
            repo_id: RepoId::new("maintenance").expect("repo"),
            revision_id: RevisionId::new("revision").expect("revision"),
            generation: ManifestGeneration::new(generation),
            base_generation: None,
            manifest_digest: format!("maintenance-{generation}"),
            batch_digest: "0".repeat(64),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload: None,
            clear_surfaces: Vec::new(),
            replace_scopes: vec![scope],
            tombstone_scopes: Vec::new(),
            semantic_replace_scopes: Vec::new(),
            semantic_tombstone_scopes: Vec::new(),
            seal: true,
        };
        current_source_fixture::finish_batch(&mut batch).expect("body binding");
        batch
    }

    fn gauge(adapter: &crate::LexicalAdapter, name: &str) -> Option<f64> {
        adapter
            .scrape()
            .expect("metrics snapshot")
            .into_iter()
            .find_map(|point| match point.value {
                MetricValueV1::Gauge(value) if point.name == name => Some(value),
                MetricValueV1::Counter(_) | MetricValueV1::Gauge(_) => None,
            })
    }

    #[test]
    fn adapter_reports_only_new_durable_seals_and_marks_restart_observation_absent() {
        use quanta_index_contract::{BatchIngestMode, ManifestGeneration};
        use quanta_index_core::LexicalMaintenancePolicy;

        let directory = tempfile::tempdir().expect("state root");
        let adapter = crate::LexicalAdapter::with_state_root(directory.path().to_path_buf());
        let available = "lexical_maintenance_last_sealed_observation_available";
        let runs = "lexical_maintenance_last_sealed_file_target_runs";
        assert_eq!(gauge(&adapter, available), Some(0.0));
        assert_eq!(gauge(&adapter, runs), None);
        let sealed = batch(1);
        let _durations = adapter.build_fixture_batch(&sealed).expect("durable seal");
        let stats = adapter.maintenance_stats().expect("sealed stats");
        let observation = stats.last_observation.expect("last sealed publication");
        assert_eq!(stats.sealed_publications, 1);
        assert_eq!(observation.file_before, counters(0, 0, 0));
        assert_eq!(observation.native_before, counters(0, 0, 0));
        assert_eq!(observation.file_after.runs(), 1);
        assert_eq!(observation.file_after.segments(), 1);
        assert_eq!(observation.native_after.runs(), 1);
        assert_eq!(observation.native_after.segments(), 1);
        assert!(observation.file_after.active_encoded_bytes() > 0);
        assert!(observation.native_after.active_encoded_bytes() > 0);
        assert_eq!(gauge(&adapter, available), Some(1.0));
        assert_eq!(gauge(&adapter, runs), Some(1.0));
        let _durations = adapter.build_fixture_batch(&sealed).expect("sealed replay");
        assert_eq!(adapter.maintenance_stats().expect("replay stats"), stats);

        let reopened = crate::LexicalAdapter::with_state_root(directory.path().to_path_buf());
        assert_eq!(gauge(&reopened, available), Some(0.0));
        assert_eq!(gauge(&reopened, runs), None);

        let mut staged = batch(2);
        staged.mode = BatchIngestMode::Delta;
        staged.base_generation = Some(ManifestGeneration::new(1));
        staged.seal = false;
        current_source_fixture::finish_batch(&mut staged).expect("staged body binding");
        let _durations = adapter
            .build_fixture_batch(&staged)
            .expect("unsealed build");
        assert_eq!(adapter.maintenance_stats().expect("staged stats"), stats);

        let adapter = adapter.with_maintenance_policy(
            LexicalMaintenancePolicy::new(1, 1, 1, 1, 2, 1, 1, 1).expect("tiny explicit limits"),
        );
        assert!(matches!(adapter.build_fixture_batch(&batch(3)),
            Err(quanta_index_core::CoreError::Typed { code, .. })
                if code == quanta_index_core::INGEST_RESOURCE_BUDGET_EXCEEDED_CODE));
        assert_eq!(adapter.maintenance_stats().expect("refusal stats"), stats);
    }

    fn counters(runs: u64, segments: u64, bytes: u64) -> TargetCounters {
        TargetCounters::new(runs, segments, bytes).expect("fixed logical counters")
    }

    #[test]
    fn last_sealed_observation_preserves_authority_scope_and_debt_rows() {
        let first = LexicalMaintenanceObservation {
            file_before: counters(0, 0, 0),
            file_after: counters(2, 3, 101),
            native_before: counters(0, 0, 0),
            native_after: counters(1, 2, 47),
            file_debt_entries: 1,
            native_debt_entries: 0,
            file_merge: FileRunMergeFacts::default(),
            native_merge: NativeRunMergeFacts::default(),
        };
        let second = LexicalMaintenanceObservation {
            file_before: counters(2, 3, 101),
            file_after: counters(1, 3, 93),
            native_before: counters(1, 2, 47),
            native_after: counters(1, 2, 52),
            file_debt_entries: 0,
            native_debt_entries: 1,
            file_merge: FileRunMergeFacts::merged(2, 3, 101, 17, 11).expect("fixed File merge"),
            native_merge: NativeRunMergeFacts::default(),
        };
        let mut stats = LexicalMaintenanceStats::default();
        assert_eq!(stats.last_observation, None);
        stats.record_sealed(first).expect("first seal");
        stats.record_sealed(second).expect("second seal");
        assert_eq!(stats.sealed_publications, 2);
        assert_eq!(stats.last_observation, Some(second));
        assert_eq!(second.file_after.runs(), 1);
        assert_eq!(second.file_after.segments(), 3);
        assert_eq!(second.file_after.active_encoded_bytes(), 93);
        assert_eq!(second.native_after.active_encoded_bytes(), 52);
        assert_eq!(second.file_merge.input_runs, 2);
        assert_eq!(second.file_merge.transferred_segment_attributed_bytes, 101);
        assert_eq!(second.native_merge, NativeRunMergeFacts::default());
    }

    #[test]
    fn overflow_keeps_previous_snapshot() {
        let first = LexicalMaintenanceObservation {
            file_before: counters(0, 0, 0),
            file_after: counters(1, 1, 20),
            native_before: counters(0, 0, 0),
            native_after: counters(1, 1, 10),
            file_debt_entries: 0,
            native_debt_entries: 0,
            file_merge: FileRunMergeFacts::default(),
            native_merge: NativeRunMergeFacts::default(),
        };
        let mut stats = LexicalMaintenanceStats::default();
        stats.record_sealed(first).expect("first seal");
        stats.sealed_publications = u64::MAX;
        let before = stats;
        assert!(stats.record_sealed(first).is_err());
        assert_eq!(stats, before);
    }

    #[test]
    fn scrape_omits_unavailable_native_output_and_preserves_verified_zero() {
        let adapter = crate::LexicalAdapter::with_state_root(std::path::PathBuf::new());
        let observation = LexicalMaintenanceObservation {
            file_before: counters(0, 0, 0),
            file_after: counters(0, 0, 0),
            native_before: counters(2, 3, 47),
            native_after: counters(1, 1, 31),
            file_debt_entries: 0,
            native_debt_entries: 0,
            file_merge: FileRunMergeFacts::default(),
            native_merge: NativeRunMergeFacts {
                input_runs: 2,
                input_segments: 3,
                input_index_encoded_bytes: 47,
                output: None,
            },
        };
        adapter
            .record_maintenance_observation(observation)
            .expect("verified observation");
        let available = "lexical_maintenance_last_sealed_native_merge_output_available";
        let output = "lexical_maintenance_last_sealed_native_merge_output_attributed_index_bytes";
        let inputs = "lexical_maintenance_last_sealed_native_merge_input_attributed_index_bytes";
        assert_eq!(gauge(&adapter, available), Some(0.0));
        assert_eq!(gauge(&adapter, output), None);
        assert_eq!(gauge(&adapter, inputs), Some(47.0));

        adapter
            .record_maintenance_observation(LexicalMaintenanceObservation {
                native_merge: NativeRunMergeFacts::default(),
                ..observation
            })
            .expect("verified no merge");
        assert_eq!(gauge(&adapter, available), Some(1.0));
        assert_eq!(gauge(&adapter, output), Some(0.0));
        assert_eq!(gauge(&adapter, inputs), Some(0.0));
    }
}
