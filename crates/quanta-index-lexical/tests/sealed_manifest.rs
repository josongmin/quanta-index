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
    GenerationQuarantineReasonV1, GenerationStorageKeyV1, IncompleteGenerationDiscardPort,
    IntegrityScrubBudgetV1, IntegrityScrubCursorV1, IntegrityScrubOutcomeV1, IntegrityScrubPort,
    LexicalIndexBuildPort, LexicalIndexOpenPort, QuarantineDiscardOutcomeV1,
    QuarantinedGenerationDiscardPort, RECLAIM_AREA_DIR_NAME, RepoCommitRecencyIngestPort,
    RepoDescriptionIngestPort, RepoMetaIngestPort, RepoTopicIngestPort, RequestBudgetV1,
    SealedGenerationIdentityProbePort, SealedGenerationReclaimOutcomeV1,
    SealedGenerationReclaimPort, SealedGenerationScanPort, SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;
use sha2::{Digest as _, Sha256};

#[path = "support/current_source_fixture.rs"]
mod current_source_fixture;

type TestResult = Result<(), Box<dyn Error>>;

/// Where, inside a generation directory, a test damages a file.
type LocateFile = fn(&Path) -> Result<PathBuf, Box<dyn Error>>;

const TEXT_AUTHORITY_DIR: &str = "text-authority";
const TEXT_AUTHORITY_MANIFEST: &str = "manifest.cbor";
const MANIFEST: &str = "search-corpus-generation-manifest.cbor";
const IDENTITY: &str = "search-corpus-generation-identity.cbor";
const QUARANTINE_RECEIPT: &str = "search-corpus-generation-quarantine.cbor";
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

#[test]
fn point_inventory_matches_full_inventory_for_digest_and_family_symlink() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("lexical");
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn point_inventory() {}")?)?;
    let exact = identity(generation);
    if !adapter.inventory_sealed_generation_identity(&exact)? {
        return Err("point inventory missed an intact sealed generation".into());
    }
    let mut wrong = exact.clone();
    wrong.manifest_digest.push_str("-wrong");
    if adapter.inventory_sealed_generation_identity(&wrong)? {
        return Err("point inventory admitted a different digest".into());
    }
    let family = generation_dir(&root, generation)
        .parent()
        .ok_or("generation has no family")?
        .to_path_buf();
    let moved = temp.path().join("moved-family");
    std::fs::rename(&family, &moved)?;
    std::os::unix::fs::symlink(&moved, &family)?;
    if adapter.inventory_sealed_generation_identity(&exact)?
        || !adapter.inventory_sealed_generations()?.sealed.is_empty()
    {
        return Err("point or full inventory followed a family symlink".into());
    }
    Ok(())
}

