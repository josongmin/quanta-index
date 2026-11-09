//! Versioned diagnostic JSONL wire adapter.
//!
//! V5 shortens only the event envelope and the three common kinds. The
//! returned value always has the original named-field shape; semantic trace
//! authority (schema, span lifecycle, sequence and footer) belongs to readers.

use std::io;

use serde_json::{Map, Value, json};

use quanta_index_core::diagnostic_trace::{DiagnosticCounterV1, DiagnosticStageV1};

macro_rules! closed_names {
    ($enum_type:ty, $forward:ident, $contains:ident; $($variant:path => $name:literal),+ $(,)?) => {
        pub(crate) fn $forward(value: $enum_type) -> Option<&'static str> {
            match value {
                $($variant => Some($name),)+
                _ => None,
            }
        }

        fn $contains(value: &str) -> bool {
            matches!(value, $($name)|+)
        }
    };
}

closed_names!(DiagnosticStageV1, known_stage_name, is_known_stage;
    DiagnosticStageV1::Request => "request",
    DiagnosticStageV1::Operation => "operation",
    DiagnosticStageV1::ClientAttempt => "client_attempt",
    DiagnosticStageV1::Ingress => "ingress",
    DiagnosticStageV1::QueueWait => "queue_wait",
    DiagnosticStageV1::Dispatch => "dispatch",
    DiagnosticStageV1::Backend => "backend",
    DiagnosticStageV1::Encode => "encode",
    DiagnosticStageV1::Write => "write",
    DiagnosticStageV1::BuildPreflight => "build_preflight",
    DiagnosticStageV1::BuildPrepare => "build_prepare",
    DiagnosticStageV1::SemanticPrepare => "semantic_prepare",
    DiagnosticStageV1::SemanticStream => "semantic_stream",
    DiagnosticStageV1::SemanticDelete => "semantic_delete",
    DiagnosticStageV1::SemanticAppend => "semantic_append",
    DiagnosticStageV1::SemanticSeal => "semantic_seal",
    DiagnosticStageV1::SemanticPromote => "semantic_promote",
    DiagnosticStageV1::SemanticBuild => "semantic_build",
    DiagnosticStageV1::LexicalBuild => "lexical_build",
    DiagnosticStageV1::NativeTextPostings => "native_text_postings",
    DiagnosticStageV1::NativeTextRoot => "native_text_root",
    DiagnosticStageV1::NativeCoverageScan => "native_coverage_scan",
    DiagnosticStageV1::NativeCoverageMapApply => "native_coverage_map_apply",
    DiagnosticStageV1::NativeSearchablePrepare => "native_searchable_prepare",
    DiagnosticStageV1::NativeCoverageSave => "native_coverage_save",
    DiagnosticStageV1::NativeMemberAssemble => "native_member_assemble",
    DiagnosticStageV1::Bm25CensusBuild => "bm25_census_build",
    DiagnosticStageV1::Bm25LiveBuild => "bm25_live_build",
    DiagnosticStageV1::Bm25Boundary => "bm25_boundary",
    DiagnosticStageV1::Finalize => "finalize",
    DiagnosticStageV1::Activate => "activate",
    DiagnosticStageV1::CatalogLockWait => "catalog_lock_wait",
    DiagnosticStageV1::CatalogLockHold => "catalog_lock_hold",
    DiagnosticStageV1::RetentionPairLockWait => "retention_pair_lock_wait",
    DiagnosticStageV1::RetentionPairLockHold => "retention_pair_lock_hold",
    DiagnosticStageV1::RetentionRootLockWait => "retention_root_lock_wait",
    DiagnosticStageV1::RetentionRootLockHold => "retention_root_lock_hold",
    DiagnosticStageV1::RetentionRootSnapshot => "retention_root_snapshot",
    DiagnosticStageV1::SourceTransactionOne => "source_transaction_one",
    DiagnosticStageV1::SourceTransactionTwo => "source_transaction_two",
    DiagnosticStageV1::NativeSourceApply => "native_source_apply",
    DiagnosticStageV1::NativeWorkSeal => "native_work_seal",
    DiagnosticStageV1::NativeSourceOwnerPrepare => "native_source_owner_prepare",
    DiagnosticStageV1::NativeSourcePathValidate => "native_source_path_validate",
    DiagnosticStageV1::NativeSourcePathInstall => "native_source_path_install",
    DiagnosticStageV1::NativeSourceMapApply => "native_source_map_apply",
    DiagnosticStageV1::RetentionPlan => "retention_plan",
    DiagnosticStageV1::RetentionMeasure => "retention_measure",
    DiagnosticStageV1::RetentionWrite => "retention_write",
    DiagnosticStageV1::RetentionReap => "retention_reap",
    DiagnosticStageV1::ClosureRead => "closure_read",
    DiagnosticStageV1::ClosureHash => "closure_hash",
    DiagnosticStageV1::ClosureDecode => "closure_decode",
    DiagnosticStageV1::SqliteWork => "sqlite_work",
    DiagnosticStageV1::PhysicalInodeCensus => "physical_inode_census",
    DiagnosticStageV1::ManagedObjectReserve => "managed_object_reserve",
    DiagnosticStageV1::ManagedObjectInstall => "managed_object_install",
    DiagnosticStageV1::ManagedObjectConfirm => "managed_object_confirm",
    DiagnosticStageV1::ManagedObjectAdvance => "managed_object_advance",
    DiagnosticStageV1::NativePublish => "native_publish",
    DiagnosticStageV1::FilePublish => "file_publish",
    DiagnosticStageV1::LexicalSeal => "lexical_seal",
    DiagnosticStageV1::LexicalProof => "lexical_proof",
    DiagnosticStageV1::Compaction => "compaction",
    DiagnosticStageV1::QueryParse => "query_parse",
    DiagnosticStageV1::QueryPlan => "query_plan",
    DiagnosticStageV1::QueryReadView => "query_read_view",
    DiagnosticStageV1::QueryOverlaySnapshot => "query_overlay_snapshot",
    DiagnosticStageV1::QueryFast => "query_fast",
    DiagnosticStageV1::QueryDeep => "query_deep",
    DiagnosticStageV1::QueryLexical => "query_lexical",
    DiagnosticStageV1::QueryEmbedding => "query_embedding",
    DiagnosticStageV1::QueryOverlayEmbedding => "query_overlay_embedding",
    DiagnosticStageV1::QueryCandidate => "query_candidate",
    DiagnosticStageV1::QueryVerify => "query_verify",
    DiagnosticStageV1::QueryRank => "query_rank",
    DiagnosticStageV1::QueryProject => "query_project",
    DiagnosticStageV1::HarnessPhase => "harness_phase",
    DiagnosticStageV1::ObserverSetup => "observer_setup",
    DiagnosticStageV1::ObserverSample => "observer_sample",
    DiagnosticStageV1::ObserverTeardown => "observer_teardown",
    DiagnosticStageV1::ArtifactWrite => "artifact_write",
    DiagnosticStageV1::RuntimeBoot => "runtime_boot",
    DiagnosticStageV1::BootRestore => "boot_restore",
    DiagnosticStageV1::BootReclaim => "boot_reclaim",
    DiagnosticStageV1::BootInventory => "boot_inventory",
    DiagnosticStageV1::BootActiveProof => "boot_active_proof",
    DiagnosticStageV1::RuntimeMaintenance => "runtime_maintenance",
    DiagnosticStageV1::RuntimeStop => "runtime_stop",
    DiagnosticStageV1::RuntimeJoin => "runtime_join",
    DiagnosticStageV1::NativeTextInventoryPrepare => "native_text_inventory_prepare",
    DiagnosticStageV1::NativeTextInventoryMapApply => "native_text_inventory_map_apply",
    DiagnosticStageV1::NativeTextInventoryDiff => "native_text_inventory_diff",
    DiagnosticStageV1::NativeTextInventoryRead => "native_text_inventory_read",
    DiagnosticStageV1::NativeTextOccurrenceRead => "native_text_occurrence_read",
    DiagnosticStageV1::NativeTextPostingEdit => "native_text_posting_edit",
    DiagnosticStageV1::MapApply => "map_apply",
);

