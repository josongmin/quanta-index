//! Fixtures shared by the readiness test modules.

#![expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning durability and CAS tests use assertions as test-failure reporting"
)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::lex::{
    LanguageCode, ParseNode, ParseTreeRecord, compute_parse_tree_source_hash,
};
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, GenerationSnapshot, ManifestGeneration,
    ReplaceLexicalScope, RepoId, RepoRelativePath, RevisionId, SearchCorpusActiveHeadV1,
    SearchCorpusGenerationIdentityV1,
    SearchCorpusReplaceScope, SearchPlaneTrackKind, SearchScopeKey, SearchScopeSurface,
    UpsertParseTree,
};
use quanta_index_core::{
    AuxiliaryAuthorityCatalogPort, AuxiliaryGenerationKeyV1, AuxiliaryMutationBatchV1, CoreError,
};

use crate::auxiliary_authority;
use crate::readiness::activation_catalog::ActivationCatalog;
use crate::readiness::auxiliary_store::AuxiliaryAuthorityStore;
use crate::readiness::keys::AuthorityKey;
use crate::readiness::ledger::Ledger;
use crate::readiness::search_corpus_generation::SearchCorpusGenerationV1;
use crate::search_corpus_retention::SearchCorpusHistoryRetentionPolicyV1;

pub(super) fn search_corpus_retention(
    max_generations: usize,
) -> Result<SearchCorpusHistoryRetentionPolicyV1, CoreError> {
    SearchCorpusHistoryRetentionPolicyV1::new(max_generations, 1024 * 1024, 64, 64 * 1024 * 1024)
}

#[derive(Debug)]
pub(super) struct AlwaysFailParentSync;

impl crate::readiness::durable_fs::ParentDirectorySyncPort for AlwaysFailParentSync {
    fn sync_parent(&self, _parent: &Path) -> std::io::Result<()> {
        Err(std::io::Error::other("injected parent sync failure"))
    }
}

#[derive(Debug)]
pub(super) struct FailAtParentSync {
    pub(super) calls: AtomicUsize,
    pub(super) fail_at: usize,
}

impl crate::readiness::durable_fs::ParentDirectorySyncPort for FailAtParentSync {
    fn sync_parent(&self, parent: &Path) -> std::io::Result<()> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == self.fail_at {
            return Err(std::io::Error::other(format!(
                "injected parent sync failure at call {call}"
            )));
        }
        std::fs::File::open(parent)?.sync_all()
    }
}

#[derive(Debug)]
pub(super) struct FailNthSyncForParent {
    pub(super) target_parent: PathBuf,
    pub(super) matching_calls: AtomicUsize,
    pub(super) fail_at_matching_call: usize,
}

impl crate::readiness::durable_fs::ParentDirectorySyncPort for FailNthSyncForParent {
    fn sync_parent(&self, parent: &Path) -> std::io::Result<()> {
        if parent == self.target_parent {
            let matching_call = self.matching_calls.fetch_add(1, Ordering::SeqCst);
            if matching_call == self.fail_at_matching_call {
                return Err(std::io::Error::other(format!(
                    "injected parent sync failure for {} at matching call {matching_call}",
                    parent.display()
                )));
            }
        }
        std::fs::File::open(parent)?.sync_all()
    }
}

#[derive(Debug)]
pub(super) struct ToggleParentSyncFailure {
    pub(super) fail: AtomicBool,
}

impl crate::readiness::durable_fs::ParentDirectorySyncPort for ToggleParentSyncFailure {
    fn sync_parent(&self, parent: &Path) -> std::io::Result<()> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(std::io::Error::other(format!(
                "injected parent sync failure for {}",
                parent.display()
            )));
        }
        std::fs::File::open(parent)?.sync_all()
    }
}

pub(super) type TestResult = Result<(), Box<dyn std::error::Error>>;

pub(super) fn repo_id() -> RepoId {
    RepoId::new("repo").expect("static fixture ID satisfies canonical policy")
}

pub(super) fn revision_id() -> RevisionId {
    RevisionId::new("rev").expect("static fixture ID satisfies canonical policy")
}

