use super::super::{BatchPublishReceipt, SearchPlaneIpcError};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

use super::*;
use crate::lex::{
    CommitRecord, CommitSha, DiffHunkRecord, DirtyRecord, LanguageCode, ParseNode, ParseRoleTag,
    ParseTreeRecord, SymbolRecord, compute_parse_tree_source_hash,
};
use crate::{
    CapabilityStatusV1, ChunkId, ChunkRecord, EmbeddingId, EmbeddingRecord, ManifestGeneration,
    OwnerDocKind, RepoId, RepoRelativePath, RevisionId, SemanticCorpusKindV1,
    SemanticSourceScopeKeyV1, SourceFileCoverage, SourceFileKey, SourcePublicationEvent,
    SourceRoleV1,
};

type TestRes = Result<(), Box<dyn std::error::Error>>;

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut buf: Vec<u8> = Vec::new();
    ciborium::into_writer(value, &mut buf)?;
    Ok(buf)
}

fn decode<T>(bytes: &[u8]) -> Result<T, Box<dyn std::error::Error>>
where
    T: for<'de> Deserialize<'de>,
{
    Ok(ciborium::from_reader(bytes)?)
}

#[test]
fn legacy_repo_map_publish_and_receipt_tags_are_refused() -> TestRes {
    let old_request = serde_json::json!({"PublishRepoMapBundle": {}});
    let old_request_cbor = encode(&old_request)?;
    let error = serde_json::from_value::<SearchPlaneIngestIpcRequest>(old_request)
        .expect_err("removed publish opcode must refuse");
    assert!(error.to_string().contains("unknown variant"), "{error}");
    assert!(decode::<SearchPlaneIngestIpcRequest>(&old_request_cbor).is_err());

    let old_response = serde_json::json!({"RepoMapReceipt": {}});
    let old_response_cbor = encode(&old_response)?;
    let error = serde_json::from_value::<SearchPlaneIngestIpcResponse>(old_response)
        .expect_err("removed receipt opcode must refuse");
    assert!(error.to_string().contains("unknown variant"), "{error}");
    assert!(decode::<SearchPlaneIngestIpcResponse>(&old_response_cbor).is_err());
    Ok(())
}

fn fixture_repo_id() -> RepoId {
    RepoId::new("repo").expect("static fixture ID satisfies canonical policy")
}

fn fixture_revision_id() -> RevisionId {
    RevisionId::new("rev").expect("static fixture ID satisfies canonical policy")
}

fn fixture_generation() -> ManifestGeneration {
    ManifestGeneration::new(7)
}

fn fixture_chunk_id() -> ChunkId {
    ChunkId::new("chunk-1")
}

fn fixture_embedding_id() -> EmbeddingId {
    EmbeddingId::new("embedding-1")
}

fn fixture_chunk_record() -> ChunkRecord {
    ChunkRecord {
        chunk_id: fixture_chunk_id(),
        repo_relative_path: RepoRelativePath::new("src/main.rs"),
        language: LanguageCode::from_code_str("rust").unwrap_or_else(|| std::process::abort()),
        start_byte: 0,
        end_byte: 12,
        start_line: 1,
        end_line: 10,
        text: "fn main() {}".to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    }
}

fn fixture_embedding_record() -> EmbeddingRecord {
    EmbeddingRecord {
        embedding_id: fixture_embedding_id(),
        record_id: "record-1".to_string().into_boxed_str(),
        owner_kind: crate::OwnerDocKind::Chunk,
        owner_id: "main".to_string().into_boxed_str(),
        corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
        parent_owner_id: Some("file:src/main.rs".to_string().into_boxed_str()),
        source_doc_id: "doc-1".to_string().into_boxed_str(),
        repo_relative_path: RepoRelativePath::new("src/main.rs"),
        language: LanguageCode::from_code_str("rust").unwrap_or_else(|| std::process::abort()),
        package: None,
        symbol_kind: None,
        visibility: None,
        source_role: SourceRoleV1::RawFallbackText,
        generated: false,
        capability_status: CapabilityStatusV1::Degraded,
        authority_digest: "auth:feed".to_string().into_boxed_str(),
        render_policy_digest: "render:feed".to_string().into_boxed_str(),
        card_schema_version: 0,
        start_byte: 0,
        end_byte: 12,
        start_line: 1,
        end_line: 10,
        snippet: "fn main() {}".to_string().into_boxed_str(),
        embedding_input_digest: "input:feed".to_string().into_boxed_str(),
        vector_digest: "vector:feed".to_string().into_boxed_str(),
        view_kind: "raw_chunk".to_string().into_boxed_str(),
        vector: vec![0.1, 0.2, 0.3],
    }
}

fn fixture_commit_sha() -> CommitSha {
    CommitSha::from_bytes([
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
        0xef, 0x01, 0x23, 0x45, 0x67,
    ])
}

fn fixture_commit_record() -> CommitRecord {
    CommitRecord {
        wire_version: 1,
        sha: fixture_commit_sha(),
        parents: Vec::new(),
        author_time_ms: 11,
        committer_time_ms: 12,
        applied_at_ms: 13,
        author: "alice".to_string().into_boxed_str(),
        author_name: Some("Alice Example".to_string().into_boxed_str()),
        author_email: Some("alice@example.com".to_string().into_boxed_str()),
        committer: "alice".to_string().into_boxed_str(),
        committer_name: Some("Alice Example".to_string().into_boxed_str()),
        committer_email: Some("alice@example.com".to_string().into_boxed_str()),
        message: "fix: sample".to_string().into_boxed_str(),
        is_merge: false,
        tags: vec!["v1.0.0".to_string().into_boxed_str()],
    }
}

fn fixture_diff_record() -> DiffHunkRecord {
    DiffHunkRecord {
        wire_version: 1,
        hunk_header: "@@ -1,1 +1,2 @@".to_string().into_boxed_str(),
        side: crate::DiffHunkSide::After,
        added_text: "todo!".to_string().into_boxed_str(),
        removed_text: String::new().into_boxed_str(),
        touched_text: "todo!".to_string().into_boxed_str(),
        byte_start: 0,
        byte_end: 5,
    }
}

fn fixture_dirty_record() -> DirtyRecord {
    DirtyRecord {
        wire_version: 1,
        doc_id: fixture_chunk_id(),
        applied_at_ms: 55,
        payload_hash: [7; 32],
    }
}

fn fixture_parse_tree_record() -> ParseTreeRecord {
    ParseTreeRecord {
        wire_version: 1,
        lang: LanguageCode::from_code_str("rust").unwrap_or_else(|| std::process::abort()),
        root: ParseNode {
            kind: "function_item".to_string().into_boxed_str(),
            byte_start: 0,
            byte_end: 10,
            children: Vec::new(),
        },
        source_hash: compute_parse_tree_source_hash("fn main() {}"),
        role_tag_schema_version: 1,
        role_tags: vec![ParseRoleTag {
            role: "expr".to_string().into_boxed_str(),
            byte_start: 0,
            byte_end: 4,
        }],
    }
}

fn fixture_scope_key() -> SearchScopeKey {
    SearchScopeKey {
        doc_surface: SearchScopeSurface::File,
        repo_relative_path: RepoRelativePath::new("src/main.rs"),
    }
}