#[test]
fn point_inventory_reports_control_file_read_failures() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;

    let temp = tempfile::tempdir()?;
    let root = temp.path().join("lexical");
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn point_inventory_io() {}")?)?;
    if !adapter.inventory_sealed_generation_identity(&identity(generation))?
        || adapter.inventory_sealed_generations()?.sealed.len() != 1
    {
        return Err("healthy lexical generation was not inventoried before fault injection".into());
    }
    let dir = generation_dir(&root, generation);
    for name in [IDENTITY, QUARANTINE_RECEIPT] {
        let path = dir.join(name);
        if name == QUARANTINE_RECEIPT {
            std::fs::write(&path, [0xff])?;
        }
        let mode = std::fs::metadata(&path)?.permissions().mode();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))?;
        let inaccessible = std::fs::File::open(&path).is_err();
        let point = adapter.inventory_sealed_generation_identity(&identity(generation));
        let boot = adapter.inventory_sealed_generations();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))?;
        if name == QUARANTINE_RECEIPT {
            std::fs::remove_file(&path)?;
        }
        if !inaccessible {
            return Err(format!(
                "test process can read mode-000 {name}; I/O failure was not injected"
            )
            .into());
        }
        if !matches!(&point, Err(CoreError::Storage(_)))
            || !matches!(&boot, Err(CoreError::Storage(_)))
        {
            return Err(
                format!("{name} I/O failure was hidden: point={point:?}, boot={boot:?}").into(),
            );
        }
    }
    if !adapter.inventory_sealed_generation_identity(&identity(generation))?
        || adapter.inventory_sealed_generations()?.sealed.len() != 1
    {
        return Err("healthy lexical generation was not inventoried after fault injection".into());
    }
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
        if name == "repo-meta.cbor" {
            std::fs::write(&path, vec![0xff; original.len()])?;
            match adapter.validate_generation_identity(&identity(generation)) {
                Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                    message,
                }) if message.contains("content digest differs from the committed digest") => {}
                other => {
                    return Err(format!(
                        "overlay decoded before its digest was checked: {other:?}"
                    )
                    .into());
                }
            }
        }

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
fn both_doors_refuse_trailing_bytes_after_a_valid_sealed_manifest() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    let path = generation_dir(&root, generation).join(MANIFEST);
    let mut bytes = std::fs::read(&path)?;
    bytes.push(0xff);
    std::fs::write(&path, bytes)?;
    expect_refused(
        &knock(&adapter, generation),
        "trailing sealed manifest bytes",
        "GENERATION_SIDECAR_CORRUPT",
    )
}

#[test]
fn full_bundle_preflight_rejects_trailing_cbor_before_generation_preparation() -> TestResult {
    use quanta_index_core::SearchCorpusPreflightPhaseV1;

    let temp = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(temp.path().to_path_buf());
    let mut batch = sealed_batch(ManifestGeneration::new(1), "fn one() { sealed_needle }")?;
    let mut payload = repo_metadata_bundle_payload("production")?;
    payload.push(0xff);
    batch.bundle_payload = Some(payload);
    current_source_fixture::finish_batch(&mut batch)?;
    match adapter.preflight_batch(&batch, SearchCorpusPreflightPhaseV1::BeforeIntent) {
        Err(CoreError::InvalidContract(message)) if message.contains("trailing CBOR bytes") => {}
        other => return Err(format!("trailing FullBundle payload was admitted: {other:?}").into()),
    }
    if std::fs::read_dir(temp.path())?.next().is_some() {
        return Err("preflight prepared a generation before rejecting the payload".into());
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

#[cfg(unix)]
#[test]
fn both_doors_refuse_uncommitted_dangling_overlay_links() -> TestResult {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    let dir = generation_dir(&root, generation);
    for name in OVERLAY_FILES {
        let path = dir.join(name);
        symlink("missing-overlay-target", &path)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("uncommitted dangling overlay {name}"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;
        std::fs::remove_file(path)?;
    }
    expect_admitted(&knock(&adapter, generation), "dangling overlays removed")
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

#[test]
fn a_same_length_segment_redirect_is_refused_and_quarantined() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(temp.path().to_path_buf());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    let dir = generation_dir(temp.path(), generation);
    let segment = segment_files(&dir)?
        .into_iter()
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "store")
        })
        .ok_or("missing store component")?;
    let outside = temp.path().join("outside.store");
    let _copied = std::fs::copy(&segment, &outside)?;
    std::fs::remove_file(&segment)?;
    std::os::unix::fs::symlink(&outside, &segment)?;
    expect_refused(
        &knock(&adapter, generation),
        "redirected committed segment",
        "GENERATION_SIDECAR_CORRUPT",
    )?;
    let fenced = std::cell::Cell::new(false);
    let report = adapter.scrub_with_quarantine_fence(
        &identity(generation),
        None,
        IntegrityScrubBudgetV1 {
            max_bytes: u64::MAX,
        },
        &|| {
            if dir
                .join("search-corpus-generation-quarantine.cbor")
                .exists()
            {
                return Err(CoreError::Storage(
                    "quarantine receipt preceded the registry fence".into(),
                ));
            }
            fenced.set(true);
            Ok(())
        },
    )?;
    if !fenced.get() {
        return Err("scrub published a quarantine without fencing the registry".into());
    }
    if !matches!(report.outcome, IntegrityScrubOutcomeV1::Corrupt { .. }) {
        return Err(format!("scrub accepted a redirected segment: {report:?}").into());
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

#[test]
fn long_manifest_damage_still_records_a_durable_quarantine() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    let dir = generation_dir(&root, generation);
    let manifest_path = dir.join(MANIFEST);
    let raw = std::fs::read(&manifest_path)?;
    let mut manifest: ciborium::Value = ciborium::from_reader(raw.as_slice())?;
    let ciborium::Value::Array(fields) = &mut manifest else {
        return Err("sealed manifest is not an array".into());
    };
    let Some(ciborium::Value::Array(meta)) = fields.get_mut(3) else {
        return Err("sealed manifest lacks index meta commitment".into());
    };
    let Some(name) = meta.first_mut() else {
        return Err("index meta commitment lacks a name".into());
    };
    *name = ciborium::Value::Text("x".repeat(70_000));
    let mut damaged = Vec::new();
    ciborium::into_writer(&manifest, &mut damaged)?;
    std::fs::write(&manifest_path, damaged)?;

    let original_reason = match adapter.validate_generation_identity(&identity(generation)) {
        Err(
            error @ CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            },
        ) => format!("a door found {error}"),
        other => return Err(format!("damaged manifest was not refused typed: {other:?}").into()),
    };
    let fingerprint = format!("{:x}", Sha256::digest(original_reason.as_bytes()));
    let DoorFindingOutcome::Quarantined { quarantined } =
        adapter.quarantine_door_finding(&identity(generation))?
    else {
        return Err("damaged manifest was not quarantined".into());
    };
    if quarantined.detail.len() > 4 * 1024
        || !quarantined
            .detail
            .contains(&format!("original_sha256={fingerprint}"))
    {
        return Err("quarantine detail was not bounded with its original digest".into());
    }
    let inventory = adapter.inventory_sealed_generations()?;
    let [entry] = inventory.quarantined.as_slice() else {
        return Err(format!("quarantine receipt was not durable: {inventory:?}").into());
    };
    if entry.path != dir {
        return Err(format!("quarantine receipt was not durable: {inventory:?}").into());
    }
    expect_refused(
        &knock(&adapter, generation),
        "long manifest damage after quarantine",
        "GENERATION_QUARANTINED",
    )?;
    Ok(())
}

