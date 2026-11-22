//! F15 file authority through the SDK and a pinned old-Q process. Fixed source
//! rows establish admission and provenance; a separate full build is the oracle
//! for delta/no-op ranking, scores and pagination. The pinned process also
//! exercises FileBody corruption, loss, and durable scrub quarantine.

#![forbid(unsafe_code)]
#![expect(
    clippy::panic_in_result_fn,
    reason = "fixed fixture assertions report product regressions"
)]

#[path = "support/current_source_fixture.rs"]
mod current_source_fixture;

#[path = "support/source_fixture.rs"]
mod source_fixture;

use std::error::Error;
use std::path::Path;
use std::time::{Duration, Instant};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, ContinuationTokenV2, GenerationPin, LexicalCandidate,
    ManifestGeneration, RepoId, RepoRelativePath, RevisionId, SearchCorpusActiveHeadV1,
    SearchCorpusIngestBatch, SearchCorpusReplaceScope, SearchCorpusTombstoneScope,
    SearchPlaneErrorCodeV2, SearchPlaneTrackKind, SourceFileRevision, TextQueryResponse,
    TextRankUnit, source_event_payload_sha256,
};
use quanta_index_core::{GenerationQuarantineReasonV1, GenerationStorageKeyV1};
use quanta_index_sdk::{QuantaIndex, SdkError, SearchCorpusBatch};
use quanta_index_searchd_harness::{
    legacy_reference::{
        LegacySearchdBinary, SearchdBinaryProcess, publish_legacy_source_batch, request_when_ready,
    },
    private_tempdir,
};
use sha2::{Digest as _, Sha256};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
type Row = (
    String,
    String,
    String,
    u32,
    Option<(u64, u64)>,
    Option<SourceFileRevision>,
);
type ExpectedSource<'a> = (&'a str, &'a str, &'a [u8]);

const SHARED: &str = "abc Café unique";
const EARLY_PATH: &str = "src/zabc.r";
const LATE_PATH: &str = "src/abc.rs";
const COPY_PATH: &str = "src/copy.rs";
const RETIRED_PATH: &str = "src/retire.rs";
const RETIRED_BODY: &str = "retiretoken";
const RETIRED_BASENAME: &str = "retire.rs";
const EARLY_BODY: &str = "neutral path owner";
const SOURCE_REVISION: &str = "f15-source-revision";

fn repo() -> Result<RepoId, Box<dyn Error>> {
    Ok(RepoId::new("f15-snapshot")?)
}

fn revision() -> Result<RevisionId, Box<dyn Error>> {
    Ok(RevisionId::new("f15-revision")?)
}

fn scope(
    owner: &str,
    path: &str,
    body: &str,
    admitted: bool,
) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let source_repo = RepoId::new(owner)?;
    let source_revision = RevisionId::new(SOURCE_REVISION)?;
    let language = LanguageCode::new("rust")?;
    // The accented witness is deliberately outside the only chunk. CodeSearch
    // must consult the committed full source, not reconstruct it from chunks.
    let chunk_text = if body == SHARED { "abc " } else { body };
    let chunks = if admitted {
        vec![ChunkRecord {
            chunk_id: ChunkId::new(format!("f15-{owner}-{path}")),
            repo_relative_path: RepoRelativePath::new(path),
            language: language.clone(),
            start_byte: 0,
            end_byte: u32::try_from(chunk_text.len())?,
            start_line: 1,
            end_line: 1,
            text: chunk_text.to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: Some(source_repo.clone()),
        }]
    } else {
        Vec::new()
    };
    current_source_fixture::text_scope(&source_repo, &source_revision, path, language, body, chunks)
}

fn batch(
    generation: u64,
    base: Option<u64>,
    replacements: Vec<SearchCorpusReplaceScope>,
    delete: &[(&str, &str)],
) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    let mut result = SearchCorpusIngestBatch {
        source_event: current_source_fixture::empty_event(),
        repo_id: repo()?,
        revision_id: revision()?,
        generation: ManifestGeneration::new(generation),
        base_generation: base.map(ManifestGeneration::new),
        manifest_digest: format!("f15-manifest-{generation}"),
        batch_digest: "0".repeat(64),
        mode: if base.is_some() {
            BatchIngestMode::Delta
        } else {
            BatchIngestMode::ReplaceGeneration
        },
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: replacements,
        tombstone_scopes: delete
            .iter()
            .map(|(owner, path)| {
                Ok(SearchCorpusTombstoneScope {
                    file: quanta_index_contract::SourceFileKey {
                        source_repo_id: RepoId::new(*owner)?,
                        repo_relative_path: RepoRelativePath::new(*path),
                    },
                })
            })
            .collect::<Result<Vec<_>, Box<dyn Error>>>()?,
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    };
    current_source_fixture::finish_batch(&mut result)?;
    Ok(result)
}