closed_names!(DiagnosticCounterV1, known_counter_name, is_known_counter;
    DiagnosticCounterV1::ThreadCpuNs => "thread_cpu_ns",
    DiagnosticCounterV1::FilesVisited => "files_visited",
    DiagnosticCounterV1::FilesRead => "files_read",
    DiagnosticCounterV1::FilesHashed => "files_hashed",
    DiagnosticCounterV1::FilesDecoded => "files_decoded",
    DiagnosticCounterV1::InodesVisited => "inodes_visited",
    DiagnosticCounterV1::UniqueInodes => "unique_inodes",
    DiagnosticCounterV1::BytesRead => "bytes_read",
    DiagnosticCounterV1::BytesHashed => "bytes_hashed",
    DiagnosticCounterV1::BytesDecoded => "bytes_decoded",
    DiagnosticCounterV1::WorkReads => "work_reads",
    DiagnosticCounterV1::WorkWrites => "work_writes",
    DiagnosticCounterV1::WorkBytes => "work_bytes",
    DiagnosticCounterV1::WorkMutations => "work_mutations",
    DiagnosticCounterV1::SqliteRows => "sqlite_rows",
    DiagnosticCounterV1::SqliteBatches => "sqlite_batches",
    DiagnosticCounterV1::SqliteWriteCalls => "sqlite_write_calls",
    DiagnosticCounterV1::SqliteWriteRequestedBytes => "sqlite_write_requested_bytes",
    DiagnosticCounterV1::SqliteWriteFailures => "sqlite_write_failures",
    DiagnosticCounterV1::SqliteWalWriteCalls => "sqlite_wal_write_calls",
    DiagnosticCounterV1::SqliteMainWriteCalls => "sqlite_main_write_calls",
    DiagnosticCounterV1::SqliteOtherWriteCalls => "sqlite_other_write_calls",
    DiagnosticCounterV1::SqliteWriteAdmissionCalls => "sqlite_write_admission_calls",
    DiagnosticCounterV1::SqliteWriteAdmissionRefusals => "sqlite_write_admission_refusals",
    DiagnosticCounterV1::SqliteWriteAdmissionElapsedNs => "sqlite_write_admission_elapsed_ns",
    DiagnosticCounterV1::SqliteWriteLowerEnvelopeElapsedNs => "sqlite_write_lower_envelope_elapsed_ns",
    DiagnosticCounterV1::SqliteWritePostCallbackElapsedNs => "sqlite_write_post_callback_elapsed_ns",
    DiagnosticCounterV1::SqliteWritePostCallbackRefusals => "sqlite_write_post_callback_refusals",
    DiagnosticCounterV1::SqliteCacheHits => "sqlite_cache_hits",
    DiagnosticCounterV1::SqliteCacheMisses => "sqlite_cache_misses",
    DiagnosticCounterV1::SqliteCacheWrites => "sqlite_cache_writes",
    DiagnosticCounterV1::SqliteCacheSpills => "sqlite_cache_spills",
    DiagnosticCounterV1::SqlitePageSizeBytes => "sqlite_page_size_bytes",
    DiagnosticCounterV1::SqliteCacheSizeMagnitude => "sqlite_cache_size_magnitude",
    DiagnosticCounterV1::SqliteCacheSizeIsKib => "sqlite_cache_size_is_kib",
    DiagnosticCounterV1::SqliteCacheSpillPages => "sqlite_cache_spill_pages",
    DiagnosticCounterV1::SqliteWalAutocheckpointPages => "sqlite_wal_autocheckpoint_pages",
    DiagnosticCounterV1::SqliteElapsedNs => "sqlite_elapsed_ns",
    DiagnosticCounterV1::QueryPostingCalls => "query_posting_calls",
    DiagnosticCounterV1::QueryPostingHits => "query_posting_hits",
    DiagnosticCounterV1::QueryPostingElapsedNs => "query_posting_elapsed_ns",
    DiagnosticCounterV1::QueryPhraseChecks => "query_phrase_checks",
    DiagnosticCounterV1::QueryPhraseElapsedNs => "query_phrase_elapsed_ns",
    DiagnosticCounterV1::Candidates => "candidates",
    DiagnosticCounterV1::CandidatesVerified => "candidates_verified",
    DiagnosticCounterV1::Matches => "matches",
    DiagnosticCounterV1::RejectedAtGuard => "rejected_at_guard",
    DiagnosticCounterV1::SyncCalls => "sync_calls",
    DiagnosticCounterV1::SyncFailures => "sync_failures",
    DiagnosticCounterV1::SyncElapsedNs => "sync_elapsed_ns",
    DiagnosticCounterV1::SqliteSyncOverwrite => "sqlite_sync_overwrite",
    DiagnosticCounterV1::SqliteSyncGrowth => "sqlite_sync_growth",
    DiagnosticCounterV1::SqliteSyncTruncate => "sqlite_sync_truncate",
    DiagnosticCounterV1::SqliteSyncCreate => "sqlite_sync_create",
    DiagnosticCounterV1::SqliteSyncNoMutation => "sqlite_sync_no_mutation",
    DiagnosticCounterV1::SqliteSyncUnclassified => "sqlite_sync_unclassified",
    DiagnosticCounterV1::NewObjects => "new_objects",
    DiagnosticCounterV1::ReusedObjects => "reused_objects",
    DiagnosticCounterV1::CompactedDocuments => "compacted_documents",
    DiagnosticCounterV1::CompactedSegments => "compacted_segments",
    DiagnosticCounterV1::CompactedBytes => "compacted_bytes",
    DiagnosticCounterV1::PostingMutations => "posting_mutations",
    DiagnosticCounterV1::MapPageMutations => "map_page_mutations",
    DiagnosticCounterV1::ManagedObjectInputs => "managed_object_inputs",
    DiagnosticCounterV1::MapRootReuseCandidates => "map_root_reuse_candidates",
    DiagnosticCounterV1::MapRootReuses => "map_root_reuses",
    DiagnosticCounterV1::MapRootBuilds => "map_root_builds",
    DiagnosticCounterV1::TextOccurrencePriorValidationAttempts => "text_occurrence_prior_validation_attempts",
    DiagnosticCounterV1::TextOccurrenceNextValidationAttempts => "text_occurrence_next_validation_attempts",
    DiagnosticCounterV1::TextOccurrenceRamReuses => "text_occurrence_ram_reuses",
    DiagnosticCounterV1::TextOccurrenceSpillReuses => "text_occurrence_spill_reuses",
    DiagnosticCounterV1::TextOccurrenceEqualReuses => "text_occurrence_equal_reuses",
    DiagnosticCounterV1::TextOccurrenceValidationFailures => "text_occurrence_validation_failures",
    DiagnosticCounterV1::InvalidAttemptTokens => "invalid_attempt_tokens",
    DiagnosticCounterV1::UniqueClosureRefs => "unique_closure_refs",
    DiagnosticCounterV1::ClosureDependencyEdges => "closure_dependency_edges",
    DiagnosticCounterV1::ClosureAuthenticatedBytes => "closure_authenticated_bytes",
    DiagnosticCounterV1::PhysicalBytesMeasured => "physical_bytes_measured",
    DiagnosticCounterV1::RetainedPairs => "retained_pairs",
    DiagnosticCounterV1::MeasureCalls => "measure_calls",
    DiagnosticCounterV1::RemovedRoots => "removed_roots",
    DiagnosticCounterV1::SemanticWindows => "semantic_windows",
    DiagnosticCounterV1::SemanticOwnerScopes => "semantic_owner_scopes",
    DiagnosticCounterV1::SemanticDeleteCalls => "semantic_delete_calls",
    DiagnosticCounterV1::SemanticAppendCalls => "semantic_append_calls",
    DiagnosticCounterV1::SemanticRowsAppended => "semantic_rows_appended",
    DiagnosticCounterV1::SemanticRowsDeleted => "semantic_rows_deleted",
    DiagnosticCounterV1::SemanticVectorBytes => "semantic_vector_bytes",
    DiagnosticCounterV1::QueryAdmittedLimit => "query_admitted_limit",
    DiagnosticCounterV1::QueryReadLimit => "query_read_limit",
    DiagnosticCounterV1::QueryByteLimit => "query_byte_limit",
    DiagnosticCounterV1::RegexLiteralCount => "regex_literal_count",
    DiagnosticCounterV1::RegexAlternativeCount => "regex_alternative_count",
    DiagnosticCounterV1::RegexNecessaryGramCount => "regex_necessary_gram_count",
    DiagnosticCounterV1::ClosureReadElapsedNs => "closure_read_elapsed_ns",
    DiagnosticCounterV1::ClosureHashElapsedNs => "closure_hash_elapsed_ns",
    DiagnosticCounterV1::ClosureDecodeElapsedNs => "closure_decode_elapsed_ns",
    DiagnosticCounterV1::QueueDropped => "queue_dropped",
    DiagnosticCounterV1::QueueAccepted => "queue_accepted",
    DiagnosticCounterV1::OutputWritten => "output_written",
    DiagnosticCounterV1::OutputUnwritten => "output_unwritten",
);

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn span_id(value: &Value) -> io::Result<u64> {
    value
        .as_u64()
        .filter(|id| *id > 0)
        .ok_or_else(|| invalid("diagnostic span ID is not a positive u64"))
}