#[test]
fn oversized_committed_control_files_are_refused_before_read() -> TestResult {
    for (index_meta, excess_bytes, expected_reason) in [
        (
            true,
            16 * 1024 * 1024 + 1,
            "index commit exceeds the control-file byte limit",
        ),
        (
            false,
            268_435_520 + 1,
            "text-authority manifest exceeds its format byte limit",
        ),
    ] {
        let temp = tempfile::tempdir()?;
        let root = temp.path().to_path_buf();
        let adapter = LexicalAdapter::with_state_root(root.clone());
        let generation = ManifestGeneration::new(1);
        adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
        let path = generation_dir(&root, generation).join(MANIFEST);
        let mut manifest: ciborium::Value =
            ciborium::from_reader(std::fs::read(&path)?.as_slice())?;
        let ciborium::Value::Array(fields) = &mut manifest else {
            return Err("sealed manifest is not an array".into());
        };
        let commitment = if index_meta {
            let Some(ciborium::Value::Array(meta)) = fields.get_mut(3) else {
                return Err("sealed manifest lacks index meta commitment".into());
            };
            meta
        } else {
            let Some(ciborium::Value::Array(files)) = fields.get_mut(7) else {
                return Err("sealed manifest lacks text authority commitments".into());
            };
            let manifest_name =
                ciborium::Value::Text(format!("{TEXT_AUTHORITY_DIR}/{TEXT_AUTHORITY_MANIFEST}"));
            let Some(ciborium::Value::Array(manifest_file)) = files.iter_mut().find(|row| {
                matches!(row, ciborium::Value::Array(parts) if parts.first() == Some(&manifest_name))
            }) else {
                return Err("sealed manifest lacks text authority manifest commitment".into());
            };
            manifest_file
        };
        let Some(bytes) = commitment.get_mut(1) else {
            return Err("commitment lacks byte length".into());
        };
        *bytes = ciborium::Value::Integer(excess_bytes.into());
        let mut encoded = Vec::new();
        ciborium::into_writer(&manifest, &mut encoded)?;
        std::fs::write(&path, encoded)?;

        match adapter.validate_generation_identity(&identity(generation)) {
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                message,
            }) if message.contains(expected_reason) => {}
            other => {
                return Err(format!(
                    "oversized commitment did not fail before file read: {other:?}"
                )
                .into());
            }
        }
        expect_refused(
            &knock(&adapter, generation),
            "oversized committed control file",
            "GENERATION_SIDECAR_CORRUPT",
        )?;
    }
    Ok(())
}

