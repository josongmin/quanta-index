#![expect(
    clippy::indexing_slicing,
    reason = "fixed independent fixtures and exact observable-shape assertions in owner tests"
)]

use super::*;
use quanta_index_contract::lex::{SymbolKindCode, SymbolRelationship, SymbolSpan};
use quanta_index_contract::{
    CapabilityStatusV1, ClusterMembershipReplaceV1, ManifestGeneration, OwnerDocKind, RepoId,
    RevisionId, SemanticCorpusKindV1, SemanticSourceRecordV1, SourcePublicationEvent, SourceRoleV1,
    SymbolId,
};

fn budgets(chunk: usize) -> PreparationBudgets {
    PreparationBudgets::new(1024, 1024, chunk, 16, 4096).unwrap()
}

fn context(path: &str, bytes: &[u8], recipe: &str, chunk: usize) -> SourceContext {
    assert_eq!(recipe, "v1", "builtin recipe is pinned");
    SourceContext::new(
        format!(
            "path:{}",
            batch_digest_token_v1(&Sha256::digest(path.as_bytes()).into())
        ),
        SourceFileRevision {
            file: SourceFileKey {
                source_repo_id: RepoId::new("source").expect("valid repo"),
                repo_relative_path: RepoRelativePath::new(path),
            },
            revision_id: RevisionId::new("source-r1").expect("valid revision"),
            source_sha256: Sha256::digest(bytes).into(),
        },
        PlainTextAdapter::profile(budgets(chunk)).unwrap(),
    )
    .unwrap()
}

fn intent(base: Option<u64>) -> ReconcileIntent {
    ReconcileIntent::new(
        RepoId::new("target").unwrap(),
        RevisionId::new("target-r1").unwrap(),
        base.map(ManifestGeneration::new),
    )
}

fn prepare(path: &str, bytes: &[u8], recipe: &str, chunk: usize) -> PreparedSource {
    let stable_key = format!(
        "path:{}",
        batch_digest_token_v1(&Sha256::digest(path.as_bytes()).into())
    );
    prepare_with_stable(&stable_key, path, bytes, recipe, chunk)
}

fn prepare_with_stable(
    stable_key: &str,
    path: &str,
    bytes: &[u8],
    recipe: &str,
    chunk: usize,
) -> PreparedSource {
    let mut context = context(path, bytes, recipe, chunk);
    context.stable_key = stable_key.into();
    PlainTextAdapter
        .prepare(TextSource { context, bytes })
        .expect("valid text")
}

fn custom_recipe(path: &str, bytes: &[u8], recipe: &str) -> PreparedSource {
    let base = prepare(path, bytes, "v1", 8);
    let mut context = base.context.clone();
    context.profile = PreparationProfile::new("custom:fixture-text", recipe, budgets(8)).unwrap();
    PreparedSource::from_lexical(
        context,
        bytes.to_vec(),
        LanguageCode::new("text").unwrap(),
        base.chunks().to_vec(),
        Vec::new(),
        SymbolCoverage::NotRequested,
        Vec::new(),
    )
    .unwrap()
}

#[test]
fn unicode_spans_preserve_exact_source_bytes_and_lines() {
    let source = prepare("docs/한글.txt", "é\n中".as_bytes(), "v1", 4);
    assert_eq!(source.bytes(), "é\n中".as_bytes());
    let chunks = source.chunks();
    assert_eq!(chunks.len(), 2);
    assert_eq!(
        (
            chunks[0].start_byte,
            chunks[0].end_byte,
            chunks[0].start_line,
            chunks[0].end_line
        ),
        (0, 3, 1, 2)
    );
    assert_eq!(
        (
            chunks[1].start_byte,
            chunks[1].end_byte,
            chunks[1].start_line,
            chunks[1].end_line
        ),
        (3, 6, 2, 2)
    );
    assert_eq!(chunks[0].text.as_ref(), "é\n");
    assert_eq!(chunks[1].text.as_ref(), "中");
    assert_eq!(
        chunks[0].chunk_id.as_str(),
        "prep:2c87dc8aa3ad5978baa24beb8af49a54a1e984f2d47d17c14b311cd893b7cd95"
    );
    assert_ne!(chunks[0].chunk_id, chunks[1].chunk_id);
    assert_eq!(
        source.chunks()[0].chunk_id,
        prepare("docs/한글.txt", "é\n中".as_bytes(), "v1", 4).chunks()[0].chunk_id
    );
    assert_ne!(
        source.chunks()[0].chunk_id,
        prepare("docs/other.txt", "é\n中".as_bytes(), "v1", 4).chunks()[0].chunk_id
    );
}

