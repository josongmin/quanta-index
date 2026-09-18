//! Oracles for the history text index: an independent BM25 over the same
//! tokenizer, keyset partitioning, epoch immutability and segment sharing
//! by inode, and the typed refusals.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use quanta_index_contract::lex::CommitSha;
use quanta_index_contract::{
    AuxEpochV1, LqCase, LqExpr, LqLeaf, LqOptions, LqPatternType, ManifestGeneration, RepoId,
    RevisionId,
};
use quanta_index_core::{
    AuxiliaryGenerationKeyV1, CoreError, HISTORY_TEXT_INDEX_CORRUPT_CODE,
    HISTORY_TEXT_INDEX_NORMALIZER_UNSUPPORTED_CODE, HISTORY_TEXT_INDEX_NOT_READY_CODE,
    HISTORY_TEXT_QUERY_UNSCORABLE_CODE, HistoryTextBuildV1, HistoryTextDiscardOutcomeV1,
    HistoryTextDocKeyV1, HistoryTextDocV1, HistoryTextEpochStatusV1, HistoryTextHitV1,
    HistoryTextIndexPort, HistoryTextKindV1, HistoryTextPageV1, HistoryTextQueryV1,
    HistoryTextSearcher, RequestBudgetV1,
};

use crate::history_text_index::adapter::HistoryTextIndexAdapter;
use crate::history_text_index::bm25::{HISTORY_BM25_B, HISTORY_BM25_K1};
use crate::history_text_index::layout::{epoch_dir, kind_dir};
use crate::history_text_index::manifest::HistoryTextManifest;
use crate::normalize::{self, CaseMode, TextNormalizerVersion};

type TestRes = Result<(), Box<dyn std::error::Error>>;

fn generation() -> AuxiliaryGenerationKeyV1 {
    AuxiliaryGenerationKeyV1 {
        repo_id: RepoId::new("repo"),
        revision_id: RevisionId::new("rev"),
        generation: ManifestGeneration::new(3),
    }
}

fn sha(byte: u8) -> CommitSha {
    CommitSha::from_bytes([byte; 20])
}

fn commit_doc(byte: u8, time: u64, text: &str) -> HistoryTextDocV1 {
    HistoryTextDocV1 {
        key: HistoryTextDocKeyV1::Commit { sha: sha(byte) },
        committer_time_ms: time,
        text: text.to_string(),
    }
}

fn diff_doc(byte: u8, path: &str, time: u64, text: &str) -> HistoryTextDocV1 {
    HistoryTextDocV1 {
        key: HistoryTextDocKeyV1::Diff {
            sha: sha(byte),
            file_path: path.to_string(),
        },
        committer_time_ms: time,
        text: text.to_string(),
    }
}

fn adapter(root: &Path) -> Result<HistoryTextIndexAdapter, CoreError> {
    HistoryTextIndexAdapter::with_root(root.to_path_buf())
}

fn keyword_query(kind: HistoryTextKindV1, expr: LqExpr) -> HistoryTextQueryV1 {
    HistoryTextQueryV1 {
        kind,
        expr,
        options: LqOptions::defaults(),
    }
}

fn keyword(text: &str) -> LqExpr {
    LqExpr::Leaf(LqLeaf::Keyword(text.to_string()))
}

fn admit_all() -> Arc<quanta_index_core::HistoryTextAdmitFn> {
    Arc::new(|_hit| Ok(true))
}

fn search(
    searcher: &dyn HistoryTextSearcher,
    query: &HistoryTextQueryV1,
    after: Option<&HistoryTextHitV1>,
    limit: usize,
) -> Result<HistoryTextPageV1, CoreError> {
    searcher.search(
        query,
        after,
        limit,
        admit_all(),
        &RequestBudgetV1::unbounded(),
    )
}

/// The fixture the BM25 oracle runs over.
///
/// Commit messages vary the frequency of `needle` and their length so the
/// scores spread; two of them (`0x21` / `0x22`) are the same text so the
/// tie breaks by time then sha. Every message is under 40 tokens, the
/// range in which the engine's field-length quantization is exact. The
/// diff hunks share the vocabulary so the two kinds' statistics would
/// collide if they were one index.
fn oracle_fixture() -> Vec<HistoryTextDocV1> {
    vec![
        commit_doc(0x11, 100, "needle"),
        commit_doc(0x12, 200, "needle needle needle in a short haystack"),
        commit_doc(
            0x13,
            300,
            "a rather long message that mentions the needle once among many other words here",
        ),
        commit_doc(0x14, 400, "no matching term at all in this one"),
        commit_doc(0x15, 500, "needle needle"),
        commit_doc(
            0x16,
            600,
            "Needle_x and needle: the second is a token, the first is not",
        ),
        commit_doc(0x21, 700, "the needle and the thread"),
        commit_doc(0x22, 800, "the needle and the thread"),
        commit_doc(0x23, 900, "thread without the other word"),
        commit_doc(0x24, 950, "needle thread needle thread needle"),
        diff_doc(0x11, "a.rs", 100, "needle needle needle needle"),
        diff_doc(0x12, "b.rs", 200, "thread"),
        diff_doc(0x13, "c.rs", 300, "needle"),
    ]
}