fn fixture_model_contract() -> EmbeddingModelContract {
    EmbeddingModelContract {
        model_id: "text-embed".to_string().into_boxed_str(),
        model_version: Some("1".to_string().into_boxed_str()),
        dimension: 3,
        normalization: EmbeddingNormalization::L2Unit,
        distance_metric: EmbeddingDistanceMetric::Cosine,
        policy_digest: "policy:feed".to_string().into_boxed_str(),
        view_policy_digest: Some("view:feed".to_string().into_boxed_str()),
    }
}

/// A token with the canonical batch-digest shape; the contract checks
/// shape only, the dispatcher proves the value.
const FIXTURE_BATCH_DIGEST: &str =
    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn fixture_search_corpus_batch() -> SearchCorpusIngestBatch {
    let chunks = vec![fixture_chunk_record()];
    let coverage = SourceFileCoverage {
        source: crate::SourceFileRevision {
            file: SourceFileKey {
                source_repo_id: fixture_repo_id(),
                repo_relative_path: RepoRelativePath::new("src/main.rs"),
            },
            revision_id: fixture_revision_id(),
            source_sha256: sha2::Sha256::digest(b"fn main() {}").into(),
        },
        language: chunks[0].language.clone(),
        producer_policy_sha256: [2; 32],
        symbol_name_source_policy: crate::SymbolNameSourcePolicyV1::Unspecified,
        unit_set_sha256: crate::source_file_unit_set_sha256(&chunks, &[])
            .expect("fixture units encode"),
        text_admitted: true,
        symbols: crate::SymbolCoverage::NotRequested,
    };
    let mut batch = SearchCorpusIngestBatch {
        source_event: SourcePublicationEvent {
            stream_id: "fixture".into(),
            event_id: "initial".into(),
            expected_base_event_id: None,
            payload_sha256: [0; 32],
        },
        repo_id: fixture_repo_id(),
        revision_id: fixture_revision_id(),
        generation: fixture_generation(),
        base_generation: None,
        manifest_digest: "manifest:feed".to_string(),
        batch_digest: FIXTURE_BATCH_DIGEST.to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![SearchCorpusReplaceScope {
            coverage,
            source_bytes: b"fn main() {}".to_vec(),
            chunks,
            symbols: vec![],
        }],
        tombstone_scopes: vec![SearchCorpusTombstoneScope {
            file: SourceFileKey {
                source_repo_id: fixture_repo_id(),
                repo_relative_path: RepoRelativePath::new("src/old.rs"),
            },
        }],
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    };
    batch.source_event.payload_sha256 =
        crate::source_event_payload_sha256(&batch).expect("fixture event encodes");
    batch
}

fn fixture_semantic_batch() -> SemanticIngestBatch {
    SemanticIngestBatch {
        repo_id: fixture_repo_id(),
        revision_id: fixture_revision_id(),
        generation: fixture_generation(),
        base_generation: Some(ManifestGeneration::new(6)),
        manifest_digest: "manifest:feed".to_string(),
        batch_digest: "batch:feed".to_string(),
        mode: BatchIngestMode::Delta,
        model_contract: fixture_model_contract(),
        required_corpora: vec![SemanticCorpusKindV1::RawCodeFallback],
        corpus_policy_digest: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![SemanticReplaceScope {
            scope: fixture_scope_key(),
            scope_digest: "scope:feed".to_string(),
            embeddings: vec![fixture_embedding_record()],
            cluster_memberships: Vec::new(),
        }],
        tombstone_scopes: vec![SemanticTombstoneScope {
            semantic_scope: SemanticSourceScopeKeyV1 {
                corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
                owner_kind: OwnerDocKind::Chunk,
                owner_id: "owner-old".to_string(),
            },
        }],
        seal: false,
    }
}

fn fixture_history_batch() -> HistoryIngestBatch {
    HistoryIngestBatch {
        repo_id: fixture_repo_id(),
        revision_id: fixture_revision_id(),
        generation: fixture_generation(),
        manifest_digest: Some("manifest:feed".to_string()),
        batch_digest: "batch:feed".to_string(),
        commits: vec![fixture_commit_record()],
        refs: vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
            name: "refs/heads/main".to_string().into_boxed_str(),
            sha: fixture_commit_sha(),
        })],
        tags: vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
            name: "v1.0.0".to_string().into_boxed_str(),
            sha: fixture_commit_sha(),
        })],
        diff_hunks: vec![HistoryDiffHunkUpsert {
            commit_sha: fixture_commit_sha(),
            file_path: "src/lib.rs".to_string().into_boxed_str(),
            record: fixture_diff_record(),
        }],
    }
}

fn fixture_repo_commit_recency_batch() -> RepoCommitRecencyIngestBatch {
    RepoCommitRecencyIngestBatch {
        repo_id: fixture_repo_id(),
        revision_id: fixture_revision_id(),
        generation: fixture_generation(),
        batch_digest: "batch:repo-commit-recency".to_string(),
        entries: vec![
            RepoCommitRecencyEntry {
                source_repo_id: RepoId::new("corp-a")
                    .expect("static fixture ID satisfies canonical policy"),
                latest_committer_time_ms: 1_717_171_717_000,
            },
            RepoCommitRecencyEntry {
                source_repo_id: RepoId::new("corp-b")
                    .expect("static fixture ID satisfies canonical policy"),
                latest_committer_time_ms: 1_617_171_717_000,
            },
        ],
    }
}

fn fixture_repo_meta_batch() -> RepoMetaIngestBatch {
    RepoMetaIngestBatch {
        repo_id: fixture_repo_id(),
        revision_id: fixture_revision_id(),
        generation: fixture_generation(),
        batch_digest: "batch:repo-meta".to_string(),
        entries: vec![
            RepoMetaEntry {
                source_repo_id: RepoId::new("corp-a")
                    .expect("static fixture ID satisfies canonical policy"),
                key: "license".to_string(),
                value: "apache-2.0".to_string(),
            },
            RepoMetaEntry {
                source_repo_id: RepoId::new("corp-b")
                    .expect("static fixture ID satisfies canonical policy"),
                key: "license".to_string(),
                value: "gpl-3.0".to_string(),
            },
        ],
    }
}

fn fixture_repo_topic_batch() -> RepoTopicIngestBatch {
    RepoTopicIngestBatch {
        repo_id: fixture_repo_id(),
        revision_id: fixture_revision_id(),
        generation: fixture_generation(),
        batch_digest: "batch:repo-topic".to_string(),
        entries: vec![
            RepoTopicEntry {
                source_repo_id: RepoId::new("corp-a")
                    .expect("static fixture ID satisfies canonical policy"),
                topic: "security".to_string(),
            },
            RepoTopicEntry {
                source_repo_id: RepoId::new("corp-b")
                    .expect("static fixture ID satisfies canonical policy"),
                topic: "ml".to_string(),
            },
        ],
    }
}

fn fixture_repo_description_batch() -> RepoDescriptionIngestBatch {
    RepoDescriptionIngestBatch {
        repo_id: fixture_repo_id(),
        revision_id: fixture_revision_id(),
        generation: fixture_generation(),
        batch_digest: "batch:repo-description".to_string(),
        entries: vec![
            RepoDescriptionEntry {
                source_repo_id: RepoId::new("corp-a")
                    .expect("static fixture ID satisfies canonical policy"),
                description: "Apache distributed systems toolkit".to_string(),
            },
            RepoDescriptionEntry {
                source_repo_id: RepoId::new("corp-b")
                    .expect("static fixture ID satisfies canonical policy"),
                description: "Machine-learning training pipelines".to_string(),
            },
        ],
    }
}