#[test]
fn empty_source_is_explicit_coverage_without_fabricated_units() {
    let source = prepare("empty.txt", b"", "v1", 8);
    assert!(source.chunks().is_empty());
    assert!(source.semantic().is_empty());
    assert!(source.coverage().text_admitted);
    assert_eq!(source.coverage().symbols, SymbolCoverage::NotRequested);
}

#[test]
fn invalid_utf8_digest_and_oversize_are_fail_closed() {
    assert_eq!(
        PlainTextAdapter
            .prepare(TextSource {
                context: context("bad.txt", &[0xff], "v1", 8),
                bytes: &[0xff],
            })
            .unwrap_err(),
        PreparationError::InvalidUtf8
    );
    let mut wrong = context("bad.txt", b"ok", "v1", 8);
    wrong.source.source_sha256 = [0; 32];
    assert_eq!(
        PlainTextAdapter
            .prepare(TextSource {
                context: wrong,
                bytes: b"ok"
            })
            .unwrap_err(),
        PreparationError::SourceDigestMismatch
    );
    assert_eq!(
        PlainTextAdapter
            .prepare(TextSource {
                context: context("long.txt", b"longline", "v1", 4),
                bytes: b"longline",
            })
            .unwrap_err(),
        PreparationError::LimitExceeded("line bytes")
    );
    let mut tight = context("many.txt", b"a\nb\nc", "v1", 2);
    tight.profile =
        PlainTextAdapter::profile(tight.profile.budgets().with_max_chunks(2).unwrap()).unwrap();
    assert_eq!(
        PlainTextAdapter
            .prepare(TextSource {
                context: tight,
                bytes: b"a\nb\nc"
            })
            .unwrap_err(),
        PreparationError::LimitExceeded("chunk count")
    );
}

#[test]
fn markdown_is_source_preserving_and_lexical_only() {
    let bytes = b"# Heading\n**bold**";
    let mut ctx = context("README.md", bytes, "v1", 32);
    ctx.profile = MarkdownAdapter::profile(budgets(32)).unwrap();
    let adapter = MarkdownAdapter;
    assert_eq!(adapter.capabilities(), lexical_only());
    let source = adapter
        .prepare(TextSource {
            context: ctx,
            bytes,
        })
        .unwrap();
    assert_eq!(source.chunks()[0].text.as_ref(), "# Heading\n**bold**");
    assert_eq!(source.chunks()[0].language.as_str(), "markdown");
    assert!(source.semantic().is_empty());
}

#[test]
fn builtin_recipe_revision_is_pinned() {
    let bytes = b"plain";
    let mut ctx = context("plain.txt", bytes, "v1", 16);
    ctx.profile =
        PreparationProfile::new(PlainTextAdapter::ADAPTER_ID, "caller-spoofed", budgets(16))
            .unwrap();
    assert_eq!(
        PlainTextAdapter
            .prepare(TextSource::new(ctx, bytes))
            .unwrap_err(),
        PreparationError::AdapterMismatch
    );
}

