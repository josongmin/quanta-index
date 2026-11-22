//! QI-BB-025 W4 — runtime-metadata and structural pages walk by keyset
//! cursor at the daemon front door.
//!
//! Each route's pages partition its result set in candidate-id order,
//! once each, and every page names the epochs it was cut from: a
//! continuation is served from exactly those epochs, so an ingest that
//! lands between two pages — rows that would sort into the next page —
//! is invisible to the walk while a fresh walk sees it; and a cursor
//! whose epoch the plane no longer retains is refused typed rather than
//! served from a newer snapshot.
//!
//! The oracle is the fixture's own chunk ids and their byte order;
//! nothing here reads the daemon's clock or waits on timing.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::path::Path;

use quanta_index_contract::{
    BatchIngestMode, CandidateCountV1, ChunkId, ContinuationTokenV2, DiagnosticAttemptFieldV1,
    GenerationPin, ManifestGeneration, QueryConstraintSetV1, QueryResultWindowV2, RepoId,
    RepoRelativePath, RevisionId, RuntimeMetadataQueryRequest, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneRuntimeMetadataQueryResponse,
    SearchPlaneStructuralQueryResponse, SearchScopeKey, SearchScopeSurface, SourceFileKey,
    StructuralIngestBatch, StructuralQueryRequest, StructuralReplaceScope, StructuralTreeRecord,
    TextQueryRequest, TextQuerySyntax,
    lex::{
        DirtyRecord, LanguageCode, ParseNode, ParseRoleTag, ParseTreeRecord,
        compute_parse_tree_source_hash,
    },
};
use quanta_index_core::{AUX_EPOCH_EXPIRED_CODE, AUX_EPOCH_RETAIN};
use quanta_index_ipc::{ClientIoPolicy, stamp_batch_digest_v1};
use quanta_index_sdk::{DirtyBatch, PublishedBatchEvidence, QuantaIndex, SdkError};
use quanta_index_searchd_harness::legacy_reference::{
    LegacySearchdBinary, daemon_socket_paths, legacy_source_batch,
};

type TestResult = Result<(), Box<dyn Error>>;

/// Chunks in the generation.
const CHUNKS: usize = 30;
/// Of those, the rows in the walked set before the walk starts.
const ORIGINAL: usize = 25;
/// The rows that land between page one and page two: five ids that, in
/// candidate-id order, sort into page two of the original walk.
const LATE_START: usize = 12;
const LATE_END: usize = 17;
/// The page size: three pages of 10 / 10 / 5.
const PAGE: u32 = 10;
/// Wide enough to hold the whole set in one page.
const WHOLE: u32 = 100;
const RUNTIME_QUERY: &str = "dirty:only needle";
const STRUCTURAL_QUERY: &str = "match { function_item { { identifier :[name] } } }";

fn content_of(index: usize) -> String {
    format!("fn walk_{index}() {{ let needle = {index}; }}")
}

fn identifier_of(index: usize) -> String {
    format!("walk_{index}")
}

/// The generation's chunks: every candidate id with the path and index
/// it was ingested for, in candidate-id (byte) order.
struct Chunks {
    by_id: BTreeMap<String, (String, usize)>,
}

impl Chunks {
    fn new() -> Result<Self, Box<dyn Error>> {
        let mut by_id = BTreeMap::new();
        for index in 0..CHUNKS {
            let path = format!("src/walk_{index:02}.rs");
            let id = format!("e2e-{}-{path}", index + 1);
            if by_id.insert(id, (path, index)).is_some() {
                return Err("fixture constructed duplicate candidate ids".into());
            }
        }
        Ok(Self { by_id })
    }

    fn files(&self) -> Vec<(String, String, u64)> {
        (0..CHUNKS)
            .map(|index| {
                (
                    format!("src/walk_{index:02}.rs"),
                    content_of(index),
                    u64::try_from(index + 1).expect("fixed fixture index fits u64"),
                )
            })
            .collect()
    }

    fn ids(&self) -> Vec<String> {
        self.by_id.keys().cloned().collect()
    }