fn base_scopes() -> Result<Vec<SearchCorpusReplaceScope>, Box<dyn Error>> {
    Ok(vec![
        scope("source-a", EARLY_PATH, EARLY_BODY, true)?,
        scope("source-b", LATE_PATH, SHARED, true)?,
        // Same source bytes, distinct key and no admitted content.
        scope("source-a", COPY_PATH, SHARED, false)?,
        scope("source-a", RETIRED_PATH, RETIRED_BODY, true)?,
    ])
}

fn final_scopes() -> Result<Vec<SearchCorpusReplaceScope>, Box<dyn Error>> {
    Ok(vec![
        scope("source-a", EARLY_PATH, EARLY_BODY, true)?,
        scope("source-b", LATE_PATH, SHARED, false)?,
        scope("source-a", COPY_PATH, SHARED, true)?,
    ])
}

fn publish(
    client: &QuantaIndex,
    batch: SearchCorpusIngestBatch,
    expected: Option<SearchCorpusActiveHeadV1>,
) -> TestResult<SearchCorpusActiveHeadV1> {
    if !batch.seal
        || batch.bundle_payload.is_some()
        || !batch.clear_surfaces.is_empty()
        || !batch.semantic_replace_scopes.is_empty()
        || !batch.semantic_tombstone_scopes.is_empty()
    {
        return Err("expected a sealed F15 Source fixture with only file mutations".into());
    }
    let mut sdk = match (batch.mode, batch.base_generation) {
        (BatchIngestMode::ReplaceGeneration, None) => SearchCorpusBatch::replace_generation(
            batch.repo_id,
            batch.revision_id,
            batch.generation,
            batch.manifest_digest,
        ),
        (BatchIngestMode::Delta, Some(base)) => SearchCorpusBatch::delta(
            batch.repo_id,
            batch.revision_id,
            batch.generation,
            base,
            batch.manifest_digest,
        ),
        (BatchIngestMode::ReplaceGeneration, Some(_)) | (BatchIngestMode::Delta, None) => {
            return Err("F15 Source fixture mode and base disagree".into());
        }
    }
    .source_event(batch.source_event);
    for scope in batch.replace_scopes {
        sdk = sdk.replace_scope(
            scope.coverage,
            scope.source_bytes,
            scope.chunks,
            scope.symbols,
        );
    }
    for scope in batch.tombstone_scopes {
        sdk = sdk.tombstone_scope(scope.file);
    }
    let (head, _receipt) = publish_legacy_source_batch(client, &sdk, expected)?;
    Ok(head)
}

fn page(
    client: &QuantaIndex,
    generation: u64,
    term: &str,
    fetch: u32,
    after: Option<ContinuationTokenV2>,
) -> TestResult<TextQueryResponse> {
    let pin = GenerationPin::new(repo()?, revision()?, ManifestGeneration::new(generation));
    let response = request_when_ready(|| {
        let query = client
            .lexical()
            .query()
            .code_search(term)
            .pinned(pin.clone())
            .top_k(fetch);
        match after.clone() {
            Some(cursor) => query.after(cursor).execute(),
            None => query.execute(),
        }
    })?;
    if response.generation != pin || response.rank_unit != TextRankUnit::File {
        return Err(format!("F15 {term:?}: expected a file page pinned to {pin:?}").into());
    }
    if response.window.has_more().is_none() {
        return Err(format!(
            "F15 G{generation} {term:?}: incomplete search window {:?}",
            response.window.outcome()
        )
        .into());
    }
    Ok(response)
}

fn project(row: &LexicalCandidate) -> Row {
    (
        row.source_repo_id.as_str().to_string(),
        row.repo_relative_path.as_str().to_string(),
        row.candidate_id.clone(),
        row.score.to_bits(),
        row.preview
            .as_ref()
            .and_then(|preview| preview.original_focus)
            .map(|span| (span.start, span.end)),
        row.source.clone(),
    )
}