#[test]
fn full_reconciliation_handles_move_delete_and_recipe_change() {
    let old = prepare("old.txt", b"old", "v1", 8);
    let gone = prepare("gone.txt", b"gone", "v1", 8);
    let same = prepare("same.txt", b"same", "v1", 8);
    let recipe_old = custom_recipe("recipe.txt", b"same", "v1");
    let prior = PriorSourceManifest::new(
        [&old, &gone, &same, &recipe_old]
            .map(|source| {
                PriorSourceEntry::new(
                    source.context.stable_key.clone(),
                    source.coverage().clone(),
                    Vec::new(),
                )
                .unwrap()
            })
            .into(),
    )
    .unwrap();
    let moved = prepare_with_stable(&old.context.stable_key, "new.txt", b"old", "v1", 8);
    let changes = reconcile_complete_universe(
        &prior,
        CompleteSourceSet::new(vec![
            moved,
            prepare("same.txt", b"same", "v1", 8),
            custom_recipe("recipe.txt", b"same", "v2"),
        ]),
        intent(Some(1)),
        4096,
    )
    .unwrap();
    assert_eq!(
        changes
            .replacements()
            .iter()
            .map(|source| source.source().file.repo_relative_path.as_str())
            .collect::<Vec<_>>(),
        vec!["new.txt", "recipe.txt"]
    );
    assert_eq!(
        changes
            .tombstones()
            .iter()
            .map(|file| file.repo_relative_path.as_str())
            .collect::<Vec<_>>(),
        vec!["gone.txt", "old.txt"]
    );
    let batch = SearchCorpusBatch::delta(
        RepoId::new("target").unwrap(),
        RevisionId::new("target-r1").unwrap(),
        ManifestGeneration::new(2),
        ManifestGeneration::new(1),
        "manifest:v2",
    )
    .source_event(SourcePublicationEvent {
        stream_id: "preparation-tests".into(),
        event_id: "delta-2".into(),
        expected_base_event_id: None,
        payload_sha256: [0; 32],
    });
    let (batch, planned) = changes.apply_to_batch(batch).unwrap();
    assert_eq!(batch.replace_scopes().len(), 2);
    assert_eq!(batch.tombstone_scopes().len(), 2);
    assert_eq!(planned.entries().len(), 3);
}

#[test]
fn duplicate_keys_and_path_owner_conflicts_are_rejected() {
    let one = prepare("same.txt", b"x", "v1", 8);
    let two = prepare("same.txt", b"x", "v1", 8);
    let empty = PriorSourceManifest::new(Vec::new()).unwrap();
    assert!(matches!(
        reconcile_complete_universe(
            &empty,
            CompleteSourceSet::new(vec![one.clone(), two]),
            intent(None),
            4096
        ),
        Err(PreparationError::DuplicateSource)
    ));
    let mut other = prepare("same.txt", b"y", "v1", 8);
    other.context.stable_key = "other".into();
    other.context.source.file.source_repo_id = RepoId::new("other").unwrap();
    assert!(matches!(
        reconcile_complete_universe(
            &empty,
            CompleteSourceSet::new(vec![one, other]),
            intent(None),
            4096
        ),
        Err(PreparationError::PathOwnershipConflict)
    ));
}