    /// The ids of the walked set before the walk: every id but the late
    /// ones.
    fn original(&self) -> Vec<String> {
        self.ids()
            .into_iter()
            .enumerate()
            .filter(|(position, _)| !Self::is_late(*position))
            .map(|(_, id)| id)
            .collect()
    }

    /// The ids that land between page one and page two.
    fn late(&self) -> Vec<String> {
        self.ids()
            .into_iter()
            .enumerate()
            .filter(|(position, _)| Self::is_late(*position))
            .map(|(_, id)| id)
            .collect()
    }

    const fn is_late(position: usize) -> bool {
        position >= LATE_START && position < LATE_END
    }

    fn path_and_index(&self, id: &str) -> Result<(&str, usize), Box<dyn Error>> {
        self.by_id
            .get(id)
            .map(|(path, index)| (path.as_str(), *index))
            .ok_or_else(|| format!("unknown candidate id {id}").into())
    }
}

fn with_source_reference(
    run: impl FnOnce(
        &QuantaIndex,
        &Path,
        &Chunks,
        &GenerationPin,
        &PublishedBatchEvidence,
    ) -> TestResult,
) -> TestResult {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let process = binary.start(&state_root, 8)?;
    let result = (|| -> TestResult {
        let client = process.connect()?;
        let chunks = Chunks::new()?;
        let files = chunks.files();
        let file_refs = files
            .iter()
            .map(|(path, body, id)| (path.as_str(), body.as_str(), *id))
            .collect::<Vec<_>>();
        let repo = RepoId::new("repo")?;
        let revision = RevisionId::new("rev-e2e")?;
        let batch = legacy_source_batch(&repo, &revision, 1, None, &file_refs)?;
        let outcome = client.search_corpus().publish_outcome(&batch)?;
        if !outcome.receipt.applied || outcome.receipt.durable_sequence == 0 {
            return Err("old-Q source fixture was not durably published".into());
        }
        let pin = GenerationPin::new(repo, revision, ManifestGeneration::new(1));
        run(
            &client,
            &state_root,
            &chunks,
            &pin,
            &PublishedBatchEvidence::from(&outcome),
        )
    })();
    let stopped = process.stop();
    result.and(stopped)?;
    binary.verify()
}

fn activate_source(
    client: &QuantaIndex,
    pin: &GenerationPin,
    evidence: &PublishedBatchEvidence,
) -> TestResult {
    let activation = client.search_corpus().activate_published(evidence, None)?;
    let observed = client
        .generations()
        .active_head(pin.repo_id.clone(), pin.revision_id.clone())?
        .ok_or("old-Q source activation has no active head")?;
    if observed != activation.active
        || observed.generation.lexical.manifest_generation != pin.manifest_generation
    {
        return Err("old-Q source activation differs from its durable publication".into());
    }
    Ok(())
}

fn publish_dirty(
    client: &QuantaIndex,
    pin: &GenerationPin,
    id: &str,
    applied_at_ms: u64,
) -> TestResult {
    let batch = DirtyBatch::new(
        pin.repo_id.clone(),
        pin.revision_id.clone(),
        pin.manifest_generation,
        applied_at_ms,
    )
    .upsert(DirtyRecord {
        wire_version: 1,
        doc_id: ChunkId::new(id),
        applied_at_ms,
        payload_hash: [0x5a; 32],
    });
    let receipt = client.runtime().publish_dirty(&batch)?;
    if !receipt.applied || receipt.durable_sequence == 0 {
        return Err("dirty fixture was not durably published".into());
    }
    Ok(())
}

fn distinct(rows: &[String]) -> BTreeSet<&String> {
    rows.iter().collect()
}

/// The rows a page of the original walk must hold: `rows[range]`, or the
/// fixture is not the size the walk assumes.
fn page_rows(rows: &[String], range: std::ops::Range<usize>) -> Result<&[String], Box<dyn Error>> {
    rows.get(range.clone())
        .ok_or_else(|| format!("the original set has no rows {range:?}").into())
}