/// Tokens of one document under the shared normalizer, as the index
/// emits them.
fn tokens(text: &str) -> Vec<String> {
    normalize::tokenize(text, CaseMode::Folded)
        .indexable()
        .map(|token| token.text.clone())
        .collect()
}

/// A scoring clause of the reference: a term, or a phrase of terms.
enum ReferenceClause<'a> {
    Term(&'a str),
    Phrase(Vec<&'a str>),
}

/// The reference BM25 (see [`crate::history_text_index::bm25`]) of one
/// clause over every document of one kind: `None` where the clause does
/// not match.
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::imprecise_flops,
    reason = "the reference computes in the engine's own f32 arithmetic from integer counts, `ln(1 + x)` included, so its scores are the engine's bit for bit"
)]
fn reference_clause_scores(
    docs: &[&HistoryTextDocV1],
    clause: &ReferenceClause<'_>,
) -> Vec<Option<f32>> {
    let tokenized: Vec<Vec<String>> = docs.iter().map(|doc| tokens(&doc.text)).collect();
    let total_docs = tokenized.len() as f32;
    let total_tokens: usize = tokenized.iter().map(Vec::len).sum();
    let average_length = total_tokens as f32 / total_docs;
    let idf = |term: &str| {
        let df = tokenized
            .iter()
            .filter(|doc| doc.iter().any(|token| token == term))
            .count() as f32;
        (1.0_f32 + (total_docs - df + 0.5) / (df + 0.5)).ln()
    };
    let (idf_sum, occurrences): (f32, Vec<u32>) = match clause {
        ReferenceClause::Term(term) => (
            idf(term),
            tokenized
                .iter()
                .map(|doc| doc.iter().filter(|token| token == term).count() as u32)
                .collect(),
        ),
        ReferenceClause::Phrase(terms) => (
            terms.iter().map(|term| idf(term)).sum(),
            tokenized
                .iter()
                .map(|doc| {
                    doc.windows(terms.len())
                        .filter(|window| window.iter().zip(terms.iter()).all(|(a, b)| a == b))
                        .count() as u32
                })
                .collect(),
        ),
    };
    let weight = idf_sum * (1.0 + HISTORY_BM25_K1);
    tokenized
        .iter()
        .zip(occurrences)
        .map(|(doc, tf)| {
            if tf == 0 {
                return None;
            }
            let norm = HISTORY_BM25_K1
                * (1.0 - HISTORY_BM25_B + HISTORY_BM25_B * doc.len() as f32 / average_length);
            let tf = tf as f32;
            Some(weight * (tf / (tf + norm)))
        })
        .collect()
}

/// How the reference combines clauses.
enum ReferenceCombine {
    /// Every clause must match; scores add.
    All,
    /// At least one clause must match; matching scores add.
    Any,
}

