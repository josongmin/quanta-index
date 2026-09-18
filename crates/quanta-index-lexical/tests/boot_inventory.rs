//! QI-BB-026 — the boot inventory reads identities, quarantines what it
//! cannot trust, and never examines content.
//!
//! Before this, the boot scanner deep-validated every sealed generation it
//! found and turned the first defect — a legacy directory, an undecodable
//! identity, a corrupt sidecar in a generation nobody serves — into a scan
//! error that stopped the daemon from binding. Now the inventory is a
//! metadata pass: a sealed identity that owns its directory is listed, a
//! directory the inventory cannot trust is quarantined with its path and a
//! reason, and content is left to the doors that serve or mutate a
//! generation.
//!
//! The oracle for "never examines content" is a sealed generation whose
//! sidecar is bit-flipped before the inventory runs: the inventory must still
//! list it, and the validator must still refuse it.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::{Path, PathBuf};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, GenerationSnapshot, ManifestGeneration, RepoId,
    RepoRelativePath, RevisionId, SearchCorpusIngestBatch, SearchCorpusReplaceScope,
    SearchPlaneTrackKind, SearchScopeKey, SearchScopeSurface,
};
use quanta_index_core::{
    CoreError, GenerationIdentityValidatePort, GenerationQuarantineReasonV1,
    GenerationStorageKeyV1, SealedGenerationScanPort, SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;

type TestResult = Result<(), Box<dyn Error>>;

const IDENTITY: &str = "search-corpus-generation-identity.cbor";
/// The text-authority manifest inside the generation directory: one of the
/// files the seal commits to and the inventory must not read.
const TEXT_AUTHORITY_MANIFEST: &str = "text-authority/manifest.cbor";

fn repo() -> RepoId {
    RepoId::new("inventory-repo")
}

fn revision() -> RevisionId {
    RevisionId::new("inventory-rev")
}

fn scope(body: &str) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let language = LanguageCode::new("rust")
        .map_err(|err| -> Box<dyn Error> { format!("language code: {err}").into() })?;
    Ok(SearchCorpusReplaceScope {
        scope: SearchScopeKey {
            doc_surface: SearchScopeSurface::File,
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        },
        scope_digest: "scope:src/lib.rs".to_string(),
        chunks: vec![ChunkRecord {
            chunk_id: ChunkId::new("chunk-lib"),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            language,
            start_byte: 0,
            end_byte: u32::try_from(body.len())?,
            start_line: 1,
            end_line: 1,
            text: body.to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        }],
        symbols: Vec::new(),
    })
}

fn sealed_batch(generation: ManifestGeneration) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    Ok(SearchCorpusIngestBatch {
        repo_id: repo(),
        revision_id: revision(),
        generation,
        base_generation: None,
        manifest_digest: format!("manifest-digest:{}", generation.get()),
        batch_digest: format!("batch-digest:{}", generation.get()),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![scope("fn inventory_needle() {}")?],
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    })
}

fn identity(generation: ManifestGeneration) -> GenerationSnapshot {
    GenerationSnapshot {
        repo_id: repo(),
        revision_id: revision(),
        track: SearchPlaneTrackKind::Lexical,
        manifest_generation: generation,
        manifest_digest: format!("manifest-digest:{}", generation.get()),
    }
}

fn generation_dir(root: &Path, generation: ManifestGeneration) -> PathBuf {
    GenerationStorageKeyV1::for_repo_revision(&repo(), &revision()).generation_dir(root, generation)
}

fn flip_last_byte(path: &Path) -> TestResult {
    let mut bytes = std::fs::read(path)?;
    let last = bytes.len().checked_sub(1).ok_or("empty file")?;
    let byte = bytes.get_mut(last).ok_or("index")?;
    *byte ^= 0xff;
    std::fs::write(path, bytes)?;
    Ok(())
}

fn typed_code(result: &Result<(), CoreError>) -> Option<String> {
    match result {
        Err(CoreError::Typed { code, .. }) => Some(code.clone()),
        _ => None,
    }
}