fn fixture_file_contributor_batch() -> FileContributorIngestBatch {
    FileContributorIngestBatch {
        repo_id: fixture_repo_id(),
        revision_id: fixture_revision_id(),
        generation: fixture_generation(),
        batch_digest: "batch:file-contributor".to_string(),
        entries: vec![
            FileContributorEntry {
                source_repo_id: RepoId::new("corp-a")
                    .expect("static fixture ID satisfies canonical policy"),
                repo_relative_path: RepoRelativePath::new("src/gate-a.rs"),
                contributors: vec![
                    FileContributorIdentityEntry {
                        canonical: "alice".to_string(),
                        name: Some("Alice Example".to_string()),
                        email: Some("alice@example.com".to_string()),
                    },
                    FileContributorIdentityEntry {
                        canonical: "carol".to_string(),
                        name: Some("Carol Example".to_string()),
                        email: Some("carol@example.com".to_string()),
                    },
                ],
            },
            FileContributorEntry {
                source_repo_id: RepoId::new("corp-b")
                    .expect("static fixture ID satisfies canonical policy"),
                repo_relative_path: RepoRelativePath::new("src/gate-b.rs"),
                contributors: vec![FileContributorIdentityEntry {
                    canonical: "bob".to_string(),
                    name: Some("Bob Example".to_string()),
                    email: Some("bob@example.com".to_string()),
                }],
            },
        ],
    }
}

fn fixture_dirty_batch() -> DirtyIngestBatch {
    DirtyIngestBatch {
        repo_id: fixture_repo_id(),
        revision_id: fixture_revision_id(),
        generation: fixture_generation(),
        overlay_epoch_ms: 1_717_171_717_000,
        batch_digest: "dirty-batch:feed".to_string(),
        entries: vec![
            DirtyMutation::Upsert(fixture_dirty_record()),
            DirtyMutation::Delete(DirtyDelete {
                doc_id: ChunkId::new("chunk-evict"),
            }),
        ],
    }
}

fn fixture_structural_batch() -> StructuralIngestBatch {
    StructuralIngestBatch {
        repo_id: fixture_repo_id(),
        revision_id: fixture_revision_id(),
        generation: fixture_generation(),
        base_generation: Some(ManifestGeneration::new(6)),
        manifest_digest: "sha256:structural-manifest".to_string(),
        batch_digest: "sha256:structural-batch".to_string(),
        mode: BatchIngestMode::Delta,
        replace_scopes: vec![StructuralReplaceScope {
            scope: fixture_scope_key(),
            scope_digest: "scope:structural-1".to_string(),
            trees: vec![StructuralTreeRecord {
                chunk_id: fixture_chunk_id(),
                record: fixture_parse_tree_record(),
            }],
        }],
        tombstone_scopes: vec![StructuralTombstoneScope {
            scope: SearchScopeKey {
                doc_surface: SearchScopeSurface::Chunk,
                repo_relative_path: RepoRelativePath::new("src/old.rs"),
            },
        }],
        seal: true,
    }
}

#[test]
fn batch_ingest_mode_round_trip() -> TestRes {
    for mode in [BatchIngestMode::ReplaceGeneration, BatchIngestMode::Delta] {
        let bytes = encode(&mode)?;
        let decoded: BatchIngestMode = decode(&bytes)?;
        assert_eq!(decoded, mode);
    }
    Ok(())
}

#[test]
fn search_corpus_ingest_batch_round_trip() -> TestRes {
    let mut batch = fixture_search_corpus_batch();
    batch.clear_surfaces = vec![SearchScopeSurface::Chunk];
    let bytes = encode(&batch)?;
    let decoded: SearchCorpusIngestBatch = decode(&bytes)?;
    assert_eq!(decoded, batch);
    Ok(())
}

#[test]
#[cfg_attr(
    miri,
    ignore = "ciborium f16 path uses aarch64 inline asm that Miri cannot execute; native f32 vec serde is exercised in stable tests + fuzz"
)]
fn semantic_ingest_batch_round_trip() -> TestRes {
    let mut batch = fixture_semantic_batch();
    batch.clear_surfaces = vec![SearchScopeSurface::Module];
    let bytes = encode(&batch)?;
    let decoded: SemanticIngestBatch = decode(&bytes)?;
    assert_eq!(decoded, batch);
    Ok(())
}

#[test]
fn old_incomplete_batches_and_receipts_are_refused_v1() -> TestRes {
    for field in [
        "bundle_payload",
        "clear_surfaces",
        "semantic_replace_scopes",
        "semantic_tombstone_scopes",
    ] {
        let mut search_value = serde_json::to_value(fixture_search_corpus_batch())?;
        let removed = search_value
            .as_object_mut()
            .ok_or("search batch fixture must encode as a map")?
            .remove(field);
        assert!(removed.is_some(), "fixture missing {field}");
        let error = serde_json::from_value::<SearchCorpusIngestBatch>(search_value)
            .expect_err("missing search batch field must refuse");
        assert!(error.to_string().contains(field), "{error}");
    }

    for field in ["required_corpora", "corpus_policy_digest", "clear_surfaces"] {
        let mut semantic_value = serde_json::to_value(fixture_semantic_batch())?;
        let removed = semantic_value
            .as_object_mut()
            .ok_or("semantic batch fixture must encode as a map")?
            .remove(field);
        assert!(removed.is_some(), "fixture missing {field}");
        let error = serde_json::from_value::<SemanticIngestBatch>(semantic_value)
            .expect_err("missing semantic batch field must refuse");
        assert!(error.to_string().contains(field), "{error}");
    }

    let semantic_fixture = fixture_semantic_batch();
    let replace = semantic_fixture
        .replace_scopes
        .first()
        .ok_or("semantic replace fixture is empty")?;
    let mut old_replace = serde_json::to_value(replace)?;
    let removed = old_replace
        .as_object_mut()
        .ok_or("semantic replace fixture must encode as a map")?
        .remove("cluster_memberships");
    assert!(removed.is_some());
    let error = serde_json::from_value::<SemanticReplaceScope>(old_replace)
        .expect_err("missing cluster memberships must refuse");
    assert!(error.to_string().contains("cluster_memberships"), "{error}");

    let tombstone = semantic_fixture
        .tombstone_scopes
        .first()
        .ok_or("semantic tombstone fixture is empty")?;
    let mut old_tombstone = serde_json::to_value(tombstone)?;
    let removed = old_tombstone
        .as_object_mut()
        .ok_or("semantic tombstone fixture must encode as a map")?
        .remove("semantic_scope");
    assert!(removed.is_some());
    let error = serde_json::from_value::<SemanticTombstoneScope>(old_tombstone)
        .expect_err("missing semantic scope field must refuse");
    assert!(error.to_string().contains("semantic_scope"), "{error}");

    let old_path = SearchScopeKey {
        doc_surface: SearchScopeSurface::Chunk,
        repo_relative_path: RepoRelativePath::new("src/old.rs"),
    };
    let legacy_only = serde_json::json!({"scope": old_path});
    let legacy_only_cbor = encode(&legacy_only)?;
    let error = serde_json::from_value::<SemanticTombstoneScope>(legacy_only)
        .expect_err("legacy path-only tombstone must refuse");
    assert!(error.to_string().contains("scope"), "{error}");
    assert!(decode::<SemanticTombstoneScope>(&legacy_only_cbor).is_err());

    let mut mixed = serde_json::to_value(tombstone)?;
    let replaced = mixed
        .as_object_mut()
        .ok_or("semantic tombstone fixture must encode as a map")?
        .insert("scope".to_string(), serde_json::to_value(old_path)?);
    assert!(replaced.is_none());
    let mixed_cbor = encode(&mixed)?;
    let error = serde_json::from_value::<SemanticTombstoneScope>(mixed)
        .expect_err("legacy scope field must refuse even with typed authority");
    assert!(error.to_string().contains("scope"), "{error}");
    assert!(decode::<SemanticTombstoneScope>(&mixed_cbor).is_err());

    let mut null_scope = serde_json::to_value(tombstone)?;
    let replaced = null_scope
        .as_object_mut()
        .ok_or("semantic tombstone fixture must encode as a map")?
        .insert("semantic_scope".to_string(), serde_json::Value::Null);
    assert!(replaced.is_some());
    assert!(serde_json::from_value::<SemanticTombstoneScope>(null_scope).is_err());

    let mut empty_owner = serde_json::to_value(tombstone)?;
    let semantic_scope = empty_owner
        .get_mut("semantic_scope")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or("semantic tombstone fixture must contain a typed scope")?;
    let replaced = semantic_scope.insert("owner_id".to_string(), serde_json::json!(""));
    assert!(replaced.is_some());
    let error = serde_json::from_value::<SemanticTombstoneScope>(empty_owner)
        .expect_err("empty semantic owner must refuse");
    assert!(error.to_string().contains("owner_id"), "{error}");

    let contributor = FileContributorIdentityEntry {
        canonical: "person:alice".to_string(),
        name: None,
        email: None,
    };
    for field in ["name", "email"] {
        let mut old_contributor = serde_json::to_value(&contributor)?;
        let removed = old_contributor
            .as_object_mut()
            .ok_or("contributor fixture must encode as a map")?
            .remove(field);
        assert!(removed.is_some());
        let error = serde_json::from_value::<FileContributorIdentityEntry>(old_contributor)
            .expect_err("missing contributor field must refuse");
        assert!(error.to_string().contains(field), "{error}");
    }

    // Receipts are produced only by the search plane and every field is
    // required (QI-BB-032): a receipt without its clear-surface count
    // does not decode instead of reading as zero.
    let mut receipt_value = serde_json::to_value(BatchPublishReceipt::empty_for(
        fixture_generation(),
        Some("manifest:legacy".to_string()),
        "batch:legacy",
    ))?;
    let _removed_accepted_clear_surfaces = receipt_value
        .as_object_mut()
        .ok_or("receipt fixture must encode as a map")?
        .remove("accepted_clear_surfaces");
    assert!(serde_json::from_value::<BatchPublishReceipt>(receipt_value).is_err());

    for field in [
        "accepted_semantic_replace_scopes",
        "accepted_semantic_tombstone_scopes",
    ] {
        let mut receipt_value = serde_json::to_value(BatchPublishReceipt::empty_for(
            fixture_generation(),
            Some("manifest:legacy".to_string()),
            "batch:legacy",
        ))?;
        drop(
            receipt_value
                .as_object_mut()
                .ok_or("receipt fixture must encode as a map")?
                .remove(field),
        );
        assert!(
            serde_json::from_value::<BatchPublishReceipt>(receipt_value).is_err(),
            "missing {field} must fail closed"
        );
    }
    Ok(())
}