/// The reference ranking of `docs` of `kind`: `(key, time, score)` in
/// relevance order.
fn reference_ranking(
    docs: &[HistoryTextDocV1],
    kind: HistoryTextKindV1,
    clauses: &[ReferenceClause<'_>],
    combine: &ReferenceCombine,
) -> Vec<(HistoryTextDocKeyV1, u64, f32)> {
    let of_kind: Vec<&HistoryTextDocV1> =
        docs.iter().filter(|doc| doc.key.kind() == kind).collect();
    let per_clause: Vec<Vec<Option<f32>>> = clauses
        .iter()
        .map(|clause| reference_clause_scores(&of_kind, clause))
        .collect();
    let mut ranked: Vec<(HistoryTextDocKeyV1, u64, f32)> = of_kind
        .iter()
        .enumerate()
        .filter_map(|(index, doc)| {
            let scores: Vec<Option<f32>> = per_clause
                .iter()
                .map(|column| column.get(index).copied().flatten())
                .collect();
            let matched = match combine {
                ReferenceCombine::All => scores.iter().all(Option::is_some),
                ReferenceCombine::Any => scores.iter().any(Option::is_some),
            };
            if !matched {
                return None;
            }
            let total: f32 = scores.into_iter().flatten().sum();
            Some((doc.key.clone(), doc.committer_time_ms, total))
        })
        .collect();
    ranked.sort_by(|left, right| {
        right
            .2
            .total_cmp(&left.2)
            .then_with(|| right.1.cmp(&left.1))
            .then_with(|| left.0.cmp_recency(&right.0))
    });
    ranked
}

fn assert_same_ranking(
    observed: &[HistoryTextHitV1],
    expected: &[(HistoryTextDocKeyV1, u64, f32)],
) -> TestRes {
    let observed_keys: Vec<&HistoryTextDocKeyV1> = observed.iter().map(|hit| &hit.key).collect();
    let expected_keys: Vec<&HistoryTextDocKeyV1> = expected.iter().map(|row| &row.0).collect();
    if observed_keys != expected_keys {
        return Err(format!(
            "ranking differs from the reference:\n  index:     {observed_keys:?}\n  reference: {expected_keys:?}"
        )
        .into());
    }
    for (hit, (key, _time, reference)) in observed.iter().zip(expected) {
        let score = hit.score.get();
        let tolerance = reference.abs().max(1.0) * 1e-5;
        if (score - reference).abs() > tolerance {
            return Err(format!(
                "score of {key:?} differs from the reference: index {score} reference {reference}"
            )
            .into());
        }
    }
    Ok(())
}

#[test]
fn bm25_top_k_equals_an_independent_reference_over_the_same_tokenizer() -> TestRes {
    let root = tempfile::tempdir()?;
    let port = adapter(root.path())?;
    let generation = generation();
    let docs = oracle_fixture();
    let receipt = port.publish_epoch(
        &generation,
        AuxEpochV1::new(1),
        HistoryTextBuildV1::Full { docs: docs.clone() },
    )?;
    if receipt.docs_written != 13 {
        return Err(format!("every document is written once: {receipt:?}").into());
    }
    let searcher = port.open_epoch(&generation, AuxEpochV1::new(1))?;

    // One keyword over commits: ten commits, eight match.
    let query = keyword_query(HistoryTextKindV1::Commit, keyword("needle"));
    let page = search(searcher.as_ref(), &query, None, 20)?;
    let expected = reference_ranking(
        &docs,
        HistoryTextKindV1::Commit,
        &[ReferenceClause::Term("needle")],
        &ReferenceCombine::All,
    );
    if expected.len() != 8 || page.matched != 8 || page.examined != 8 {
        return Err(format!(
            "eight commits mention needle: reference {} matched {} examined {}",
            expected.len(),
            page.matched,
            page.examined
        )
        .into());
    }
    assert_same_ranking(&page.hits, &expected)?;
    // The two identical messages tie on score and break by time
    // descending: 0x22 (800) before 0x21 (700).
    let tie: Vec<u8> = page
        .hits
        .iter()
        .filter(|hit| matches!(hit.key.sha().as_bytes().first(), Some(0x21 | 0x22)))
        .map(|hit| {
            hit.key
                .sha()
                .as_bytes()
                .first()
                .copied()
                .unwrap_or_default()
        })
        .collect();
    if tie != vec![0x22, 0x21] {
        return Err(format!("equal scores break by newer time first, got {tie:?}").into());
    }

    // Top-k is the head of the same ranking.
    let top = search(searcher.as_ref(), &query, None, 3)?;
    if top.matched != 8 || top.hits.len() != 3 {
        return Err(format!("top-3 keeps the exact count: {top:?}").into());
    }
    assert_same_ranking(&top.hits, expected.get(..3).ok_or("three expected")?)?;

    // The same keyword over diffs scores against the diff statistics only.
    let diff_query = keyword_query(HistoryTextKindV1::Diff, keyword("needle"));
    let diff_page = search(searcher.as_ref(), &diff_query, None, 20)?;
    let diff_expected = reference_ranking(
        &docs,
        HistoryTextKindV1::Diff,
        &[ReferenceClause::Term("needle")],
        &ReferenceCombine::All,
    );
    if diff_expected.len() != 2 || diff_page.matched != 2 {
        return Err(format!("two hunks mention needle: {diff_page:?}").into());
    }
    assert_same_ranking(&diff_page.hits, &diff_expected)?;

    // A conjunction sums both terms; a disjunction sums what matches.
    let both = keyword_query(
        HistoryTextKindV1::Commit,
        LqExpr::All(vec![keyword("needle"), keyword("thread")]),
    );
    let both_page = search(searcher.as_ref(), &both, None, 20)?;
    let both_expected = reference_ranking(
        &docs,
        HistoryTextKindV1::Commit,
        &[
            ReferenceClause::Term("needle"),
            ReferenceClause::Term("thread"),
        ],
        &ReferenceCombine::All,
    );
    if both_expected.len() != 3 {
        return Err(format!("three commits mention both: {both_expected:?}").into());
    }
    assert_same_ranking(&both_page.hits, &both_expected)?;
    let either = keyword_query(
        HistoryTextKindV1::Commit,
        LqExpr::Any(vec![keyword("needle"), keyword("thread")]),
    );
    let either_page = search(searcher.as_ref(), &either, None, 20)?;
    let either_expected = reference_ranking(
        &docs,
        HistoryTextKindV1::Commit,
        &[
            ReferenceClause::Term("needle"),
            ReferenceClause::Term("thread"),
        ],
        &ReferenceCombine::Any,
    );
    if either_expected.len() != 9 {
        return Err(format!("nine commits mention either: {either_expected:?}").into());
    }
    assert_same_ranking(&either_page.hits, &either_expected)?;

    // A phrase scores its occurrences under the summed idf.
    let phrase = keyword_query(
        HistoryTextKindV1::Commit,
        LqExpr::Leaf(LqLeaf::Phrase("needle thread".to_string())),
    );
    let phrase_page = search(searcher.as_ref(), &phrase, None, 20)?;
    let phrase_expected = reference_ranking(
        &docs,
        HistoryTextKindV1::Commit,
        &[ReferenceClause::Phrase(vec!["needle", "thread"])],
        &ReferenceCombine::All,
    );
    if phrase_expected.len() != 1 {
        return Err(format!("one commit holds the phrase: {phrase_expected:?}").into());
    }
    assert_same_ranking(&phrase_page.hits, &phrase_expected)?;

    // A negation beside a positive clause excludes without scoring.
    let except = keyword_query(
        HistoryTextKindV1::Commit,
        LqExpr::All(vec![
            keyword("needle"),
            LqExpr::Not(Box::new(keyword("thread"))),
        ]),
    );
    let except_page = search(searcher.as_ref(), &except, None, 20)?;
    let except_expected: Vec<(HistoryTextDocKeyV1, u64, f32)> = expected
        .iter()
        .filter(|row| !both_expected.iter().any(|other| other.0 == row.0))
        .cloned()
        .collect();
    assert_same_ranking(&except_page.hits, &except_expected)?;

    // `case:yes` scores the case-preserving terms: `Needle_x` is one token
    // and `needle` matches nothing capitalized.
    let sensitive = HistoryTextQueryV1 {
        kind: HistoryTextKindV1::Commit,
        expr: keyword("Needle_x"),
        options: LqOptions {
            case: Some(LqCase::Sensitive),
            ..LqOptions::defaults()
        },
    };
    let sensitive_page = search(searcher.as_ref(), &sensitive, None, 20)?;
    let sensitive_keys: Vec<u8> = sensitive_page
        .hits
        .iter()
        .map(|hit| {
            hit.key
                .sha()
                .as_bytes()
                .first()
                .copied()
                .unwrap_or_default()
        })
        .collect();
    if sensitive_keys != vec![0x16] {
        return Err(
            format!("case-sensitive keyword finds the one spelling: {sensitive_keys:?}").into(),
        );
    }
    Ok(())
}

#[test]
fn keyset_continuation_partitions_the_ranking_without_gap_or_overlap() -> TestRes {
    let root = tempfile::tempdir()?;
    let port = adapter(root.path())?;
    let generation = generation();
    let docs = oracle_fixture();
    let _receipt = port.publish_epoch(
        &generation,
        AuxEpochV1::new(1),
        HistoryTextBuildV1::Full { docs },
    )?;
    let searcher = port.open_epoch(&generation, AuxEpochV1::new(1))?;
    let query = keyword_query(
        HistoryTextKindV1::Commit,
        LqExpr::Any(vec![keyword("needle"), keyword("thread")]),
    );
    let whole = search(searcher.as_ref(), &query, None, 100)?;
    if whole.hits.len() != 9 {
        return Err(format!("nine matches expected: {whole:?}").into());
    }
    for page_size in [1_usize, 3, 5, 8, 9] {
        let mut walked: Vec<HistoryTextHitV1> = Vec::new();
        let mut after: Option<HistoryTextHitV1> = None;
        for _page in 0..16 {
            let page = search(searcher.as_ref(), &query, after.as_ref(), page_size)?;
            let remaining = u64::try_from(whole.hits.len().saturating_sub(walked.len()))?;
            if page.matched != remaining {
                return Err(format!(
                    "page size {page_size}: each page counts exactly what is after its cursor: matched {} remaining {remaining}",
                    page.matched
                )
                .into());
            }
            if page.hits.is_empty() {
                break;
            }
            walked.extend(page.hits.iter().cloned());
            after = page.hits.last().cloned();
            if page.hits.len() < page_size {
                break;
            }
        }
        if walked != whole.hits {
            return Err(format!(
                "page size {page_size}: pages must partition the ranking in order:\n  walked {walked:?}\n  whole  {:?}",
                whole.hits
            )
            .into());
        }
        let distinct: BTreeSet<&HistoryTextDocKeyV1> = walked.iter().map(|hit| &hit.key).collect();
        if distinct.len() != walked.len() {
            return Err(format!("page size {page_size}: a hit appeared twice").into());
        }
    }
    // A cursor of the other kind is refused.
    let diff_cursor = HistoryTextHitV1 {
        key: HistoryTextDocKeyV1::Diff {
            sha: sha(1),
            file_path: "a.rs".to_string(),
        },
        committer_time_ms: 1,
        score: quanta_index_contract::HistoryScoreV1::try_new(1.0)?,
    };
    match search(searcher.as_ref(), &query, Some(&diff_cursor), 3) {
        Err(CoreError::InvalidContract(_)) => Ok(()),
        other => Err(format!("a diff cursor on a commit page is refused, got {other:?}").into()),
    }
}

#[test]
fn the_admit_predicate_bounds_the_page_and_counts_exactly() -> TestRes {
    let root = tempfile::tempdir()?;
    let port = adapter(root.path())?;
    let generation = generation();
    let docs = oracle_fixture();
    let _receipt = port.publish_epoch(
        &generation,
        AuxEpochV1::new(1),
        HistoryTextBuildV1::Full { docs },
    )?;
    let searcher = port.open_epoch(&generation, AuxEpochV1::new(1))?;
    let query = keyword_query(HistoryTextKindV1::Commit, keyword("needle"));
    // Admit only commits at an even hundred of committer time.
    let admit: Arc<quanta_index_core::HistoryTextAdmitFn> =
        Arc::new(|hit| Ok(hit.committer_time_ms % 200 == 0));
    let page = searcher.search(&query, None, 2, admit, &RequestBudgetV1::unbounded())?;
    let times: Vec<u64> = page.hits.iter().map(|hit| hit.committer_time_ms).collect();
    if page.examined != 8 || page.matched != 3 || times.len() != 2 {
        return Err(format!(
            "examined counts every visited document, matched only the admitted: {page:?}"
        )
        .into());
    }
    if times.iter().any(|time| time % 200 != 0) {
        return Err(format!("only admitted rows reach the page: {times:?}").into());
    }
    // An error from the predicate aborts the search with that error.
    let failing: Arc<quanta_index_core::HistoryTextAdmitFn> = Arc::new(|_hit| {
        Err(CoreError::Typed {
            code: "ROW_MISSING".to_string(),
            message: "the row is not in the snapshot".to_string(),
        })
    });
    match searcher.search(&query, None, 2, failing, &RequestBudgetV1::unbounded()) {
        Err(CoreError::Typed { code, .. }) if code == "ROW_MISSING" => Ok(()),
        other => Err(format!("the predicate's error propagates, got {other:?}").into()),
    }
}

#[cfg(unix)]
fn file_identities(dir: &Path) -> Result<BTreeMap<String, (u64, u64)>, Box<dyn std::error::Error>> {
    use std::os::unix::fs::MetadataExt as _;
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let metadata = entry.metadata()?;
        if metadata.is_file() {
            let _previous = out.insert(name, (metadata.ino(), metadata.len()));
        }
    }
    Ok(out)
}

