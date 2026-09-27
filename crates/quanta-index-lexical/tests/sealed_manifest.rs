//! QI-BB-030 — a sealed lexical generation commits to every byte a query
//! needs, and both doors that admit it check the commitment.
//!
//! Before this, the sealed identity proved only itself: the activation
//! validator checked the identity and opened the Tantivy index, while the
//! query open additionally decoded text-authority sidecars it had never
//! verified. A sidecar lost, truncated or bit-flipped after the seal passed
//! activation and failed the first query.
//!
//! Now the seal writes a manifest (the Tantivy commit and every segment
//! file it references, every file of the `text-authority/` tree — its
//! manifest and each shard — and every repo-metadata overlay, each with
//! length and SHA-256, plus the identity's digest) before the identity,
//! and both `validate_generation_identity` (activation, restart) and
//! `open` (query) walk it through one function. The oracle here is fault
//! injection on the real files: every text-authority file and every
//! overlay in turn is removed, truncated, bit-flipped and replaced by a
//! stale copy from another generation, and both doors must refuse under
//! the typed code and admit again once the file is restored. Segment files
//! are proved by presence and length at the doors and by content at the
//! seal and the scrub, so a same-length flip is the scrub's to refuse.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::{Path, PathBuf};

use quanta_index_contract::channel::{LexicalChannelOp, LexicalFullBundle, UpsertChunk};
use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, FileContributorEntry, FileContributorIdentityEntry,
    FileContributorIngestBatch, FileOwnershipEntry, FileOwnershipIngestBatch, GenerationSnapshot,
    LQ_VERSION_TAG, LqExpr, LqLeaf, LqOptions, LqQuery, LqSpan, ManifestGeneration,
    RepoCommitRecencyEntry, RepoCommitRecencyIngestBatch, RepoDescriptionEntry,
    RepoDescriptionIngestBatch, RepoId, RepoMetaEntry, RepoMetaIngestBatch, RepoRelativePath,
    RepoTopicEntry, RepoTopicIngestBatch, RevisionId, SearchCorpusIngestBatch,
    SearchCorpusReplaceScope, SearchPlaneTrackKind,
};
use quanta_index_core::{
    CoreError, DoorFindingOutcome, DoorFindingQuarantinePort, FileContributorIngestPort,
    FileOwnershipIngestPort, FinishedReclaims, GenerationIdentityValidatePort,
    GenerationQuarantineReasonV1, GenerationStorageKeyV1, IntegrityScrubBudgetV1,
    IntegrityScrubCursorV1, IntegrityScrubOutcomeV1, IntegrityScrubPort, LexicalIndexBuildPort,
    LexicalIndexOpenPort, QuarantineDiscardOutcomeV1, QuarantinedGenerationDiscardPort,
    RECLAIM_AREA_DIR_NAME, RepoCommitRecencyIngestPort, RepoDescriptionIngestPort,
    RepoMetaIngestPort, RepoTopicIngestPort, RequestBudgetV1, SealedGenerationReclaimOutcomeV1,
    SealedGenerationReclaimPort, SealedGenerationScanPort, SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;

#[path = "support/current_source_fixture.rs"]
mod current_source_fixture;

type TestResult = Result<(), Box<dyn Error>>;

/// Where, inside a generation directory, a test damages a file.
type LocateFile = fn(&Path) -> Result<PathBuf, Box<dyn Error>>;

const TEXT_AUTHORITY_DIR: &str = "text-authority";
const TEXT_AUTHORITY_MANIFEST: &str = "manifest.cbor";
const MANIFEST: &str = "search-corpus-generation-manifest.cbor";
const IDENTITY: &str = "search-corpus-generation-identity.cbor";
const TANTIVY_META: &str = "meta.json";
/// The repo-metadata overlay sidecars a query decodes at open, by file name.
const OVERLAY_FILES: [&str; 7] = [
    "repo-metadata.cbor",
    "repo-commit-recency.cbor",
    "repo-meta.cbor",
    "repo-topic.cbor",
    "repo-description.cbor",
    "file-ownership.cbor",
    "file-contributor.cbor",
];

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn repo() -> RepoId {
    RepoId::new("manifest-repo").expect("static fixture ID satisfies canonical policy")
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn revision() -> RevisionId {
    RevisionId::new("manifest-rev").expect("static fixture ID satisfies canonical policy")
}

fn scope(
    path: &str,
    chunk_id: &str,
    body: &str,
) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let language = LanguageCode::new("rust")
        .map_err(|err| -> Box<dyn Error> { format!("language code: {err}").into() })?;
    current_source_fixture::text_scope(
        &repo(),
        &revision(),
        path,
        language.clone(),
        body,
        vec![ChunkRecord {
            chunk_id: ChunkId::new(chunk_id),
            repo_relative_path: RepoRelativePath::new(path),
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
    )
}

fn sealed_batch(
    generation: ManifestGeneration,
    body: &str,
) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    let mut batch = SearchCorpusIngestBatch {
        source_event: current_source_fixture::empty_event(),
        repo_id: repo(),
        revision_id: revision(),
        generation,
        base_generation: None,
        manifest_digest: format!("manifest-digest:{}", generation.get()),
        batch_digest: "0".repeat(64),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![scope("src/lib.rs", "chunk-lib", body)?],
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    };
    current_source_fixture::finish_batch(&mut batch)?;
    Ok(batch)
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

fn query(term: &str) -> LqQuery {
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(LqLeaf::Keyword(term.to_string())),
        filters: Vec::new(),
        options: LqOptions::defaults(),
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

/// The `FullBundle` repo-metadata payload: the wire map the adapter decodes.
fn repo_metadata_bundle_payload(context: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut visibility = Vec::new();
    ciborium::into_writer(
        &quanta_index_contract::LqVisibility::Private,
        &mut visibility,
    )?;
    let visibility: ciborium::Value = ciborium::from_reader(visibility.as_slice())?;
    let wire = ciborium::Value::Map(vec![
        (
            ciborium::Value::Text("fork".to_string()),
            ciborium::Value::Bool(false),
        ),
        (
            ciborium::Value::Text("archived".to_string()),
            ciborium::Value::Bool(false),
        ),
        (ciborium::Value::Text("visibility".to_string()), visibility),
        (
            ciborium::Value::Text("contexts".to_string()),
            ciborium::Value::Array(vec![ciborium::Value::Text(context.to_string())]),
        ),
    ]);
    let mut payload = Vec::new();
    ciborium::into_writer(&wire, &mut payload)?;
    Ok(payload)
}

/// What every overlay publish of one generation answered.
struct OverlayPublishOutcome {
    what: &'static str,
    result: Result<(), CoreError>,
}

/// Publish all seven repo-metadata overlays into `generation`, stamped with
/// `label` so two generations' overlays differ byte for byte.
///
/// Returns one outcome per overlay so a caller can demand that every one
/// landed (before the seal) or that every one was refused (after it).
fn publish_overlays(
    adapter: &LexicalAdapter,
    generation: ManifestGeneration,
    label: &str,
) -> Result<Vec<OverlayPublishOutcome>, Box<dyn Error>> {
    let source = RepoId::new(format!("source-{label}"))?;
    let digest = |family: &str| format!("overlay:{family}:{label}:{}", generation.get());
    let bundle = LexicalChannelOp::FullBundle(LexicalFullBundle {
        repo_id: repo(),
        revision_id: revision(),
        generation,
        payload: repo_metadata_bundle_payload(label)?,
    });
    Ok(vec![
        OverlayPublishOutcome {
            what: "repo commit recency",
            result: RepoCommitRecencyIngestPort::publish_batch(
                adapter,
                &RepoCommitRecencyIngestBatch {
                    repo_id: repo(),
                    revision_id: revision(),
                    generation,
                    batch_digest: digest("commit-recency"),
                    entries: vec![RepoCommitRecencyEntry {
                        source_repo_id: source.clone(),
                        latest_committer_time_ms: 1_700_000_000_000,
                    }],
                },
            )
            .map(|_receipt| ()),
        },
        OverlayPublishOutcome {
            what: "repo meta",
            result: RepoMetaIngestPort::publish_batch(
                adapter,
                &RepoMetaIngestBatch {
                    repo_id: repo(),
                    revision_id: revision(),
                    generation,
                    batch_digest: digest("meta"),
                    entries: vec![RepoMetaEntry {
                        source_repo_id: source.clone(),
                        key: "lifecycle".to_string(),
                        value: label.to_string(),
                    }],
                },
            )
            .map(|_receipt| ()),
        },
        OverlayPublishOutcome {
            what: "repo topic",
            result: RepoTopicIngestPort::publish_batch(
                adapter,
                &RepoTopicIngestBatch {
                    repo_id: repo(),
                    revision_id: revision(),
                    generation,
                    batch_digest: digest("topic"),
                    entries: vec![RepoTopicEntry {
                        source_repo_id: source.clone(),
                        topic: format!("topic-{label}"),
                    }],
                },
            )
            .map(|_receipt| ()),
        },
        OverlayPublishOutcome {
            what: "repo description",
            result: RepoDescriptionIngestPort::publish_batch(
                adapter,
                &RepoDescriptionIngestBatch {
                    repo_id: repo(),
                    revision_id: revision(),
                    generation,
                    batch_digest: digest("description"),
                    entries: vec![RepoDescriptionEntry {
                        source_repo_id: source.clone(),
                        description: format!("{label} description"),
                    }],
                },
            )
            .map(|_receipt| ()),
        },
        OverlayPublishOutcome {
            what: "file ownership",
            result: FileOwnershipIngestPort::publish_batch(
                adapter,
                &FileOwnershipIngestBatch {
                    repo_id: repo(),
                    revision_id: revision(),
                    generation,
                    batch_digest: digest("ownership"),
                    entries: vec![FileOwnershipEntry {
                        source_repo_id: source.clone(),
                        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
                        owners: vec![format!("@{label}-owner")],
                    }],
                },
            )
            .map(|_receipt| ()),
        },
        OverlayPublishOutcome {
            what: "file contributor",
            result: FileContributorIngestPort::publish_batch(
                adapter,
                &FileContributorIngestBatch {
                    repo_id: repo(),
                    revision_id: revision(),
                    generation,
                    batch_digest: digest("contributor"),
                    entries: vec![FileContributorEntry {
                        source_repo_id: source,
                        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
                        contributors: vec![FileContributorIdentityEntry {
                            canonical: format!("{label}-contributor"),
                            name: Some(format!("{label} contributor")),
                            email: Some(format!("{label}@example.test")),
                        }],
                    }],
                },
            )
            .map(|_receipt| ()),
        },
        OverlayPublishOutcome {
            what: "repo-metadata bundle",
            result: adapter.build(&repo(), &revision(), generation, &[bundle]),
        },
    ])
}

/// Every overlay publish landed.
fn expect_overlays_landed(outcomes: Vec<OverlayPublishOutcome>) -> TestResult {
    for outcome in outcomes {
        if let Err(err) = outcome.result {
            return Err(format!("{} publish before the seal failed: {err}", outcome.what).into());
        }
    }
    Ok(())
}

/// A sealed generation carrying every overlay, built from `body`.
fn sealed_generation_with_overlays(
    adapter: &LexicalAdapter,
    generation: ManifestGeneration,
    label: &str,
    body: &str,
) -> TestResult {
    expect_overlays_landed(publish_overlays(adapter, generation, label)?)?;
    adapter.build_batch(&sealed_batch(generation, body)?)?;
    Ok(())
}

/// One regular file directly under a generation directory: its name and
/// its bytes.
type NamedFile = (String, Vec<u8>);

/// `(name, bytes)` of every regular file directly under `dir`, sorted.
fn top_level_files(dir: &Path) -> Result<Vec<NamedFile>, Box<dyn Error>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        files.push((name, std::fs::read(entry.path())?));
    }
    files.sort();
    Ok(files)
}

/// What every door said about a generation: the validator's verdict,
/// whether a query could open and serve it, and whether the activation's
/// proven open (the proof whose handle is promoted) could.
struct Doors {
    validate: Result<(), CoreError>,
    open: Result<usize, CoreError>,
    proven: Result<usize, CoreError>,
}

fn knock(adapter: &LexicalAdapter, generation: ManifestGeneration) -> Doors {
    let validate = adapter.validate_generation_identity(&identity(generation));
    let open = adapter
        .open(&repo(), &revision(), generation)
        .and_then(|searcher| {
            searcher.search(&query("sealed_needle"), 5, &RequestBudgetV1::unbounded())
        })
        .map(|hits| hits.len());
    let proven = adapter
        .open_proven(&identity(generation))
        .and_then(|searcher| {
            searcher.search(&query("sealed_needle"), 5, &RequestBudgetV1::unbounded())
        })
        .map(|hits| hits.len());
    Doors {
        validate,
        open,
        proven,
    }
}

fn typed_code(result: &Result<(), CoreError>) -> Option<String> {
    match result {
        Err(CoreError::Typed { code, .. }) => Some(code.to_string()),
        _ => None,
    }
}

fn typed_open_code(result: &Result<usize, CoreError>) -> Option<String> {
    match result {
        Err(CoreError::Typed { code, .. }) => Some(code.to_string()),
        _ => None,
    }
}

fn expect_admitted(doors: &Doors, what: &str) -> TestResult {
    if let Err(err) = &doors.validate {
        return Err(format!("{what}: validator refused an intact generation: {err}").into());
    }
    for (door, result) in [("open", &doors.open), ("proven open", &doors.proven)] {
        match result {
            Ok(1) => {}
            Ok(other) => {
                return Err(format!(
                    "{what}: {door} of an intact generation served {other} hits, expected 1"
                )
                .into());
            }
            Err(err) => {
                return Err(format!("{what}: {door} refused an intact generation: {err}").into());
            }
        }
    }
    Ok(())
}

fn expect_refused(doors: &Doors, what: &str, code: &str) -> TestResult {
    if typed_code(&doors.validate).as_deref() != Some(code) {
        return Err(format!(
            "{what}: validator answered {:?}, expected typed {code}",
            doors.validate
        )
        .into());
    }
    for (door, result) in [("open", &doors.open), ("proven open", &doors.proven)] {
        if typed_open_code(result).as_deref() != Some(code) {
            return Err(format!(
                "{what}: {door} answered {:?}, expected typed {code}",
                result.as_ref().map(|_| "served")
            )
            .into());
        }
    }
    Ok(())
}

/// The files of a generation's `text-authority/` tree: its manifest and
/// its shards, as the seal committed them.
fn text_authority_files(generation_dir: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(generation_dir.join(TEXT_AUTHORITY_DIR))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()?;
    files.sort();
    if files.len() < 2 {
        return Err(format!("expected a manifest and at least one shard, got {files:?}").into());
    }
    Ok(files)
}

/// The stale generation's counterpart of one text-authority file.
///
/// Its manifest for the manifest, its one shard for a shard. Shard files
/// are named by content, so the counterpart is found by kind, not by name.
fn stale_counterpart(stale_dir: &Path, path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let is_manifest =
        path.file_name().and_then(|name| name.to_str()) == Some(TEXT_AUTHORITY_MANIFEST);
    let mut candidates: Vec<PathBuf> = text_authority_files(stale_dir)?
        .into_iter()
        .filter(|stale| {
            (stale.file_name().and_then(|name| name.to_str()) == Some(TEXT_AUTHORITY_MANIFEST))
                == is_manifest
        })
        .collect();
    let [counterpart] = candidates.as_mut_slice() else {
        return Err(format!(
            "expected one stale counterpart for {}, got {candidates:?}",
            path.display()
        )
        .into());
    };
    Ok(counterpart.clone())
}

/// Every text-authority file, under every corruption, is refused by both
/// doors and admitted again once restored.
#[test]
fn both_doors_refuse_a_sidecar_that_does_not_match_the_manifest() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    let stale_generation = ManifestGeneration::new(2);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    adapter.build_batch(&sealed_batch(
        stale_generation,
        "fn two() { sealed_needle other }",
    )?)?;
    expect_admitted(&knock(&adapter, generation), "intact")?;

    let dir = generation_dir(&root, generation);
    let stale_dir = generation_dir(&root, stale_generation);
    for path in text_authority_files(&dir)? {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or("name")?;
        let original = std::fs::read(&path)?;

        std::fs::remove_file(&path)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} missing"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;

        let half = original.len().div_euclid(2);
        std::fs::write(
            &path,
            original.get(..half).ok_or("sidecar shorter than half")?,
        )?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} truncated"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;

        let mut flipped = original.clone();
        let middle = flipped.len().div_euclid(2);
        if let Some(byte) = flipped.get_mut(middle) {
            *byte ^= 0x01;
        }
        std::fs::write(&path, &flipped)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} bit-flipped"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;

        let stale = std::fs::read(stale_counterpart(&stale_dir, &path)?)?;
        std::fs::write(&path, &stale)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} stale copy"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;

        std::fs::write(&path, &original)?;
        expect_admitted(&knock(&adapter, generation), &format!("{name} restored"))?;
    }
    Ok(())
}

