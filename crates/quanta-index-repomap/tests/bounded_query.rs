//! QI-BB-008 — the `RepoMap` query is a bounded selection over a shared,
//! indexed snapshot, and it answers exactly what the full sort answered.
//!
//! Two oracles. The reference query below is the previous engine — clone
//! every entry, sort them all, walk with the budget — kept as the definition
//! of the ranking; the bounded engine must agree with it on every randomized
//! snapshot and request, focus or none, budget tight or loose. And the
//! response for a fixed `top_k` must be the same size whether the snapshot
//! holds a thousand entries or ten thousand: the rows come from the page,
//! the rest is a count.

#![forbid(unsafe_code)]
#![expect(
    clippy::unreachable,
    reason = "test fixtures use invariant literal constructors for repo and revision IDs"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;

use quanta_index_contract::{
    ManifestGeneration, RepoId, RepoMapDocType, RepoMapExactnessSummary, RepoMapFocusSubjectDto,
    RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapQueryRequest,
    RepoMapRedactionState, RepoMapSnapshotMeta, RevisionId,
};
use quanta_index_repomap::{
    RepoMapEntry, RepoMapIndexedSnapshot, RepoMapQueryEngine, RepoMapSnapshot,
};

type TestResult = Result<(), Box<dyn Error>>;

/// A small deterministic generator; the fixtures must be reproducible.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next()
            .checked_rem(bound.max(1))
            .map_or(0, |value| value)
    }

    /// A value in `0..bound`, as the small integer it is.
    fn below_u32(&mut self, bound: u32) -> u32 {
        u32::try_from(self.below(u64::from(bound))).map_or(0, |value| value)
    }

    /// A position in `0..len`.
    fn index(&mut self, len: usize) -> usize {
        let bound = u64::try_from(len).map_or(u64::MAX, |value| value);
        usize::try_from(self.below(bound)).map_or(0, |value| value)
    }

    fn word(&mut self, fallback: &'static str, spread: usize) -> &'static str {
        let position = self.index(spread);
        WORDS.get(position).copied().map_or(fallback, |word| word)
    }
}

const WORDS: &[&str] = &[
    "alpha", "beta", "gamma", "delta", "query", "owner", "runtime", "index", "map", "search",
];

fn entry(rng: &mut Lcg, position: u64) -> RepoMapEntry {
    let owner_path = format!("src/dir{}/file{}.rs", rng.below(7), rng.below(5));
    let is_symbol = rng.below(2) == 1;
    let subject_identity = if is_symbol {
        format!("{owner_path}::sym{position}")
    } else {
        format!("{owner_path}#{position}")
    };
    let search_text = (0..3)
        .map(|_| rng.word("x", 10))
        .collect::<Vec<_>>()
        .join(" ");
    let final_score_millis = rng.below_u32(50).saturating_mul(20);
    RepoMapEntry {
        subject_identity,
        subject_doc_type: if is_symbol {
            RepoMapDocType::Symbol
        } else {
            RepoMapDocType::File
        },
        subject_kind: if is_symbol { "function" } else { "file" }.to_string(),
        owner_path,
        score: 0.0,
        final_score_millis,
        importance_score_millis: 0,
        utility_score_millis: 0,
        freshness_score_millis: 0,
        evidence_priority_millis: 0,
        token_budget_hint: rng.below_u32(120).saturating_add(1),
        contributing_signals: BTreeMap::new(),
        projection_evidence_kind: "bundle".to_string(),
        projection_authority_artifact_id: "artifact".to_string(),
        projection_authority_digest: "digest".to_string(),
        projection_status: "fresh".to_string(),
        redaction_state: RepoMapRedactionState::Unredacted,
        search_text,
        source_symbol_count: 0,
        source_chunk_token_total: 0,
        source_call_incoming_edges: 0,
        source_call_outgoing_edges: 0,
        source_import_incoming_edges: 0,
        source_import_outgoing_edges: 0,
    }
}

fn snapshot(rng: &mut Lcg, entries: u64) -> RepoMapSnapshot {
    let mut seen = BTreeSet::new();
    let mut rows = Vec::new();
    for position in 0..entries {
        let row = entry(rng, position);
        if seen.insert((row.subject_identity.clone(), row.subject_doc_type)) {
            rows.push(row);
        }
    }
    let repo = match RepoId::new("repo") {
        Ok(repo) => repo,
        Err(err) => unreachable!("static fixture ID satisfies canonical policy: {err}"),
    };
    let revision = match RevisionId::new("rev") {
        Ok(revision) => revision,
        Err(err) => unreachable!("static fixture ID satisfies canonical policy: {err}"),
    };
    RepoMapSnapshot {
        repo_id: repo,
        revision_id: revision,
        manifest_generation: ManifestGeneration::new(1),
        snapshot_meta: RepoMapSnapshotMeta {
            snapshot_id: "snap".to_string(),
            projection_version: 1,
            authority_digest: "authority".to_string(),
            item_index_availability: RepoMapItemIndexAvailability::Full,
            graph_coverage_class: RepoMapGraphCoverageClass::Full,
            exactness_summary: RepoMapExactnessSummary::Exact,
        },
        entries: rows,
    }
}