/// What one served page must satisfy against the oracle: its rows, its
/// window and whether it continues.
fn check_page(
    what: &str,
    rows: &[String],
    window: &QueryResultWindowV2,
    expected_rows: &[String],
    expected_count: CandidateCountV1,
    continues: bool,
) -> TestResult {
    if rows != expected_rows {
        return Err(
            format!("{what}: rows drifted: got {rows:?}, expected {expected_rows:?}").into(),
        );
    }
    if window.candidate_count() != expected_count {
        return Err(format!(
            "{what}: candidate count {:?}, expected {expected_count:?}",
            window.candidate_count()
        )
        .into());
    }
    if window.has_more() != Some(continues) {
        return Err(format!(
            "{what}: has_more={:?} with cursor_present={continues}",
            window.has_more()
        )
        .into());
    }
    Ok(())
}

// ---------------------------------------------------------------------
// runtime-metadata
// ---------------------------------------------------------------------

fn runtime_ids(page: &SearchPlaneRuntimeMetadataQueryResponse) -> Vec<String> {
    page.results
        .iter()
        .map(|row| row.candidate_id.clone())
        .collect()
}

fn runtime_page(
    client: &QuantaIndex,
    pin: &GenerationPin,
    top_k: u32,
    cursor: Option<ContinuationTokenV2>,
) -> Result<SearchPlaneRuntimeMetadataQueryResponse, Box<dyn Error>> {
    Ok(client
        .runtime()
        .query_request(RuntimeMetadataQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: RUNTIME_QUERY.into(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k,
                cursor: None,
            },
            cursor,
        })?)
}

#[test]
fn runtime_metadata_pages_walk_the_dirty_overlay_once_and_pin_their_epochs() -> TestResult {
    with_source_reference(|client, _root, chunks, pin, evidence| {
        let original = chunks.original();
        let late = chunks.late();
        if original.len() != ORIGINAL || original.len().saturating_add(late.len()) != CHUNKS {
            return Err(
                format!("fixture split drifted: {} / {}", original.len(), late.len()).into(),
            );
        }
        for id in &original {
            publish_dirty(client, pin, id, 5)?;
        }
        activate_source(client, pin, evidence)?;

        // The whole set in one page: the oracle for the walk.
        let whole = runtime_page(client, pin, WHOLE, None)?;
        check_page(
            "whole",
            &runtime_ids(&whole),
            &whole.window,
            &original,
            CandidateCountV1::Exact(u64::try_from(ORIGINAL)?),
            false,
        )?;
        if whole.examined != u64::try_from(ORIGINAL)? {
            return Err(format!(
                "the whole walk examines every dirty row: {}",
                whole.examined
            )
            .into());
        }

        // Page one stops at its probe row.
        let first = runtime_page(client, pin, PAGE, None)?;
        let walk_epochs = (first.read_epoch, first.universe_epoch);
        let cursor = first
            .next_cursor
            .clone()
            .ok_or("25 rows continue past page one")?;
        check_page(
            "page one",
            &runtime_ids(&first),
            &first.window,
            page_rows(&original, 0..10)?,
            CandidateCountV1::AtLeast(11),
            true,
        )?;
        if first.examined != 11 {
            return Err(format!(
                "page one examines the page and its probe: {}",
                first.examined
            )
            .into());
        }

        // Five rows that sort into page two land between page one and two.
        for id in &late {
            publish_dirty(client, pin, id, 6)?;
        }

        // Pages two and three read the walk's epochs: exactly the original
        // rows page one did not return, none of the late ones.
        let second = runtime_page(client, pin, PAGE, Some(cursor.clone()))?;
        if (second.read_epoch, second.universe_epoch) != walk_epochs {
            return Err(format!(
                "page two reads the walk's epochs {walk_epochs:?}, read {:?}",
                (second.read_epoch, second.universe_epoch)
            )
            .into());
        }
        let second_cursor = second.next_cursor.clone().ok_or("page two continues")?;
        check_page(
            "page two",
            &runtime_ids(&second),
            &second.window,
            page_rows(&original, 10..20)?,
            CandidateCountV1::AtLeast(11),
            true,
        )?;
        let third = runtime_page(client, pin, PAGE, Some(second_cursor))?;
        if (third.read_epoch, third.universe_epoch) != walk_epochs {
            return Err("page three reads the walk's epochs".into());
        }
        check_page(
            "page three",
            &runtime_ids(&third),
            &third.window,
            page_rows(&original, 20..ORIGINAL)?,
            CandidateCountV1::Exact(5),
            false,
        )?;
        if third.examined != 5 {
            return Err(format!("the last page examines what is left: {}", third.examined).into());
        }
        let mut walked = runtime_ids(&first);
        walked.extend(runtime_ids(&second));
        walked.extend(runtime_ids(&third));
        if walked != original || distinct(&walked).len() != walked.len() {
            return Err(format!("the walk is the original rows once each: {walked:?}").into());
        }
        if walked.iter().any(|id| late.contains(id)) {
            return Err("a row ingested after the walk started leaked into it".into());
        }

        // A fresh walk reads the current epochs and sees all thirty.
        let fresh = runtime_page(client, pin, WHOLE, None)?;
        if fresh.read_epoch <= walk_epochs.0 {
            return Err(format!(
                "five ingests advance the runtime epoch past {:?}, fresh read {:?}",
                walk_epochs.0, fresh.read_epoch
            )
            .into());
        }
        check_page(
            "fresh whole",
            &runtime_ids(&fresh),
            &fresh.window,
            &chunks.ids(),
            CandidateCountV1::Exact(u64::try_from(CHUNKS)?),
            false,
        )?;

        // `AUX_EPOCH_RETAIN` more overlay mutations push the walk's runtime
        // epoch out of retention: the old cursor is refused typed, never
        // served from the current snapshot.
        for step in 0..AUX_EPOCH_RETAIN {
            let id = original.first().ok_or("the original set is not empty")?;
            publish_dirty(client, pin, id, 7u64.saturating_add(u64::try_from(step)?))?;
        }
        match runtime_page(client, pin, PAGE, Some(cursor)) {
            Err(error) if matches!(error.downcast_ref::<SdkError>(), Some(SdkError::Remote { code, .. }) if *code == AUX_EPOCH_EXPIRED_CODE) => {
                Ok(())
            }
            Err(error) => Err(format!(
                "a pruned epoch is refused {AUX_EPOCH_EXPIRED_CODE}, got {error}"
            )
            .into()),
            Ok(page) => Err(format!(
                "a pruned epoch must not be served from the current snapshot: {:?}",
                runtime_ids(&page)
            )
            .into()),
        }
    })
}