fn rows(
    client: &QuantaIndex,
    generation: u64,
    term: &str,
    fetch: u32,
) -> Result<Vec<Row>, Box<dyn Error>> {
    let mut result = Vec::new();
    let mut after = None;
    loop {
        let next = page(client, generation, term, fetch, after)?;
        result.extend(next.results.iter().map(project));
        if result.len() > 8 {
            return Err("file page walk exceeded fixed fixture universe".into());
        }
        let Some(cursor) = next.next_cursor else {
            return Ok(result);
        };
        if next.results.is_empty() {
            return Err("empty file page returned a continuation".into());
        }
        after = Some(cursor);
    }
}

fn file_body_object(root: &Path, witness: &[u8]) -> Result<std::path::PathBuf, Box<dyn Error>> {
    let objects = root.join("lexical-object-pool/objects");
    let mut matching = Vec::new();
    for shard in std::fs::read_dir(objects)? {
        let shard = shard?;
        if !shard.file_type()?.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(shard.path())? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let bytes = std::fs::read(entry.path())?;
            if bytes.get(..8) == Some(b"QLSTOR01".as_slice())
                && bytes.get(10) == Some(&1)
                && bytes.windows(witness.len()).any(|window| window == witness)
            {
                matching.push(entry.path());
            }
        }
    }
    let [path] = matching.as_slice() else {
        return Err(format!("expected one FileBody carrying witness, got {matching:?}").into());
    };
    Ok(path.clone())
}