fn request(rng: &mut Lcg, snapshot: &RepoMapSnapshot) -> RepoMapQueryRequest {
    // Twelve slots over ten words: some terms match nothing at all.
    let terms = (0..rng.below(3))
        .map(|_| rng.word("zzz", 12))
        .collect::<Vec<_>>()
        .join(" ");
    // Focus subjects are always resolvable here: an unresolved focus is a
    // typed refusal (see `an_unresolved_focus_subject_is_a_typed_refusal`),
    // not a degraded page.
    let mut focus_subjects = Vec::new();
    for _ in 0..rng.below(3) {
        if let Some(picked) = snapshot.entries.get(rng.index(snapshot.entries.len())) {
            focus_subjects.push(RepoMapFocusSubjectDto {
                subject_identity: picked.subject_identity.clone(),
                subject_doc_type: picked.subject_doc_type,
            });
        }
    }
    RepoMapQueryRequest {
        repo_id: snapshot.repo_id.clone(),
        revision_id: snapshot.revision_id.clone(),
        manifest_generation: snapshot.manifest_generation,
        query_text: if terms.is_empty() {
            "q".to_string()
        } else {
            terms
        },
        top_k: rng.below_u32(12).saturating_add(1),
        token_budget: rng.below_u32(600).saturating_add(1),
        focus_subjects,
    }
}

/// The page the previous engine produced: identities with ranks, the
/// dropped count, and both reason sets.
#[derive(Debug, PartialEq, Eq)]
struct Page {
    rows: Vec<(String, u32)>,
    dropped: u32,
    drop_reasons: Vec<String>,
    degraded_reasons: Vec<String>,
}

fn tokenize(query_text: &str) -> Vec<String> {
    quanta_index_lq_text_normalizer::tokenize(
        query_text,
        quanta_index_lq_text_normalizer::CaseMode::Folded,
    )
    .indexable()
    .map(|token| token.text.clone())
    .collect()
}

/// The previous engine, verbatim in behaviour: clone, filter, full sort,
/// budget walk over every entry.
fn reference_query(snapshot: &RepoMapSnapshot, request: &RepoMapQueryRequest) -> Page {
    let query_terms = tokenize(&request.query_text);
    let focus_keys = request
        .focus_subjects
        .iter()
        .map(|focus| (focus.subject_identity.clone(), focus.subject_doc_type))
        .collect::<BTreeSet<_>>();
    let mut entries = snapshot.entries.clone();
    let focus_owner_paths = entries
        .iter()
        .filter(|entry| {
            focus_keys.contains(&(entry.subject_identity.clone(), entry.subject_doc_type))
        })
        .map(|entry| entry.owner_path.clone())
        .collect::<BTreeSet<_>>();
    let mut degraded = BTreeSet::new();
    if !focus_keys.is_empty() && focus_owner_paths.is_empty() {
        let _inserted = degraded.insert("focus_subjects_unresolved".to_string());
    }
    if !focus_owner_paths.is_empty() {
        entries.retain(|entry| {
            focus_keys.contains(&(entry.subject_identity.clone(), entry.subject_doc_type))
                || focus_owner_paths.contains(&entry.owner_path)
        });
    }
    let score = |entry: &RepoMapEntry| -> u32 {
        if query_terms.is_empty() {
            return 0;
        }
        let haystack = quanta_index_lq_text_normalizer::fold(entry.search_text.as_str());
        u32::try_from(
            query_terms
                .iter()
                .filter(|term| haystack.contains(term.as_str()))
                .count(),
        )
        .map_or(u32::MAX, |count| count)
    };
    if !query_terms.is_empty() && !entries.iter().any(|entry| score(entry) > 0) {
        let _inserted = degraded.insert("query_terms_unmatched".to_string());
    }
    entries.sort_by(|lhs, rhs| {
        let exact = |entry: &RepoMapEntry| {
            focus_keys.contains(&(entry.subject_identity.clone(), entry.subject_doc_type))
        };
        let owner = |entry: &RepoMapEntry| focus_owner_paths.contains(&entry.owner_path);
        exact(rhs)
            .cmp(&exact(lhs))
            .then(owner(rhs).cmp(&owner(lhs)))
            .then(score(rhs).cmp(&score(lhs)))
            .then(rhs.final_score_millis.cmp(&lhs.final_score_millis))
            .then(lhs.subject_identity.cmp(&rhs.subject_identity))
    });
    let top_k = usize::try_from(request.top_k).map_or(usize::MAX, |limit| limit);
    let mut rows = Vec::new();
    let mut consumed = 0_u32;
    let mut drop_reasons = BTreeSet::new();
    let mut floor = false;
    for entry in &entries {
        let hint = entry.token_budget_hint.max(1);
        let within_top_k = rows.len() < top_k;
        let within_budget = consumed.saturating_add(hint) <= request.token_budget;
        let apply_floor = rows.is_empty() && within_top_k && !within_budget;
        if within_top_k && (within_budget || apply_floor) {
            floor |= apply_floor;
            consumed = consumed.saturating_add(hint);
            rows.push((
                entry.subject_identity.clone(),
                u32::try_from(rows.len().saturating_add(1)).map_or(u32::MAX, |rank| rank),
            ));
            continue;
        }
        let _inserted = drop_reasons.insert(
            if within_top_k {
                "token_budget_exhausted"
            } else {
                "top_k_exhausted"
            }
            .to_string(),
        );
    }
    if floor {
        let _inserted = degraded.insert("token_budget_floor_applied".to_string());
    }
    Page {
        dropped: u32::try_from(entries.len().saturating_sub(rows.len()))
            .map_or(u32::MAX, |count| count),
        rows,
        drop_reasons: drop_reasons.into_iter().collect(),
        degraded_reasons: degraded.into_iter().collect(),
    }
}

