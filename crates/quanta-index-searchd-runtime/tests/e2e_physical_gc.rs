//! QI-BB-003 — retention reclaims the bytes, not just the authority record.
//!
//! Before this, reaping a sealed generation removed its authority CBOR file
//! and left the Tantivy and `LanceDB` directories behind, so state-root disk
//! use grew with every generation ever sealed. Now the search plane, after
//! reaping the authority and reconciling the ledger, fences the snapshot
//! registry and reclaims each retired generation's directory on both tracks,
//! verifying the sealed identity before it deletes anything.
//!
//! Oracles are the filesystem: which generation directories exist under the
//! pair on each track, and that the retained ones still serve.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::path::{Path, PathBuf};

use quanta_index_contract::{ManifestGeneration, TextQuerySyntax};
use quanta_index_core::GenerationStorageKeyV1;
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::E2eRuntime;

type TestResult = Result<(), Box<dyn Error>>;

const SEALED: u64 = 5;
const KEEP: usize = 2;

fn pair_dir(rt: &E2eRuntime, track_root: &str) -> Result<PathBuf, Box<dyn Error>> {
    let canonical = std::fs::canonicalize(rt.state_root())?;
    Ok(canonical
        .join(track_root)
        .join(GenerationStorageKeyV1::for_repo_revision(&rt.repo(), &rt.revision()).as_str()))
}

/// Generation numbers whose directories exist under `pair`.
fn generations_on_disk(pair: &Path) -> Result<BTreeSet<u64>, Box<dyn Error>> {
    let mut out = BTreeSet::new();
    if !pair.is_dir() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(pair)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(number) = name.strip_prefix('g') {
            let _new = out.insert(number.parse::<u64>()?);
        }
    }
    Ok(out)
}

/// Seal `SEALED` generations, activating each as it lands, so the reaped set
/// is `SEALED - KEEP` and the retained set is the active one plus the newest.
fn seal_generations(rt: &mut E2eRuntime) -> Result<Vec<ManifestGeneration>, Box<dyn Error>> {
    let mut sealed = Vec::new();
    for index in 0..SEALED {
        rt.ingest_text(
            "repo",
            &format!("src/item_{index}.rs"),
            &format!("fn item_{index}() {{ needle_{index} }}"),
        )?;
        sealed.push(rt.seal()?);
        rt.activate_last_sealed_generation()?;
    }
    Ok(sealed)
}

#[test]
fn reaping_a_generation_removes_its_directories_on_both_tracks() -> TestResult {
    let mut rt = E2eRuntime::boot_with_history_max_generations(KEEP)?;
    let sealed = seal_generations(&mut rt)?;
    let expected: BTreeSet<u64> = sealed
        .iter()
        .rev()
        .take(KEEP)
        .map(|generation| generation.get())
        .collect();

    for track_root in ["indexes/lexical", "indexes/semantic"] {
        let on_disk = generations_on_disk(&pair_dir(&rt, track_root)?)?;
        if on_disk != expected {
            return Err(format!(
                "{track_root}: expected exactly the retained generations {expected:?} on disk, found {on_disk:?}"
            )
            .into());
        }
    }
    Ok(())
}

/// The retained generations are untouched: the active one still serves and
/// the newest is the pinned query target.
#[test]
fn retained_generations_still_serve_after_gc() -> TestResult {
    let mut rt = E2eRuntime::boot_with_history_max_generations(KEEP)?;
    let sealed = seal_generations(&mut rt)?;
    let newest = sealed.last().copied().ok_or("no generation was sealed")?;
    let result = rt.query_text(
        TextQuerySyntax::Native,
        &format!("needle_{}", SEALED.saturating_sub(1)),
        5,
    );
    if let Some(error) = result.typed_error {
        return Err(format!("newest generation {} does not serve: {error}", newest.get()).into());
    }
    if result.candidate_ids.len() != 1 {
        return Err(format!(
            "newest generation served {} rows for its own needle, expected 1",
            result.candidate_ids.len()
        )
        .into());
    }
    let semantic = rt.query_semantic("needle", 5, None);
    if let Some(error) = semantic.typed_error {
        return Err(format!("semantic track does not serve after GC: {error}").into());
    }
    Ok(())
}

/// A directory whose sealed identity contradicts its path is never swept.
///
/// The sweep fails closed with the scope-mismatch code and the directory
/// stays under either identity. (The boot scanner refuses the same
/// directory; quarantining it instead of refusing is QI-BB-026 / W2.) The
/// crash-orphan case — a directory whose identity is correct but whose
/// authority record was already reaped — is pinned at the materializer
/// level, where the crash point can be placed.
#[test]
fn a_directory_whose_identity_contradicts_its_path_fails_closed() -> TestResult {
    let mut rt = E2eRuntime::boot_with_history_max_generations(KEEP)?;
    let sealed = seal_generations(&mut rt)?;
    let oldest_retained = sealed
        .iter()
        .rev()
        .nth(KEEP.saturating_sub(1))
        .copied()
        .ok_or("fewer generations than KEEP were sealed")?;

    let lexical_pair = pair_dir(&rt, "indexes/lexical")?;
    let donor = lexical_pair.join(format!("g{}", oldest_retained.get()));
    let impostor = lexical_pair.join("g1");
    copy_tree(&donor, &impostor)?;
    if !impostor.is_dir() {
        return Err("impostor fixture was not created".into());
    }

    rt.ingest_text("repo", "src/extra.rs", "fn extra() { needle_extra }")?;
    match rt.seal() {
        Ok(generation) => Err(format!(
            "seal of generation {} succeeded although the sweep met a directory whose identity contradicts its path",
            generation.get()
        )
        .into()),
        Err(error) => {
            let rendered = error.to_string();
            if !rendered.contains("GENERATION_IDENTITY_SCOPE_MISMATCH") {
                return Err(format!("seal failed under an unexpected error: {rendered}").into());
            }
            if !impostor.is_dir() {
                return Err("a directory with a contradicting identity was deleted".into());
            }
            Ok(())
        }
    }
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            let _bytes = std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}