/// The Tantivy commit is part of the commitment: a `meta.json` other than
/// the sealed one is refused even though Tantivy itself would open it.
#[test]
fn both_doors_refuse_an_index_commit_other_than_the_sealed_one() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    let meta = generation_dir(&root, generation).join(TANTIVY_META);
    let original = std::fs::read(&meta)?;
    // Same JSON, different bytes: whitespace Tantivy tolerates, the manifest does not.
    let mut altered = original.clone();
    altered.push(b'\n');
    std::fs::write(&meta, &altered)?;
    expect_refused(
        &knock(&adapter, generation),
        "meta.json altered",
        "GENERATION_SIDECAR_CORRUPT",
    )?;
    std::fs::write(&meta, &original)?;
    expect_admitted(&knock(&adapter, generation), "meta.json restored")
}

/// A sealed identity without its manifest is not a sealed generation this
/// adapter will serve: the manifest is missing, and that is a typed refusal
/// with a migration message, not a silent "no sidecars".
#[test]
fn a_sealed_identity_without_a_manifest_is_refused_by_both_doors() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    let manifest = generation_dir(&root, generation).join(MANIFEST);
    let original = std::fs::read(&manifest)?;
    std::fs::remove_file(&manifest)?;
    expect_refused(
        &knock(&adapter, generation),
        "manifest missing",
        "GENERATION_MANIFEST_MISSING",
    )?;
    std::fs::write(&manifest, &original)?;
    expect_admitted(&knock(&adapter, generation), "manifest restored")
}