pub(super) fn generation() -> ManifestGeneration {
    ManifestGeneration::new(17)
}

pub(super) fn rust_language() -> Result<LanguageCode, Box<dyn std::error::Error>> {
    LanguageCode::new("rust").map_err(|err| format!("invalid hard-coded language: {err}").into())
}

pub(super) fn scope(path: &str) -> SearchScopeKey {
    SearchScopeKey {
        doc_surface: SearchScopeSurface::Chunk,
        repo_relative_path: RepoRelativePath::new(path),
    }
}

pub(super) fn chunk_record(
    path: &str,
    text: &str,
) -> Result<ChunkRecord, Box<dyn std::error::Error>> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new("chunk-1"),
        repo_relative_path: RepoRelativePath::new(path),
        language: rust_language()?,
        start_byte: 0,
        end_byte: u32::try_from(text.len()).map_err(|err| format!("text len overflow: {err}"))?,
        start_line: 1,
        end_line: 1,
        text: text.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    })
}

pub(super) fn parse_tree_record(text: &str) -> Result<ParseTreeRecord, Box<dyn std::error::Error>> {
    Ok(ParseTreeRecord {
        wire_version: 1,
        lang: rust_language()?,
        root: ParseNode {
            kind: "function_item".to_string().into_boxed_str(),
            byte_start: 0,
            byte_end: u32::try_from(text.len())
                .map_err(|err| format!("text len overflow: {err}"))?,
            children: Vec::new(),
        },
        source_hash: compute_parse_tree_source_hash(text),
        role_tag_schema_version: 0,
        role_tags: Vec::new(),
    })
}

pub(super) fn encode_cbor<T: serde::Serialize>(
    value: &T,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    quanta_index_ipc::encode_cbor_payload(value)
        .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })
}

pub(super) fn install_chunk(ledger: &mut Ledger, path: &str, text: &str) -> TestResult {
    install_chunk_with_id(ledger, "chunk-1", path, text)
}

pub(super) fn install_chunk_with_id(
    ledger: &mut Ledger,
    chunk_id: &str,
    path: &str,
    text: &str,
) -> TestResult {
    let op = LexicalChannelOp::ReplaceLexicalScope(ReplaceLexicalScope {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation: generation(),
        payload: encode_cbor(&(
            BatchIngestMode::ReplaceGeneration,
            None::<ManifestGeneration>,
            SearchCorpusReplaceScope {
                scope: scope(path),
                scope_digest: "scope:lex".to_string(),
                chunks: vec![{
                    let mut record = chunk_record(path, text)?;
                    record.chunk_id = ChunkId::new(chunk_id);
                    record
                }],
                symbols: Vec::new(),
            },
        ))?,
    });
    ledger.apply_lexical_authority_op(&op, std::time::Instant::now())?;
    Ok(())
}

pub(super) fn install_parse_tree(ledger: &mut Ledger, text: &str) -> TestResult {
    let op = LexicalChannelOp::UpsertParseTree(UpsertParseTree {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation: generation(),
        chunk_id: ChunkId::new("chunk-1"),
        payload: encode_cbor(&parse_tree_record(text)?)?,
    });
    ledger.apply_lexical_authority_op(&op, std::time::Instant::now())?;
    Ok(())
}

pub(super) fn expect_decode_fail(err: CoreError, needle: &str) -> TestResult {
    match err {
        CoreError::Typed { code, message } => {
            if code
                != quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                    quanta_index_contract::lex::LexicalErrorCode::StrParseTreeDecodeFail,
                )
            {
                return Err(format!("expected STR_PARSE_TREE_DECODE_FAIL, got {code}").into());
            }
            if !message.contains(needle) {
                return Err(
                    format!("expected message to contain `{needle}`, got `{message}`").into(),
                );
            }
            Ok(())
        }
        other @ (CoreError::InvalidContract(_)
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)) => {
            Err(format!("expected typed decode fail, got {other:?}").into())
        }
    }
}