fn nonempty_string<'a>(value: &'a Value, message: &'static str) -> io::Result<&'a str> {
    value
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid(message))
}

fn normalize_kind(kind: Value) -> io::Result<Value> {
    match kind {
        Value::Array(fields) => {
            let tag = fields.first().and_then(Value::as_str);
            match tag {
                Some("b") => {
                    let [_, id, parent, stage, lane, root]: [Value; 6] = fields
                        .try_into()
                        .map_err(|_fields| invalid("V5 span begin tuple has wrong arity"))?;
                    let id = span_id(&id)?;
                    let parent = if parent.is_null() {
                        None
                    } else {
                        Some(span_id(&parent)?)
                    };
                    let stage = nonempty_string(&stage, "V5 span stage is absent")?;
                    if !is_known_stage(stage) {
                        return Err(invalid("V5 span stage is unknown"));
                    }
                    let lane = lane
                        .as_u64()
                        .filter(|lane| u16::try_from(*lane).is_ok())
                        .ok_or_else(|| invalid("V5 span lane is outside u16"))?;
                    let root = if root.is_null() {
                        None
                    } else {
                        let name = root
                            .as_str()
                            .ok_or_else(|| invalid("V5 root kind is not a string"))?;
                        if !matches!(name, "request" | "operation" | "capture") {
                            return Err(invalid("V5 root kind is unknown"));
                        }
                        Some(name)
                    };
                    let mut result = Map::new();
                    let _previous = result.insert("type".into(), json!("span_begin"));
                    let _previous = result.insert("span_id".into(), json!(id));
                    let _previous = result.insert("stage".into(), json!(stage));
                    let _previous = result.insert("lane_id".into(), json!(lane));
                    if let Some(parent) = parent {
                        let _previous = result.insert("parent_span_id".into(), json!(parent));
                    }
                    if let Some(root) = root {
                        let _previous = result.insert("root_kind".into(), json!(root));
                    }
                    Ok(Value::Object(result))
                }
                Some("e") => {
                    let [_, id, outcome, code]: [Value; 4] = fields
                        .try_into()
                        .map_err(|_fields| invalid("V5 span end tuple has wrong arity"))?;
                    let id = span_id(&id)?;
                    let outcome = outcome
                        .as_str()
                        .ok_or_else(|| invalid("V5 span outcome is not a string"))?;
                    let has_code = matches!(outcome, "error" | "cancelled");
                    if !matches!(outcome, "ok" | "error" | "cancelled" | "aborted") {
                        return Err(invalid("V5 span outcome is unknown"));
                    }
                    let code = if code.is_null() {
                        None
                    } else {
                        Some(
                            code.as_u64()
                                .filter(|code| u32::try_from(*code).is_ok())
                                .ok_or_else(|| invalid("V5 span error code is outside u32"))?,
                        )
                    };
                    if has_code != code.is_some() {
                        return Err(invalid("V5 span outcome and error code disagree"));
                    }
                    let mut result = Map::new();
                    let _previous = result.insert("type".into(), json!("span_end"));
                    let _previous = result.insert("span_id".into(), json!(id));
                    let _previous = result.insert("outcome".into(), json!(outcome));
                    if let Some(code) = code {
                        let _previous = result.insert("error_code".into(), json!(code));
                    }
                    Ok(Value::Object(result))
                }
                Some("c") => {
                    let [_, id, key, value]: [Value; 4] = fields
                        .try_into()
                        .map_err(|_fields| invalid("V5 counter tuple has wrong arity"))?;
                    let id = span_id(&id)?;
                    let key = nonempty_string(&key, "V5 counter key is absent")?;
                    if !is_known_counter(key) {
                        return Err(invalid("V5 counter key is unknown"));
                    }
                    let value = value
                        .as_u64()
                        .ok_or_else(|| invalid("V5 counter value is not a u64"))?;
                    Ok(json!({"type":"counter", "span_id":id, "key":key, "value":value}))
                }
                _ => Err(invalid("V5 common kind tag is unknown")),
            }
        }
        Value::Object(kind) => {
            let name = kind.get("type").and_then(Value::as_str);
            if !matches!(
                name,
                Some(
                    "request_identity"
                        | "phase_identity"
                        | "ann_region_observation"
                        | "sqlite_sync_observation"
                        | "owned_renewal_refusal"
                        | "sync_aggregate"
                        | "object_install_aggregate"
                        | "object_kind_aggregate"
                        | "adoption_group_aggregate"
                        | "bm25_census_aggregate"
                        | "bm25_boundary_aggregate"
                        | "bm25_live_build_aggregate"
                        | "link"
                        | "health"
                )
            ) {
                return Err(invalid(
                    "V5 rare kind is unknown or duplicates a tuple kind",
                ));
            }
            // The canonical reader validates the full closed shape and owner.
            Ok(Value::Object(kind))
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Err(invalid(
            "V5 event kind is neither a tuple nor a rare object",
        )),
    }
}

/// Normalize a bounded, already-decoded JSONL record to the original object IR.
///
/// JSON duplicate-key rejection must happen while decoding raw bytes. A
/// `serde_json::Value` cannot reveal duplicate keys after they were parsed.
pub fn normalize_record(schema_version: u64, record: Value) -> io::Result<Value> {
    if !(1..=5).contains(&schema_version) {
        return Err(invalid("diagnostic schema version is unsupported"));
    }
    match record {
        Value::Object(record) => match record.get("record_type").and_then(Value::as_str) {
            Some("header" | "footer") => {
                if record.get("schema_version").and_then(Value::as_u64) != Some(schema_version) {
                    return Err(invalid("diagnostic header/footer version differs"));
                }
                Ok(Value::Object(record))
            }
            Some("event") if schema_version <= 4 => Ok(Value::Object(record)),
            Some("event") => Err(invalid("V5 event object is forbidden")),
            _ => Err(invalid("diagnostic record type is unknown")),
        },
        Value::Array(fields) if schema_version == 5 => {
            let [tag, sequence, capture, timestamp, kind]: [Value; 5] = fields
                .try_into()
                .map_err(|_fields| invalid("V5 event tuple has wrong arity"))?;
            if tag.as_str() != Some("e") {
                return Err(invalid("V5 event tuple tag is unknown"));
            }
            let sequence = sequence
                .as_u64()
                .filter(|seq| *seq > 0)
                .ok_or_else(|| invalid("V5 output sequence is not positive u64"))?;
            let capture = capture
                .as_str()
                .filter(|id| {
                    id.len() == 32
                        && id
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
                .ok_or_else(|| invalid("V5 capture ID is not lowercase 128-bit hex"))?;
            let timestamp = timestamp
                .as_u64()
                .ok_or_else(|| invalid("V5 timestamp is not a u64"))?;
            let kind = normalize_kind(kind)?;
            Ok(json!({"record_type":"event", "output_seq":sequence,
                "capture_id":capture, "ts_ns":timestamp, "kind":kind}))
        }
        Value::Array(_) => Err(invalid("legacy diagnostic event tuple is forbidden")),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Err(invalid(
            "diagnostic record must be an object or V5 event tuple",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPTURE: &str = "03030303030303030303030303030303";

    #[test]
    fn native_text_inventory_stages_have_closed_v5_names() {
        for (stage, name) in [
            (
                DiagnosticStageV1::NativeTextInventoryPrepare,
                "native_text_inventory_prepare",
            ),
            (
                DiagnosticStageV1::NativeTextInventoryMapApply,
                "native_text_inventory_map_apply",
            ),
            (
                DiagnosticStageV1::NativeTextInventoryDiff,
                "native_text_inventory_diff",
            ),
            (
                DiagnosticStageV1::NativeTextInventoryRead,
                "native_text_inventory_read",
            ),
            (
                DiagnosticStageV1::NativeTextOccurrenceRead,
                "native_text_occurrence_read",
            ),
            (
                DiagnosticStageV1::NativeTextPostingEdit,
                "native_text_posting_edit",
            ),
        ] {
            assert_eq!(known_stage_name(stage), Some(name));
            let record = json!(["e", 1, CAPTURE, 11, ["b", 7, null, name, 3, null]]);
            assert_eq!(
                normalize_record(5, record).expect("valid V5 inventory stage"),
                json!({"record_type":"event","output_seq":1,"capture_id":CAPTURE,
                    "ts_ns":11,"kind":{"type":"span_begin","span_id":7,
                    "stage":name,"lane_id":3}})
            );
        }
    }

    #[test]
    fn managed_object_install_stage_and_aggregate_use_v5_rare_object_path() {
        assert_eq!(
            known_stage_name(DiagnosticStageV1::ManagedObjectInstall),
            Some("managed_object_install")
        );
        assert_eq!(
            known_stage_name(DiagnosticStageV1::ManagedObjectAdvance),
            Some("managed_object_advance")
        );
        let stage = json!([
            "e",
            1,
            CAPTURE,
            11,
            ["b", 7, null, "managed_object_install", 3, null]
        ]);
        assert_eq!(
            normalize_record(5, stage).expect("managed install stage"),
            json!({"record_type":"event","output_seq":1,"capture_id":CAPTURE,
                "ts_ns":11,"kind":{"type":"span_begin","span_id":7,
                "stage":"managed_object_install","lane_id":3}})
        );
        let aggregate = json!(["e", 2, CAPTURE, 12,
            {"type":"object_install_aggregate", "span_id":7,
             "phase":"file_sync", "calls":2, "elapsed_sum_ns":19,
             "thread_cpu_ns":null, "failures":1}]);
        assert_eq!(
            normalize_record(5, aggregate).expect("object install aggregate"),
            json!({"record_type":"event","output_seq":2,"capture_id":CAPTURE,
                "ts_ns":12,"kind":{"type":"object_install_aggregate", "span_id":7,
                    "phase":"file_sync", "calls":2, "elapsed_sum_ns":19,
                    "thread_cpu_ns":null, "failures":1}})
        );
    }

    #[test]
    fn literal_v5_common_records_normalize_to_fixed_canonical_objects() {
        let rows = [
            (
                r#"["e",1,"03030303030303030303030303030303",11,["b",7,null,"managed_object_reserve",3,null]]"#,
                json!({"record_type":"event","output_seq":1,"capture_id":CAPTURE,"ts_ns":11,
                    "kind":{"type":"span_begin","span_id":7,"stage":"managed_object_reserve","lane_id":3}}),
            ),
            (
                r#"["e",2,"03030303030303030303030303030303",11,["c",7,"managed_object_inputs",9]]"#,
                json!({"record_type":"event","output_seq":2,"capture_id":CAPTURE,"ts_ns":11,
                    "kind":{"type":"counter","span_id":7,"key":"managed_object_inputs","value":9}}),
            ),
            (
                r#"["e",4,"03030303030303030303030303030303",11,["b",7,1,"request",0,"request"]]"#,
                json!({"record_type":"event","output_seq":4,"capture_id":CAPTURE,"ts_ns":11,
                    "kind":{"type":"span_begin","span_id":7,"parent_span_id":1,
                            "stage":"request","lane_id":0,"root_kind":"request"}}),
            ),
            (
                r#"["e",3,"03030303030303030303030303030303",12,["e",7,"error",4294967295]]"#,
                json!({"record_type":"event","output_seq":3,"capture_id":CAPTURE,"ts_ns":12,
                    "kind":{"type":"span_end","span_id":7,"outcome":"error","error_code":4_294_967_295_u64}}),
            ),
        ];
        for (raw, expected) in rows {
            let record = serde_json::from_str(raw).expect("valid literal V5 wire");
            assert_eq!(
                normalize_record(5, record).expect("valid V5 record"),
                expected
            );
        }
    }

    #[test]
    fn legacy_objects_remain_objects_and_legacy_tuples_are_rejected() {
        for version in 1..=4 {
            let event = json!({"record_type":"event","output_seq":1,
                "capture_id":CAPTURE,"ts_ns":11,"kind":{"type":"counter",
                "span_id":7,"key":"managed_object_inputs","value":9}});
            assert_eq!(
                normalize_record(version, event.clone()).expect("legacy object event"),
                event
            );
            assert!(
                normalize_record(version, json!(["e", 1, CAPTURE, 11, ["c", 7, "k", 9]])).is_err()
            );
        }
    }

    #[test]
    fn v5_rare_kind_keeps_named_payload_inside_fixed_event_envelope() {
        let raw = json!(["e", 5, CAPTURE, 12,
            {"type":"health", "code":"counter_overflow"}]);
        let expected = json!({"record_type":"event", "output_seq":5,
            "capture_id":CAPTURE, "ts_ns":12,
            "kind":{"type":"health", "code":"counter_overflow"}});
        assert_eq!(
            normalize_record(5, raw).expect("valid V5 rare event"),
            expected
        );
    }

    #[test]
    fn v5_rejects_mixed_aliases_bad_arity_ranges_and_terminal_codes() {
        let bad = [
            json!({"record_type":"event","output_seq":1}),
            json!([
                "e",
                1,
                CAPTURE,
                11,
                ["span_begin", 7, null, "request", 0, null]
            ]),
            json!(["e", 1, CAPTURE, 11, ["b", 7, null, "request", 0]]),
            json!(["e", 1, CAPTURE, 11, ["b", 7, null, "request", 65536, null]]),
            json!([
                "e",
                1,
                CAPTURE,
                11,
                ["b", 7, null, "unknown_stage", 0, null]
            ]),
            json!(["e", 1, CAPTURE, 11, ["b", 7, null, "request", 0, "unknown"]]),
            json!(["e", 1, CAPTURE, 11, ["e", 7, "error", null]]),
            json!(["e", 1, CAPTURE, 11, ["e", 7, "ok", 1]]),
            json!([
                "e",
                1,
                CAPTURE,
                11,
                ["e", 7, "cancelled", 4_294_967_296_u64]
            ]),
            json!(["e",1,CAPTURE,11,{"type":"span_begin","span_id":7}]),
            json!(["e",1,CAPTURE,11,{"type":"unknown","span_id":7}]),
            json!(["e", 1, CAPTURE, 11, ["c", 7, "unknown_counter", 9]]),
            json!(["e", 0, CAPTURE, 11, ["c", 7, "k", 9]]),
            json!([
                "e",
                1,
                "ABCDEFABCDEFABCDEFABCDEFABCDEFAB",
                11,
                ["c", 7, "k", 9]
            ]),
        ];
        for record in bad {
            assert!(normalize_record(5, record).is_err());
        }
    }

    #[test]
    fn matching_header_footer_versions_are_required() {
        for record_type in ["header", "footer"] {
            let record = json!({"record_type":record_type,"schema_version":5});
            assert!(normalize_record(5, record.clone()).is_ok());
            assert!(normalize_record(4, record).is_err());
        }
        assert!(normalize_record(6, json!({"record_type":"header","schema_version":6})).is_err());
    }
}