/// §3.4 for the history index.
///
/// An incremental epoch links the previous epoch's segment files rather
/// than copying them, the previous epoch's files are untouched, and each
/// epoch answers with its own content.
#[cfg(unix)]
#[test]
fn an_incremental_epoch_shares_unchanged_segments_by_inode() -> TestRes {
    let root = tempfile::tempdir()?;
    let port = adapter(root.path())?;
    let generation = generation();
    let docs = oracle_fixture();
    let _receipt = port.publish_epoch(
        &generation,
        AuxEpochV1::new(1),
        HistoryTextBuildV1::Full { docs },
    )?;
    let first_dir = epoch_dir(root.path(), &generation, AuxEpochV1::new(1));
    let first_commits = kind_dir(&first_dir, HistoryTextKindV1::Commit);
    let before = file_identities(&first_commits)?;
    let before_digest = crate::history_text_index::manifest::kind_commit_sha256(
        &first_dir,
        HistoryTextKindV1::Commit,
    )?;

    // Epoch 2: one new commit and one rewritten message.
    let receipt = port.publish_epoch(
        &generation,
        AuxEpochV1::new(2),
        HistoryTextBuildV1::Incremental {
            base: AuxEpochV1::new(1),
            upserts: vec![
                commit_doc(0x31, 1_000, "needle arrives in epoch two"),
                commit_doc(0x14, 400, "now this one says needle too"),
            ],
        },
    )?;
    if receipt.docs_written != 2 {
        return Err(format!("an incremental epoch writes its upserts: {receipt:?}").into());
    }
    let second_dir = epoch_dir(root.path(), &generation, AuxEpochV1::new(2));
    let second_commits = kind_dir(&second_dir, HistoryTextKindV1::Commit);
    let after_in_first = file_identities(&first_commits)?;
    if after_in_first != before {
        return Err("publishing epoch 2 must not touch epoch 1's files".into());
    }
    if crate::history_text_index::manifest::kind_commit_sha256(
        &first_dir,
        HistoryTextKindV1::Commit,
    )? != before_digest
    {
        return Err("epoch 1's commit file is rewritten by epoch 2".into());
    }
    let second = file_identities(&second_commits)?;
    let mut shared = 0_usize;
    let mut private = Vec::new();
    for (name, (inode, len)) in &before {
        if name == "meta.json" || name == ".managed.json" || name.starts_with(".tantivy") {
            continue;
        }
        match second.get(name) {
            Some((second_inode, second_len)) if second_inode == inode && second_len == len => {
                shared = shared.saturating_add(1);
            }
            Some(_) => private.push(name.clone()),
            None => {
                // The engine may have merged the base segment away; then
                // no file of the base survives, which the assertion below
                // catches through the shared count.
            }
        }
    }
    if !private.is_empty() {
        return Err(
            format!("base segment files were copied instead of linked: {private:?}").into(),
        );
    }
    if shared == 0 {
        return Err(format!(
            "epoch 2 shares no segment file with epoch 1: before {before:?} after {second:?}"
        )
        .into());
    }
    for name in ["meta.json", ".managed.json"] {
        match (before.get(name), second.get(name)) {
            (Some((first_inode, _)), Some((second_inode, _))) if first_inode != second_inode => {}
            other => {
                return Err(
                    format!("{name} must be a private copy per epoch, got {other:?}").into(),
                );
            }
        }
    }

    // Each epoch answers with its own content.
    let query = keyword_query(HistoryTextKindV1::Commit, keyword("needle"));
    let first = port.open_epoch(&generation, AuxEpochV1::new(1))?;
    let second = port.open_epoch(&generation, AuxEpochV1::new(2))?;
    let first_page = search(first.as_ref(), &query, None, 100)?;
    let second_page = search(second.as_ref(), &query, None, 100)?;
    let keys = |page: &HistoryTextPageV1| -> BTreeSet<u8> {
        page.hits
            .iter()
            .map(|hit| {
                hit.key
                    .sha()
                    .as_bytes()
                    .first()
                    .copied()
                    .unwrap_or_default()
            })
            .collect()
    };
    let first_keys = keys(&first_page);
    let second_keys = keys(&second_page);
    if first_keys.contains(&0x31) || first_keys.contains(&0x14) {
        return Err(format!("epoch 1 must not see epoch 2's rows: {first_keys:?}").into());
    }
    if !second_keys.contains(&0x31) || !second_keys.contains(&0x14) {
        return Err(format!("epoch 2 must see its upserts: {second_keys:?}").into());
    }
    if second_page.matched != first_page.matched.saturating_add(2) {
        return Err(format!(
            "the rewritten commit is one document, not two: {} vs {}",
            second_page.matched, first_page.matched
        )
        .into());
    }
    let epochs = port.durable_epochs(&generation)?;
    if epochs != vec![AuxEpochV1::new(1), AuxEpochV1::new(2)] {
        return Err(format!("both epochs are durable: {epochs:?}").into());
    }
    Ok(())
}