#[test]
fn malformed_manifest_is_quarantined_by_door_reproof_and_scrub() -> TestResult {
    for scrub_instead_of_door in [false, true] {
        let temp = tempfile::tempdir()?;
        let root = temp.path().to_path_buf();
        let adapter = LexicalAdapter::with_state_root(root.clone());
        let generation = ManifestGeneration::new(1);
        adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
        let dir = generation_dir(&root, generation);
        let path = dir.join(MANIFEST);
        let original = std::fs::read(&path)?;
        let damaged = if scrub_instead_of_door {
            let mut value: ciborium::Value = ciborium::from_reader(original.as_slice())?;
            let ciborium::Value::Array(fields) = &mut value else {
                return Err("sealed manifest is not an array".into());
            };
            fields.truncate(1);
            let mut bytes = Vec::new();
            ciborium::into_writer(&value, &mut bytes)?;
            bytes
        } else {
            vec![0xff]
        };
        std::fs::write(&path, damaged)?;
        expect_refused(
            &knock(&adapter, generation),
            "malformed sealed manifest",
            "GENERATION_SIDECAR_CORRUPT",
        )?;
        let before = adapter.inventory_sealed_generations()?;
        if before.sealed.len() != 1 || !before.quarantined.is_empty() {
            return Err(format!("a read door quarantined without reproof: {before:?}").into());
        }

        let quarantined = if scrub_instead_of_door {
            let report = adapter.scrub(
                &identity(generation),
                None,
                IntegrityScrubBudgetV1 { max_bytes: 1 },
            )?;
            if report.files_verified != 0 || report.bytes_read != 0 {
                return Err(format!("malformed manifest was hashed: {report:?}").into());
            }
            match report.outcome {
                IntegrityScrubOutcomeV1::Corrupt { quarantined } => quarantined,
                other @ (IntegrityScrubOutcomeV1::Completed
                | IntegrityScrubOutcomeV1::Paused { .. }) => {
                    return Err(
                        format!("scrub did not quarantine malformed manifest: {other:?}").into(),
                    );
                }
            }
        } else {
            let DoorFindingOutcome::Quarantined { quarantined } =
                adapter.quarantine_door_finding(&identity(generation))?
            else {
                return Err("door reproof did not quarantine malformed manifest".into());
            };
            quarantined
        };
        if quarantined.path != dir
            || quarantined.reason != GenerationQuarantineReasonV1::ContentCorrupt
        {
            return Err(format!("wrong malformed-manifest quarantine: {quarantined:?}").into());
        }
        std::fs::write(&path, original)?;
        expect_refused(
            &knock(&adapter, generation),
            "malformed manifest after durable quarantine",
            "GENERATION_QUARANTINED",
        )?;
        let after = adapter.inventory_sealed_generations()?;
        let [entry] = after.quarantined.as_slice() else {
            return Err(format!("malformed-manifest quarantine was not durable: {after:?}").into());
        };
        if entry.path != dir {
            return Err(format!("malformed-manifest quarantine was not durable: {after:?}").into());
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

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "assertions report independent scrub outcomes in this regression test"
)]
fn malformed_completion_receipt_is_rescrubbed_without_blocking_other_candidates() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(temp.path().to_path_buf());
    let damaged = ManifestGeneration::new(81);
    let other = ManifestGeneration::new(82);
    adapter.build_batch(&sealed_batch(damaged, "fn damaged() {}")?)?;
    adapter.build_batch(&sealed_batch(other, "fn other() {}")?)?;
    let report = adapter.scrub(
        &identity(other),
        None,
        IntegrityScrubBudgetV1 {
            max_bytes: u64::MAX,
        },
    )?;
    assert_eq!(report.outcome, IntegrityScrubOutcomeV1::Completed);
    std::fs::write(
        generation_dir(temp.path(), damaged).join("search-corpus-generation-scrub.cbor"),
        b"malformed",
    )?;
    let candidates = adapter.scrub_candidates()?;
    assert_eq!(candidates.len(), 2);
    assert!(candidates.iter().any(|candidate| {
        candidate.identity == identity(damaged) && candidate.last_completed_unix.is_none()
    }));
    assert!(candidates.iter().any(|candidate| {
        candidate.identity == identity(other) && candidate.last_completed_unix.is_some()
    }));
    let report = adapter.scrub(
        &identity(damaged),
        None,
        IntegrityScrubBudgetV1 {
            max_bytes: u64::MAX,
        },
    )?;
    assert_eq!(report.outcome, IntegrityScrubOutcomeV1::Completed);
    assert!(adapter.scrub_candidates()?.iter().any(|candidate| {
        candidate.identity == identity(damaged) && candidate.last_completed_unix.is_some()
    }));
    Ok(())
}