/// The identity and the manifest bind each other: an identity rewritten
/// for a different digest no longer matches the manifest that was sealed
/// with it.
#[test]
fn an_identity_that_does_not_match_the_manifest_is_refused() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    let dir = generation_dir(&root, generation);
    let identity_path = dir.join(IDENTITY);
    let original = std::fs::read(&identity_path)?;
    let mut forged = identity(generation);
    forged.manifest_digest = "manifest-digest:forged".to_string();
    let mut bytes = Vec::new();
    ciborium::into_writer(&forged, &mut bytes)?;
    std::fs::write(&identity_path, &bytes)?;
    // The validator sees the identity disagree with its candidate first; the
    // open, which has no candidate digest, is caught by the manifest binding.
    let doors = knock(&adapter, generation);
    if typed_code(&doors.validate).as_deref() != Some("GENERATION_IDENTITY_DIGEST_MISMATCH") {
        return Err(format!("validator answered {:?}", doors.validate).into());
    }
    if typed_open_code(&doors.open).as_deref() != Some("GENERATION_IDENTITY_DIGEST_MISMATCH") {
        return Err(format!("open answered {:?}", doors.open.as_ref().map(|_| "served")).into());
    }
    if typed_open_code(&doors.proven).as_deref() != Some("GENERATION_IDENTITY_DIGEST_MISMATCH") {
        return Err(format!(
            "proven open answered {:?}",
            doors.proven.as_ref().map(|_| "served")
        )
        .into());
    }
    std::fs::write(&identity_path, &original)?;
    expect_admitted(&knock(&adapter, generation), "identity restored")
}