#[test]
fn an_index_stamped_with_another_normalizer_is_unsupported_until_rebuilt() -> TestRes {
    let root = tempfile::tempdir()?;
    let port = adapter(root.path())?;
    let generation = generation();
    let _receipt = port.publish_epoch(
        &generation,
        AuxEpochV1::new(1),
        HistoryTextBuildV1::Full {
            docs: vec![commit_doc(1, 1, "needle")],
        },
    )?;
    if port.epoch_status(&generation, AuxEpochV1::new(1))? != HistoryTextEpochStatusV1::Servable {
        return Err("a freshly published epoch is servable".into());
    }
    if port.epoch_status(&generation, AuxEpochV1::new(2))? != HistoryTextEpochStatusV1::Absent {
        return Err("an unpublished epoch is absent".into());
    }
    // Re-stamp the manifest as built under normalizer 1.0.
    let dir = epoch_dir(root.path(), &generation, AuxEpochV1::new(1));
    let mut manifest = HistoryTextManifest::read(&dir)?;
    manifest.normalizer = TextNormalizerVersion { major: 1, minor: 0 };
    manifest.write(&dir)?;
    match port.epoch_status(&generation, AuxEpochV1::new(1))? {
        HistoryTextEpochStatusV1::Unsupported { built_with } if built_with == "1.0" => {}
        other @ (HistoryTextEpochStatusV1::Absent
        | HistoryTextEpochStatusV1::Servable
        | HistoryTextEpochStatusV1::Unsupported { .. }) => {
            return Err(format!("another stamp is unsupported, got {other:?}").into());
        }
    }
    match port.open_epoch(&generation, AuxEpochV1::new(1)) {
        Err(CoreError::Typed { code, .. })
            if code == HISTORY_TEXT_INDEX_NORMALIZER_UNSUPPORTED_CODE => {}
        other => {
            return Err(format!("open refuses another stamp typed, got {:?}", other.err()).into());
        }
    }
    match port.publish_epoch(
        &generation,
        AuxEpochV1::new(2),
        HistoryTextBuildV1::Incremental {
            base: AuxEpochV1::new(1),
            upserts: Vec::new(),
        },
    ) {
        Err(CoreError::Typed { code, .. })
            if code == HISTORY_TEXT_INDEX_NORMALIZER_UNSUPPORTED_CODE => {}
        other => {
            return Err(format!(
                "an incremental build over another stamp is refused typed, got {:?}",
                other.err()
            )
            .into());
        }
    }
    match port.publish_epoch(
        &generation,
        AuxEpochV1::new(2),
        HistoryTextBuildV1::Incremental {
            base: AuxEpochV1::new(7),
            upserts: Vec::new(),
        },
    ) {
        Err(CoreError::InvalidContract(_)) => {}
        other => {
            return Err(format!(
                "an epoch cannot be built over a later one, got {:?}",
                other.err()
            )
            .into());
        }
    }
    match port.publish_epoch(
        &generation,
        AuxEpochV1::new(3),
        HistoryTextBuildV1::Incremental {
            base: AuxEpochV1::new(2),
            upserts: Vec::new(),
        },
    ) {
        Err(CoreError::Typed { code, .. }) if code == HISTORY_TEXT_INDEX_NOT_READY_CODE => {}
        other => {
            return Err(format!(
                "an incremental build over an absent base is refused typed, got {:?}",
                other.err()
            )
            .into());
        }
    }
    // A full rebuild at the next epoch is servable again and answers.
    let _receipt = port.publish_epoch(
        &generation,
        AuxEpochV1::new(2),
        HistoryTextBuildV1::Full {
            docs: vec![
                commit_doc(1, 1, "needle"),
                commit_doc(2, 2, "needle twice needle"),
            ],
        },
    )?;
    let searcher = port.open_epoch(&generation, AuxEpochV1::new(2))?;
    let page = search(
        searcher.as_ref(),
        &keyword_query(HistoryTextKindV1::Commit, keyword("needle")),
        None,
        10,
    )?;
    if page.matched != 2 {
        return Err(format!("the rebuilt epoch answers: {page:?}").into());
    }
    Ok(())
}