#[test]
fn search_corpus_clear_surface_authority_rejects_conflicts_v1() {
    let mut batch = fixture_search_corpus_batch();
    batch.clear_surfaces = vec![SearchScopeSurface::Chunk];
    assert_eq!(
        batch.validate_surface_mutations_v1(),
        Err(SearchCorpusSurfaceMutationConflictV1::ClearAndReplace(
            SearchScopeSurface::Chunk
        ))
    );

    batch.clear_surfaces = vec![SearchScopeSurface::Chunk, SearchScopeSurface::Chunk];
    assert_eq!(
        batch.validate_surface_mutations_v1(),
        Err(SearchCorpusSurfaceMutationConflictV1::DuplicateClear(
            SearchScopeSurface::Chunk
        ))
    );

    batch.clear_surfaces = vec![SearchScopeSurface::Symbol, SearchScopeSurface::Chunk];
    assert_eq!(
        batch.validate_surface_mutations_v1(),
        Err(SearchCorpusSurfaceMutationConflictV1::NonCanonicalClearOrder)
    );

    let mut batch = fixture_search_corpus_batch();
    let duplicate_replace = batch
        .replace_scopes
        .first()
        .expect("fixture must contain one replace scope")
        .clone();
    batch.replace_scopes.push(duplicate_replace);
    assert_eq!(
        batch.validate_surface_mutations_v1(),
        Err(
            SearchCorpusSurfaceMutationConflictV1::DuplicateReplaceScope(SearchScopeSurface::Chunk)
        )
    );

    let mut batch = fixture_search_corpus_batch();
    batch
        .tombstone_scopes
        .first_mut()
        .expect("fixture must contain one tombstone scope")
        .file = SourceFileKey {
        source_repo_id: fixture_repo_id(),
        repo_relative_path: RepoRelativePath::new("src/main.rs"),
    };
    assert_eq!(
        batch.validate_surface_mutations_v1(),
        Err(SearchCorpusSurfaceMutationConflictV1::ReplaceAndTombstone(
            SearchScopeSurface::Chunk
        ))
    );
}

#[test]
fn g0_duplicate_replacements_cannot_overwrite_one_file() {
    for reverse in [false, true] {
        let mut batch = fixture_search_corpus_batch();
        batch.tombstone_scopes.clear();
        let other = batch.replace_scopes[0].clone();
        batch.replace_scopes.push(other);
        if reverse {
            batch.replace_scopes.reverse();
        }
        assert!(batch.validate_surface_mutations_v1().is_err());
    }
}

#[test]
fn g0_duplicate_tombstones_cannot_address_one_file_twice() {
    let mut batch = fixture_search_corpus_batch();
    batch.replace_scopes.clear();
    let other = batch.tombstone_scopes[0].clone();
    batch.tombstone_scopes.push(other);
    assert!(batch.validate_surface_mutations_v1().is_err());
}

#[test]
fn g0_replace_and_tombstone_conflict_across_surface_aliases() {
    let mut batch = fixture_search_corpus_batch();
    batch.tombstone_scopes[0].file.repo_relative_path = RepoRelativePath::new("src/main.rs");
    assert!(batch.validate_surface_mutations_v1().is_err());
}

#[test]
fn g0_replacement_cannot_carry_a_different_chunk_path() {
    let mut batch = fixture_search_corpus_batch();
    batch.tombstone_scopes.clear();
    batch.replace_scopes[0].chunks[0].repo_relative_path = RepoRelativePath::new("src/other.rs");
    let scope = &mut batch.replace_scopes[0];
    scope.coverage.unit_set_sha256 =
        crate::source_file_unit_set_sha256(&scope.chunks, &scope.symbols)
            .expect("fixture units encode");
    assert_eq!(
        batch.validate_surface_mutations_v1(),
        Err(SearchCorpusSurfaceMutationConflictV1::RecordPathMismatch(
            SearchScopeSurface::Chunk
        ))
    );
}

#[test]
fn g0_clear_cannot_overlap_file_wide_mutation() {
    for surface in [SearchScopeSurface::Chunk, SearchScopeSurface::Symbol] {
        let mut batch = fixture_search_corpus_batch();
        batch.tombstone_scopes.clear();
        batch.clear_surfaces = vec![surface];
        assert!(batch.validate_surface_mutations_v1().is_err());
        batch.replace_scopes.clear();
        batch.tombstone_scopes = vec![SearchCorpusTombstoneScope {
            file: SourceFileKey {
                source_repo_id: fixture_repo_id(),
                repo_relative_path: RepoRelativePath::new("src/main.rs"),
            },
        }];
        assert!(batch.validate_surface_mutations_v1().is_err());
    }
}