// ---------------------------------------------------------------------
// structural
// ---------------------------------------------------------------------

fn function_tree(content: &str, identifier: &str) -> Result<ParseTreeRecord, Box<dyn Error>> {
    let identifier_start = content
        .find(identifier)
        .ok_or("fixture identifier is missing")?;
    let identifier_end = identifier_start + identifier.len();
    let byte_end = u32::try_from(content.len())?;
    let identifier_start = u32::try_from(identifier_start)?;
    let identifier_end = u32::try_from(identifier_end)?;
    let block_start = byte_end.saturating_sub(2);
    Ok(ParseTreeRecord {
        wire_version: 1,
        lang: LanguageCode::new("rust")?,
        root: ParseNode {
            kind: "function_item".into(),
            byte_start: 0,
            byte_end,
            children: vec![
                ParseNode {
                    kind: "identifier".into(),
                    byte_start: identifier_start,
                    byte_end: identifier_end,
                    children: Vec::new(),
                },
                ParseNode {
                    kind: "block".into(),
                    byte_start: block_start,
                    byte_end,
                    children: Vec::new(),
                },
            ],
        },
        source_hash: compute_parse_tree_source_hash(content),
        role_tag_schema_version: 1,
        role_tags: vec![
            ParseRoleTag {
                role: "item".into(),
                byte_start: 0,
                byte_end,
            },
            ParseRoleTag {
                role: "expr".into(),
                byte_start: identifier_start,
                byte_end: identifier_end,
            },
            ParseRoleTag {
                role: "stmt".into(),
                byte_start: block_start,
                byte_end,
            },
        ],
    })
}