#[test]
fn a_directory_that_contradicts_its_manifest_is_refused_corrupt() -> TestRes {
    let root = tempfile::tempdir()?;
    let port = adapter(root.path())?;
    let generation = generation();
    let _receipt = port.publish_epoch(
        &generation,
        AuxEpochV1::new(1),
        HistoryTextBuildV1::Full {
            docs: vec![commit_doc(1, 1, "needle")],
        },
    )?;
    let dir = epoch_dir(root.path(), &generation, AuxEpochV1::new(1));
    let commit_file = kind_dir(&dir, HistoryTextKindV1::Commit).join("meta.json");
    let mut bytes = std::fs::read(&commit_file)?;
    bytes.push(b'\n');
    std::fs::write(&commit_file, bytes)?;
    match port.epoch_status(&generation, AuxEpochV1::new(1)) {
        Err(CoreError::Typed { code, .. }) if code == HISTORY_TEXT_INDEX_CORRUPT_CODE => {}
        other => return Err(format!("a torn epoch is corrupt, got {other:?}").into()),
    }
    match port.open_epoch(&generation, AuxEpochV1::new(1)) {
        Err(CoreError::Typed { code, .. }) if code == HISTORY_TEXT_INDEX_CORRUPT_CODE => {}
        other => return Err(format!("a torn epoch does not open, got {:?}", other.err()).into()),
    }
    // A manifest that does not decode is corrupt too.
    std::fs::write(dir.join("history-text-manifest.cbor"), b"not cbor")?;
    match port.epoch_status(&generation, AuxEpochV1::new(1)) {
        Err(CoreError::Typed { code, .. }) if code == HISTORY_TEXT_INDEX_CORRUPT_CODE => Ok(()),
        other => Err(format!("an undecodable manifest is corrupt, got {other:?}").into()),
    }
}