pub(super) fn corpus_snapshot(
    track: SearchPlaneTrackKind,
    generation: u64,
    digest: &str,
) -> GenerationSnapshot {
    GenerationSnapshot {
        repo_id: RepoId::new("repo-corpus").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev-corpus")
            .expect("static fixture ID satisfies canonical policy"),
        track,
        manifest_generation: ManifestGeneration::new(generation),
        manifest_digest: digest.to_string(),
    }
}

pub(super) fn corpus_generation(
    generation: u64,
    digest: &str,
) -> Result<SearchCorpusGenerationV1, CoreError> {
    SearchCorpusGenerationV1::new(
        corpus_snapshot(SearchPlaneTrackKind::Lexical, generation, digest),
        corpus_snapshot(SearchPlaneTrackKind::Semantic, generation, digest),
        crate::content_roots_test_support::roots_for_generation(generation),
    )
}

pub(super) fn corpus_identity(
    generation: &SearchCorpusGenerationV1,
) -> SearchCorpusGenerationIdentityV1 {
    SearchCorpusGenerationIdentityV1 {
        lexical: generation.lexical().clone(),
        semantic: generation.semantic().clone(),
        semantic_content: generation.semantic_content().clone(),
    }
}

pub(super) fn active_head(
    catalog: &ActivationCatalog,
    generation: &SearchCorpusGenerationV1,
) -> Result<SearchCorpusActiveHeadV1, CoreError> {
    let (observed, activation_token) = catalog
        .active_search_corpus_with_token_v1(generation.repo_id(), generation.revision_id())?
        .ok_or_else(|| CoreError::NotReady("test expected an active corpus head".to_string()))?;
    Ok(SearchCorpusActiveHeadV1 {
        generation: observed.to_contract_v1(),
        activation_token,
    })
}

pub(super) fn assert_active_composite_v1(
    catalog: &ActivationCatalog,
    generation: &SearchCorpusGenerationV1,
) -> TestResult {
    for track in [
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Semantic,
    ] {
        let observed =
            catalog.resolve_record(generation.repo_id(), generation.revision_id(), track)?;
        assert_eq!(
            observed.manifest_generation,
            generation.manifest_generation()
        );
        assert_eq!(observed.manifest_digest, generation.manifest_digest());
    }
    Ok(())
}

pub(super) fn search_corpus_history_file_names_v1(
    store: &AuxiliaryAuthorityStore,
    repo_id: &RepoId,
    revision_id: &RevisionId,
) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let pair_dir = store.search_corpus_pair_dir(repo_id, revision_id);
    if !pair_dir.exists() {
        return Ok(BTreeSet::new());
    }
    std::fs::read_dir(pair_dir)?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(Into::into)
}

/// Encode every auxiliary authority of `ledger` as rows into a test catalog.
pub(super) fn persist_whole_ledger(
    catalog: &dyn AuxiliaryAuthorityCatalogPort,
    ledger: &Ledger,
) -> Result<(), CoreError> {
    let generation_key = |key: &AuthorityKey| AuxiliaryGenerationKeyV1 {
        repo_id: key.repo_id.clone(),
        revision_id: key.revision_id.clone(),
        generation: key.generation,
    };
    let mut batch = AuxiliaryMutationBatchV1::default();
    for (key, registry) in &ledger.history {
        let read = registry.read_current();
        batch.rows.extend(auxiliary_authority::history_state_rows(
            &generation_key(key),
            read.epoch,
            &read.state,
        )?);
    }
    for (key, registry) in &ledger.runtime_metadata {
        let read = registry.read_current();
        batch.rows.extend(auxiliary_authority::runtime_state_rows(
            &generation_key(key),
            read.epoch,
            &read.state,
        )?);
    }
    for (key, registry) in &ledger.structural {
        let read = registry.read_current();
        batch
            .rows
            .extend(auxiliary_authority::structural_state_rows(
                &generation_key(key),
                read.epoch,
                &read.state,
            )?);
    }
    for (key, state) in &ledger.search_tracks {
        if key.track == SearchPlaneTrackKind::Structural {
            batch.tracks.push(auxiliary_authority::structural_track_row(
                &key.repo_id,
                &key.revision_id,
                state,
            )?);
        }
    }
    let _receipt = catalog.apply(&batch)?;
    Ok(())
}