/// Every overlay sidecar, under every corruption, is refused by both doors
/// and admitted again once restored (QI-BB-030, the audit's crash scenario).
///
/// The scenario the audit reproduced: a publish that crashes mid-write
/// leaves `repo-meta.cbor` truncated, activation and restart validate the
/// generation, and the first query fails decoding the overlay. Here the
/// truncation is injected on the real file and the validator must refuse it
/// exactly as the open does.
#[test]
fn both_doors_refuse_an_overlay_that_does_not_match_the_manifest() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    let stale_generation = ManifestGeneration::new(2);
    sealed_generation_with_overlays(&adapter, generation, "one", "fn one() { sealed_needle }")?;
    sealed_generation_with_overlays(
        &adapter,
        stale_generation,
        "two",
        "fn two() { sealed_needle other }",
    )?;
    expect_admitted(&knock(&adapter, generation), "intact")?;

    let dir = generation_dir(&root, generation);
    let stale_dir = generation_dir(&root, stale_generation);
    for name in OVERLAY_FILES {
        let path = dir.join(name);
        let original = std::fs::read(&path)
            .map_err(|err| format!("{name} was not written before the seal: {err}"))?;

        std::fs::remove_file(&path)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} missing"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;

        std::fs::write(&path, b"")?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} truncated to zero bytes"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;

        let half = original.len().div_euclid(2);
        std::fs::write(
            &path,
            original.get(..half).ok_or("overlay shorter than half")?,
        )?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} truncated"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;

        let mut flipped = original.clone();
        let middle = flipped.len().div_euclid(2);
        if let Some(byte) = flipped.get_mut(middle) {
            *byte ^= 0x01;
        }
        std::fs::write(&path, &flipped)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} bit-flipped"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;

        let stale = std::fs::read(stale_dir.join(name))?;
        if stale == original {
            return Err(format!("{name}: the stale fixture is byte-identical").into());
        }
        std::fs::write(&path, &stale)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} stale copy"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;

        std::fs::write(&path, &original)?;
        expect_admitted(&knock(&adapter, generation), &format!("{name} restored"))?;
    }
    Ok(())
}