#[test]
fn unscorable_expressions_are_refused_typed() -> TestRes {
    let root = tempfile::tempdir()?;
    let port = adapter(root.path())?;
    let generation = generation();
    let _receipt = port.publish_epoch(
        &generation,
        AuxEpochV1::new(1),
        HistoryTextBuildV1::Full {
            docs: vec![commit_doc(1, 1, "needle")],
        },
    )?;
    let searcher = port.open_epoch(&generation, AuxEpochV1::new(1))?;
    let cases: Vec<(&str, HistoryTextQueryV1)> = vec![
        (
            "empty",
            keyword_query(HistoryTextKindV1::Commit, LqExpr::Empty),
        ),
        (
            "raw string",
            keyword_query(
                HistoryTextKindV1::Commit,
                LqExpr::Leaf(LqLeaf::RawString("needle".to_string())),
            ),
        ),
        (
            "regex",
            keyword_query(
                HistoryTextKindV1::Commit,
                LqExpr::Leaf(LqLeaf::Regex("need.*".to_string())),
            ),
        ),
        (
            "bare negation",
            keyword_query(
                HistoryTextKindV1::Commit,
                LqExpr::Not(Box::new(keyword("needle"))),
            ),
        ),
        (
            "negation only conjunction",
            keyword_query(
                HistoryTextKindV1::Commit,
                LqExpr::All(vec![LqExpr::Not(Box::new(keyword("needle")))]),
            ),
        ),
        (
            "negation in a disjunction",
            keyword_query(
                HistoryTextKindV1::Commit,
                LqExpr::Any(vec![
                    keyword("needle"),
                    LqExpr::Not(Box::new(keyword("thread"))),
                ]),
            ),
        ),
        (
            "empty disjunction",
            keyword_query(HistoryTextKindV1::Commit, LqExpr::Any(Vec::new())),
        ),
        (
            "regexp pattern type",
            HistoryTextQueryV1 {
                kind: HistoryTextKindV1::Commit,
                expr: keyword("needle"),
                options: LqOptions {
                    pattern_type: LqPatternType::Regexp,
                    ..LqOptions::defaults()
                },
            },
        ),
        (
            "literal pattern type",
            HistoryTextQueryV1 {
                kind: HistoryTextKindV1::Commit,
                expr: keyword("needle"),
                options: LqOptions {
                    pattern_type: LqPatternType::Literal,
                    ..LqOptions::defaults()
                },
            },
        ),
    ];
    for (label, query) in cases {
        match search(searcher.as_ref(), &query, None, 10) {
            Err(CoreError::Typed { code, .. }) if code == HISTORY_TEXT_QUERY_UNSCORABLE_CODE => {}
            other => {
                return Err(format!("{label}: must be refused unscorable, got {other:?}").into());
            }
        }
    }
    // A literal without a token is the shared token-surface refusal.
    match search(
        searcher.as_ref(),
        &keyword_query(HistoryTextKindV1::Commit, keyword("👍")),
        None,
        10,
    ) {
        Err(CoreError::Typed { code, .. }) if code == "LEX_TEXT_QUERY_NO_TOKENS" => Ok(()),
        other => Err(format!("a token-less literal is refused typed, got {other:?}").into()),
    }
}

