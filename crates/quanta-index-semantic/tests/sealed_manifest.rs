//! QI-BB-017 — a sealed semantic generation commits to its files once, and
//! both doors that admit it re-measure files, never rows.
//!
//! Before this, the activation validator and every cold open streamed every
//! row of the main table and the membership table to re-derive the roots the
//! scope manifest carries. Now the seal writes a sealed manifest naming
//! every dataset file, the scope manifest and the build contract with length
//! and SHA-256, and `validate_generation_identity` (activation, restart) and
//! `open` (query) verify it with a bounded-memory hash.
//!
//! The oracle is fault injection on the real files: every dataset file in
//! turn is truncated, bit-flipped and removed, a foreign file is added, the
//! sidecars are forged, and both doors must refuse under the typed code and
//! admit again once the file is restored. A generation that opens twice in a
//! row proves that opening writes nothing the commitment would notice.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::{Path, PathBuf};

use quanta_index_contract::{
    EmbeddingRecord, GenerationSnapshot, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneTrackKind,
};
use quanta_index_core::{
    CoreError, GenerationIdentityValidatePort, GenerationStorageKeyV1, RequestBudgetV1,
    SemanticIndexOpenPort,
};
use quanta_index_semantic::{
    SemanticAdapter, build_resident_batch_v1, legacy_chunk_embedding_record_v1,
    sealed_replace_batch_v1,
};

type TestResult = Result<(), Box<dyn Error>>;

const SEALED_MANIFEST: &str = "semantic-sealed-manifest.cbor";
const SCOPE_MANIFEST: &str = "semantic-manifest.cbor";
const BUILD_CONTRACT: &str = "semantic-build-contract.cbor";
const DATASET: &str = "dataset";

fn repo() -> RepoId {
    RepoId::new("sealed-repo")
}

fn revision() -> RevisionId {
    RevisionId::new("sealed-rev")
}

fn embedding(id: &str, path: &str, vector: Vec<f32>) -> Result<EmbeddingRecord, Box<dyn Error>> {
    legacy_chunk_embedding_record_v1(id, path, vector)
        .map_err(|err| -> Box<dyn Error> { err.into() })
}

fn seal(adapter: &SemanticAdapter, generation: ManifestGeneration) -> TestResult {
    build_resident_batch_v1(
        adapter,
        &sealed_replace_batch_v1(
            repo(),
            revision(),
            generation,
            "src/lib.rs",
            vec![
                embedding("emb-a", "src/lib.rs", vec![1.0, 0.0, 0.0])?,
                embedding("emb-b", "src/lib.rs", vec![0.0, 1.0, 0.0])?,
            ],
            3,
        ),
    )?;
    Ok(())
}

fn identity(generation: ManifestGeneration) -> GenerationSnapshot {
    GenerationSnapshot {
        repo_id: repo(),
        revision_id: revision(),
        track: SearchPlaneTrackKind::Semantic,
        manifest_generation: generation,
        manifest_digest: format!("manifest:{}", generation.get()),
    }
}

fn generation_dir(root: &Path, generation: ManifestGeneration) -> PathBuf {
    GenerationStorageKeyV1::for_repo_revision(&repo(), &revision()).generation_dir(root, generation)
}