#[test]
fn prior_manifest_serialization_is_sorted_and_rejects_duplicate_semantic_keys() {
    let a = prepare("a.txt", b"a", "v1", 8);
    let b = prepare("b.txt", b"b", "v1", 8);
    let key = SemanticSourceScopeKeyV1 {
        corpus_kind: SemanticCorpusKindV1::DocumentLeaf,
        owner_kind: OwnerDocKind::File,
        owner_id: "file-a".into(),
    };
    let entry = |source: &PreparedSource, semantic_scopes| {
        PriorSourceEntry::new(
            source.context.stable_key.clone(),
            source.coverage().clone(),
            semantic_scopes,
        )
        .unwrap()
    };
    let manifest = PriorSourceManifest::new(vec![
        entry(&b, Vec::new()),
        entry(&a, vec![(key.clone(), [1; 32])]),
    ])
    .unwrap();
    assert!(
        manifest
            .entries()
            .windows(2)
            .all(|pair| pair[0].stable_key < pair[1].stable_key)
    );
    let mut encoded = Vec::new();
    ciborium::into_writer(&manifest, &mut encoded).unwrap();
    let decoded: PriorSourceManifest = ciborium::from_reader(encoded.as_slice()).unwrap();
    assert_eq!(decoded, manifest);
    let decoded_a = decoded
        .entries()
        .iter()
        .find(|entry| entry.stable_key() == a.context.stable_key.as_str())
        .unwrap();
    assert_eq!(decoded_a.coverage(), a.coverage());
    assert_eq!(decoded_a.source(), a.source());
    assert_eq!(decoded_a.profile_sha256(), a.profile_sha256());
    assert_eq!(decoded_a.unit_set_sha256(), a.coverage().unit_set_sha256);
    let reversed: Vec<_> = manifest
        .entries()
        .iter()
        .rev()
        .map(|entry| (&entry.stable_key, &entry.coverage, &entry.semantic_scopes))
        .collect();
    let mut reversed_bytes = Vec::new();
    ciborium::into_writer(&reversed, &mut reversed_bytes).unwrap();
    assert!(ciborium::from_reader::<PriorSourceManifest, _>(reversed_bytes.as_slice()).is_err());
    let legacy_rows = vec![(
        a.context.stable_key.clone(),
        a.source().clone(),
        a.profile_sha256(),
        a.coverage().unit_set_sha256,
        Vec::<(SemanticSourceScopeKeyV1, [u8; 32])>::new(),
    )];
    let mut legacy_bytes = Vec::new();
    ciborium::into_writer(&legacy_rows, &mut legacy_bytes).unwrap();
    assert!(ciborium::from_reader::<PriorSourceManifest, _>(legacy_bytes.as_slice()).is_err());
    assert!(matches!(
        PriorSourceManifest::new(vec![
            entry(&a, vec![(key.clone(), [1; 32])]),
            entry(&b, vec![(key, [2; 32])]),
        ]),
        Err(PreparationError::ManifestConflict)
    ));
}

#[test]
fn deleted_source_keeps_old_semantic_scope_for_tombstone() {
    let old = prepare("old.md", b"old", "v1", 8);
    let semantic = SemanticSourceScopeKeyV1 {
        corpus_kind: SemanticCorpusKindV1::DocumentLeaf,
        owner_kind: OwnerDocKind::File,
        owner_id: "old-doc".into(),
    };
    let prior = PriorSourceManifest::new(vec![
        PriorSourceEntry::new(
            old.context.stable_key.clone(),
            old.coverage().clone(),
            vec![(semantic.clone(), [1; 32])],
        )
        .unwrap(),
    ])
    .unwrap();
    let changes = reconcile_complete_universe(
        &prior,
        CompleteSourceSet::new(Vec::new()),
        intent(Some(1)),
        4096,
    )
    .unwrap();
    assert_eq!(changes.tombstones(), &[old.source().file.clone()]);
    assert_eq!(changes.semantic_tombstones(), &[semantic]);
}

#[test]
fn custom_constructor_rejects_unbacked_cluster_membership_and_cbor_oversize() {
    let context = context("a.txt", b"a", "v1", 8);
    let cluster = SemanticSourceReplaceScopeV1 {
        scope: SemanticSourceScopeKeyV1 {
            corpus_kind: SemanticCorpusKindV1::ClusterCard,
            owner_kind: OwnerDocKind::File,
            owner_id: "cluster".into(),
        },
        scope_digest: "digest".into(),
        sources: Vec::new(),
        cluster_memberships: Vec::new(),
    };
    assert!(matches!(
        PreparedSource::from_lexical(
            context.clone(),
            b"a".to_vec(),
            LanguageCode::new("text").unwrap(),
            Vec::new(),
            Vec::new(),
            SymbolCoverage::NotRequested,
            vec![cluster],
        ),
        Err(PreparationError::InvalidContribution(_))
    ));
    let tiny = PreparationBudgets::new(1024, 16, 8, 16, 16).unwrap();
    let mut context = context;
    context.profile = PlainTextAdapter::profile(tiny).unwrap();
    assert_eq!(
        PreparedSource::from_lexical(
            context,
            b"a".to_vec(),
            LanguageCode::new("text").unwrap(),
            Vec::new(),
            Vec::new(),
            SymbolCoverage::NotRequested,
            Vec::new(),
        )
        .unwrap_err(),
        PreparationError::LimitExceeded("batch bytes")
    );
}

