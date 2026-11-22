//! Exact Legacy retention and serving oracles through a pinned old-Q SDK process.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::path::{Path, PathBuf};

use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusActiveHeadV1, lex::LanguageCode,
};
use quanta_index_core::GenerationStorageKeyV1;
use quanta_index_sdk::QuantaIndex;
use quanta_index_searchd_harness::{
    legacy_reference::{
        LegacySearchdBinary, SearchdBinaryProcess, legacy_source_batch,
        legacy_source_delta_from_chunks, publish_legacy_source_batch,
    },
    private_tempdir,
};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

const SEALED: u64 = 5;
const KEEP: usize = 2;

fn with_published_fixture(
    test: impl FnOnce(&QuantaIndex, &Path, &[ManifestGeneration]) -> TestResult,
) -> TestResult {
    let parent = private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let command = binary.benchmark_command(&state_root, KEEP)?;
    let process = SearchdBinaryProcess::start_with_pinned_command(&state_root, command)?;
    let outcome = (|| {
        let client = process.connect()?;
        let sealed = publish_five_generations(&client)?;
        test(&client, &state_root, &sealed)
    })();
    let stopped = process.stop();
    outcome.and(stopped)?;
    binary.verify()
}

fn added_chunk(generation: u64, path: &str, body: &str) -> TestResult<ChunkRecord> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(format!("e2e-{generation}-{path}")),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new("rust")?,
        start_byte: 0,
        end_byte: u32::try_from(body.len())?,
        start_line: 1,
        end_line: 2,
        text: body.to_owned().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    })
}

/// G1 is a replacement; G2–G5 each add exactly one file without retiring an
/// older owner. Each publication is sealed and activated before the next one.
fn publish_five_generations(client: &QuantaIndex) -> TestResult<Vec<ManifestGeneration>> {
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    let mut active: Option<SearchCorpusActiveHeadV1> = None;
    let mut sealed = Vec::new();
    for index in 0..SEALED {
        let generation = index + 1;
        let path = format!("src/item_{index}.rs");
        let body = format!("fn item_{index}() {{ needle_{index} }}");
        let batch = if generation == 1 {
            legacy_source_batch(
                &repo,
                &revision,
                generation,
                None,
                &[(path.as_str(), body.as_str(), generation)],
            )?
        } else {
            legacy_source_delta_from_chunks(
                &repo,
                &revision,
                generation,
                &[],
                &[],
                vec![added_chunk(generation, &path, &body)?],
            )?
        };
        let (head, _receipt) = publish_legacy_source_batch(client, &batch, active)?;
        if head.generation.semantic.manifest_generation != ManifestGeneration::new(generation) {
            return Err(format!("G{generation} activated without its semantic peer").into());
        }
        active = Some(head);
        sealed.push(ManifestGeneration::new(generation));
    }
    Ok(sealed)
}

fn pair_dir(state_root: &Path, track_root: &str) -> TestResult<PathBuf> {
    let canonical = std::fs::canonicalize(state_root)?;
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    Ok(canonical
        .join(track_root)
        .join(GenerationStorageKeyV1::for_repo_revision(&repo, &revision).as_str()))
}

fn generations_on_disk(pair: &Path) -> TestResult<BTreeSet<u64>> {
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

#[test]
fn reaping_a_generation_removes_its_directories_on_both_tracks() -> TestResult {
    with_published_fixture(|_client, state_root, sealed| {
        let expected: BTreeSet<u64> = sealed
            .iter()
            .rev()
            .take(KEEP)
            .map(|generation| generation.get())
            .collect();
        for track_root in ["indexes/lexical", "indexes/semantic"] {
            let on_disk = generations_on_disk(&pair_dir(state_root, track_root)?)?;
            if on_disk != expected {
                return Err(format!(
                    "{track_root}: expected exactly the retained generations {expected:?} on disk, found {on_disk:?}"
                )
                .into());
            }
        }
        Ok(())
    })
}

/// The retained generations are untouched: the active one still serves and
/// the newest is the pinned query target.
#[test]
fn retained_generations_still_serve_after_gc() -> TestResult {
    with_published_fixture(|client, _state_root, sealed| {
        let newest = sealed.last().copied().ok_or("no generation was sealed")?;
        let pin = GenerationPin::new(
            RepoId::new("repo-e2e")?,
            RevisionId::new("rev-e2e")?,
            newest,
        );
        let result = client
            .lexical()
            .query()
            .native(format!("needle_{}", SEALED.saturating_sub(1)))
            .pinned(pin.clone())
            .top_k(5)
            .execute()?;
        if result.generation != pin {
            return Err(format!("newest generation {} was not selected", newest.get()).into());
        }
        if result.results.len() != 1 {
            return Err(format!(
                "newest generation served {} rows for its own needle, expected 1",
                result.results.len()
            )
            .into());
        }
        let semantic = client
            .semantic()
            .query()
            .text("needle")
            .pinned(pin.clone())
            .top_k(5)
            .execute()?;
        if semantic.generation != pin {
            return Err("semantic track selected a different generation after GC".into());
        }
        Ok(())
    })
}