#[test]
fn a_caller_cannot_skip_the_scrub_prefix_with_an_unissued_cursor() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(temp.path().to_path_buf());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    let result = adapter.scrub(
        &identity(generation),
        Some(IntegrityScrubCursorV1 { next_artifact: 1 }),
        IntegrityScrubBudgetV1 {
            max_bytes: u64::MAX,
        },
    );
    if !matches!(result, Err(CoreError::InvalidContract(_))) {
        return Err(format!("unissued scrub cursor was accepted: {result:?}").into());
    }
    if adapter.scrub_candidates()?.iter().any(|candidate| {
        candidate.identity == identity(generation) && candidate.last_completed_unix.is_some()
    }) {
        return Err("an unissued cursor recorded scrub completion".into());
    }
    Ok(())
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

#[test]
fn reclaim_and_quarantine_discard_of_other_generations_preserve_a_paused_scrub() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let (scrubbed, reclaimed, quarantined) = (
        ManifestGeneration::new(1),
        ManifestGeneration::new(2),
        ManifestGeneration::new(3),
    );
    for generation in [scrubbed, reclaimed, quarantined] {
        adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    }
    let one_byte = IntegrityScrubBudgetV1 { max_bytes: 1 };
    let first = adapter.scrub(&identity(scrubbed), None, one_byte)?;
    let IntegrityScrubOutcomeV1::Paused { cursor } = first.outcome else {
        return Err(format!("the first scrub step must pause: {first:?}").into());
    };
    if !matches!(
        adapter.reclaim_sealed_generation(&identity(reclaimed))?,
        SealedGenerationReclaimOutcomeV1::Reclaimed { .. }
    ) {
        return Err("the other generation was not reclaimed".into());
    }
    let _resumed = adapter.scrub(&identity(scrubbed), Some(cursor), one_byte)?;

    let first = adapter.scrub(&identity(scrubbed), None, one_byte)?;
    let IntegrityScrubOutcomeV1::Paused { cursor } = first.outcome else {
        return Err(format!("the second scrub pass must pause: {first:?}").into());
    };
    let _original = damage(&generation_dir(&root, quarantined).join(MANIFEST))?;
    if !matches!(
        adapter.quarantine_door_finding(&identity(quarantined))?,
        DoorFindingOutcome::Quarantined { .. }
    ) {
        return Err("the damaged generation was not quarantined".into());
    }
    let inventory = adapter.inventory_sealed_generations()?;
    let entry = inventory
        .quarantined
        .iter()
        .find(|entry| entry.path == generation_dir(&root, quarantined))
        .ok_or("the damaged generation is absent from quarantine inventory")?;
    let mut stale = entry.clone();
    stale.path = generation_dir(&root, scrubbed);
    if !matches!(
        adapter.discard_quarantined_generation(&stale),
        Err(CoreError::Typed { .. })
    ) {
        return Err("discard accepted an intact generation as quarantined".into());
    }
    if !matches!(
        adapter.discard_quarantined_generation(entry)?,
        QuarantineDiscardOutcomeV1::Discarded { .. }
    ) {
        return Err("the quarantined generation was not discarded".into());
    }
    let _resumed = adapter.scrub(&identity(scrubbed), Some(cursor), one_byte)?;
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