#[test]
fn both_doors_refuse_missing_changed_or_uncommitted_ranked_keys() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    let dir = generation_dir(&root, generation);
    let entries = std::fs::read_dir(&dir)?.collect::<Result<Vec<_>, _>>()?;
    let tables: Vec<_> = entries
        .into_iter()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("ranked-keys-")
        })
        .map(|entry| entry.path())
        .collect();
    if tables.len() != 1 {
        return Err(format!("expected one ranked-key table, found {}", tables.len()).into());
    }
    let path = tables.first().ok_or("ranked-key table missing")?;
    let original = std::fs::read(path)?;
    expect_admitted(&knock(&adapter, generation), "intact ranked keys")?;
    std::fs::remove_file(path)?;
    expect_refused(
        &knock(&adapter, generation),
        "missing ranked keys",
        "GENERATION_SIDECAR_CORRUPT",
    )?;
    let shortened = original
        .get(
            ..original
                .len()
                .checked_sub(1)
                .ok_or("empty ranked-key table")?,
        )
        .ok_or("ranked-key truncation range missing")?;
    std::fs::write(path, shortened)?;
    expect_refused(
        &knock(&adapter, generation),
        "truncated ranked keys",
        "GENERATION_SIDECAR_CORRUPT",
    )?;
    let mut flipped = original.clone();
    *flipped.get_mut(40).ok_or("ranked-key payload missing")? ^= 1;
    std::fs::write(path, &flipped)?;
    expect_refused(
        &knock(&adapter, generation),
        "changed ranked keys",
        "GENERATION_SIDECAR_CORRUPT",
    )?;
    // Change the fixture's only source-repository key without changing the
    // table length, offsets, UTF-8, term count, or key order. Structural
    // validation alone admits this table; the sealed digest must reject it.
    let repo_id = repo();
    let repo_bytes = repo_id.as_str().as_bytes();
    let repo_positions: Vec<usize> = original
        .windows(repo_bytes.len())
        .enumerate()
        .filter_map(|(offset, key)| (key == repo_bytes).then_some(offset))
        .collect();
    let [repo_offset] = repo_positions.as_slice() else {
        return Err(format!(
            "expected exactly one fixture repository key, found {repo_positions:?}"
        )
        .into());
    };
    let mut changed_key = original.clone();
    let first = changed_key
        .get_mut(*repo_offset)
        .ok_or("fixture repository key missing")?;
    if *first != b'm' {
        return Err("fixture repository key does not start with m".into());
    }
    *first = b'n';
    std::fs::write(path, &changed_key)?;
    expect_refused(
        &knock(&adapter, generation),
        "same-length structurally valid ranked key substitution",
        "GENERATION_SIDECAR_CORRUPT",
    )?;
    std::fs::write(path, &original)?;
    expect_admitted(&knock(&adapter, generation), "restored ranked keys")?;
    let extra = dir.join("ranked-keys-ffffffffffffffffffffffffffffffff.bin");
    std::fs::write(&extra, &original)?;
    expect_refused(
        &knock(&adapter, generation),
        "uncommitted ranked keys",
        "GENERATION_SIDECAR_CORRUPT",
    )?;
    std::fs::remove_file(extra)?;
    expect_admitted(
        &knock(&adapter, generation),
        "uncommitted ranked keys removed",
    )?;
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStringExt;

        let invalid_name = std::ffi::OsString::from_vec(b"ranked-keys-\xff.bin".to_vec());
        let invalid_extra = dir.join(invalid_name);
        std::fs::write(&invalid_extra, &original)?;
        expect_refused(
            &knock(&adapter, generation),
            "uncommitted non-UTF-8 ranked key",
            "GENERATION_SIDECAR_CORRUPT",
        )?;
        std::fs::remove_file(invalid_extra)?;
        expect_admitted(
            &knock(&adapter, generation),
            "uncommitted non-UTF-8 ranked key removed",
        )?;
    }
    Ok(())
}

/// An overlay file the seal did not commit to is refused when it appears:
/// a generation sealed without an overlay family says so in its manifest,
/// and a file that shows up later is not silently decoded.
#[test]
fn both_doors_refuse_an_overlay_the_seal_did_not_commit_to() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    let donor = ManifestGeneration::new(2);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    sealed_generation_with_overlays(&adapter, donor, "donor", "fn two() { sealed_needle }")?;
    expect_admitted(&knock(&adapter, generation), "intact")?;
    let dir = generation_dir(&root, generation);
    let donor_dir = generation_dir(&root, donor);
    for name in OVERLAY_FILES {
        let path = dir.join(name);
        if path.exists() {
            return Err(format!("{name} exists although no overlay was published").into());
        }
        let _copied: u64 = std::fs::copy(donor_dir.join(name), &path)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} appeared after the seal"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;
        std::fs::remove_file(&path)?;
        expect_admitted(
            &knock(&adapter, generation),
            &format!("{name} removed again"),
        )?;
    }
    Ok(())
}

/// After the seal nothing may change the generation.
///
/// An index-mutating op and every overlay publish are refused as
/// immutable, the directory is byte for byte what the seal measured, and
/// both doors still admit it. Before this, the overlay publishes landed in the sealed directory with
/// plain writes the manifest never covered, so a crash mid-publish left a
/// sealed generation that activation admitted and a query could not open.
#[test]
fn a_sealed_generation_refuses_every_mutation_and_keeps_its_bytes() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    sealed_generation_with_overlays(&adapter, generation, "one", "fn one() { sealed_needle }")?;
    let dir = generation_dir(&root, generation);
    let sealed_files = top_level_files(&dir)?;

    let mutation = LexicalChannelOp::UpsertChunk(UpsertChunk {
        repo_id: repo(),
        revision_id: revision(),
        generation,
        chunk_id: ChunkId::new("late"),
        payload: Vec::new(),
    });
    match adapter.build(&repo(), &revision(), generation, &[mutation]) {
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationImmutable,
            ..
        }) => {}
        other => return Err(format!("index mutation after seal answered {other:?}").into()),
    }
    for outcome in publish_overlays(&adapter, generation, "late")? {
        match outcome.result {
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationImmutable,
                ..
            }) => {}
            other => {
                return Err(format!(
                    "{} publish after the seal answered {other:?}, expected typed GENERATION_IMMUTABLE",
                    outcome.what
                )
                .into());
            }
        }
    }
    let files_now = top_level_files(&dir)?;
    if files_now != sealed_files {
        let names: Vec<&String> = files_now.iter().map(|(name, _)| name).collect();
        return Err(format!("refused publishes changed the sealed directory: {names:?}").into());
    }
    expect_admitted(&knock(&adapter, generation), "after refused mutations")
}

/// The segment component files the sealed commit references: every
/// top-level file named `<segment id>.<component>`, sorted.
fn segment_files(generation_dir: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(generation_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some((stem, _extension)) = name.split_once('.') else {
            continue;
        };
        if stem.len() == 32 && stem.chars().all(|ch| ch.is_ascii_hexdigit()) {
            files.push(entry.path());
        }
    }
    files.sort();
    if files.len() < 6 {
        return Err(format!("expected at least six segment component files, got {files:?}").into());
    }
    Ok(files)
}

/// Every segment file the commit references is proved by length at the
/// doors.
///
/// A length change or missing file is refused by every door. A same-length
/// flip may be caught early when the ranked-key verifier opens a fast field;
/// other components remain the scrub's to hash (next test).
#[test]
fn segment_files_are_length_proved_at_the_doors() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    expect_admitted(&knock(&adapter, generation), "intact")?;
    let dir = generation_dir(&root, generation);
    for path in segment_files(&dir)? {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or("name")?;
        let original = std::fs::read(&path)?;

        std::fs::remove_file(&path)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} missing"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;

        let mut longer = original.clone();
        longer.push(0);
        std::fs::write(&path, &longer)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} one byte longer"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;

        let half = original.len().div_euclid(2);
        std::fs::write(
            &path,
            original.get(..half).ok_or("segment shorter than half")?,
        )?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} truncated"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;

        let mut flipped = original.clone();
        let middle = flipped.len().div_euclid(2);
        if let Some(byte) = flipped.get_mut(middle) {
            *byte ^= 0x01;
        }
        std::fs::write(&path, &flipped)?;
        match adapter.validate_generation_identity(&identity(generation)) {
            Ok(())
            | Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            }) => {}
            Err(err) => return Err(format!("{name} flipped: unexpected verdict: {err}").into()),
        }

        std::fs::write(&path, &original)?;
        expect_admitted(&knock(&adapter, generation), &format!("{name} restored"))?;
    }
    Ok(())
}