#[test]
fn the_bounded_engine_agrees_with_the_full_sort_on_every_randomized_request() -> TestResult {
    let mut rng = Lcg(0x5EED_1234_ABCD_0001);
    for round in 0..120_u64 {
        let size = 1 + rng.below(if round % 10 == 0 { 400 } else { 40 });
        let snapshot = snapshot(&mut rng, size);
        let indexed = RepoMapIndexedSnapshot::new(snapshot.clone());
        for _ in 0..6 {
            let request = request(&mut rng, &snapshot);
            let expected = reference_query(&snapshot, &request);
            let response = RepoMapQueryEngine::query(&indexed, &request)?;
            let actual = Page {
                rows: response
                    .entries
                    .iter()
                    .map(|row| (row.subject_identity.clone(), row.rank))
                    .collect(),
                dropped: response.dropped_entries_count,
                drop_reasons: response.drop_reason_codes.clone(),
                degraded_reasons: response.degraded_reason_codes.clone(),
            };
            if actual != expected {
                return Err(format!(
                    "round {round}: bounded engine diverged for {request:?}\nexpected {expected:?}\nactual   {actual:?}"
                )
                .into());
            }
        }
    }
    Ok(())
}

#[test]
fn a_fixed_top_k_response_does_not_grow_with_the_snapshot() -> TestResult {
    // The same thousand ranked entries, then nine thousand more that rank
    // below every one of them: the page is identical, so any growth in the
    // response would be the snapshot leaking into it.
    let mut rng = Lcg(0x0BAD_F00D_0000_0007);
    let base = snapshot(&mut rng, 1_000);
    let mut sizes = Vec::new();
    for filler in [0_u64, 9_000_u64] {
        let mut padded = base.clone();
        for position in 0..filler {
            let mut extra = entry(&mut rng, 100_000 + position);
            extra.final_score_millis = 0;
            extra.search_text = "unrelated filler text".to_string();
            padded.entries.push(extra);
        }
        let indexed = RepoMapIndexedSnapshot::new(padded.clone());
        let request = RepoMapQueryRequest {
            repo_id: padded.repo_id.clone(),
            revision_id: padded.revision_id.clone(),
            manifest_generation: padded.manifest_generation,
            query_text: "query owner".to_string(),
            top_k: 8,
            token_budget: 1_000_000,
            focus_subjects: Vec::new(),
        };
        let response = RepoMapQueryEngine::query(&indexed, &request)?;
        if response.entries.len() != 8 {
            return Err(format!("top_k rows: {}", response.entries.len()).into());
        }
        if u64::from(response.dropped_entries_count).saturating_add(8)
            != u64::try_from(padded.entries.len())?
        {
            return Err("every entry is a row or counted".into());
        }
        let mut bytes = Vec::new();
        ciborium::into_writer(&response, &mut bytes)?;
        sizes.push((
            bytes.len(),
            response
                .entries
                .iter()
                .map(|row| row.subject_identity.clone())
                .collect::<Vec<_>>(),
        ));
    }
    let (Some((small, small_page)), Some((large, large_page))) = (sizes.first(), sizes.get(1))
    else {
        return Err("two sizes".into());
    };
    if small_page != large_page {
        return Err("the filler must not reach the page".into());
    }
    // Only the dropped count differs, and both counts encode in the same
    // width.
    if large != small {
        return Err(format!("response bytes grew with the snapshot: {small} -> {large}").into());
    }
    Ok(())
}

#[test]
fn an_unresolved_focus_subject_is_a_typed_refusal() {
    let mut rng = Lcg(0x5EED_5EED);
    let snap = snapshot(&mut rng, 24);
    let indexed = RepoMapIndexedSnapshot::new(snap.clone());
    let request = RepoMapQueryRequest {
        repo_id: snap.repo_id.clone(),
        revision_id: snap.revision_id.clone(),
        manifest_generation: snap.manifest_generation,
        query_text: "alpha".to_string(),
        top_k: 4,
        token_budget: 256,
        focus_subjects: vec![RepoMapFocusSubjectDto {
            subject_identity: "nowhere".to_string(),
            subject_doc_type: RepoMapDocType::File,
        }],
    };
    match RepoMapQueryEngine::query(&indexed, &request) {
        Err(quanta_index_core::CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::FocusSubjectNotFound,
            ..
        }) => {}
        other => panic!("expected FocusSubjectNotFound typed refusal, got {other:?}"),
    }
}