#[test]
fn g0_valid_combined_file_and_disjoint_tombstone_remain_accepted() {
    let mut batch = fixture_search_corpus_batch();
    batch.tombstone_scopes[0].file.repo_relative_path = RepoRelativePath::new("src/old.rs");
    assert!(batch.validate_surface_mutations_v1().is_ok());
}

#[test]
fn source_bytes_are_required_and_bound_to_file_revision_digest() {
    let good = fixture_search_corpus_batch();
    assert!(good.validate_surface_mutations_v1().is_ok());

    let mut cbor = Vec::new();
    ciborium::into_writer(&good.replace_scopes[0], &mut cbor).expect("encode scope CBOR");
    let wire: ciborium::value::Value =
        ciborium::from_reader(cbor.as_slice()).expect("decode scope CBOR value");
    let ciborium::value::Value::Map(fields) = wire else {
        panic!("scope must be a CBOR map");
    };
    assert!(fields.iter().any(|(key, value)| {
            matches!(key, ciborium::value::Value::Text(name) if name == "source_bytes")
                && matches!(value, ciborium::value::Value::Bytes(bytes) if bytes == &good.replace_scopes[0].source_bytes)
        }));
    let decoded: SearchCorpusReplaceScope =
        ciborium::from_reader(cbor.as_slice()).expect("decode scope CBOR");
    assert_eq!(decoded, good.replace_scopes[0]);

    let mut changed_bytes = good.clone();
    changed_bytes.replace_scopes[0].source_bytes[0] ^= 1;
    assert_eq!(
        changed_bytes.validate_surface_mutations_v1(),
        Err(SearchCorpusSurfaceMutationConflictV1::SourceBytesDigestMismatch)
    );

    let mut changed_digest = good.clone();
    changed_digest.replace_scopes[0]
        .coverage
        .source
        .source_sha256 = [7; 32];
    assert_eq!(
        changed_digest.validate_surface_mutations_v1(),
        Err(SearchCorpusSurfaceMutationConflictV1::SourceBytesDigestMismatch)
    );

    let mut encoded = serde_json::to_value(&good.replace_scopes[0]).expect("encode scope");
    let json_decoded: SearchCorpusReplaceScope =
        serde_json::from_value(encoded.clone()).expect("decode scope JSON");
    assert_eq!(json_decoded, good.replace_scopes[0]);
    drop(
        encoded
            .as_object_mut()
            .expect("object")
            .remove("source_bytes"),
    );
    assert!(serde_json::from_value::<SearchCorpusReplaceScope>(encoded).is_err());
}

#[test]
fn chunk_bytes_must_match_the_immutable_source() {
    let mut batch = fixture_search_corpus_batch();
    batch.replace_scopes[0].chunks[0].text = "fn Main() {}".into();
    let scope = &mut batch.replace_scopes[0];
    scope.coverage.unit_set_sha256 =
        crate::source_file_unit_set_sha256(&scope.chunks, &scope.symbols)
            .expect("fixture units encode");
    assert_eq!(
        batch.validate_surface_mutations_v1(),
        Err(SearchCorpusSurfaceMutationConflictV1::ChunkSourceMismatch)
    );
}

#[test]
fn raw_ascii_symbol_name_policy_checks_definition_bytes() {
    let mut batch = fixture_search_corpus_batch();
    let scope = &mut batch.replace_scopes[0];
    scope.coverage.symbol_name_source_policy = crate::SymbolNameSourcePolicyV1::RawAsciiLocalName;
    scope.coverage.symbols = crate::SymbolCoverage::Complete { symbol_count: 1 };
    scope.symbols.push(SymbolRecord {
        symbol_id: crate::SymbolId::new("symbol-main"),
        repo_relative_path: RepoRelativePath::new("src/main.rs"),
        language: scope.coverage.language.clone(),
        symbol_kind: crate::lex::SymbolKindCode::new("function").expect("valid kind"),
        symbol_kind_family: None,
        local_name: "main".into(),
        qualified_name: "main".into(),
        signature: None,
        visibility: None,
        definition_span: crate::lex::SymbolSpan {
            path: "src/main.rs".into(),
            byte_start: 0,
            byte_end: 9,
            line_start: 1,
            line_end: 1,
        },
        container_qualified_name: None,
        relationship: crate::lex::SymbolRelationship::Def,
    });
    scope.coverage.unit_set_sha256 =
        crate::source_file_unit_set_sha256(&scope.chunks, &scope.symbols)
            .expect("fixture units encode");
    assert_eq!(batch.validate_surface_mutations_v1(), Ok(()));

    // A name elsewhere in the file cannot justify a mismatched span.
    batch.replace_scopes[0].symbols[0].definition_span.byte_end = 2;
    assert_eq!(
        batch.validate_surface_mutations_v1(),
        Err(SearchCorpusSurfaceMutationConflictV1::SymbolNameSourceMismatch)
    );
    batch.replace_scopes[0].symbols[0].definition_span.byte_end = 9;
    batch.replace_scopes[0].symbols[0].local_name = "other".into();
    assert_eq!(
        batch.validate_surface_mutations_v1(),
        Err(SearchCorpusSurfaceMutationConflictV1::SymbolNameSourceMismatch)
    );
}

#[test]
fn g0_coverage_counts_units_and_source_ownership_are_independent_checks() {
    let good = fixture_search_corpus_batch();
    let mut wrong_digest = good.clone();
    wrong_digest.replace_scopes[0].coverage.unit_set_sha256 = [0; 32];
    assert_eq!(
        wrong_digest.validate_surface_mutations_v1(),
        Err(SearchCorpusSurfaceMutationConflictV1::CoverageUnitMismatch)
    );
    let mut false_count = good.clone();
    false_count.replace_scopes[0].coverage.symbols =
        crate::SymbolCoverage::Complete { symbol_count: 1 };
    assert_eq!(
        false_count.validate_surface_mutations_v1(),
        Err(SearchCorpusSurfaceMutationConflictV1::CoverageUnitMismatch)
    );
    let mut wrong_owner = good.clone();
    wrong_owner.replace_scopes[0].chunks[0].source_repo_id =
        Some(RepoId::new("other").expect("static repo"));
    assert_eq!(
        wrong_owner.validate_surface_mutations_v1(),
        Err(SearchCorpusSurfaceMutationConflictV1::RecordSourceMismatch)
    );
    let mut wrong_span = good.clone();
    wrong_span.replace_scopes[0].chunks[0].end_byte = 1;
    assert_eq!(
        wrong_span.validate_surface_mutations_v1(),
        Err(SearchCorpusSurfaceMutationConflictV1::InvalidRecordRange)
    );
    let mut zero_symbols = good;
    zero_symbols.replace_scopes[0].coverage.symbols =
        crate::SymbolCoverage::Complete { symbol_count: 0 };
    assert!(zero_symbols.validate_surface_mutations_v1().is_ok());
}