/// One sealed generation beside four untrusted directories and a build.
///
/// The inventory lists the sealed one, quarantines the four with their
/// reasons, skips the in-progress build, and does not notice that the
/// sealed one's sidecar is corrupt — the validator does.
#[test]
fn inventory_quarantines_untrusted_directories_and_never_reads_content() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let g1 = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(g1)?)?;
    let sealed_dir = generation_dir(&root, g1);
    let family_dir = sealed_dir
        .parent()
        .ok_or("generation dir has a family parent")?
        .to_path_buf();

    // In-progress build: no sealed identity, silently skipped.
    std::fs::create_dir_all(family_dir.join("g2"))?;
    // Legacy raw layout at the root.
    let legacy_family = root.join("repo-alpha");
    std::fs::create_dir_all(legacy_family.join("rev-alpha/g1"))?;
    // Non-canonical generation directory inside a canonical family.
    let non_canonical = family_dir.join("g03");
    std::fs::create_dir_all(&non_canonical)?;
    // An identity that does not decode.
    let unreadable = family_dir.join("g4");
    std::fs::create_dir_all(&unreadable)?;
    std::fs::write(unreadable.join(IDENTITY), b"\xff\x00not-cbor")?;
    // g1's identity copied under a directory it does not own.
    let moved = family_dir.join("g5");
    std::fs::create_dir_all(&moved)?;
    let _copied = std::fs::copy(sealed_dir.join(IDENTITY), moved.join(IDENTITY))?;
    // Content corruption the inventory must not look for.
    flip_last_byte(&sealed_dir.join(TEXT_AUTHORITY_MANIFEST))?;

    let inventory = adapter.inventory_sealed_generations()?;
    let sealed: Vec<(GenerationSnapshot, PathBuf)> = inventory
        .sealed
        .iter()
        .map(|entry| (entry.identity.clone(), entry.path.clone()))
        .collect();
    if sealed != vec![(identity(g1), sealed_dir)] {
        return Err(format!("expected only g1 inventoried at its own path, got {sealed:?}").into());
    }
    let mut quarantined = inventory
        .quarantined
        .iter()
        .map(|entry| (entry.path.clone(), entry.reason, entry.track))
        .collect::<Vec<_>>();
    quarantined.sort();
    let expected = vec![
        (
            legacy_family,
            GenerationQuarantineReasonV1::NonCanonicalLayout,
            SearchPlaneTrackKind::Lexical,
        ),
        (
            non_canonical,
            GenerationQuarantineReasonV1::NonCanonicalLayout,
            SearchPlaneTrackKind::Lexical,
        ),
        (
            unreadable,
            GenerationQuarantineReasonV1::IdentityUnreadable,
            SearchPlaneTrackKind::Lexical,
        ),
        (
            moved,
            GenerationQuarantineReasonV1::ScopeMismatch,
            SearchPlaneTrackKind::Lexical,
        ),
    ];
    let mut expected_sorted = expected;
    expected_sorted.sort();
    if quarantined != expected_sorted {
        return Err(format!("unexpected quarantine set: {quarantined:?}").into());
    }
    if inventory
        .quarantined
        .iter()
        .any(|entry| entry.detail.is_empty())
    {
        return Err("every quarantine record must carry a detail".into());
    }

    let refused = adapter.validate_generation_identity(&identity(g1));
    if typed_code(&refused).as_deref() != Some("GENERATION_SIDECAR_CORRUPT") {
        return Err(format!(
            "the validator must refuse the corrupt sidecar the inventory ignored: {refused:?}"
        )
        .into());
    }
    Ok(())
}

/// A missing track root is an empty inventory, not an error: a fresh state
/// root has no generations to quarantine.
#[test]
fn a_missing_root_inventories_nothing() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(temp.path().join("never-created"));
    let inventory = adapter.inventory_sealed_generations()?;
    if !inventory.sealed.is_empty() || !inventory.quarantined.is_empty() {
        return Err(format!("expected an empty inventory, got {inventory:?}").into());
    }
    Ok(())
}