#[cfg(unix)]
#[test]
fn both_doors_refuse_uncommitted_text_authority_entries() -> TestResult {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    let mut empty = sealed_batch(generation, "unused")?;
    empty.replace_scopes.clear();
    current_source_fixture::finish_batch(&mut empty)?;
    adapter.build_batch(&empty)?;
    let path = generation_dir(&root, generation).join(TEXT_AUTHORITY_DIR);
    symlink("missing-text-authority-target", &path)?;
    expect_refused(
        &knock(&adapter, generation),
        "uncommitted dangling text authority",
        "GENERATION_SIDECAR_CORRUPT",
    )?;
    std::fs::remove_file(&path)?;
    std::fs::write(&path, b"not a directory")?;
    expect_refused(
        &knock(&adapter, generation),
        "uncommitted regular text authority",
        "GENERATION_SIDECAR_CORRUPT",
    )?;
    std::fs::remove_file(path)?;
    adapter.validate_generation_identity(&identity(generation))?;
    Ok(())
}

#[test]
fn coverage_pages_are_bound_at_both_doors_and_by_scrub() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    let dir = generation_dir(&root, generation);
    let page = std::fs::read_dir(&dir)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("source-file-coverage-page-")
        })
        .ok_or("missing committed coverage page")?
        .path();
    let original = std::fs::read(&page)?;
    expect_admitted(&knock(&adapter, generation), "intact coverage page")?;
    std::fs::remove_file(&page)?;
    expect_refused(
        &knock(&adapter, generation),
        "missing coverage page",
        "GENERATION_SIDECAR_CORRUPT",
    )?;
    std::fs::write(&page, &original)?;
    let extra = dir.join("source-file-coverage-page-uncommitted.cbor");
    std::fs::write(&extra, &original)?;
    expect_refused(
        &knock(&adapter, generation),
        "uncommitted coverage page",
        "GENERATION_SIDECAR_CORRUPT",
    )?;
    std::fs::remove_file(extra)?;
    let mut flipped = original.clone();
    let byte = flipped.last_mut().ok_or("empty coverage page")?;
    *byte ^= 1;
    std::fs::write(&page, flipped)?;
    expect_refused(
        &knock(&adapter, generation),
        "changed coverage page",
        "GENERATION_SIDECAR_CORRUPT",
    )?;
    let report = adapter.scrub(
        &identity(generation),
        None,
        IntegrityScrubBudgetV1 {
            max_bytes: u64::MAX,
        },
    )?;
    if !matches!(report.outcome, IntegrityScrubOutcomeV1::Corrupt { .. }) {
        return Err(format!("scrub missed changed coverage page: {report:?}").into());
    }
    std::fs::write(&page, original)?;
    expect_refused(
        &knock(&adapter, generation),
        "coverage quarantine is durable",
        "GENERATION_QUARANTINED",
    )?;
    Ok(())
}