/// Every regular file under the dataset tree, sorted, relative to the
/// generation directory.
fn dataset_files(generation_dir: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut files = Vec::new();
    let mut pending = vec![generation_dir.join(DATASET)];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

/// What every door said about a generation: the validator, the query's
/// open, and the activation's proven open (the proof whose handle is
/// promoted).
struct Doors {
    validate: Result<(), CoreError>,
    open: Result<usize, CoreError>,
    proven: Result<usize, CoreError>,
}

fn knock(adapter: &SemanticAdapter, generation: ManifestGeneration) -> Doors {
    let validate = adapter.validate_generation_identity(&identity(generation));
    let open = adapter
        .open(&repo(), &revision(), generation)
        .and_then(|searcher| searcher.search(&[1.0, 0.0, 0.0], 5, &RequestBudgetV1::unbounded()))
        .map(|hits| hits.len());
    let proven = adapter
        .open_proven(&identity(generation))
        .and_then(|searcher| searcher.search(&[1.0, 0.0, 0.0], 5, &RequestBudgetV1::unbounded()))
        .map(|hits| hits.len());
    Doors {
        validate,
        open,
        proven,
    }
}

fn typed_code<T>(result: &Result<T, CoreError>) -> Option<String> {
    match result {
        Err(CoreError::Typed { code, .. }) => Some(code.clone()),
        _ => None,
    }
}

fn expect_admitted(doors: &Doors, what: &str) -> TestResult {
    if let Err(err) = &doors.validate {
        return Err(format!("{what}: validator refused an intact generation: {err}").into());
    }
    for (door, result) in [("open", &doors.open), ("proven open", &doors.proven)] {
        match result {
            Ok(2) => {}
            Ok(other) => {
                return Err(format!(
                    "{what}: {door} of an intact generation served {other} hits, expected 2"
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
        if typed_code(result).as_deref() != Some(code) {
            return Err(format!(
                "{what}: {door} answered {:?}, expected typed {code}",
                result.as_ref().map(|_| "served")
            )
            .into());
        }
    }
    Ok(())
}

fn flip_last_byte(path: &Path) -> TestResult {
    let mut bytes = std::fs::read(path)?;
    let last = bytes.len().checked_sub(1).ok_or("empty file")?;
    let byte = bytes.get_mut(last).ok_or("index")?;
    *byte ^= 0xff;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Every dataset file, under every corruption, is refused by both doors.
///
/// Each is admitted again once restored; a foreign file inside the dataset
/// is refused too, because for a versioned dataset an extra file can change
/// what opens.
#[test]
fn both_doors_refuse_a_dataset_file_that_does_not_match_the_manifest() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let generation = ManifestGeneration::new(1);
    seal(&adapter, generation)?;
    let generation_dir = generation_dir(&root, generation);
    expect_admitted(&knock(&adapter, generation), "intact")?;

    let files = dataset_files(&generation_dir)?;
    if files.is_empty() {
        return Err("a sealed generation has dataset files".into());
    }
    for file in &files {
        let name = file.display().to_string();
        let original = std::fs::read(file)?;

        // Truncated.
        let truncated = original
            .get(..original.len().saturating_sub(1))
            .ok_or("slice")?;
        std::fs::write(file, truncated)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} truncated"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;
        std::fs::write(file, &original)?;

        // Bit-flipped, same length.
        if !original.is_empty() {
            flip_last_byte(file)?;
            expect_refused(
                &knock(&adapter, generation),
                &format!("{name} bit-flipped"),
                "GENERATION_SIDECAR_CORRUPT",
            )?;
            std::fs::write(file, &original)?;
        }

        // Removed.
        std::fs::remove_file(file)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} removed"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;
        std::fs::write(file, &original)?;
        expect_admitted(&knock(&adapter, generation), &format!("{name} restored"))?;
    }

    // A foreign file the seal never committed to.
    let foreign = generation_dir
        .join(DATASET)
        .join("semantic.lance")
        .join("_versions")
        .join("999.manifest");
    std::fs::create_dir_all(foreign.parent().ok_or("foreign parent")?)?;
    std::fs::write(&foreign, b"foreign version")?;
    expect_refused(
        &knock(&adapter, generation),
        "foreign file",
        "GENERATION_SIDECAR_CORRUPT",
    )?;
    std::fs::remove_file(&foreign)?;
    expect_admitted(&knock(&adapter, generation), "foreign file removed")
}

/// The sidecars the manifest commits to are refused when forged, missing,
/// or absent as a whole.
#[test]
fn both_doors_refuse_forged_or_missing_sidecars() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let generation = ManifestGeneration::new(2);
    seal(&adapter, generation)?;
    let generation_dir = generation_dir(&root, generation);

    for sidecar in [SCOPE_MANIFEST, BUILD_CONTRACT] {
        let path = generation_dir.join(sidecar);
        let original = std::fs::read(&path)?;
        flip_last_byte(&path)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{sidecar} bit-flipped"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;
        std::fs::remove_file(&path)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{sidecar} removed"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;
        std::fs::write(&path, &original)?;
        expect_admitted(&knock(&adapter, generation), &format!("{sidecar} restored"))?;
    }

    // No sealed manifest at all: a generation sealed before the format.
    let sealed_manifest = generation_dir.join(SEALED_MANIFEST);
    let original = std::fs::read(&sealed_manifest)?;
    std::fs::remove_file(&sealed_manifest)?;
    expect_refused(
        &knock(&adapter, generation),
        "sealed manifest missing",
        "GENERATION_MANIFEST_MISSING",
    )?;
    std::fs::write(&sealed_manifest, &original)?;
    expect_admitted(&knock(&adapter, generation), "sealed manifest restored")
}

/// A sealed manifest copied from another generation binds to that
/// generation's identity digest and is refused here.
#[test]
fn the_sealed_manifest_binds_the_identity_digest() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let first = ManifestGeneration::new(3);
    let second = ManifestGeneration::new(4);
    seal(&adapter, first)?;
    seal(&adapter, second)?;
    let _copied = std::fs::copy(
        generation_dir(&root, first).join(SEALED_MANIFEST),
        generation_dir(&root, second).join(SEALED_MANIFEST),
    )?;
    expect_refused(
        &knock(&adapter, second),
        "sealed manifest from another generation",
        "GENERATION_IDENTITY_DIGEST_MISMATCH",
    )
}

/// Opening writes nothing the commitment would notice: the same generation
/// is admitted by both doors repeatedly, with a search in between.
#[test]
fn a_sealed_generation_is_admitted_repeatedly() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root)?;
    let generation = ManifestGeneration::new(5);
    seal(&adapter, generation)?;
    for round in 0..3 {
        expect_admitted(&knock(&adapter, generation), &format!("round {round}"))?;
    }
    Ok(())
}