/// A same-length flip the doors admit is found by the scrub, which
/// quarantines the generation durably (QI-BB-017, QI-BB-026).
///
/// After the corrupt step every door refuses the generation typed
/// `GENERATION_QUARANTINED`, the inventory lists it as
/// `GENERATION_QUARANTINE_CONTENT_CORRUPT` at its own directory instead of
/// seeding it, the scrub no longer offers it, and scrubbing it again is
/// refused — restoring the bytes does not lift the quarantine; discarding
/// the directory does.
#[test]
fn a_same_length_flip_is_found_by_the_scrub_and_quarantines_the_generation() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    let dir = generation_dir(&root, generation);
    let segment = segment_files(&dir)?
        .into_iter()
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "store")
        })
        .ok_or("a sealed generation has a stored-field segment component")?;
    let original = std::fs::read(&segment)?;
    let mut flipped = original.clone();
    let middle = flipped.len().div_euclid(2);
    let byte = flipped.get_mut(middle).ok_or("segment is empty")?;
    *byte ^= 0x01;
    std::fs::write(&segment, &flipped)?;
    adapter.validate_generation_identity(&identity(generation))?;

    let report = adapter.scrub(
        &identity(generation),
        None,
        IntegrityScrubBudgetV1 {
            max_bytes: u64::MAX,
        },
    )?;
    let IntegrityScrubOutcomeV1::Corrupt { quarantined } = &report.outcome else {
        return Err(format!("the scrub admitted a flipped segment: {report:?}").into());
    };
    if quarantined.reason != GenerationQuarantineReasonV1::ContentCorrupt
        || quarantined.track != SearchPlaneTrackKind::Lexical
        || quarantined.path != dir
    {
        return Err(format!("the quarantine names the wrong entry: {quarantined:?}").into());
    }

    std::fs::write(&segment, &original)?;
    expect_refused(
        &knock(&adapter, generation),
        "quarantined, bytes restored",
        "GENERATION_QUARANTINED",
    )?;
    let inventory = adapter.inventory_sealed_generations()?;
    if !inventory.sealed.is_empty()
        || !inventory.quarantined.iter().any(|entry| {
            entry.path == dir && entry.reason == GenerationQuarantineReasonV1::ContentCorrupt
        })
    {
        return Err(
            format!("the inventory did not quarantine the generation: {inventory:?}").into(),
        );
    }
    if !adapter.scrub_candidates()?.is_empty() {
        return Err("a quarantined generation is still offered to the scrub".into());
    }
    match adapter.scrub(
        &identity(generation),
        None,
        IntegrityScrubBudgetV1 {
            max_bytes: u64::MAX,
        },
    ) {
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationQuarantined,
            ..
        }) => Ok(()),
        other => Err(format!("scrubbing a quarantined generation answered {other:?}").into()),
    }
}

/// Damage one committed file the way the doors can see: a decoded sidecar
/// rewritten at its length, a segment file one byte short.
fn damage(path: &Path) -> Result<Vec<u8>, Box<dyn Error>> {
    let original = std::fs::read(path)?;
    let mut damaged = original.clone();
    if path
        .extension()
        .is_some_and(|extension| extension == "cbor")
    {
        let last = damaged.len().checked_sub(1).ok_or("empty sidecar")?;
        let byte = damaged.get_mut(last).ok_or("index")?;
        *byte ^= 0xff;
    } else {
        let _dropped = damaged.pop().ok_or("empty segment file")?;
    }
    std::fs::write(path, &damaged)?;
    Ok(original)
}

