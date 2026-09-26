//! QI-BB-021 follow-up #2 — the scope-streamed semantic build contract:
//! the window policy, the resident source that windows an already-resident
//! batch by owner scope, the residency leases every window carries, and the
//! resident build wrapper's tally cross-check.
//!
//! Every expected number is arithmetic the test does itself from the rows
//! it built (`rows × dimension × 4`) and from how it laid the owners out;
//! nothing is read back from the accounting under test and compared with
//! itself.

#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, CapabilityStatusV1, ClusterMembershipReplaceV1, EmbeddingDistanceMetric,
    EmbeddingId, EmbeddingModelContract, EmbeddingNormalization, EmbeddingRecord,
    ManifestGeneration, OwnerDocKind, RepoId, RepoRelativePath, RevisionId, SearchScopeKey,
    SearchScopeSurface, SemanticCorpusKindV1, SemanticIngestBatch, SemanticReplaceScope,
    SourceRoleV1, SymbolId,
};
use quanta_index_core::{
    CoreError, ResidentScopeSource, SEMANTIC_STREAM_OWNER_SCOPE_OVER_WINDOW_CODE,
    SEMANTIC_STREAM_WINDOW_EXCEEDED_CODE, SEMANTIC_STREAM_WINDOW_SCOPES,
    SEMANTIC_STREAM_WINDOW_STILL_RESIDENT_CODE, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES,
    SemanticIngestHeaderV1, SemanticScopeSource, SemanticScopeStreamBuildPort,
    SemanticScopeWindowV1, SemanticStreamTallyV1, SemanticStreamWindowPolicy, SemanticWindowFillV1,
    SemanticWindowPlacementV1, SemanticWindowResidencyV1, build_resident_semantic_batch_v1,
    owner_key_v1,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const DIMENSION: usize = 4;
/// Bytes one row's vector occupies at [`DIMENSION`].
const ROW_BYTES: u64 = 16;

fn record(
    id: &str,
    path: &str,
    owner_kind: OwnerDocKind,
    owner_id: &str,
    corpus_kind: SemanticCorpusKindV1,
) -> Result<EmbeddingRecord, Box<dyn std::error::Error>> {
    Ok(EmbeddingRecord {
        embedding_id: EmbeddingId::new(id),
        record_id: format!("record-{id}").into_boxed_str(),
        owner_kind,
        owner_id: owner_id.to_string().into_boxed_str(),
        corpus_kind,
        parent_owner_id: None,
        source_doc_id: format!("doc-{id}").into_boxed_str(),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new("rust")
            .map_err(|err| -> Box<dyn std::error::Error> { format!("language: {err}").into() })?,
        package: None,
        symbol_kind: None,
        visibility: None,
        source_role: SourceRoleV1::CardText,
        generated: false,
        capability_status: CapabilityStatusV1::Full,
        authority_digest: format!("auth:{id}").into_boxed_str(),
        render_policy_digest: format!("render:{id}").into_boxed_str(),
        card_schema_version: 1,
        start_byte: 0,
        end_byte: 1,
        start_line: 1,
        end_line: 1,
        snippet: id.to_string().into_boxed_str(),
        embedding_input_digest: format!("input:{id}").into_boxed_str(),
        vector_digest: format!("vector:{id}").into_boxed_str(),
        view_kind: "symbol.card".to_string().into_boxed_str(),
        vector: vec![1.0, 0.0, 0.0, 0.0],
    })
}

/// A chunk row: its own owner, as the legacy derivation emits them.
fn chunk(id: &str, path: &str) -> Result<EmbeddingRecord, Box<dyn std::error::Error>> {
    record(
        id,
        path,
        OwnerDocKind::Chunk,
        id,
        SemanticCorpusKindV1::RawCodeFallback,
    )
}

fn scope(path: &str, embeddings: Vec<EmbeddingRecord>) -> SemanticReplaceScope {
    SemanticReplaceScope {
        scope: SearchScopeKey {
            doc_surface: SearchScopeSurface::Chunk,
            repo_relative_path: RepoRelativePath::new(path),
        },
        scope_digest: format!("scope:{path}"),
        embeddings,
        cluster_memberships: Vec::new(),
    }
}

fn model_contract() -> EmbeddingModelContract {
    EmbeddingModelContract {
        model_id: "test-model".to_string().into_boxed_str(),
        model_version: Some("1".to_string().into_boxed_str()),
        dimension: 4,
        normalization: EmbeddingNormalization::L2Unit,
        distance_metric: EmbeddingDistanceMetric::Cosine,
        policy_digest: "policy:test".to_string().into_boxed_str(),
        view_policy_digest: None,
    }
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn batch(replace_scopes: Vec<SemanticReplaceScope>) -> SemanticIngestBatch {
    SemanticIngestBatch {
        repo_id: RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
        generation: ManifestGeneration::new(3),
        base_generation: None,
        manifest_digest: "manifest:3".to_string(),
        batch_digest: "batch:3".to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        model_contract: model_contract(),
        required_corpora: vec![SemanticCorpusKindV1::SymbolCard],
        corpus_policy_digest: Some("semantic-source.v1".to_string()),
        clear_surfaces: Vec::new(),
        replace_scopes,
        tombstone_scopes: Vec::new(),
        seal: true,
    }
}

fn ids(window: &SemanticScopeWindowV1) -> Vec<String> {
    window
        .scopes()
        .iter()
        .flat_map(|scope| {
            scope
                .embeddings
                .iter()
                .map(|record| record.embedding_id.as_str().to_string())
        })
        .collect()
}

fn typed_code(
    result: &Result<Option<SemanticScopeWindowV1>, CoreError>,
) -> Option<quanta_index_contract::SearchPlaneErrorCodeV2> {
    match result {
        Err(CoreError::Typed { code, .. }) => Some(*code),
        Err(
            CoreError::InvalidContract(_)
            | CoreError::NotReady(_)
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_)
            | CoreError::Storage(_),
        )
        | Ok(_) => None,
    }
}

#[test]
fn the_policy_defaults_to_the_named_constants_and_refuses_zero() -> TestResult {
    let policy = SemanticStreamWindowPolicy::DEFAULT;
    if policy.max_owner_scopes() != SEMANTIC_STREAM_WINDOW_SCOPES
        || policy.max_vector_bytes() != SEMANTIC_STREAM_WINDOW_VECTOR_BYTES
    {
        return Err("the default window is the two named constants".into());
    }
    if SemanticStreamWindowPolicy::new(0, 1).is_ok()
        || SemanticStreamWindowPolicy::new(1, 0).is_ok()
    {
        return Err("a zero ceiling is a configuration defect".into());
    }
    if SemanticStreamWindowPolicy::vector_bytes(3, DIMENSION)? != ROW_BYTES.saturating_mul(3) {
        return Err("vector bytes are rows × dimension × 4".into());
    }
    if SemanticStreamWindowPolicy::vector_bytes(usize::MAX, 2).is_ok() {
        return Err("vector bytes overflow is refused".into());
    }
    Ok(())
}

#[test]
fn placement_joins_within_both_ceilings_and_opens_the_next_window_past_either() -> TestResult {
    let policy = SemanticStreamWindowPolicy::new(2, ROW_BYTES.saturating_mul(3))?;
    let empty = SemanticWindowFillV1::default();
    if policy.place(empty, ROW_BYTES.saturating_mul(3))? != SemanticWindowPlacementV1::Joins {
        return Err("the first owner always opens the window".into());
    }
    let one_owner_one_row = SemanticWindowFillV1 {
        owner_scopes: 1,
        vector_bytes: ROW_BYTES,
    };
    if policy.place(one_owner_one_row, ROW_BYTES.saturating_mul(2))?
        != SemanticWindowPlacementV1::Joins
    {
        return Err("a second owner joins while both ceilings hold".into());
    }
    if policy.place(one_owner_one_row, ROW_BYTES.saturating_mul(3))?
        != SemanticWindowPlacementV1::OpensNext
    {
        return Err("an owner that would exceed the byte ceiling opens the next window".into());
    }
    let two_owners = SemanticWindowFillV1 {
        owner_scopes: 2,
        vector_bytes: ROW_BYTES,
    };
    if policy.place(two_owners, ROW_BYTES)? != SemanticWindowPlacementV1::OpensNext {
        return Err("an owner past the scope ceiling opens the next window".into());
    }
    match policy.place(empty, ROW_BYTES.saturating_mul(4)) {
        Err(CoreError::Typed { code, .. })
            if code == SEMANTIC_STREAM_OWNER_SCOPE_OVER_WINDOW_CODE => {}
        other => {
            return Err(format!(
                "an owner over the byte ceiling by itself is refused typed, got {other:?}"
            )
            .into());
        }
    }
    Ok(())
}

// CASE-COVERS: a resident batch of legacy chunk rows (one owner each)
// streams under a two-owner window as ceil(rows / 2) windows; a path whose
// rows span windows is issued as one replace scope per window; the rows
// come out in producer order; the residency never had two windows out.
#[test]
fn resident_scopes_stream_by_owner_under_the_scope_ceiling() -> TestResult {
    let scopes = vec![
        scope(
            "a.rs",
            vec![
                chunk("a-1", "a.rs")?,
                chunk("a-2", "a.rs")?,
                chunk("a-3", "a.rs")?,
            ],
        ),
        scope("b.rs", vec![chunk("b-1", "b.rs")?]),
        scope("c.rs", vec![chunk("c-1", "c.rs")?]),
    ];
    let policy = SemanticStreamWindowPolicy::new(2, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES)?;
    let mut source = ResidentScopeSource::new(&scopes, policy)?;
    let residency = Arc::clone(source.residency());
    let mut seen: Vec<(Vec<String>, Vec<String>)> = Vec::new();
    while let Some(window) = source.next_window()? {
        if residency.outstanding_windows() != 1 || residency.outstanding_vector_bytes() == 0 {
            return Err("one window is out while it is held".into());
        }
        seen.push((
            window
                .scopes()
                .iter()
                .map(|scope| scope.scope.repo_relative_path.as_str().to_string())
                .collect(),
            ids(&window),
        ));
        drop(window);
        if residency.outstanding_windows() != 0 || residency.outstanding_vector_bytes() != 0 {
            return Err("a dropped window releases its lease".into());
        }
    }
    let expected: Vec<(Vec<String>, Vec<String>)> = vec![
        (vec!["a.rs".into()], vec!["a-1".into(), "a-2".into()]),
        (
            vec!["a.rs".into(), "b.rs".into()],
            vec!["a-3".into(), "b-1".into()],
        ),
        (vec!["c.rs".into()], vec!["c-1".into()]),
    ];
    if seen != expected {
        return Err(format!("windows cut wrong: {seen:?}").into());
    }
    let tally = source.tally();
    let expected_tally = SemanticStreamTallyV1 {
        windows: 3,
        replace_scopes: 4,
        rows: 5,
        peak_vector_bytes: ROW_BYTES.saturating_mul(2),
    };
    if tally != expected_tally {
        return Err(format!("tally {tally:?} != {expected_tally:?}").into());
    }
    if residency.peak_windows() != 1 || residency.peak_vector_bytes() != ROW_BYTES.saturating_mul(2)
    {
        return Err(format!(
            "peak residency is one two-row window: windows={} bytes={}",
            residency.peak_windows(),
            residency.peak_vector_bytes()
        )
        .into());
    }
    Ok(())
}

// CASE-COVERS: the byte ceiling cuts too, an owner with several rows
// travels whole even when the producer interleaved its rows with another
// owner's, its cluster memberships travel with it, and an owner whose rows
// alone exceed the byte ceiling is refused typed.
#[test]
fn owners_travel_whole_with_their_memberships_and_an_oversize_owner_is_refused() -> TestResult {
    let mut cluster_a1 = record(
        "ca-1",
        "src/cluster.rs",
        OwnerDocKind::OwnerMap,
        "cluster-a",
        SemanticCorpusKindV1::ClusterCard,
    )?;
    cluster_a1.authority_digest = "auth:cluster-a".to_string().into_boxed_str();
    let mut cluster_a2 = record(
        "ca-2",
        "src/cluster.rs",
        OwnerDocKind::OwnerMap,
        "cluster-a",
        SemanticCorpusKindV1::ClusterCard,
    )?;
    cluster_a2.authority_digest = "auth:cluster-a".to_string().into_boxed_str();
    let symbol_b = record(
        "sb-1",
        "src/cluster.rs",
        OwnerDocKind::Symbol,
        "symbol-b",
        SemanticCorpusKindV1::SymbolCard,
    )?;
    let membership = |id: &str| ClusterMembershipReplaceV1 {
        cluster_record_id: format!("record-{id}"),
        authority_digest: "auth:cluster-a".to_string(),
        members: vec![SymbolId::new("symbol:x")],
    };
    // Producer order interleaves cluster-a's two rows around symbol-b.
    let mut interleaved = scope("src/cluster.rs", vec![cluster_a1, symbol_b, cluster_a2]);
    interleaved.cluster_memberships = vec![membership("ca-1"), membership("ca-2")];
    let scopes = vec![interleaved];

    // Two rows per window by bytes: cluster-a (2 rows) fills a window
    // alone; symbol-b follows in its own.
    let two_rows = SemanticStreamWindowPolicy::new(
        SEMANTIC_STREAM_WINDOW_SCOPES,
        ROW_BYTES.saturating_mul(2),
    )?;
    let mut source = ResidentScopeSource::new(&scopes, two_rows)?;
    let mut windows: Vec<(Vec<String>, Vec<String>)> = Vec::new();
    while let Some(window) = source.next_window()? {
        let owners: Vec<String> = window
            .scopes()
            .iter()
            .flat_map(|scope| {
                scope.embeddings.iter().map(|record| {
                    let (_corpus, _kind, owner_id) = owner_key_v1(record);
                    owner_id.to_string()
                })
            })
            .collect();
        let memberships: Vec<String> = window
            .scopes()
            .iter()
            .flat_map(|scope| {
                scope
                    .cluster_memberships
                    .iter()
                    .map(|membership| membership.cluster_record_id.clone())
            })
            .collect();
        windows.push((owners, memberships));
        drop(window);
    }
    let expected: Vec<(Vec<String>, Vec<String>)> = vec![
        (
            vec!["cluster-a".into(), "cluster-a".into()],
            vec!["record-ca-1".into(), "record-ca-2".into()],
        ),
        (vec!["symbol-b".into()], Vec::new()),
    ];
    if windows != expected {
        return Err(format!("owner grouping wrong: {windows:?}").into());
    }

    // One row per window by bytes: cluster-a's two rows fit no window.
    let one_row = SemanticStreamWindowPolicy::new(SEMANTIC_STREAM_WINDOW_SCOPES, ROW_BYTES)?;
    let mut source = ResidentScopeSource::new(&scopes, one_row)?;
    if typed_code(&source.next_window()) != Some(SEMANTIC_STREAM_OWNER_SCOPE_OVER_WINDOW_CODE) {
        return Err("an owner over the byte ceiling is refused typed".into());
    }
    Ok(())
}

// CASE-COVERS: a source refuses a second window while the first is still
// resident, and the sink's admission refuses a window over either ceiling
// or with no rows.
#[test]
fn a_resident_window_blocks_the_next_and_admission_refuses_over_policy() -> TestResult {
    let scopes = vec![scope(
        "a.rs",
        vec![chunk("a-1", "a.rs")?, chunk("a-2", "a.rs")?],
    )];
    let one_owner = SemanticStreamWindowPolicy::new(1, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES)?;
    let mut source = ResidentScopeSource::new(&scopes, one_owner)?;
    let first = source
        .next_window()?
        .ok_or("two owners issue a first window")?;
    if typed_code(&source.next_window()) != Some(SEMANTIC_STREAM_WINDOW_STILL_RESIDENT_CODE) {
        return Err("a second window while one is resident is refused typed".into());
    }
    drop(first);
    let second = source
        .next_window()?
        .ok_or("the second owner is issued after the drop")?;
    if ids(&second) != vec!["a-2".to_string()] {
        return Err("the second window carries the second owner".into());
    }
    drop(second);
    if source.next_window()?.is_some() {
        return Err("the stream ends after the last owner".into());
    }

    // Admission on the sink's side: a two-owner window under a one-owner
    // policy, a two-row window under a one-row byte policy, an empty one.
    let residency = Arc::new(SemanticWindowResidencyV1::default());
    let two_owners = SemanticScopeWindowV1::lease(scopes.clone(), &residency)?;
    match one_owner.admit(&two_owners) {
        Err(CoreError::Typed { code, .. }) if code == SEMANTIC_STREAM_WINDOW_EXCEEDED_CODE => {}
        other => return Err(format!("over the scope ceiling is refused typed: {other:?}").into()),
    }
    let one_row = SemanticStreamWindowPolicy::new(SEMANTIC_STREAM_WINDOW_SCOPES, ROW_BYTES)?;
    match one_row.admit(&two_owners) {
        Err(CoreError::Typed { code, .. }) if code == SEMANTIC_STREAM_WINDOW_EXCEEDED_CODE => {}
        other => return Err(format!("over the byte ceiling is refused typed: {other:?}").into()),
    }
    let fill = SemanticStreamWindowPolicy::DEFAULT.admit(&two_owners)?;
    if fill.owner_scopes != 2 || fill.vector_bytes != ROW_BYTES.saturating_mul(2) {
        return Err(format!("admission measures the window: {fill:?}").into());
    }
    drop(two_owners);
    let empty = SemanticScopeWindowV1::lease(vec![scope("e.rs", Vec::new())], &residency)?;
    if !matches!(
        SemanticStreamWindowPolicy::DEFAULT.admit(&empty),
        Err(CoreError::InvalidContract(_))
    ) {
        return Err("an empty window is a source defect".into());
    }
    Ok(())
}

/// A build port double that drains the stream and reports what it saw,
/// optionally lying about it by one window.
struct DrainingPort {
    seen: Mutex<Vec<Vec<String>>>,
    undercount: bool,
}

impl SemanticScopeStreamBuildPort for DrainingPort {
    fn build_stream(
        &self,
        header: &SemanticIngestHeaderV1,
        scopes: &mut dyn SemanticScopeSource,
) -> Result<(SemanticStreamTallyV1, quanta_index_contract::IngestStageReport), CoreError> {
        if header.dimension()? != DIMENSION {
            return Err(CoreError::InvalidContract("header dimension".to_string()));
        }
        let mut tally = SemanticStreamTallyV1::default();
        while let Some(window) = scopes.next_window()? {
            tally.count_window(window.scopes().len(), window.rows()?, window.vector_bytes())?;
            self.seen
                .lock()
                .map_err(|err| CoreError::Storage(format!("port poisoned: {err}")))?
                .push(ids(&window));
            drop(window);
        }
        if self.undercount {
            tally.windows = tally.windows.saturating_sub(1);
        }
        Ok((tally, quanta_index_contract::IngestStageReport::default()))
    }
}

// CASE-COVERS: the resident build wrapper hands the port the header of
// the batch and its scopes windowed by the policy, and refuses typed when
// the port's tally does not add up to what the source issued.
#[test]
fn the_resident_build_wrapper_streams_the_batch_and_cross_checks_the_tally() -> TestResult {
    let batch = batch(vec![
        scope("a.rs", vec![chunk("a-1", "a.rs")?, chunk("a-2", "a.rs")?]),
        scope("b.rs", vec![chunk("b-1", "b.rs")?]),
    ]);
    let header = SemanticIngestHeaderV1::of_batch(&batch);
    if header.pin.manifest_generation != batch.generation
        || header.contract.model_contract != batch.model_contract
        || header.contract.required_corpora != batch.required_corpora
        || header.contract.corpus_policy_digest != batch.corpus_policy_digest
        || header.batch.manifest_digest != batch.manifest_digest
        || header.batch.batch_digest != batch.batch_digest
        || header.batch.seal != batch.seal
        || header.mutations.clear_surfaces != batch.clear_surfaces
        || header.mutations.tombstone_scopes != batch.tombstone_scopes
    {
        return Err("the header carries every field of the batch but its scopes".into());
    }
    let honest = DrainingPort {
        seen: Mutex::new(Vec::new()),
        undercount: false,
    };
    let policy = SemanticStreamWindowPolicy::new(2, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES)?;
    let tally = build_resident_semantic_batch_v1(&honest, &batch, policy)?;
    let expected = SemanticStreamTallyV1 {
        windows: 2,
        replace_scopes: 2,
        rows: 3,
        peak_vector_bytes: ROW_BYTES.saturating_mul(2),
    };
    if tally != expected {
        return Err(format!("tally {tally:?} != {expected:?}").into());
    }
    let seen = honest
        .seen
        .lock()
        .map_err(|err| format!("port poisoned: {err}"))?
        .clone();
    let expected_seen: Vec<Vec<String>> =
        vec![vec!["a-1".into(), "a-2".into()], vec!["b-1".into()]];
    if seen != expected_seen {
        return Err(format!("the port saw {seen:?}").into());
    }

    let lying = DrainingPort {
        seen: Mutex::new(Vec::new()),
        undercount: true,
    };
    match build_resident_semantic_batch_v1(&lying, &batch, policy) {
        Err(CoreError::InvalidContract(message)) if message.contains("source issued") => Ok(()),
        other => Err(format!("a tally mismatch is refused typed, got {other:?}").into()),
    }
}