#[test]
fn g0_source_event_binds_payload_not_materialization_target() -> TestRes {
    let batch = fixture_search_corpus_batch();
    let original = crate::source_event_payload_sha256(&batch)?;
    let mut retargeted = batch.clone();
    retargeted.generation = ManifestGeneration::new(9);
    retargeted.manifest_digest = "other-target".into();
    retargeted.revision_id = RevisionId::new("other-containing-revision")?;
    assert_eq!(crate::source_event_payload_sha256(&retargeted)?, original);
    retargeted.validate_v1()?;
    let mut changed = batch.clone();
    changed.replace_scopes[0].coverage.source.source_sha256 = [3; 32];
    assert_eq!(
        changed.validate_v1(),
        Err(SearchCorpusBatchShapeErrorV1::SourceEventPayloadMismatch)
    );
    let mut unsealed = batch.clone();
    unsealed.seal = false;
    assert_eq!(
        unsealed.validate_v1(),
        Err(SearchCorpusBatchShapeErrorV1::UnsealedSourceEvent)
    );
    let mut wire = serde_json::to_value(&batch)?;
    let _removed = wire
        .as_object_mut()
        .ok_or("expected map")?
        .remove("source_event");
    assert!(serde_json::from_value::<SearchCorpusIngestBatch>(wire).is_err());
    let mut old_scope = serde_json::to_value(&batch.replace_scopes[0])?;
    let map = old_scope.as_object_mut().ok_or("expected map")?;
    let _removed = map.remove("coverage");
    let _old = map.insert("scope".into(), serde_json::to_value(fixture_scope_key())?);
    assert!(serde_json::from_value::<SearchCorpusReplaceScope>(old_scope).is_err());
    Ok(())
}

#[test]
fn history_ingest_batch_round_trip() -> TestRes {
    let batch = fixture_history_batch();
    let bytes = encode(&batch)?;
    let decoded: HistoryIngestBatch = decode(&bytes)?;
    assert_eq!(decoded, batch);
    Ok(())
}

#[test]
fn repo_commit_recency_ingest_batch_round_trip() -> TestRes {
    let batch = fixture_repo_commit_recency_batch();
    let bytes = encode(&batch)?;
    let decoded: RepoCommitRecencyIngestBatch = decode(&bytes)?;
    assert_eq!(decoded, batch);
    Ok(())
}

#[test]
fn repo_meta_ingest_batch_round_trip() -> TestRes {
    let batch = fixture_repo_meta_batch();
    let bytes = encode(&batch)?;
    let decoded: RepoMetaIngestBatch = decode(&bytes)?;
    assert_eq!(decoded, batch);
    Ok(())
}

#[test]
fn repo_topic_ingest_batch_round_trip() -> TestRes {
    let batch = fixture_repo_topic_batch();
    let bytes = encode(&batch)?;
    let decoded: RepoTopicIngestBatch = decode(&bytes)?;
    assert_eq!(decoded, batch);
    Ok(())
}

#[test]
fn file_contributor_ingest_batch_round_trip() -> TestRes {
    let batch = fixture_file_contributor_batch();
    let bytes = encode(&batch)?;
    let decoded: FileContributorIngestBatch = decode(&bytes)?;
    assert_eq!(decoded, batch);
    Ok(())
}

#[test]
fn file_contributor_identity_manual_serde_enforces_wire_contract() {
    let missing_optional =
        serde_json::from_str::<FileContributorIdentityEntry>(r#"{"canonical":"alice"}"#);
    assert!(matches!(
        missing_optional,
        Err(error) if error.to_string().contains("missing field `name`")
    ));

    let duplicate = serde_json::from_str::<FileContributorIdentityEntry>(
        r#"{"canonical":"alice","canonical":"bob"}"#,
    );
    assert!(matches!(
        duplicate,
        Err(error) if error.to_string().contains("duplicate field `canonical`")
    ));

    let missing = serde_json::from_str::<FileContributorIdentityEntry>(
        r#"{"name":"Alice","email":"alice@example.com"}"#,
    );
    assert!(matches!(
        missing,
        Err(error) if error.to_string().contains("missing field `canonical`")
    ));

    let unknown = serde_json::from_str::<FileContributorIdentityEntry>(
        r#"{"canonical":"alice","unexpected":true}"#,
    );
    assert!(matches!(
        unknown,
        Err(error) if error.to_string().contains("unknown field `unexpected`")
    ));
}

#[test]
fn dirty_ingest_batch_round_trip() -> TestRes {
    let batch = fixture_dirty_batch();
    let bytes = encode(&batch)?;
    let decoded: DirtyIngestBatch = decode(&bytes)?;
    assert_eq!(decoded, batch);
    Ok(())
}

#[test]
fn structural_ingest_batch_round_trip() -> TestRes {
    let batch = fixture_structural_batch();
    let bytes = encode(&batch)?;
    let decoded: StructuralIngestBatch = decode(&bytes)?;
    assert_eq!(decoded, batch);
    Ok(())
}

#[test]
fn batch_publish_receipt_round_trip() -> TestRes {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(9),
        manifest_digest: Some("sha256:feed".to_string()),
        batch_digest: "batch:fixture".to_string(),
        applied: true,
        durable_sequence: 7,
        semantic_content: None,
        accepted_replace_scopes: 2,
        accepted_tombstone_scopes: 1,
        accepted_semantic_replace_scopes: 3,
        accepted_semantic_tombstone_scopes: 1,
        accepted_clear_surfaces: 0,
        sealed: true,
    };
    let bytes = encode(&receipt)?;
    let decoded: BatchPublishReceipt = decode(&bytes)?;
    assert_eq!(decoded, receipt);
    Ok(())
}

#[test]
fn search_plane_ingest_request_envelope_round_trip_search_corpus() -> TestRes {
    let envelope = SearchPlaneIngestIpcRequestEnvelope {
        request_id: 1,
        payload: SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(
            fixture_search_corpus_batch(),
        ),
    };
    let bytes = encode(&envelope)?;
    let decoded: SearchPlaneIngestIpcRequestEnvelope = decode(&bytes)?;
    assert_eq!(decoded, envelope);
    Ok(())
}

#[test]
fn search_plane_ingest_request_envelope_round_trip_history() -> TestRes {
    let envelope = SearchPlaneIngestIpcRequestEnvelope {
        request_id: 3,
        payload: SearchPlaneIngestIpcRequest::PublishHistoryBatch(fixture_history_batch()),
    };
    let bytes = encode(&envelope)?;
    let decoded: SearchPlaneIngestIpcRequestEnvelope = decode(&bytes)?;
    assert_eq!(decoded, envelope);
    Ok(())
}

#[test]
fn search_plane_ingest_request_envelope_round_trip_repo_commit_recency() -> TestRes {
    let envelope = SearchPlaneIngestIpcRequestEnvelope {
        request_id: 4,
        payload: SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(
            fixture_repo_commit_recency_batch(),
        ),
    };
    let bytes = encode(&envelope)?;
    let decoded: SearchPlaneIngestIpcRequestEnvelope = decode(&bytes)?;
    assert_eq!(decoded, envelope);
    Ok(())
}

#[test]
fn search_plane_ingest_request_envelope_round_trip_repo_meta() -> TestRes {
    let envelope = SearchPlaneIngestIpcRequestEnvelope {
        request_id: 5,
        payload: SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(fixture_repo_meta_batch()),
    };
    let bytes = encode(&envelope)?;
    let decoded: SearchPlaneIngestIpcRequestEnvelope = decode(&bytes)?;
    assert_eq!(decoded, envelope);
    Ok(())
}