#[test]
fn scrub_quarantines_a_tampered_coverage_root_before_page_expansion() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    let path = generation_dir(&root, generation).join("source-file-coverage.cbor");
    let original = std::fs::read(&path)?;
    let mut changed = original.clone();
    let last = changed.last_mut().ok_or("empty coverage root")?;
    *last ^= 1;
    std::fs::write(&path, changed)?;
    expect_refused(
        &knock(&adapter, generation),
        "changed coverage root",
        "GENERATION_SIDECAR_CORRUPT",
    )?;
    let report = adapter.scrub(
        &identity(generation),
        None,
        IntegrityScrubBudgetV1 {
            max_bytes: u64::MAX,
        },
    )?;
    if !matches!(report.outcome, IntegrityScrubOutcomeV1::Corrupt { .. }) {
        return Err(format!("scrub did not quarantine the coverage root: {report:?}").into());
    }
    std::fs::write(path, original)?;
    expect_refused(
        &knock(&adapter, generation),
        "coverage-root quarantine is durable",
        "GENERATION_QUARANTINED",
    )?;
    Ok(())
}

#[test]
fn format_eight_requires_explicit_rebuild() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    let manifest = generation_dir(&root, generation).join(MANIFEST);
    let raw = std::fs::read(&manifest)?;
    let mut value: ciborium::Value = ciborium::from_reader(raw.as_slice())?;
    let ciborium::Value::Array(fields) = &mut value else {
        return Err("manifest is not an array".into());
    };
    let version = fields.first_mut().ok_or("missing manifest version")?;
    *version = ciborium::Value::Integer(8.into());
    let mut legacy = Vec::new();
    ciborium::into_writer(&value, &mut legacy)?;
    std::fs::write(manifest, legacy)?;
    expect_refused(
        &knock(&adapter, generation),
        "format eight requires rebuild",
        "GENERATION_MANIFEST_FORMAT_UNSUPPORTED",
    )?;
    Ok(())
}

#[test]
fn oversized_legacy_format_nine_requires_rebuild_at_every_door() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(generation, "fn one() { sealed_needle }")?)?;
    let manifest = generation_dir(&root, generation).join(MANIFEST);
    let file = std::fs::OpenOptions::new().write(true).open(manifest)?;
    file.set_len(16 * 1024 * 1024 + 1)?;
    expect_refused(
        &knock(&adapter, generation),
        "oversized format nine requires rebuild",
        "GENERATION_MANIFEST_FORMAT_UNSUPPORTED",
    )?;
    Ok(())
}

#[test]
fn a_dangling_sealed_identity_is_never_discarded_as_incomplete() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(81);
    adapter.build_batch(&sealed_batch(generation, "fn sealed() { preserve_me }")?)?;
    let dir = generation_dir(&root, generation);
    let identity_path = dir.join(IDENTITY);
    std::fs::remove_file(&identity_path)?;
    std::os::unix::fs::symlink("missing-identity", &identity_path)?;

    expect_refused(
        &knock(&adapter, generation),
        "dangling sealed identity",
        "GENERATION_SIDECAR_CORRUPT",
    )?;
    let inventory = adapter.inventory_sealed_generations()?;
    if !inventory.sealed.is_empty()
        || !matches!(
            inventory.quarantined.as_slice(),
            [entry] if entry.path == dir && entry.reason == GenerationQuarantineReasonV1::IdentityUnreadable
        )
    {
        return Err(format!("dangling identity was not quarantined: {inventory:?}").into());
    }
    match adapter.discard_incomplete_generation(&identity(generation)) {
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
            ..
        }) => {}
        other => {
            return Err(format!("incomplete discard accepted sealed damage: {other:?}").into());
        }
    }
    if !dir.is_dir()
        || !std::fs::symlink_metadata(identity_path)?
            .file_type()
            .is_symlink()
    {
        return Err("incomplete discard removed the damaged sealed generation".into());
    }
    Ok(())
}