fn semantic_source(text: &str) -> PreparedSource {
    semantic_source_with_budget(text, "auth:1", budgets(16))
}

fn semantic_source_with_budget(
    text: &str,
    authority_digest: &str,
    budget: PreparationBudgets,
) -> PreparedSource {
    try_semantic_source_with_budget(text, authority_digest, budget).unwrap()
}

fn try_semantic_source_with_budget(
    text: &str,
    authority_digest: &str,
    budget: PreparationBudgets,
) -> Result<PreparedSource, PreparationError> {
    let bytes = b"source";
    let mut context = context("src/a.txt", bytes, "v1", 16);
    context.profile = PlainTextAdapter::profile(budget).unwrap();
    let scope = SemanticSourceScopeKeyV1 {
        corpus_kind: SemanticCorpusKindV1::SymbolCard,
        owner_kind: OwnerDocKind::Symbol,
        owner_id: "symbol-1".into(),
    };
    let record = SemanticSourceRecordV1 {
        record_id: "record-1".into(),
        corpus_kind: scope.corpus_kind,
        owner_kind: scope.owner_kind,
        owner_id: scope.owner_id.clone(),
        source_doc_id: "doc-1".into(),
        parent_owner_id: None,
        repo_relative_path: context.source.file.repo_relative_path.clone(),
        language: Some("text".into()),
        package: None,
        symbol_kind: Some("function".into()),
        visibility: None,
        source_role: SourceRoleV1::CardText,
        generated: false,
        capability_status: CapabilityStatusV1::Full,
        raw_fallback_reason: None,
        authority_digest: authority_digest.into(),
        render_policy_digest: "render:1".into(),
        card_schema_version: 1,
        text: text.into(),
    };
    PreparedSource::from_lexical(
        context,
        bytes.to_vec(),
        LanguageCode::new("text").unwrap(),
        Vec::new(),
        Vec::new(),
        SymbolCoverage::NotRequested,
        vec![SemanticSourceReplaceScopeV1 {
            scope,
            scope_digest: "producer-scope-v1".into(),
            sources: vec![record],
            cluster_memberships: Vec::new(),
        }],
    )
}

fn cluster_source(reversed: bool, changed_member: bool, bytes: &[u8]) -> PreparedSource {
    let source = semantic_source("card");
    let mut context = source.context.clone();
    context.profile = PlainTextAdapter::profile(budgets(16)).unwrap();
    context.source.source_sha256 = Sha256::digest(bytes).into();
    let scope = SemanticSourceScopeKeyV1 {
        corpus_kind: SemanticCorpusKindV1::ClusterCard,
        owner_kind: OwnerDocKind::Module,
        owner_id: "cluster".into(),
    };
    let mut records = Vec::new();
    let mut memberships = Vec::new();
    for id in ["a", "b"] {
        let mut record = source.semantic()[0].sources[0].clone();
        record.record_id = format!("cluster-{id}");
        record.corpus_kind = scope.corpus_kind;
        record.owner_kind = scope.owner_kind;
        record.owner_id = scope.owner_id.clone();
        record.authority_digest = format!("auth:{id}");
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
        records.push(record);
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
            scope_digest: "cluster:v1".into(),
            sources: records,
            cluster_memberships: memberships,
        }],
    )
    .unwrap()
}