#[test]
fn search_plane_ingest_request_envelope_round_trip_repo_topic() -> TestRes {
    let envelope = SearchPlaneIngestIpcRequestEnvelope {
        request_id: 6,
        payload: SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(fixture_repo_topic_batch()),
    };
    let bytes = encode(&envelope)?;
    let decoded: SearchPlaneIngestIpcRequestEnvelope = decode(&bytes)?;
    assert_eq!(decoded, envelope);
    Ok(())
}

#[test]
fn search_plane_ingest_request_envelope_round_trip_repo_description() -> TestRes {
    let envelope = SearchPlaneIngestIpcRequestEnvelope {
        request_id: 12,
        payload: SearchPlaneIngestIpcRequest::PublishRepoDescriptionBatch(
            fixture_repo_description_batch(),
        ),
    };
    let bytes = encode(&envelope)?;
    let decoded: SearchPlaneIngestIpcRequestEnvelope = decode(&bytes)?;
    assert_eq!(decoded, envelope);
    Ok(())
}

#[test]
fn search_plane_ingest_request_envelope_round_trip_file_contributor() -> TestRes {
    let envelope = SearchPlaneIngestIpcRequestEnvelope {
        request_id: 11,
        payload: SearchPlaneIngestIpcRequest::PublishFileContributorBatch(
            fixture_file_contributor_batch(),
        ),
    };
    let bytes = encode(&envelope)?;
    let decoded: SearchPlaneIngestIpcRequestEnvelope = decode(&bytes)?;
    assert_eq!(decoded, envelope);
    Ok(())
}

#[test]
fn search_plane_ingest_request_envelope_round_trip_dirty() -> TestRes {
    let envelope = SearchPlaneIngestIpcRequestEnvelope {
        request_id: 6,
        payload: SearchPlaneIngestIpcRequest::PublishDirtyBatch(fixture_dirty_batch()),
    };
    let bytes = encode(&envelope)?;
    let decoded: SearchPlaneIngestIpcRequestEnvelope = decode(&bytes)?;
    assert_eq!(decoded, envelope);
    Ok(())
}

#[test]
fn search_plane_ingest_request_envelope_round_trip_structural() -> TestRes {
    let envelope = SearchPlaneIngestIpcRequestEnvelope {
        request_id: 7,
        payload: SearchPlaneIngestIpcRequest::PublishStructuralBatch(fixture_structural_batch()),
    };
    let bytes = encode(&envelope)?;
    let decoded: SearchPlaneIngestIpcRequestEnvelope = decode(&bytes)?;
    assert_eq!(decoded, envelope);
    Ok(())
}

#[test]
fn search_plane_ingest_response_envelope_round_trip_receipt() -> TestRes {
    let envelope = SearchPlaneIngestIpcResponseEnvelope {
        request_id: 3,
        payload: SearchPlaneIngestIpcResponse::SearchCorpusReceipt(
            crate::SearchCorpusPublishOutcome {
                publication: crate::SourcePublicationBinding::for_batch(
                    &fixture_search_corpus_batch(),
                ),
                observation: None,
                receipt: BatchPublishReceipt {
                    generation: ManifestGeneration::new(1),
                    manifest_digest: Some("digest-lex".to_string()),
                    batch_digest: "batch:fixture".to_string(),
                    applied: true,
                    durable_sequence: 7,
                    semantic_content: None,
                    accepted_replace_scopes: 1,
                    accepted_tombstone_scopes: 0,
                    accepted_semantic_replace_scopes: 0,
                    accepted_semantic_tombstone_scopes: 0,
                    accepted_clear_surfaces: 0,
                    sealed: true,
                },
            },
        ),
    };
    let bytes = encode(&envelope)?;
    let decoded: SearchPlaneIngestIpcResponseEnvelope = decode(&bytes)?;
    assert_eq!(decoded, envelope);
    Ok(())
}

#[test]
fn search_plane_ingest_response_envelope_round_trip_error() -> TestRes {
    let envelope = SearchPlaneIngestIpcResponseEnvelope {
        request_id: 4,
        payload: SearchPlaneIngestIpcResponse::Error(SearchPlaneIpcError {
            code: crate::SearchPlaneErrorCodeV2::Internal,
            message: "channel write rejected".to_string(),
            repair: None,
        }),
    };
    let bytes = encode(&envelope)?;
    let decoded: SearchPlaneIngestIpcResponseEnvelope = decode(&bytes)?;
    assert_eq!(decoded, envelope);
    Ok(())
}

#[test]
fn search_plane_ingest_response_envelope_round_trip_history_receipt() -> TestRes {
    let envelope = SearchPlaneIngestIpcResponseEnvelope {
        request_id: 5,
        payload: SearchPlaneIngestIpcResponse::HistoryReceipt(BatchPublishReceipt {
            generation: ManifestGeneration::new(3),
            manifest_digest: Some("digest-hist".to_string()),
            batch_digest: "batch:fixture".to_string(),
            applied: true,
            durable_sequence: 7,
            semantic_content: None,
            accepted_replace_scopes: 4,
            accepted_tombstone_scopes: 0,
            accepted_semantic_replace_scopes: 0,
            accepted_semantic_tombstone_scopes: 0,
            accepted_clear_surfaces: 0,
            sealed: false,
        }),
    };
    let bytes = encode(&envelope)?;
    let decoded: SearchPlaneIngestIpcResponseEnvelope = decode(&bytes)?;
    assert_eq!(decoded, envelope);
    Ok(())
}

#[test]
fn search_plane_ingest_response_envelope_round_trip_repo_commit_recency_receipt() -> TestRes {
    let envelope = SearchPlaneIngestIpcResponseEnvelope {
        request_id: 6,
        payload: SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(BatchPublishReceipt {
            generation: ManifestGeneration::new(4),
            manifest_digest: Some("digest-repo-commit-recency".to_string()),
            batch_digest: "batch:fixture".to_string(),
            applied: true,
            durable_sequence: 7,
            semantic_content: None,
            accepted_replace_scopes: 2,
            accepted_tombstone_scopes: 0,
            accepted_semantic_replace_scopes: 0,
            accepted_semantic_tombstone_scopes: 0,
            accepted_clear_surfaces: 0,
            sealed: false,
        }),
    };
    let bytes = encode(&envelope)?;
    let decoded: SearchPlaneIngestIpcResponseEnvelope = decode(&bytes)?;
    assert_eq!(decoded, envelope);
    Ok(())
}

#[test]
fn search_plane_ingest_response_envelope_round_trip_repo_meta_receipt() -> TestRes {
    let envelope = SearchPlaneIngestIpcResponseEnvelope {
        request_id: 7,
        payload: SearchPlaneIngestIpcResponse::RepoMetaReceipt(BatchPublishReceipt {
            generation: ManifestGeneration::new(5),
            manifest_digest: Some("digest-repo-meta".to_string()),
            batch_digest: "batch:fixture".to_string(),
            applied: true,
            durable_sequence: 7,
            semantic_content: None,
            accepted_replace_scopes: 2,
            accepted_tombstone_scopes: 0,
            accepted_semantic_replace_scopes: 0,
            accepted_semantic_tombstone_scopes: 0,
            accepted_clear_surfaces: 0,
            sealed: false,
        }),
    };
    let bytes = encode(&envelope)?;
    let decoded: SearchPlaneIngestIpcResponseEnvelope = decode(&bytes)?;
    assert_eq!(decoded, envelope);
    Ok(())
}

