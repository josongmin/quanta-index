#![expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "fixed independent fixtures and exact observable-shape assertions in owner tests"
)]

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    CapabilityStatusV1, ClusterMembershipReplaceV1, OwnerDocKind, SemanticCorpusKindV1,
    SemanticSourceRecordV1, SemanticSourceReplaceScopeV1, SemanticSourceScopeKeyV1,
    SourcePublicationEvent, SourceRoleV1, SymbolCoverage, SymbolId,
};
use quanta_index_sdk::preparation::{
    CompleteSourceSet, MarkdownAdapter, PlainTextAdapter, PreparationBudgets,
    PreparationCapabilities, PreparationError, PreparationProfile, PreparedSource,
    PriorSourceManifest, ReconcileIntent, SourceAdapter, SourceContext, TextSource,
    reconcile_complete_universe,
};
use quanta_index_sdk::{
    ChunkId, ChunkRecord, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusBatch, SearchScopeSurface, SourceFileKey, SourceFileRevision,
};
use sha2::{Digest as _, Sha256};

fn budgets() -> PreparationBudgets {
    PreparationBudgets::new(1024, 1024, 1024, 16, 64 * 1024).unwrap()
}

#[test]
fn public_budget_names_emitted_text_separately_from_input_and_batch() {
    let budget = PreparationBudgets::new(8, 4, 2, 16, 4096).unwrap();
    assert_eq!(budget.max_input_bytes(), 8);
    assert_eq!(budget.max_emitted_text_bytes(), 4);
    assert_eq!(budget.max_batch_bytes(), 4096);
    assert_eq!(
        budget
            .with_max_emitted_text_bytes(3)
            .unwrap()
            .max_emitted_text_bytes(),
        3
    );
}

fn context(stable: &str, path: &str, bytes: &[u8], profile: PreparationProfile) -> SourceContext {
    context_in_repo(stable, "source", path, bytes, profile)
}

fn context_in_repo(
    stable: &str,
    source_repo: &str,
    path: &str,
    bytes: &[u8],
    profile: PreparationProfile,
) -> SourceContext {
    SourceContext::new(
        stable,
        SourceFileRevision {
            file: SourceFileKey {
                source_repo_id: RepoId::new(source_repo).unwrap(),
                repo_relative_path: RepoRelativePath::new(path),
            },
            revision_id: RevisionId::new("source-r1").unwrap(),
            source_sha256: Sha256::digest(bytes).into(),
        },
        profile,
    )
    .unwrap()
}

fn plain(stable: &str, path: &str, bytes: &[u8]) -> PreparedSource {
    plain_in_repo(stable, "source", path, bytes)
}

fn plain_in_repo(stable: &str, source_repo: &str, path: &str, bytes: &[u8]) -> PreparedSource {
    PlainTextAdapter
        .prepare(TextSource::new(
            context_in_repo(
                stable,
                source_repo,
                path,
                bytes,
                PlainTextAdapter::profile(budgets()).unwrap(),
            ),
            bytes,
        ))
        .unwrap()
}

fn cluster_source(reversed: bool, changed_member: bool) -> PreparedSource {
    let bytes = b"source";
    let context = context(
        "cluster",
        "src/cluster.txt",
        bytes,
        PreparationProfile::new("fixture:cluster", "v1", budgets()).unwrap(),
    );
    let scope = SemanticSourceScopeKeyV1 {
        corpus_kind: SemanticCorpusKindV1::ClusterCard,
        owner_kind: OwnerDocKind::Module,
        owner_id: "cluster".into(),
    };
    let mut sources = Vec::new();
    let mut memberships = Vec::new();
    for id in ["a", "b"] {
        let record = SemanticSourceRecordV1 {
            record_id: format!("record-{id}"),
            corpus_kind: scope.corpus_kind,
            owner_kind: scope.owner_kind,
            owner_id: scope.owner_id.clone(),
            source_doc_id: format!("doc-{id}"),
            parent_owner_id: None,
            repo_relative_path: context.source().file.repo_relative_path.clone(),
            language: Some("text".into()),
            package: None,
            symbol_kind: None,
            visibility: None,
            source_role: SourceRoleV1::CardText,
            generated: false,
            capability_status: CapabilityStatusV1::Full,
            raw_fallback_reason: None,
            authority_digest: format!("auth:{id}"),
            render_policy_digest: "render:v1".into(),
            card_schema_version: 1,
            text: format!("card {id}"),
        };
        memberships.push(ClusterMembershipReplaceV1 {
            cluster_record_id: record.record_id.clone(),
            authority_digest: record.authority_digest.clone(),
            members: vec![SymbolId::new(if id == "b" && changed_member {
                "symbol:changed"
            } else if id == "a" {
                "symbol:a"
            } else {
                "symbol:b"
            })],
        });
        sources.push(record);
    }
    if reversed {
        memberships.reverse();
    }
    PreparedSource::from_lexical(
        context,
        bytes.to_vec(),
        LanguageCode::new("text").unwrap(),
        Vec::new(),
        Vec::new(),
        SymbolCoverage::NotRequested,
        vec![SemanticSourceReplaceScopeV1 {
            scope,
            scope_digest: "scope:v1".into(),
            sources,
            cluster_memberships: memberships,
        }],
    )
    .unwrap()
}