#[test]
fn cluster_membership_order_matches_wire_and_prior_stamp() {
    let forward = cluster_source(false, false, b"source");
    let reverse = cluster_source(true, false, b"source");
    assert_eq!(forward.semantic(), reverse.semantic());
    assert_eq!(
        forward.semantic()[0].cluster_memberships[0].cluster_record_id,
        "cluster-a"
    );
    let empty = PriorSourceManifest::new(Vec::new()).unwrap();
    let first = reconcile_complete_universe(
        &empty,
        CompleteSourceSet::new(vec![forward]),
        intent(None),
        4096,
    )
    .unwrap();
    let (wire, prior) = first
        .apply_to_batch(
            SearchCorpusBatch::replace_generation(
                RepoId::new("target").unwrap(),
                RevisionId::new("target-r1").unwrap(),
                ManifestGeneration::new(1),
                "cluster",
            )
            .source_event(SourcePublicationEvent {
                stream_id: "cluster-stream".into(),
                event_id: "cluster-event".into(),
                expected_base_event_id: None,
                payload_sha256: [1; 32],
            }),
        )
        .unwrap();
    assert_eq!(
        wire.semantic_replace_scopes()[0].cluster_memberships[0].cluster_record_id,
        "cluster-a"
    );
    let mut encoded = Vec::new();
    ciborium::into_writer(&prior, &mut encoded).unwrap();
    let prior: PriorSourceManifest = ciborium::from_reader(encoded.as_slice()).unwrap();
    let no_op = reconcile_complete_universe(
        &prior,
        CompleteSourceSet::new(vec![reverse]),
        intent(Some(1)),
        4096,
    )
    .unwrap();
    assert_eq!(
        no_op.next_manifest().entries()[0].semantic_scopes(),
        prior.entries()[0].semantic_scopes()
    );
    assert!(no_op.replacements().is_empty());
    let changed = reconcile_complete_universe(
        &prior,
        CompleteSourceSet::new(vec![cluster_source(true, true, b"source")]),
        intent(Some(1)),
        4096,
    )
    .unwrap();
    assert_eq!(changed.replacements().len(), 1);
    assert_ne!(
        changed.next_manifest().entries()[0].semantic_scopes(),
        prior.entries()[0].semantic_scopes()
    );
    let source_changed = reconcile_complete_universe(
        &prior,
        CompleteSourceSet::new(vec![cluster_source(true, false, b"source changed")]),
        intent(Some(1)),
        4096,
    )
    .unwrap();
    assert_eq!(source_changed.replacements().len(), 1);
    assert_eq!(
        source_changed.next_manifest().entries()[0].semantic_scopes(),
        prior.entries()[0].semantic_scopes()
    );
}

#[test]
fn emitted_text_budget_is_independent_of_input_and_metadata() {
    let budget = PreparationBudgets::new(6, 4, 1, 16, 4096).unwrap();
    assert_eq!(budget.max_emitted_text_bytes(), 4);
    assert_eq!(
        budget
            .with_max_emitted_text_bytes(3)
            .unwrap()
            .max_emitted_text_bytes(),
        3
    );
    let metadata = "x".repeat(256);
    let accepted = semantic_source_with_budget("card", &metadata, budget);
    assert_eq!(accepted.bytes().len(), 6);
    assert_eq!(accepted.semantic()[0].sources[0].text.len(), 4);
    assert!(accepted.measured_batch_bytes > budget.max_emitted_text_bytes());
    assert_eq!(
        try_semantic_source_with_budget(
            "card",
            &metadata,
            budget.with_max_emitted_text_bytes(3).unwrap()
        ),
        Err(PreparationError::LimitExceeded("emitted text bytes"))
    );
    let exact = budget
        .with_max_batch_bytes(accepted.measured_batch_bytes)
        .unwrap();
    assert!(try_semantic_source_with_budget("card", &metadata, exact).is_ok());
    let short = budget
        .with_max_batch_bytes(accepted.measured_batch_bytes - 1)
        .unwrap();
    assert_eq!(
        try_semantic_source_with_budget("card", &metadata, short),
        Err(PreparationError::LimitExceeded("batch bytes"))
    );
    let mut oversized_input = context("src/a.txt", b"source!", "v1", 16);
    oversized_input.profile = PlainTextAdapter::profile(budget).unwrap();
    assert_eq!(
        PreparedSource::from_lexical(
            oversized_input,
            b"source!".to_vec(),
            LanguageCode::new("text").unwrap(),
            Vec::new(),
            Vec::new(),
            SymbolCoverage::NotRequested,
            Vec::new(),
        ),
        Err(PreparationError::LimitExceeded("input bytes"))
    );
}