fn structural_ids(page: &SearchPlaneStructuralQueryResponse) -> Vec<String> {
    page.results
        .iter()
        .map(|row| row.candidate_id.clone())
        .collect()
}

fn structural_page(
    client: &QuantaIndex,
    pin: &GenerationPin,
    top_k: u32,
    cursor: Option<ContinuationTokenV2>,
) -> Result<SearchPlaneStructuralQueryResponse, Box<dyn Error>> {
    Ok(client.structural().query_request(StructuralQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: STRUCTURAL_QUERY.into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: Some(pin.clone()),
            generation_selector: None,
            top_k,
            cursor: None,
        },
        cursor,
    })?)
}

/// Publish the function tree of the chunk `id` into `generation`.
fn publish_tree(
    state_root: &Path,
    pin: &GenerationPin,
    chunks: &Chunks,
    id: &str,
    sequence: &mut u64,
) -> TestResult {
    let (path, index) = chunks.path_and_index(id)?;
    *sequence = sequence
        .checked_add(1)
        .ok_or("structural sequence overflow")?;
    let mut batch = StructuralIngestBatch {
        repo_id: pin.repo_id.clone(),
        revision_id: pin.revision_id.clone(),
        generation: pin.manifest_generation,
        base_generation: None,
        manifest_digest: format!("struct:{path}:{}", pin.manifest_generation.get()),
        batch_digest: String::new(),
        mode: BatchIngestMode::Delta,
        replace_scopes: vec![StructuralReplaceScope {
            scope: SearchScopeKey {
                doc_surface: SearchScopeSurface::Chunk,
                source_file: SourceFileKey {
                    source_repo_id: pin.repo_id.clone(),
                    repo_relative_path: RepoRelativePath::new(path),
                },
            },
            scope_digest: format!("struct-scope:{path}:{sequence}"),
            trees: vec![StructuralTreeRecord {
                chunk_id: ChunkId::new(id),
                record: function_tree(&content_of(index), &identifier_of(index))?,
            }],
        }],
        tombstone_scopes: Vec::new(),
        seal: false,
    };
    stamp_batch_digest_v1(&mut batch)?;
    let expected_digest = batch.batch_digest.clone();
    let expected_manifest = batch.manifest_digest.clone();
    let request = SearchPlaneIngestIpcRequestEnvelope {
        request_id: *sequence + 1000,
        payload: SearchPlaneIngestIpcRequest::PublishStructuralBatch(batch),
        diagnostic_attempt: DiagnosticAttemptFieldV1::Absent,
    };
    let response: SearchPlaneIngestIpcResponseEnvelope = quanta_index_ipc::send_request(
        &daemon_socket_paths(state_root)[2],
        &request,
        ClientIoPolicy::default(),
    )?;
    if response.request_id != request.request_id {
        return Err("structural reply request ID differs".into());
    }
    match response.payload {
        SearchPlaneIngestIpcResponse::StructuralReceipt(receipt)
            if receipt.applied
                && receipt.durable_sequence > 0
                && receipt.generation == pin.manifest_generation
                && receipt.batch_digest == expected_digest
                && receipt.manifest_digest.as_deref() == Some(expected_manifest.as_str())
                && receipt.accepted_replace_scopes == 1
                && receipt.accepted_tombstone_scopes == 0
                && !receipt.sealed => {}
        other => {
            return Err(format!("structural publish was not durably applied: {other:?}").into());
        }
    }
    Ok(())
}