#[test]
fn public_cluster_membership_order_is_canonical_across_wire_and_prior() {
    let empty = PriorSourceManifest::new(Vec::new()).unwrap();
    let forward = reconcile_complete_universe(
        &empty,
        CompleteSourceSet::new(vec![cluster_source(false, false)]),
        intent(None),
        64 * 1024,
    )
    .unwrap();
    let reverse = reconcile_complete_universe(
        &empty,
        CompleteSourceSet::new(vec![cluster_source(true, false)]),
        intent(None),
        64 * 1024,
    )
    .unwrap();
    let (forward_batch, prior) = forward.apply_to_batch(batch(1, None, "cluster")).unwrap();
    let (reverse_batch, reverse_prior) = reverse.apply_to_batch(batch(1, None, "cluster")).unwrap();
    assert_eq!(forward_batch, reverse_batch);
    assert_eq!(
        forward_batch.batch_digest().unwrap(),
        reverse_batch.batch_digest().unwrap()
    );
    assert_eq!(prior, reverse_prior);
    let mut encoded = Vec::new();
    ciborium::into_writer(&prior, &mut encoded).unwrap();
    let prior: PriorSourceManifest = ciborium::from_reader(encoded.as_slice()).unwrap();
    let no_op = reconcile_complete_universe(
        &prior,
        CompleteSourceSet::new(vec![cluster_source(true, false)]),
        intent(Some(1)),
        64 * 1024,
    )
    .unwrap();
    assert!(no_op.replacements().is_empty());
    assert_eq!(no_op.next_manifest(), &prior);
    let changed = reconcile_complete_universe(
        &prior,
        CompleteSourceSet::new(vec![cluster_source(true, true)]),
        intent(Some(1)),
        64 * 1024,
    )
    .unwrap();
    assert_eq!(changed.replacements().len(), 1);
}

#[test]
fn public_reconcile_allows_cross_repo_same_path_replacement_with_old_key_tombstone() {
    let empty = PriorSourceManifest::new(vec![]).unwrap();
    let initial = reconcile_complete_universe(
        &empty,
        CompleteSourceSet::new(vec![plain_in_repo("file", "repo-a", "x.txt", b"old")]),
        intent(None),
        64 * 1024,
    )
    .unwrap();
    let (_, prior) = initial
        .apply_to_batch(batch(1, None, "old-source"))
        .unwrap();
    let old_key = prior.entries()[0].source().file.clone();

    let changes = reconcile_complete_universe(
        &prior,
        CompleteSourceSet::new(vec![plain_in_repo("file", "repo-b", "x.txt", b"new")]),
        intent(Some(1)),
        64 * 1024,
    )
    .expect("old and current path ownership are separate universes");
    assert_eq!(changes.replacements().len(), 1);
    assert_eq!(changes.tombstones(), std::slice::from_ref(&old_key));

    let (delta, planned) = changes
        .apply_to_batch(batch(2, Some(1), "new-source"))
        .expect("canonical wire permits distinct old/new source keys at one path");
    assert_eq!(delta.replace_scopes().len(), 1);
    assert_eq!(delta.tombstone_scopes().len(), 1);
    assert_eq!(delta.tombstone_scopes()[0].file, old_key);
    assert_eq!(
        delta.replace_scopes()[0]
            .coverage
            .source
            .file
            .source_repo_id,
        RepoId::new("repo-b").unwrap()
    );
    assert_eq!(
        planned.entries()[0].source().file.source_repo_id,
        RepoId::new("repo-b").unwrap()
    );
}

#[test]
fn public_reconcile_refuses_preexisting_clear_on_no_op_delta() {
    let empty = PriorSourceManifest::new(vec![]).unwrap();
    let initial = reconcile_complete_universe(
        &empty,
        CompleteSourceSet::new(vec![plain("keep", "keep.txt", b"kept")]),
        intent(None),
        64 * 1024,
    )
    .unwrap();
    let (_, prior) = initial
        .apply_to_batch(batch(1, None, "keep-initial"))
        .unwrap();
    let changes = reconcile_complete_universe(
        &prior,
        CompleteSourceSet::new(vec![plain("keep", "keep.txt", b"kept")]),
        intent(Some(1)),
        64 * 1024,
    )
    .unwrap();
    assert!(changes.replacements().is_empty());
    assert!(changes.tombstones().is_empty());
    assert_eq!(
        changes.apply_to_batch(
            batch(2, Some(1), "hidden-clear").clear_surface(SearchScopeSurface::Chunk)
        ),
        Err(PreparationError::ManifestConflict)
    );
}