#[test]
fn discarding_epochs_and_generations_reclaims_bytes_and_is_idempotent() -> TestRes {
    let root = tempfile::tempdir()?;
    let port = adapter(root.path())?;
    let generation = generation();
    for epoch in 1..=3_u64 {
        let build = if epoch == 1 {
            HistoryTextBuildV1::Full {
                docs: vec![commit_doc(1, 1, "needle")],
            }
        } else {
            HistoryTextBuildV1::Incremental {
                base: AuxEpochV1::new(epoch.saturating_sub(1)),
                upserts: vec![commit_doc(u8::try_from(epoch)?, epoch, "needle again")],
            }
        };
        let _receipt = port.publish_epoch(&generation, AuxEpochV1::new(epoch), build)?;
    }
    if port.durable_epochs(&generation)?.len() != 3 {
        return Err("three epochs are durable".into());
    }
    match port.discard_epoch(&generation, AuxEpochV1::new(1))? {
        HistoryTextDiscardOutcomeV1::Discarded { bytes } if bytes > 0 => {}
        other @ (HistoryTextDiscardOutcomeV1::Absent
        | HistoryTextDiscardOutcomeV1::Discarded { .. }) => {
            return Err(format!("discarding an epoch reclaims bytes, got {other:?}").into());
        }
    }
    if port.discard_epoch(&generation, AuxEpochV1::new(1))? != HistoryTextDiscardOutcomeV1::Absent {
        return Err("a second discard is absent".into());
    }
    let epochs = port.durable_epochs(&generation)?;
    if epochs != vec![AuxEpochV1::new(2), AuxEpochV1::new(3)] {
        return Err(format!("the other epochs remain: {epochs:?}").into());
    }
    // Epoch 2 still opens and answers after epoch 1, its base, is gone:
    // shared segments are links, not references.
    let searcher = port.open_epoch(&generation, AuxEpochV1::new(2))?;
    let page = search(
        searcher.as_ref(),
        &keyword_query(HistoryTextKindV1::Commit, keyword("needle")),
        None,
        10,
    )?;
    if page.matched != 2 {
        return Err(format!("epoch 2 answers without its base: {page:?}").into());
    }
    match port.discard_generation(&generation)? {
        HistoryTextDiscardOutcomeV1::Discarded { bytes } if bytes > 0 => {}
        other @ (HistoryTextDiscardOutcomeV1::Absent
        | HistoryTextDiscardOutcomeV1::Discarded { .. }) => {
            return Err(format!("discarding a generation reclaims bytes, got {other:?}").into());
        }
    }
    if !port.durable_epochs(&generation)?.is_empty() {
        return Err("nothing remains after the generation is discarded".into());
    }
    if port.discard_generation(&generation)? != HistoryTextDiscardOutcomeV1::Absent {
        return Err("a second generation discard is absent".into());
    }
    Ok(())
}

#[test]
fn a_stale_staging_directory_and_an_unpublished_leftover_are_replaced() -> TestRes {
    let root = tempfile::tempdir()?;
    let port = adapter(root.path())?;
    let generation = generation();
    let _receipt = port.publish_epoch(
        &generation,
        AuxEpochV1::new(1),
        HistoryTextBuildV1::Full {
            docs: vec![commit_doc(1, 1, "needle")],
        },
    )?;
    // A leftover epoch 2 whose rows never landed, plus a stale staging dir.
    let _receipt = port.publish_epoch(
        &generation,
        AuxEpochV1::new(2),
        HistoryTextBuildV1::Full {
            docs: vec![commit_doc(9, 9, "stale")],
        },
    )?;
    let staging = crate::history_text_index::layout::staging_dir(
        root.path(),
        &generation,
        AuxEpochV1::new(2),
    );
    std::fs::create_dir_all(&staging)?;
    std::fs::write(staging.join("junk"), b"junk")?;
    let _receipt = port.publish_epoch(
        &generation,
        AuxEpochV1::new(2),
        HistoryTextBuildV1::Incremental {
            base: AuxEpochV1::new(1),
            upserts: vec![commit_doc(2, 2, "needle two")],
        },
    )?;
    if staging.exists() {
        return Err("the staging directory is consumed by the publish".into());
    }
    let searcher = port.open_epoch(&generation, AuxEpochV1::new(2))?;
    let page = search(
        searcher.as_ref(),
        &keyword_query(HistoryTextKindV1::Commit, keyword("needle")),
        None,
        10,
    )?;
    if page.matched != 2 {
        return Err(format!("the replaced epoch is the incremental one: {page:?}").into());
    }
    let stale = search(
        searcher.as_ref(),
        &keyword_query(HistoryTextKindV1::Commit, keyword("stale")),
        None,
        10,
    )?;
    if stale.matched != 0 {
        return Err("the leftover's content is gone".into());
    }
    Ok(())
}