#[test]
fn search_plane_ingest_response_envelope_round_trip_repo_description_receipt() -> TestRes {
    let envelope = SearchPlaneIngestIpcResponseEnvelope {
        request_id: 13,
        payload: SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(BatchPublishReceipt {
            generation: ManifestGeneration::new(7),
            manifest_digest: Some("digest-repo-description".to_string()),
            batch_digest: "batch:fixture".to_string(),
            applied: true,
            durable_sequence: 7,
            semantic_content: None,
            accepted_replace_scopes: 2,
            accepted_tombstone_scopes: 0,
            accepted_semantic_replace_scopes: 0,
            accepted_semantic_tombstone_scopes: 0,
            accepted_clear_surfaces: 0,
            sealed: false,
        }),
    };
    let bytes = encode(&envelope)?;
    let decoded: SearchPlaneIngestIpcResponseEnvelope = decode(&bytes)?;
    assert_eq!(decoded, envelope);
    Ok(())
}

#[test]
fn search_plane_ingest_response_envelope_round_trip_repo_topic_receipt() -> TestRes {
    let envelope = SearchPlaneIngestIpcResponseEnvelope {
        request_id: 8,
        payload: SearchPlaneIngestIpcResponse::RepoTopicReceipt(BatchPublishReceipt {
            generation: ManifestGeneration::new(6),
            manifest_digest: Some("digest-repo-topic".to_string()),
            batch_digest: "batch:fixture".to_string(),
            applied: true,
            durable_sequence: 7,
            semantic_content: None,
            accepted_replace_scopes: 2,
            accepted_tombstone_scopes: 0,
            accepted_semantic_replace_scopes: 0,
            accepted_semantic_tombstone_scopes: 0,
            accepted_clear_surfaces: 0,
            sealed: false,
        }),
    };
    let bytes = encode(&envelope)?;
    let decoded: SearchPlaneIngestIpcResponseEnvelope = decode(&bytes)?;
    assert_eq!(decoded, envelope);
    Ok(())
}

#[test]
fn search_plane_ingest_response_envelope_round_trip_file_contributor_receipt() -> TestRes {
    let envelope = SearchPlaneIngestIpcResponseEnvelope {
        request_id: 12,
        payload: SearchPlaneIngestIpcResponse::FileContributorReceipt(BatchPublishReceipt {
            generation: ManifestGeneration::new(7),
            manifest_digest: Some("digest-file-contributor".to_string()),
            batch_digest: "batch:fixture".to_string(),
            applied: true,
            durable_sequence: 7,
            semantic_content: None,
            accepted_replace_scopes: 2,
            accepted_tombstone_scopes: 0,
            accepted_semantic_replace_scopes: 0,
            accepted_semantic_tombstone_scopes: 0,
            accepted_clear_surfaces: 0,
            sealed: false,
        }),
    };
    let bytes = encode(&envelope)?;
    let decoded: SearchPlaneIngestIpcResponseEnvelope = decode(&bytes)?;
    assert_eq!(decoded, envelope);
    Ok(())
}

#[test]
fn search_plane_ingest_response_envelope_round_trip_dirty_receipt() -> TestRes {
    let envelope = SearchPlaneIngestIpcResponseEnvelope {
        request_id: 8,
        payload: SearchPlaneIngestIpcResponse::DirtyReceipt(BatchPublishReceipt {
            generation: ManifestGeneration::new(4),
            manifest_digest: Some("digest-dirty".to_string()),
            batch_digest: "batch:fixture".to_string(),
            applied: true,
            durable_sequence: 7,
            semantic_content: None,
            accepted_replace_scopes: 1,
            accepted_tombstone_scopes: 1,
            accepted_semantic_replace_scopes: 0,
            accepted_semantic_tombstone_scopes: 0,
            accepted_clear_surfaces: 0,
            sealed: false,
        }),
    };
    let bytes = encode(&envelope)?;
    let decoded: SearchPlaneIngestIpcResponseEnvelope = decode(&bytes)?;
    assert_eq!(decoded, envelope);
    Ok(())
}

#[test]
fn search_plane_ingest_response_envelope_round_trip_structural_receipt() -> TestRes {
    let envelope = SearchPlaneIngestIpcResponseEnvelope {
        request_id: 9,
        payload: SearchPlaneIngestIpcResponse::StructuralReceipt(BatchPublishReceipt {
            generation: ManifestGeneration::new(5),
            manifest_digest: Some("digest-struct".to_string()),
            batch_digest: "batch:fixture".to_string(),
            applied: true,
            durable_sequence: 7,
            semantic_content: None,
            accepted_replace_scopes: 1,
            accepted_tombstone_scopes: 0,
            accepted_semantic_replace_scopes: 0,
            accepted_semantic_tombstone_scopes: 0,
            accepted_clear_surfaces: 0,
            sealed: false,
        }),
    };
    let bytes = encode(&envelope)?;
    let decoded: SearchPlaneIngestIpcResponseEnvelope = decode(&bytes)?;
    assert_eq!(decoded, envelope);
    Ok(())
}

#[test]
fn unknown_batch_ingest_mode_tag_rejected() {
    let bad = serde_json::json!("Unknown");
    let result = BatchIngestMode::deserialize(bad);
    assert!(
        result.is_err(),
        "unknown batch ingest mode tag should fail closed: {result:?}"
    );
    let Err(err) = result else {
        return;
    };
    assert!(err.to_string().contains("unknown variant"));
}

// QI-BB-029: the batch shape is refused by the contract before any
// adapter observes it, one defect per refusal, and a well-formed batch
// of either mode passes.
#[test]
fn search_corpus_batch_shape_is_validated_before_any_adapter() -> TestRes {
    let mut batch = fixture_search_corpus_batch();
    batch.validate_v1()?;

    batch.base_generation = Some(ManifestGeneration::new(1));
    assert!(matches!(
        batch.validate_v1(),
        Err(SearchCorpusBatchShapeErrorV1::ModeBaseMismatch {
            mode: BatchIngestMode::ReplaceGeneration,
            ..
        })
    ));

    batch.mode = BatchIngestMode::Delta;
    batch.base_generation = None;
    assert!(matches!(
        batch.validate_v1(),
        Err(SearchCorpusBatchShapeErrorV1::ModeBaseMismatch {
            mode: BatchIngestMode::Delta,
            base_generation: None,
        })
    ));

    batch.base_generation = Some(batch.generation);
    assert!(matches!(
        batch.validate_v1(),
        Err(SearchCorpusBatchShapeErrorV1::BaseNotOlderThanTarget { .. })
    ));

    batch.base_generation = Some(ManifestGeneration::new(
        batch.generation.get().saturating_sub(1),
    ));
    batch.source_event.payload_sha256 = crate::source_event_payload_sha256(&batch)?;
    batch.validate_v1()?;

    for value in ["", "has space"] {
        let mut malformed = fixture_search_corpus_batch();
        malformed.manifest_digest = value.to_string();
        assert_eq!(
            malformed.validate_v1(),
            Err(SearchCorpusBatchShapeErrorV1::DigestNotCanonical {
                field: "manifest_digest"
            }),
            "value {value:?}"
        );
    }
    // The batch digest must have the canonical shape: 64 lowercase hex.
    let uppercase = FIXTURE_BATCH_DIGEST.to_ascii_uppercase();
    for value in ["", "batch:feed", "tab\there", "\u{e9}", &uppercase] {
        let mut malformed = fixture_search_corpus_batch();
        malformed.batch_digest = value.to_string();
        assert_eq!(
            malformed.validate_v1(),
            Err(SearchCorpusBatchShapeErrorV1::BatchDigestNotCanonical),
            "value {value:?}"
        );
    }
    Ok(())
}