#[test]
fn damaged_file_bodies_stay_quarantined_after_restart() -> TestResult {
    const DORMANT_INTERVAL_MS: &str = "600000";
    const SCRUB_INTERVAL_MS: &str = "100";
    const SCRUB_WAIT: Duration = Duration::from_secs(30);

    let legacy = LegacySearchdBinary::from_environment()?;
    let pin = GenerationPin::new(repo()?, revision()?, ManifestGeneration::new(1));
    // The pinned daemon refuses a preexisting symlink before opening sockets.
    for fault in ["corrupt", "missing"] {
        let root = private_tempdir()?;
        let root_path = root.path().canonicalize()?;
        let start = |interval_ms: &str| -> TestResult<SearchdBinaryProcess> {
            let mut command = legacy.benchmark_command(&root_path, 8)?;
            let _configured = command
                .env("QUANTA_INDEX_MAINTENANCE_TICK_MS", "50")
                .env("QUANTA_INDEX_INTEGRITY_SCRUB_INTERVAL_MS", interval_ms)
                .env(
                    "QUANTA_INDEX_INTEGRITY_SCRUB_MAX_BYTES_PER_STEP",
                    u64::MAX.to_string(),
                );
            SearchdBinaryProcess::start_with_pinned_command(&root_path, command)
        };
        let writer = start(DORMANT_INTERVAL_MS)?;
        let client = writer.connect()?;
        let g1 = publish(&client, batch(1, None, base_scopes()?, &[])?, None)?;
        let _witness = file_source_hit(
            &client,
            1,
            RETIRED_BODY,
            ("source-a", RETIRED_PATH, RETIRED_BODY.as_bytes()),
        )?;
        // Keep G1 reachable through its pin and let an independent healthy FULL
        // generation own the active head. A DELTA can retain the retired G1
        // object in its shared closure and must then be quarantined with G1.
        let mut full = batch(2, None, final_scopes()?, &[])?;
        full.source_event.expected_base_event_id = Some(format!(
            "lexical-fixture-{}-{}-g1",
            full.repo_id.as_str(),
            full.revision_id.as_str()
        ));
        full.source_event.payload_sha256 = source_event_payload_sha256(&full)?;
        let _g2 = publish(&client, full, Some(g1))?;

        writer.stop()?;

        let lexical_root = root_path.join("indexes/lexical");
        let generation_dir =
            GenerationStorageKeyV1::for_repo_revision(&pin.repo_id, &pin.revision_id)
                .generation_dir(&lexical_root, pin.manifest_generation);
        let object = file_body_object(&lexical_root, RETIRED_BODY.as_bytes())?;
        let original = std::fs::read(&object)?;
        let object_digest: [u8; 32] = Sha256::digest(&original).into();
        let expected_object = format!("{object_digest:x?}");
        match fault {
            "corrupt" => {
                let mut damaged = original.clone();
                *damaged.first_mut().ok_or("empty FileBody")? ^= 0xff;
                std::fs::write(&object, damaged)?;
            }
            "missing" => std::fs::remove_file(&object)?,
            _ => return Err("unknown fixed FileBody fault".into()),
        }

        let cold = start(DORMANT_INTERVAL_MS)?;
        let client = cold.connect()?;
        assert_file_body_rejection(
            &client,
            fault,
            SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
        )?;
        assert!(client.quarantine().inventory()?.lexical.is_empty());
        cold.stop()?;

        let scrub = start(SCRUB_INTERVAL_MS)?;
        let client = scrub.connect()?;
        let started = Instant::now();
        loop {
            let metrics = request_when_ready(|| client.observability().metrics_snapshot())?;
            let corruptions = metrics
                .counters
                .iter()
                .find(|counter| counter.name == "scrub_corruptions_total")
                .ok_or("missing scrub corruption counter")?
                .value;
            if corruptions > 0 {
                assert_eq!(corruptions, 1, "{fault}: scrub found unexpected damage");
                break;
            }
            if started.elapsed() >= SCRUB_WAIT {
                return Err(format!(
                    "{fault}: scrub did not find the FileBody within {SCRUB_WAIT:?}"
                )
                .into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let inventory = client.quarantine().inventory()?;
        let [entry] = inventory.lexical.as_slice() else {
            return Err(format!("{fault}: unexpected scrub quarantine {inventory:?}").into());
        };
        assert_eq!(Path::new(&entry.path), generation_dir);
        assert_eq!(entry.track, SearchPlaneTrackKind::Lexical);
        assert_eq!(
            entry.reason,
            GenerationQuarantineReasonV1::ContentCorrupt.as_code_str()
        );
        assert!(
            entry.detail.contains(&expected_object),
            "{fault}: scrub named a different object: {entry:?}"
        );
        scrub.stop()?;

        // Restore the original object while stopped to isolate the persisted fence.
        std::fs::write(&object, original)?;
        let reopened = start(DORMANT_INTERVAL_MS)?;
        let client = reopened.connect()?;
        assert_file_body_rejection(
            &client,
            fault,
            SearchPlaneErrorCodeV2::GenerationQuarantined,
        )?;
        assert_eq!(client.quarantine().inventory()?.lexical, inventory.lexical);
        let metrics = client.observability().metrics_snapshot()?;
        let sealed = metrics
            .gauges
            .iter()
            .find(|gauge| gauge.name == "boot_lexical_sealed_generations")
            .ok_or("missing boot lexical sealed count")?;
        assert_eq!(
            sealed.value.to_bits(),
            1.0_f64.to_bits(),
            "{fault}: only G2 stays sealed"
        );
        let _g2_witness = file_source_hit(
            &client,
            2,
            "neutral",
            ("source-a", EARLY_PATH, EARLY_BODY.as_bytes()),
        )?;
        reopened.stop()?;
    }
    legacy.verify()?;
    Ok(())
}

fn assert_file_body_rejection(
    client: &QuantaIndex,
    fault: &str,
    code: SearchPlaneErrorCodeV2,
) -> TestResult {
    let result = page(client, 1, RETIRED_BODY, 5, None);
    if matches!(&result, Err(error) if matches!(error.downcast_ref::<SdkError>(), Some(SdkError::Remote { code: observed, .. }) if *observed == code))
    {
        return Ok(());
    }
    Err(format!("{fault}: expected {code:?} from G1 FileBody lookup, got {result:?}").into())
}

#[test]
fn delta_delete_noop_and_cold_open_match_independent_full_build() -> TestResult {
    let legacy = LegacySearchdBinary::from_environment()?;
    let incremental_root = private_tempdir()?;
    let incremental = legacy.start(incremental_root.path(), 8)?;
    let client = incremental.connect()?;
    let g1 = publish(&client, batch(1, None, base_scopes()?, &[])?, None)?;

    let base_abc = rows(&client, 1, "abc", 1)?;
    assert_eq!(
        base_abc
            .iter()
            .map(|row| (row.0.as_str(), row.1.as_str()))
            .collect::<Vec<_>>(),
        vec![("source-b", LATE_PATH), ("source-a", EARLY_PATH)]
    );
    let base_full = page(&client, 1, "abc", 10, None)?.results;
    assert_eq!(base_abc, base_full.iter().map(project).collect::<Vec<_>>());
    let witness = file_source_hit(
        &client,
        1,
        "CAFÉ",
        ("source-b", LATE_PATH, SHARED.as_bytes()),
    )?;
    assert_eq!(
        witness
            .preview
            .as_ref()
            .and_then(|preview| preview.original_focus)
            .map(|span| (span.start, span.end)),
        Some((4, 9))
    );
    let retired = ("source-a", RETIRED_PATH, RETIRED_BODY.as_bytes());
    for term in [RETIRED_BODY, RETIRED_BASENAME] {
        let _hit = file_source_hit(&client, 1, term, retired)?;
    }

    // The same bytes change admission in opposite directions at distinct
    // keys. A retired key disappears; untouched early path remains.
    let changes = vec![
        scope("source-b", LATE_PATH, SHARED, false)?,
        scope("source-a", COPY_PATH, SHARED, true)?,
    ];
    let g2 = publish(
        &client,
        batch(2, Some(1), changes, &[("source-a", RETIRED_PATH)])?,
        Some(g1),
    )?;
    let _g3 = publish(&client, batch(3, Some(2), Vec::new(), &[])?, Some(g2))?;
    incremental.stop()?;

    let rebuilt_root = private_tempdir()?;
    let rebuilt = legacy.start(rebuilt_root.path(), 8)?;
    let _full = publish(
        &rebuilt.connect()?,
        batch(2, None, final_scopes()?, &[])?,
        None,
    )?;
    rebuilt.stop()?;

    // Both catalogs and indexes are reopened by new processes.
    let incremental = legacy.start(incremental_root.path(), 8)?;
    let client = incremental.connect()?;
    let rebuilt = legacy.start(rebuilt_root.path(), 8)?;
    let full = rebuilt.connect()?;
    for term in ["abc", "CAFÉ", "neutral"] {
        let expected = rows(&full, 2, term, 1)?;
        assert_eq!(rows(&client, 2, term, 1)?, expected, "delta: {term}");
        assert_eq!(rows(&client, 3, term, 1)?, expected, "no-op: {term}");
        assert_eq!(rows(&client, 2, term, 10)?, expected, "page cut: {term}");
    }
    for term in [RETIRED_BODY, RETIRED_BASENAME] {
        for (client, generation) in [(&client, 2), (&client, 3), (&full, 2)] {
            assert!(
                rows(client, generation, term, 1)?.is_empty(),
                "G{generation}: {term} survived deletion"
            );
        }
    }
    let delta_abc = rows(&client, 2, "abc", 1)?;
    assert_eq!(
        delta_abc
            .first()
            .map(|row| (row.0.as_str(), row.1.as_str())),
        Some(("source-b", LATE_PATH))
    );
    assert_eq!(delta_abc.len(), 3);
    let _untouched = file_source_hit(
        &client,
        2,
        "neutral",
        ("source-a", EARLY_PATH, EARLY_BODY.as_bytes()),
    )?;
    let unicode = file_source_hit(
        &client,
        2,
        "CAFÉ",
        ("source-a", COPY_PATH, SHARED.as_bytes()),
    )?;
    assert_eq!(
        unicode
            .preview
            .as_ref()
            .and_then(|preview| preview.original_focus)
            .map(|span| (span.start, span.end)),
        Some((4, 9))
    );

    rebuilt.stop()?;
    incremental.stop()?;
    legacy.verify()?;
    Ok(())
}

fn raw_scope(path: &str, body: &[u8]) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    Ok(source_fixture::complete_file(
        source_fixture::file_key(&RepoId::new("source-a")?, path),
        &RevisionId::new(SOURCE_REVISION)?,
        LanguageCode::new("rust")?,
        body,
        Vec::new(),
        Vec::new(),
    )?)
}

fn file_source_hit(
    client: &QuantaIndex,
    generation: u64,
    term: &str,
    expected: ExpectedSource<'_>,
) -> TestResult<LexicalCandidate> {
    let page = page(client, generation, term, 1, None)?;
    if page.next_cursor.is_some() {
        return Err(format!("F15 G{generation} {term:?}: unexpected extra result").into());
    }
    let [hit] = <[_; 1]>::try_from(page.results).map_err(|rows| {
        format!(
            "F15 G{generation} {term:?}: expected one result, got {}",
            rows.len()
        )
    })?;
    assert_source(&hit, expected)?;
    Ok(hit)
}

fn assert_source(hit: &LexicalCandidate, (owner, path, body): ExpectedSource<'_>) -> TestResult {
    assert_eq!(
        (hit.source_repo_id.as_str(), hit.repo_relative_path.as_str()),
        (owner, path),
        "F15 {path:?}: file projection differs"
    );
    let source = hit
        .source
        .as_ref()
        .ok_or_else(|| format!("F15 {path:?}: result lacks a Source identity"))?;
    assert_eq!(
        (
            source.file.source_repo_id.as_str(),
            source.file.repo_relative_path.as_str(),
            source.revision_id.as_str(),
            source.source_sha256,
        ),
        (
            owner,
            path,
            SOURCE_REVISION,
            <[u8; 32]>::from(Sha256::digest(body)),
        ),
        "F15 {path:?}: published Source differs"
    );
    Ok(())
}

#[test]
fn nontext_source_stays_path_only_across_cold_delta_noop_and_delete() -> TestResult {
    const BINARY_PATH: &str = "src/binarystable.dat";
    const UTF8_PATH: &str = "src/utf8stable.dat";
    const BINARY_BODY: &[u8] = b"\xff\x00";
    const EXCLUDED_BODY: &str = "hiddenbodytoken";
    const ADMITTED_BODY: &str = "visiblebodytoken";
    let root = private_tempdir()?;
    let legacy = LegacySearchdBinary::from_environment()?;
    let process = legacy.start(root.path(), 8)?;
    let mut initial = source_fixture::sealed_batch(
        &repo()?,
        &revision()?,
        ManifestGeneration::new(1),
        vec![
            raw_scope(BINARY_PATH, BINARY_BODY)?,
            raw_scope(UTF8_PATH, EXCLUDED_BODY.as_bytes())?,
        ],
    )?;
    current_source_fixture::finish_batch(&mut initial)?;
    let g1 = publish(&process.connect()?, initial, None)?;
    process.stop()?;

    let process = legacy.start(root.path(), 8)?;
    let client = process.connect()?;
    let binary_source = ("source-a", BINARY_PATH, BINARY_BODY);
    let excluded_source = ("source-a", UTF8_PATH, EXCLUDED_BODY.as_bytes());
    let admitted_source = ("source-a", BINARY_PATH, ADMITTED_BODY.as_bytes());
    let assert_g1_path_only = |client: &QuantaIndex| -> TestResult {
        let _binary = file_source_hit(client, 1, "binarystable", binary_source)?;
        let _utf8 = file_source_hit(client, 1, "utf8stable", excluded_source)?;
        for term in [EXCLUDED_BODY, ADMITTED_BODY] {
            assert!(rows(client, 1, term, 1)?.is_empty(), "G1 returned {term}");
        }
        Ok(())
    };
    assert_g1_path_only(&client)?;

    let g2 = publish(
        &client,
        batch(
            2,
            Some(1),
            vec![scope("source-a", BINARY_PATH, ADMITTED_BODY, true)?],
            &[],
        )?,
        Some(g1),
    )?;
    let g3 = publish(&client, batch(3, Some(2), Vec::new(), &[])?, Some(g2))?;
    process.stop()?;

    let process = legacy.start(root.path(), 8)?;
    let client = process.connect()?;
    for generation in [2, 3] {
        let _binary_path = file_source_hit(&client, generation, "binarystable", admitted_source)?;
        let _binary_body = file_source_hit(&client, generation, ADMITTED_BODY, admitted_source)?;
        let _utf8_path = file_source_hit(&client, generation, "utf8stable", excluded_source)?;
        assert!(rows(&client, generation, EXCLUDED_BODY, 1)?.is_empty());
    }
    let _g4 = publish(
        &client,
        batch(
            4,
            Some(3),
            Vec::new(),
            &[("source-a", BINARY_PATH), ("source-a", UTF8_PATH)],
        )?,
        Some(g3),
    )?;
    process.stop()?;

    let process = legacy.start(root.path(), 8)?;
    let client = process.connect()?;
    for term in ["binarystable", "utf8stable", EXCLUDED_BODY, ADMITTED_BODY] {
        assert!(rows(&client, 4, term, 1)?.is_empty());
    }
    assert_g1_path_only(&client)?;
    process.stop()?;
    legacy.verify()?;
    Ok(())
}