#[test]
fn an_invalid_quarantine_receipt_does_not_block_sibling_inventory() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let damaged = ManifestGeneration::new(82);
    let healthy = ManifestGeneration::new(83);
    for generation in [damaged, healthy] {
        adapter.build_batch(&sealed_batch(generation, "fn sealed() { sealed_needle }")?)?;
    }
    std::fs::write(
        generation_dir(&root, damaged).join(QUARANTINE_RECEIPT),
        [0xff],
    )?;
    let inventory = adapter.inventory_sealed_generations()?;
    if !matches!(inventory.sealed.as_slice(), [entry] if entry.identity == identity(healthy))
        || !matches!(
            inventory.quarantined.as_slice(),
            [entry] if entry.reason == GenerationQuarantineReasonV1::IdentityUnreadable
                && entry.path == generation_dir(&root, damaged)
        )
    {
        return Err(format!("invalid receipt blocked sibling inventory: {inventory:?}").into());
    }
    let [quarantined] = inventory.quarantined.as_slice() else {
        return Err("expected one quarantined generation".into());
    };
    expect_admitted(&knock(&adapter, healthy), "healthy sibling")?;
    if !matches!(
        adapter.discard_quarantined_generation(quarantined)?,
        QuarantineDiscardOutcomeV1::Discarded { .. }
    ) || generation_dir(&root, damaged).exists()
    {
        return Err("invalid receipt quarantine was not discardable".into());
    }
    Ok(())
}

#[test]
fn a_scrub_quarantine_does_not_stop_an_unrelated_generation_build() -> TestResult {
    use std::sync::mpsc;
    use std::time::Duration;

    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = LexicalAdapter::with_state_root(root.clone());
    let damaged = ManifestGeneration::new(1);
    let unrelated = ManifestGeneration::new(2);
    adapter.build_batch(&sealed_batch(damaged, "fn damaged() { sealed_needle }")?)?;
    let manifest = generation_dir(&root, damaged).join(MANIFEST);
    let mut bytes = std::fs::read(&manifest)?;
    let last = bytes.last_mut().ok_or("empty sealed manifest")?;
    *last ^= 0xff;
    std::fs::write(&manifest, bytes)?;

    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (build_tx, build_rx) = mpsc::channel();
    let batch = sealed_batch(unrelated, "fn unrelated() { sealed_needle }")?;
    std::thread::scope(|scope| -> TestResult {
        let adapter_ref = &adapter;
        let scrub = scope.spawn(move || {
            adapter_ref
                .scrub_with_quarantine_fence(
                    &identity(damaged),
                    None,
                    IntegrityScrubBudgetV1 {
                        max_bytes: u64::MAX,
                    },
                    &|| {
                        entered_tx
                            .send(())
                            .map_err(|error| CoreError::Storage(error.to_string()))?;
                        release_rx
                            .recv()
                            .map_err(|error| CoreError::Storage(error.to_string()))?;
                        Ok(())
                    },
                )
                .map_err(|error| error.to_string())
        });
        entered_rx.recv_timeout(Duration::from_secs(30))?;
        let adapter_ref = &adapter;
        let build = scope.spawn(move || {
            let result = adapter_ref
                .build_batch(&batch)
                .map_err(|error| error.to_string());
            let _sent = build_tx.send(result.clone());
            result
        });
        let while_scrub_is_fenced = build_rx.recv_timeout(Duration::from_secs(60));
        release_tx.send(())?;
        let scrubbed = scrub
            .join()
            .map_err(|panic| format!("scrub thread panicked: {panic:?}"))??;
        build
            .join()
            .map_err(|panic| format!("build thread panicked: {panic:?}"))??;
        if !matches!(scrubbed.outcome, IntegrityScrubOutcomeV1::Corrupt { .. }) {
            return Err(format!("damaged generation was not quarantined: {scrubbed:?}").into());
        }
        if let Err(error) = while_scrub_is_fenced {
            return Err(format!("unrelated build waited for scrub quarantine: {error}").into());
        }
        Ok(())
    })?;
    expect_admitted(&knock(&adapter, unrelated), "unrelated generation")
}