#[test]
fn structural_pages_walk_the_match_set_once_and_pin_their_epoch() -> TestResult {
    with_source_reference(|client, state_root, chunks, pin, evidence| {
        let mut sequence = 0_u64;
        let original = chunks.original();
        let late = chunks.late();
        for id in &original {
            publish_tree(state_root, pin, chunks, id, &mut sequence)?;
        }
        activate_source(client, pin, evidence)?;

        // The whole match set in one page: the oracle for the walk.
        let whole = structural_page(client, pin, WHOLE, None)?;
        check_page(
            "whole",
            &structural_ids(&whole),
            &whole.window,
            &original,
            CandidateCountV1::Exact(u64::try_from(ORIGINAL)?),
            false,
        )?;

        // Page one: the count after the cursor is exact, the whole match set
        // is walked.
        let first = structural_page(client, pin, PAGE, None)?;
        let walk_epoch = first.read_epoch;
        let cursor = first
            .next_cursor
            .clone()
            .ok_or("25 matches continue past page one")?;
        check_page(
            "page one",
            &structural_ids(&first),
            &first.window,
            page_rows(&original, 0..10)?,
            CandidateCountV1::Exact(u64::try_from(ORIGINAL)?),
            true,
        )?;
        if first.examined != u64::try_from(ORIGINAL)? {
            return Err(format!("page one walks the whole match set: {}", first.examined).into());
        }

        // Five matches that sort into page two land between page one and two.
        for id in &late {
            publish_tree(state_root, pin, chunks, id, &mut sequence)?;
        }

        // Pages two and three are evaluated against the walk's epoch.
        let second = structural_page(client, pin, PAGE, Some(cursor.clone()))?;
        if second.read_epoch != walk_epoch {
            return Err(format!(
                "page two reads the walk's epoch {walk_epoch}, read {}",
                second.read_epoch
            )
            .into());
        }
        let second_cursor = second.next_cursor.clone().ok_or("page two continues")?;
        check_page(
            "page two",
            &structural_ids(&second),
            &second.window,
            page_rows(&original, 10..20)?,
            CandidateCountV1::Exact(15),
            true,
        )?;
        if second.examined != u64::try_from(ORIGINAL)? {
            return Err(format!(
                "page two walks the walk's match set, not the current one: {}",
                second.examined
            )
            .into());
        }
        let third = structural_page(client, pin, PAGE, Some(second_cursor))?;
        if third.read_epoch != walk_epoch {
            return Err("page three reads the walk's epoch".into());
        }
        check_page(
            "page three",
            &structural_ids(&third),
            &third.window,
            page_rows(&original, 20..ORIGINAL)?,
            CandidateCountV1::Exact(5),
            false,
        )?;
        let mut walked = structural_ids(&first);
        walked.extend(structural_ids(&second));
        walked.extend(structural_ids(&third));
        if walked != original || distinct(&walked).len() != walked.len() {
            return Err(format!("the walk is the original matches once each: {walked:?}").into());
        }
        if walked.iter().any(|id| late.contains(id)) {
            return Err("a match ingested after the walk started leaked into it".into());
        }

        // A fresh walk reads the current epoch and sees all thirty.
        let fresh = structural_page(client, pin, WHOLE, None)?;
        if fresh.read_epoch <= walk_epoch {
            return Err(format!(
                "five ingests advance the structural epoch past {walk_epoch}, fresh read {}",
                fresh.read_epoch
            )
            .into());
        }
        check_page(
            "fresh whole",
            &structural_ids(&fresh),
            &fresh.window,
            &chunks.ids(),
            CandidateCountV1::Exact(u64::try_from(CHUNKS)?),
            false,
        )?;

        // `AUX_EPOCH_RETAIN` more structural mutations push the walk's epoch
        // out of retention: the old cursor is refused typed.
        for _step in 0..AUX_EPOCH_RETAIN {
            let id = original.first().ok_or("the original set is not empty")?;
            publish_tree(state_root, pin, chunks, id, &mut sequence)?;
        }
        match structural_page(client, pin, PAGE, Some(cursor)) {
            Err(error) if matches!(error.downcast_ref::<SdkError>(), Some(SdkError::Remote { code, .. }) if *code == AUX_EPOCH_EXPIRED_CODE) => {
                Ok(())
            }
            Err(error) => Err(format!(
                "a pruned epoch is refused {AUX_EPOCH_EXPIRED_CODE}, got {error}"
            )
            .into()),
            Ok(page) => Err(format!(
                "a pruned epoch must not be served from the current snapshot: {:?}",
                structural_ids(&page)
            )
            .into()),
        }
    })
}