/// A door's content verdict is recorded only by the adapter's re-proof
/// (QI-BB-026).
///
/// The doors are reads. A committed file that no longer matches the seal
/// is refused `GENERATION_SIDECAR_CORRUPT` by every door, and no door
/// writes: the inventory still lists the generation sealed, and restoring
/// the file admits it again. The re-proof activation and rollback ask for
/// admits an intact generation and records nothing, refuses an identity
/// the directory does not hold, and on a damaged generation writes the
/// content-corrupt receipt: the inventory lists it quarantined at its own
/// directory, every door refuses it `GENERATION_QUARANTINED` once the bytes
/// are restored, asking again answers the same entry, and the quarantine
/// discard removes it. Both kinds of damage a door sees are covered: a
/// decoded sidecar and a segment file.
#[test]
fn a_door_finding_is_quarantined_only_by_the_adapters_re_proof() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    let dir = generation_dir(&root, generation);
    let damaged_files: [LocateFile; 2] = [
        |dir| Ok(dir.join(TEXT_AUTHORITY_DIR).join(TEXT_AUTHORITY_MANIFEST)),
        |dir| {
            segment_files(dir)?
                .into_iter()
                .next()
                .ok_or_else(|| "a sealed generation has segment files".into())
        },
    ];
    for damaged_file in damaged_files {
        adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
        let file = damaged_file(&dir)?;
        let what = file.display().to_string();

        match adapter.quarantine_door_finding(&identity(generation))? {
            DoorFindingOutcome::NotReproduced => {}
            other @ DoorFindingOutcome::Quarantined { .. } => {
                return Err(
                    format!("{what}: an intact generation was quarantined: {other:?}").into(),
                );
            }
        }
        let mut foreign = identity(generation);
        foreign.manifest_digest = "manifest:another-seal".to_string();
        if let Ok(outcome) = adapter.quarantine_door_finding(&foreign) {
            return Err(format!("{what}: a foreign identity was re-proved: {outcome:?}").into());
        }
        expect_admitted(&knock(&adapter, generation), &format!("{what}: intact"))?;

        let original = damage(&file)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{what}: damaged"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;
        // No door wrote: once the bytes are back the generation is sealed
        // and admitted, which a receipt would forbid.
        std::fs::write(&file, &original)?;
        let inventory = adapter.inventory_sealed_generations()?;
        if inventory.sealed.len() != 1 || !inventory.quarantined.is_empty() {
            return Err(format!("{what}: a door wrote to the inventory: {inventory:?}").into());
        }
        expect_admitted(&knock(&adapter, generation), &format!("{what}: restored"))?;

        let _original = damage(&file)?;
        let DoorFindingOutcome::Quarantined { quarantined } =
            adapter.quarantine_door_finding(&identity(generation))?
        else {
            return Err(format!("{what}: the re-proof admitted a damaged generation").into());
        };
        if quarantined.reason != GenerationQuarantineReasonV1::ContentCorrupt
            || quarantined.track != SearchPlaneTrackKind::Lexical
            || quarantined.path != dir
            || !quarantined.detail.starts_with("a door found ")
        {
            return Err(
                format!("{what}: the quarantine names the wrong entry: {quarantined:?}").into(),
            );
        }
        std::fs::write(&file, &original)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{what}: quarantined, bytes restored"),
            "GENERATION_QUARANTINED",
        )?;
        let inventory = adapter.inventory_sealed_generations()?;
        let [entry] = inventory.quarantined.as_slice() else {
            return Err(
                format!("{what}: the inventory lists one quarantine: {inventory:?}").into(),
            );
        };
        if !inventory.sealed.is_empty()
            || entry.path != dir
            || entry.reason != GenerationQuarantineReasonV1::ContentCorrupt
        {
            return Err(
                format!("{what}: the inventory did not quarantine it: {inventory:?}").into(),
            );
        }
        match adapter.quarantine_door_finding(&identity(generation))? {
            DoorFindingOutcome::Quarantined { quarantined: again }
                if again.path == dir
                    && again.reason == GenerationQuarantineReasonV1::ContentCorrupt => {}
            other
            @ (DoorFindingOutcome::Quarantined { .. } | DoorFindingOutcome::NotReproduced) => {
                return Err(format!("{what}: asking again answered {other:?}").into());
            }
        }
        match adapter.discard_quarantined_generation(entry)? {
            QuarantineDiscardOutcomeV1::Discarded { .. } if !dir.exists() => {}
            other @ (QuarantineDiscardOutcomeV1::Discarded { .. }
            | QuarantineDiscardOutcomeV1::Absent) => {
                return Err(format!("{what}: the discard answered {other:?}").into());
            }
        }
    }
    Ok(())
}

/// An intact generation scrubs in bounded, resumable steps (QI-BB-017).
///
/// With a one-byte budget each step hashes exactly one committed file and
/// pauses at the next index, so the pass takes one step per committed file
/// and the cursor walks them in order; the completed pass is recorded, the
/// scrub then reports when, and the doors still admit the generation.
#[test]
fn an_intact_generation_scrubs_in_bounded_resumable_steps() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(temp.path().to_path_buf());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let candidates = adapter.scrub_candidates()?;
    if candidates.len() != 1 || candidates.iter().any(|c| c.last_completed_unix.is_some()) {
        return Err(format!("a fresh seal is one never-scrubbed candidate: {candidates:?}").into());
    }

    let mut cursor: Option<IntegrityScrubCursorV1> = None;
    let mut steps = 0_u64;
    let mut files = 0_u64;
    loop {
        let report = adapter.scrub(
            &identity(generation),
            cursor,
            IntegrityScrubBudgetV1 { max_bytes: 1 },
        )?;
        steps = steps.saturating_add(1);
        files = files.saturating_add(report.files_verified);
        if report.files_verified != 1 {
            return Err(format!("a one-byte step hashes exactly one file: {report:?}").into());
        }
        match report.outcome {
            IntegrityScrubOutcomeV1::Paused { cursor: next } => {
                if next.next_artifact != steps {
                    return Err(format!("step {steps} paused at {next:?}").into());
                }
                cursor = Some(next);
            }
            IntegrityScrubOutcomeV1::Completed => break,
            IntegrityScrubOutcomeV1::Corrupt { quarantined } => {
                return Err(
                    format!("an intact generation was quarantined: {quarantined:?}").into(),
                );
            }
        }
    }
    if files != steps || files < 3 {
        return Err(format!("{steps} steps hashed {files} files").into());
    }
    let completed = adapter
        .scrub_candidates()?
        .into_iter()
        .next()
        .and_then(|candidate| candidate.last_completed_unix)
        .ok_or("the completed pass is recorded")?;
    if completed < before {
        return Err(format!(
            "the pass completed at {completed}, before the test began at {before}"
        )
        .into());
    }
    expect_admitted(&knock(&adapter, generation), "after the scrub")
}

/// A scrub resumed over a reclaimed generation meets it gone, typed.
///
/// The generation is reclaimed between two steps of one pass. The next
/// step is refused `NotFound` — never a corruption verdict over files that
/// were removed on purpose — and leaves nothing behind: no directory
/// recreated for a receipt, nothing quarantined, no candidate left.
#[test]
fn a_scrub_resumed_over_a_reclaimed_generation_is_refused_not_quarantined() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    let one_byte = IntegrityScrubBudgetV1 { max_bytes: 1 };
    let first = adapter.scrub(&identity(generation), None, one_byte)?;
    let IntegrityScrubOutcomeV1::Paused { cursor } = first.outcome else {
        return Err(format!("a one-byte first step pauses: {first:?}").into());
    };
    let reclaimed = adapter.reclaim_sealed_generation(&identity(generation))?;
    if !matches!(
        reclaimed,
        SealedGenerationReclaimOutcomeV1::Reclaimed { .. }
    ) {
        return Err(format!("the sealed generation is reclaimed: {reclaimed:?}").into());
    }
    match adapter.scrub(&identity(generation), Some(cursor), one_byte) {
        Err(CoreError::NotFound(_)) => {}
        other => {
            return Err(format!("a reclaimed generation is refused NotFound: {other:?}").into());
        }
    }
    if generation_dir(&root, generation).exists() {
        return Err("the refused step recreated the reclaimed directory".into());
    }
    let inventory = adapter.inventory_sealed_generations()?;
    if !inventory.sealed.is_empty() || !inventory.quarantined.is_empty() {
        return Err(format!("nothing is left to list: {inventory:?}").into());
    }
    if !adapter.scrub_candidates()?.is_empty() {
        return Err("a reclaimed generation is no scrub candidate".into());
    }
    Ok(())
}