fn markdown(stable: &str, path: &str, bytes: &[u8]) -> PreparedSource {
    MarkdownAdapter
        .prepare(TextSource::new(
            context(
                stable,
                path,
                bytes,
                MarkdownAdapter::profile(budgets()).unwrap(),
            ),
            bytes,
        ))
        .unwrap()
}

struct CustomTextAdapter;

impl<'a> SourceAdapter<TextSource<'a>> for CustomTextAdapter {
    fn capabilities(&self) -> PreparationCapabilities {
        PreparationCapabilities::new(true, false, false)
    }

    fn prepare(&self, input: TextSource<'a>) -> Result<PreparedSource, PreparationError> {
        let (context, bytes) = input.into_parts();
        let text = std::str::from_utf8(bytes).map_err(|_error| PreparationError::InvalidUtf8)?;
        let language = LanguageCode::new("text").map_err(PreparationError::InvalidSource)?;
        let chunks = if text.is_empty() {
            Vec::new()
        } else {
            vec![ChunkRecord {
                chunk_id: ChunkId::new(format!("custom:{}", context.stable_key())),
                repo_relative_path: context.source().file.repo_relative_path.clone(),
                language: language.clone(),
                start_byte: 0,
                end_byte: u32::try_from(bytes.len())
                    .map_err(|_error| PreparationError::LimitExceeded("byte offset"))?,
                start_line: 1,
                end_line: 1,
                text: text.into(),
                structural: None,
                parent_chunk_id: None,
                source_repo_id: Some(context.source().file.source_repo_id.clone()),
            }]
        };
        PreparedSource::from_lexical(
            context,
            bytes.to_vec(),
            language,
            chunks,
            Vec::new(),
            SymbolCoverage::NotRequested,
            Vec::new(),
        )
    }
}

fn custom(stable: &str, path: &str, bytes: &[u8]) -> PreparedSource {
    let profile = PreparationProfile::new("fixture:custom-text", "custom-v1", budgets()).unwrap();
    CustomTextAdapter
        .prepare(TextSource::new(
            context(stable, path, bytes, profile),
            bytes,
        ))
        .unwrap()
}

fn empty_coverage_source(language: &str, symbols: SymbolCoverage) -> PreparedSource {
    let profile = PreparationProfile::new("fixture:coverage-only", "v1", budgets()).unwrap();
    PreparedSource::from_lexical(
        context("empty", "empty.txt", b"", profile),
        Vec::new(),
        LanguageCode::new(language).unwrap(),
        Vec::new(),
        Vec::new(),
        symbols,
        Vec::new(),
    )
    .unwrap()
}

fn assert_empty_coverage_transition_replaces(
    prior_source: PreparedSource,
    current_source: PreparedSource,
    case: &str,
) {
    assert_eq!(prior_source.source(), current_source.source(), "{case}");
    assert_eq!(
        prior_source.profile_sha256(),
        current_source.profile_sha256(),
        "{case}"
    );
    assert_eq!(
        prior_source.coverage().unit_set_sha256,
        current_source.coverage().unit_set_sha256,
        "{case}"
    );
    assert_ne!(prior_source.coverage(), current_source.coverage(), "{case}");
    let expected_coverage = current_source.coverage().clone();
    let empty = PriorSourceManifest::new(Vec::new()).unwrap();
    let initial = reconcile_complete_universe(
        &empty,
        CompleteSourceSet::new(vec![prior_source]),
        intent(None),
        64 * 1024,
    )
    .unwrap();
    let (_, prior) = initial.apply_to_batch(batch(1, None, case)).unwrap();
    let mut encoded = Vec::new();
    ciborium::into_writer(&prior, &mut encoded).unwrap();
    let prior: PriorSourceManifest = ciborium::from_reader(encoded.as_slice()).unwrap();

    let changes = reconcile_complete_universe(
        &prior,
        CompleteSourceSet::new(vec![current_source]),
        intent(Some(1)),
        64 * 1024,
    )
    .unwrap();
    assert_eq!(changes.replacements().len(), 1, "{case}");
    assert!(changes.tombstones().is_empty(), "{case}");
    let (delta, _) = changes.apply_to_batch(batch(2, Some(1), case)).unwrap();
    assert_eq!(delta.replace_scopes().len(), 1, "{case}");
    assert_eq!(
        delta.replace_scopes()[0].coverage,
        expected_coverage,
        "{case}"
    );
}