#[test]
fn emitted_text_limit_counts_chunks_and_semantic_text_together() {
    let bytes = b"ab";
    let lexical = prepare("src/a.txt", bytes, "v1", 2);
    let mut context = lexical.context.clone();
    let budget = PreparationBudgets::new(2, 4, 2, 16, 4096).unwrap();
    context.profile = PlainTextAdapter::profile(budget).unwrap();
    let mut semantic = semantic_source("cd").semantic;
    let prepared = PreparedSource::from_lexical(
        context.clone(),
        bytes.to_vec(),
        LanguageCode::new("text").unwrap(),
        lexical.chunks.clone(),
        Vec::new(),
        SymbolCoverage::NotRequested,
        semantic.clone(),
    )
    .unwrap();
    assert_eq!(
        prepared.chunks()[0].text.len() + prepared.semantic()[0].sources[0].text.len(),
        4
    );
    semantic[0].sources[0].text = "cde".into();
    assert_eq!(
        PreparedSource::from_lexical(
            context,
            bytes.to_vec(),
            LanguageCode::new("text").unwrap(),
            lexical.chunks,
            Vec::new(),
            SymbolCoverage::NotRequested,
            semantic,
        ),
        Err(PreparationError::LimitExceeded("emitted text bytes"))
    );
}

#[test]
fn symbol_metadata_uses_batch_budget_not_emitted_text_budget() {
    let bytes = b"x";
    let mut context = context("src/symbol.txt", bytes, "v1", 1);
    context.profile =
        PlainTextAdapter::profile(PreparationBudgets::new(1, 1, 1, 16, 4096).unwrap()).unwrap();
    let symbol = SymbolRecord {
        symbol_id: SymbolId::new("symbol:x"),
        repo_relative_path: context.source.file.repo_relative_path.clone(),
        language: LanguageCode::new("text").unwrap(),
        symbol_kind: SymbolKindCode::new("function").unwrap(),
        symbol_kind_family: None,
        local_name: "x".into(),
        qualified_name: "fixture::x".into(),
        signature: Some("s".repeat(256).into_boxed_str()),
        visibility: None,
        definition_span: SymbolSpan {
            path: "src/symbol.txt".into(),
            byte_start: 0,
            byte_end: 1,
            line_start: 1,
            line_end: 1,
        },
        container_qualified_name: None,
        relationship: SymbolRelationship::Def,
    };
    let prepared = PreparedSource::from_lexical(
        context,
        bytes.to_vec(),
        LanguageCode::new("text").unwrap(),
        Vec::new(),
        vec![symbol],
        SymbolCoverage::Complete { symbol_count: 1 },
        Vec::new(),
    )
    .unwrap();
    assert_eq!(prepared.bytes(), bytes);
    assert!(prepared.measured_batch_bytes > 1);
}

#[test]
fn changed_typed_semantic_content_replaces_same_source_and_recipe() {
    let first = semantic_source("first card");
    let empty = PriorSourceManifest::new(Vec::new()).unwrap();
    let initial = reconcile_complete_universe(
        &empty,
        CompleteSourceSet::new(vec![first]),
        intent(None),
        4096,
    )
    .unwrap();
    let prior = initial.next_manifest().clone();
    let changed = reconcile_complete_universe(
        &prior,
        CompleteSourceSet::new(vec![semantic_source("second card")]),
        intent(Some(1)),
        4096,
    )
    .unwrap();
    assert_eq!(changed.replacements().len(), 1);
    assert!(changed.tombstones().is_empty());
    assert!(changed.semantic_tombstones().is_empty());
}