/// The length of every regular file under `dir`, summed by the test's own
/// walk (a lexical generation holds no hard links).
fn tree_file_bytes(dir: &Path) -> Result<u64, Box<dyn Error>> {
    let mut total = 0_u64;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                total = total
                    .checked_add(entry.metadata()?.len())
                    .ok_or("tree bytes overflow")?;
            }
        }
    }
    Ok(total)
}

/// A reclaim is crash-atomic (QI-BB-003 보완 #3, #4).
///
/// A whole reclaim leaves neither the generation nor a reclaim-area entry.
/// What a crash between the move and the removal leaves — the whole
/// generation under the reclaim area — is out of the generation namespace:
/// not listed, not quarantined, no scrub candidate, and a retried reclaim
/// finds it `Absent`. Finishing the interrupted reclaims removes it and
/// reports exactly its bytes, once.
#[test]
fn an_interrupted_reclaim_is_out_of_the_namespace_and_finished_once() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let (first, second) = (ManifestGeneration::new(1), ManifestGeneration::new(2));
    adapter.build_batch(&sealed_batch(first, "fn one() { sealed_needle }")?)?;
    adapter.build_batch(&sealed_batch(second, "fn two() { sealed_needle }")?)?;
    let reclaimed = adapter.reclaim_sealed_generation(&identity(first))?;
    if !matches!(
        reclaimed,
        SealedGenerationReclaimOutcomeV1::Reclaimed { .. }
    ) {
        return Err(format!("the first generation is reclaimed: {reclaimed:?}").into());
    }
    let area = root.join(RECLAIM_AREA_DIR_NAME);
    if generation_dir(&root, first).exists() || std::fs::read_dir(&area)?.next().is_some() {
        return Err("a whole reclaim leaves neither the generation nor an area entry".into());
    }
    // The crash: the second generation moved into the area, never removed.
    let entry = area.join(
        GenerationStorageKeyV1::for_repo_revision(&repo(), &revision()).reclaim_entry_name(second),
    );
    let left_bytes = tree_file_bytes(&generation_dir(&root, second))?;
    std::fs::rename(generation_dir(&root, second), &entry)?;
    let inventory = adapter.inventory_sealed_generations()?;
    if !inventory.sealed.is_empty() || !inventory.quarantined.is_empty() {
        return Err(format!("the reclaim area is not a generation family: {inventory:?}").into());
    }
    if !adapter.scrub_candidates()?.is_empty() {
        return Err("an interrupted reclaim is no scrub candidate".into());
    }
    let retried = adapter.reclaim_sealed_generation(&identity(second))?;
    if retried != SealedGenerationReclaimOutcomeV1::Absent {
        return Err(format!("a retried reclaim finds the generation gone: {retried:?}").into());
    }
    let finished = adapter.finish_interrupted_reclaims()?;
    let expected = FinishedReclaims {
        entries: 1,
        bytes: left_bytes,
    };
    if finished != expected || entry.exists() {
        return Err(format!("finishing removes the entry, {expected:?}: {finished:?}").into());
    }
    if adapter.finish_interrupted_reclaims()? != FinishedReclaims::default() {
        return Err("a second finish has nothing to do".into());
    }
    Ok(())
}

/// A durable overlay publish leaves no torn file behind: the file is the
/// batch's whole snapshot, and a temporary a crashed publish left is not
/// what the seal commits to.
#[test]
fn an_interrupted_overlay_publish_leaves_nothing_the_seal_commits_to() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    expect_overlays_landed(publish_overlays(&adapter, generation, "one")?)?;
    let dir = generation_dir(&root, generation);
    // The crash: a temporary the durable write had opened but never renamed.
    let leftover = dir.join(".repo-meta.cbor.tmp-99999-7");
    std::fs::write(&leftover, b"torn")?;
    let published = std::fs::read(dir.join("repo-meta.cbor"))?;
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    if leftover.exists() {
        return Err("the seal left a crashed publish's temporary in the sealed directory".into());
    }
    if std::fs::read(dir.join("repo-meta.cbor"))? != published {
        return Err("the seal changed a published overlay".into());
    }
    let names: Vec<String> = top_level_files(&dir)?
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    if names.iter().any(|name| name.contains(".tmp-")) {
        return Err(format!("a temporary survived the seal: {names:?}").into());
    }
    expect_admitted(
        &knock(&adapter, generation),
        "sealed after a crashed publish",
    )
}

/// A generation that indexed nothing still seals to an openable state: the
/// seal creates the empty index it commits to, and the manifest says
/// explicitly that there is no text authority.
#[test]
fn a_generation_that_indexed_nothing_seals_openable() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(temp.path().to_path_buf());
    let generation = ManifestGeneration::new(1);
    let mut empty = sealed_batch(generation, "unused")?;
    empty.replace_scopes.clear();
    current_source_fixture::finish_batch(&mut empty)?;
    adapter.build_batch(&empty)?;
    let doors = knock(&adapter, generation);
    if let Err(err) = doors.validate {
        return Err(format!("validator refused an empty sealed generation: {err}").into());
    }
    for (door, result) in [("open", doors.open), ("proven open", doors.proven)] {
        match result {
            Ok(0) => {}
            Ok(hits) => return Err(format!("{door}: empty generation served {hits} hits").into()),
            Err(err) => {
                return Err(format!("{door} refused an empty sealed generation: {err}").into());
            }
        }
    }
    Ok(())
}