#[test]
fn public_reconcile_replaces_coverage_status_without_unit_changes() {
    for (before, after, case) in [
        (
            SymbolCoverage::NotRequested,
            SymbolCoverage::Complete { symbol_count: 0 },
            "not-requested-to-complete",
        ),
        (
            SymbolCoverage::NotRequested,
            SymbolCoverage::ParseFailed,
            "not-requested-to-parse-failed",
        ),
        (
            SymbolCoverage::ParseFailed,
            SymbolCoverage::Complete { symbol_count: 0 },
            "parse-failed-to-complete",
        ),
    ] {
        assert_empty_coverage_transition_replaces(
            empty_coverage_source("text", before),
            empty_coverage_source("text", after),
            case,
        );
    }
}

#[test]
fn public_reconcile_replaces_empty_source_language_change() {
    assert_empty_coverage_transition_replaces(
        empty_coverage_source("text", SymbolCoverage::NotRequested),
        empty_coverage_source("markdown", SymbolCoverage::NotRequested),
        "text-to-markdown",
    );
}

fn intent(base: Option<u64>) -> ReconcileIntent {
    ReconcileIntent::new(
        RepoId::new("target").unwrap(),
        RevisionId::new("target-r1").unwrap(),
        base.map(ManifestGeneration::new),
    )
}

fn batch(generation: u64, base: Option<u64>, event_id: &str) -> SearchCorpusBatch {
    let repo = RepoId::new("target").unwrap();
    let revision = RevisionId::new("target-r1").unwrap();
    let batch = match base {
        Some(base) => SearchCorpusBatch::delta(
            repo,
            revision,
            ManifestGeneration::new(generation),
            ManifestGeneration::new(base),
            format!("manifest:{event_id}"),
        ),
        None => SearchCorpusBatch::replace_generation(
            repo,
            revision,
            ManifestGeneration::new(generation),
            format!("manifest:{event_id}"),
        ),
    };
    batch.source_event(SourcePublicationEvent {
        stream_id: "preparation-public-test".into(),
        event_id: event_id.into(),
        expected_base_event_id: None,
        payload_sha256: [0; 32],
    })
}

#[test]
fn public_preparation_reconciles_complete_universe_without_private_sdk_access() {
    assert!(PlainTextAdapter.capabilities().lexical());
    assert!(!MarkdownAdapter.capabilities().typed_semantic());
    assert!(CustomTextAdapter.capabilities().lexical());

    let empty = PriorSourceManifest::new(Vec::new()).unwrap();
    let first = reconcile_complete_universe(
        &empty,
        CompleteSourceSet::new(vec![
            plain("move", "old.txt", b"old"),
            markdown("keep", "README.md", b"# keep\n"),
            plain("delete", "delete.txt", b"gone"),
            custom("custom", "meta.cfg", b"value"),
        ]),
        intent(None),
        64 * 1024,
    )
    .unwrap();
    assert_eq!(first.replacements().len(), 4);
    let (initial, prior) = first.apply_to_batch(batch(1, None, "initial")).unwrap();
    assert_eq!(initial.replace_scopes().len(), 4);

    let mut encoded = Vec::new();
    ciborium::into_writer(&prior, &mut encoded).unwrap();
    let prior: PriorSourceManifest = ciborium::from_reader(encoded.as_slice()).unwrap();
    assert_eq!(prior.entries().len(), 4);

    assert!(matches!(
        reconcile_complete_universe(
            &prior,
            CompleteSourceSet::new(vec![plain("move", "old.txt", b"old")]),
            intent(None),
            64 * 1024,
        ),
        Err(PreparationError::ManifestConflict)
    ));

    let no_op = reconcile_complete_universe(
        &prior,
        CompleteSourceSet::new(vec![
            plain("move", "old.txt", b"old"),
            markdown("keep", "README.md", b"# keep\n"),
            plain("delete", "delete.txt", b"gone"),
            custom("custom", "meta.cfg", b"value"),
        ]),
        intent(Some(1)),
        64 * 1024,
    )
    .unwrap();
    assert!(no_op.replacements().is_empty());
    assert!(no_op.tombstones().is_empty());
    let (no_op_batch, _) = no_op.apply_to_batch(batch(2, Some(1), "no-op")).unwrap();
    assert!(no_op_batch.replace_scopes().is_empty());

    let changed = reconcile_complete_universe(
        &prior,
        CompleteSourceSet::new(vec![
            plain("move", "new.txt", b"old"),
            markdown("keep", "README.md", b"# keep\n"),
            custom("custom", "meta.cfg", b"value"),
        ]),
        intent(Some(1)),
        64 * 1024,
    )
    .unwrap();
    assert_eq!(changed.replacements().len(), 1);
    assert_eq!(changed.tombstones().len(), 2);
    let (delta, _) = changed
        .apply_to_batch(batch(3, Some(1), "move-delete"))
        .unwrap();
    assert_eq!(delta.replace_scopes().len(), 1);
    assert_eq!(delta.tombstone_scopes().len(), 2);
}
