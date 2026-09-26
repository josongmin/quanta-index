//! L3 regressions: independent native visit counters and fixture-defined rows.

#![expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning tests use assertions for independently specified outcomes"
)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use quanta_index_core::{
    CoreError, LEXICAL_EXAMINED_BUDGET_EXCEEDED_CODE, LexicalCollectionBudget,
    LexicalExecutionBudgetV1, REQUEST_CANCELLED_CODE, REQUEST_DEADLINE_EXCEEDED_CODE,
    RequestBudgetV1,
};
use tantivy::collector::Collector;
use tantivy::merge_policy::NoMergePolicy;
use tantivy::query::{EnableScoring, Explanation, Query, Scorer, Weight};
use tantivy::schema::{FAST, STRING, Schema};
use tantivy::{
    DocId, DocSet, Index, IndexWriter, Score, SegmentReader, TERMINATED, TantivyDocument, doc,
};

use super::{GroupedPageCollector, ProjectionGroup, RankedPageCollector};
use crate::budgeted_search::{CollectionBudget, budgeted_collection, budgeted_search};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Clone, Debug, Default)]
struct MeasuredQuery {
    high_segment: Option<tantivy::SegmentId>,
    first_score: Option<f32>,
    scored: Arc<AtomicUsize>,
    advanced: Arc<AtomicUsize>,
    opened: Arc<AtomicUsize>,
}

impl Query for MeasuredQuery {
    fn weight(&self, _: EnableScoring<'_>) -> tantivy::Result<Box<dyn Weight>> {
        Ok(Box::new(MeasuredWeight(self.clone())))
    }
}

struct MeasuredWeight(MeasuredQuery);

impl Weight for MeasuredWeight {
    fn scorer(&self, reader: &SegmentReader, _: Score) -> tantivy::Result<Box<dyn Scorer>> {
        let _prior = self.0.opened.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(MeasuredScorer {
            query: self.0.clone(),
            score: if self.0.high_segment == Some(reader.segment_id()) {
                10.0
            } else {
                1.0
            },
            doc: 0,
            end: reader.max_doc(),
        }))
    }

    fn explain(&self, _: &SegmentReader, _: DocId) -> tantivy::Result<Explanation> {
        Ok(Explanation::new("fixture score", 1.0))
    }
}

struct MeasuredScorer {
    score: f32,
    query: MeasuredQuery,
    doc: DocId,
    end: DocId,
}

impl DocSet for MeasuredScorer {
    fn advance(&mut self) -> DocId {
        let _prior = self.query.advanced.fetch_add(1, Ordering::Relaxed);
        self.doc = self.doc.saturating_add(1);
        self.doc()
    }

    fn doc(&self) -> DocId {
        if self.doc < self.end {
            self.doc
        } else {
            TERMINATED
        }
    }

    fn size_hint(&self) -> u32 {
        self.end
    }
}

impl Scorer for MeasuredScorer {
    fn score(&mut self) -> Score {
        let _prior = self.query.scored.fetch_add(1, Ordering::Relaxed);
        if self.doc == 0 {
            self.query.first_score.unwrap_or(self.score)
        } else {
            self.score
        }
    }
}

fn index_with(segments: &[&[&str]]) -> Result<Index, Box<dyn std::error::Error>> {
    let mut builder = Schema::builder();
    let repo = builder.add_text_field("repo_id", STRING | FAST);
    let path = builder.add_text_field("repo_relative_path", STRING | FAST);
    let id = builder.add_text_field("candidate_id", STRING | FAST);
    let start = builder.add_u64_field("start_line", FAST);
    let end = builder.add_u64_field("end_line", FAST);
    let index = Index::create_in_ram(builder.build());
    let mut writer: IndexWriter<TantivyDocument> = index.writer_with_num_threads(1, 15_000_000)?;
    writer.set_merge_policy(Box::new(NoMergePolicy));
    let mut occurrences: BTreeMap<&str, u64> = BTreeMap::new();
    for paths in segments {
        for name in *paths {
            let occurrence = occurrences.entry(name).or_default();
            let _op = writer.add_document(doc!(
                repo => "fixture-repo",
                path => *name,
                id => format!("{name}-{occurrence}"),
                start => 1_u64,
                end => 1_u64
            ))?;
            *occurrence = occurrence.saturating_add(1);
        }
        let _commit = writer.commit()?;
    }
    writer.wait_merging_threads()?;
    Ok(index)
}

fn collection(cap: usize, segments: usize) -> Result<CollectionBudget, CoreError> {
    let policy = LexicalExecutionBudgetV1::new(cap)?;
    Ok(CollectionBudget::new(
        policy,
        policy.collection_budget(segments)?,
    ))
}

fn assert_budget_error<T>(result: Result<T, CoreError>) {
    assert!(matches!(result, Err(CoreError::Typed { code, .. })
        if code == LEXICAL_EXAMINED_BUDGET_EXCEEDED_CODE));
}

#[test]
fn l3_group_limit_stops_native_walk_before_materializing_excess() -> TestResult {
    let paths: Vec<String> = (0..64).map(|n| format!("file-{n}.rs")).collect();
    let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
    let index = index_with(&[&paths])?;
    let searcher = index.reader()?.searcher();
    let query = MeasuredQuery::default();
    let ledger = collection(3, searcher.segment_readers().len())?;
    let collector = GroupedPageCollector::new(ProjectionGroup::Path, 1.0, ledger.clone());
    assert_budget_error(budgeted_collection(
        &searcher,
        &query,
        &collector,
        &RequestBudgetV1::unbounded(),
        ledger,
        "test:projection",
    ));
    assert_eq!(
        query.scored.load(Ordering::Relaxed),
        4,
        "three admissions plus one refusal probe"
    );
    assert_eq!(
        query.advanced.load(Ordering::Relaxed),
        3,
        "no advance after refusal"
    );
    Ok(())
}

#[test]
fn l3_one_group_does_not_hide_excess_walk_work() -> TestResult {
    let paths = vec!["same.rs"; 64];
    let index = index_with(&[&paths])?;
    let searcher = index.reader()?.searcher();
    for group in [ProjectionGroup::Repo, ProjectionGroup::Path] {
        let query = MeasuredQuery::default();
        let ledger = collection(2, searcher.segment_readers().len())?;
        let collector = GroupedPageCollector::new(group, 1.0, ledger.clone());
        assert_budget_error(budgeted_collection(
            &searcher,
            &query,
            &collector,
            &RequestBudgetV1::unbounded(),
            ledger,
            "test:projection",
        ));
        assert_eq!(query.scored.load(Ordering::Relaxed), 3);
    }
    Ok(())
}

#[test]
fn l3_budget_is_shared_across_segments_and_stops_before_later_segments() -> TestResult {
    // Equal-sized committed segments keep the same work oracle regardless of
    // Tantivy's segment order. No segment merge is allowed in this fixture.
    let index = index_with(&[&["a", "b"], &["c", "d"], &["e", "f"]])?;
    let searcher = index.reader()?.searcher();
    assert_eq!(searcher.segment_readers().len(), 3);
    let query = MeasuredQuery::default();
    let ledger = collection(2, searcher.segment_readers().len())?;
    let collector = GroupedPageCollector::new(ProjectionGroup::Path, 1.0, ledger.clone());
    assert_budget_error(budgeted_collection(
        &searcher,
        &query,
        &collector,
        &RequestBudgetV1::unbounded(),
        ledger,
        "test:projection",
    ));
    assert_eq!(query.scored.load(Ordering::Relaxed), 3);
    assert_eq!(query.opened.load(Ordering::Relaxed), 2);
    Ok(())
}

#[test]
fn l3_exact_cap_keeps_global_best_representative_and_deterministic_ties() -> TestResult {
    for segments in [
        vec![vec!["z", "b"], vec!["a", "b"]],
        vec![vec!["b", "a"], vec!["b", "z"]],
    ] {
        let refs: Vec<&[&str]> = segments.iter().map(Vec::as_slice).collect();
        let index = index_with(&refs)?;
        let searcher = index.reader()?.searcher();
        let ledger = collection(4, searcher.segment_readers().len())?;
        let collector = GroupedPageCollector::new(ProjectionGroup::Path, 1.0, ledger.clone());
        let fruit = budgeted_collection(
            &searcher,
            &MeasuredQuery::default(),
            &collector,
            &RequestBudgetV1::unbounded(),
            ledger,
            "test:projection",
        )?;
        assert_eq!(fruit.matched, 4);
        let paths: Vec<&str> = fruit
            .representatives
            .iter()
            .map(|row| row.key.repo_relative_path.as_str())
            .collect();
        assert_eq!(paths, ["a", "b", "z"]);
        let ids: Vec<&str> = fruit
            .representatives
            .iter()
            .map(|row| row.key.candidate_id.as_str())
            .collect();
        assert_eq!(ids, ["a-0", "b-0", "z-0"]);
        assert!(
            fruit
                .representatives
                .iter()
                .all(|row| row.key.score.to_bits() == 1.0_f32.to_bits())
        );
    }
    Ok(())
}

#[test]
fn l3_whole_set_limit_stops_native_walk_too() -> TestResult {
    let index = index_with(&[&["a", "b", "c", "d", "e"]])?;
    let searcher = index.reader()?.searcher();
    let query = MeasuredQuery::default();
    let ledger = collection(2, searcher.segment_readers().len())?;
    let collector =
        RankedPageCollector::new(2, None, 1.0, true).with_collection_budget(ledger.clone());
    assert_budget_error(budgeted_collection(
        &searcher,
        &query,
        &collector,
        &RequestBudgetV1::unbounded(),
        ledger,
        "test:whole-set",
    ));
    assert_eq!(query.scored.load(Ordering::Relaxed), 3);
    // The ordinary ranked page's count remains a streaming operation.
    let fruit = budgeted_search(
        &searcher,
        &MeasuredQuery::default(),
        &RankedPageCollector::new(2, None, 1.0, true),
        &RequestBudgetV1::unbounded(),
        "test:count",
    )?;
    assert_eq!((fruit.matched, fruit.rows.len()), (5, 2));
    Ok(())
}

#[test]
fn l3_grouped_cancellation_and_deadline_remain_typed() -> TestResult {
    let index = index_with(&[&["a", "b"]])?;
    let searcher = index.reader()?.searcher();
    let cancelled = RequestBudgetV1::unbounded();
    cancelled.cancel_handle().cancel();
    let deadline = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .ok_or("clock underflow")?;
    for (budget, expected) in [
        (cancelled, REQUEST_CANCELLED_CODE),
        (
            RequestBudgetV1::until(deadline),
            REQUEST_DEADLINE_EXCEEDED_CODE,
        ),
    ] {
        let query = MeasuredQuery::default();
        let ledger = collection(2, searcher.segment_readers().len())?;
        let collector = GroupedPageCollector::new(ProjectionGroup::Path, 1.0, ledger.clone());
        let result = budgeted_collection(
            &searcher,
            &query,
            &collector,
            &budget,
            ledger,
            "test:projection",
        );
        assert!(matches!(result, Err(CoreError::Typed { code, .. }) if code == expected));
        assert_eq!(query.scored.load(Ordering::Relaxed), 0);
    }
    Ok(())
}

#[test]
fn l3_failed_admission_never_builds_group_output() -> TestResult {
    let index = index_with(&[&["a", "b"]])?;
    let searcher = index.reader()?.searcher();
    let ledger = collection(1, searcher.segment_readers().len())?;
    let collector = GroupedPageCollector::new(ProjectionGroup::Path, 1.0, ledger);
    // Even a direct Tantivy caller cannot harvest a partial success.
    assert!(
        searcher
            .search(&MeasuredQuery::default(), &collector)
            .is_err()
    );
    assert!(collector.requires_scoring());
    Ok(())
}

#[test]
fn l3_corrupt_order_key_stops_walk_without_becoming_budget_refusal() -> TestResult {
    let mut builder = Schema::builder();
    let repo = builder.add_text_field("repo_id", STRING | FAST);
    let path = builder.add_text_field("repo_relative_path", STRING | FAST);
    let id = builder.add_text_field("candidate_id", STRING | FAST);
    let start = builder.add_u64_field("start_line", FAST);
    let end = builder.add_u64_field("end_line", FAST);
    let index = Index::create_in_ram(builder.build());
    let mut writer: IndexWriter<TantivyDocument> = index.writer_with_num_threads(1, 15_000_000)?;
    let _op = writer
        .add_document(doc!(repo => "fixture-repo", path => "bad", id => "bad", end => 1_u64))?;
    for n in 0..8 {
        let _op = writer.add_document(
            doc!(repo => "fixture-repo", path => "valid", id => format!("{n}"), start => 1_u64, end => 1_u64),
        )?;
    }
    let _commit = writer.commit()?;
    writer.wait_merging_threads()?;
    let searcher = index.reader()?.searcher();
    let query = MeasuredQuery::default();
    let ledger = collection(100, searcher.segment_readers().len())?;
    let collector = GroupedPageCollector::new(ProjectionGroup::Path, 1.0, ledger.clone());
    let result = budgeted_collection(
        &searcher,
        &query,
        &collector,
        &RequestBudgetV1::unbounded(),
        ledger,
        "test:corruption",
    );
    assert!(matches!(result, Err(CoreError::Storage(message)) if message.contains("start_line")));
    assert_eq!(query.scored.load(Ordering::Relaxed), 1);
    assert_eq!(query.advanced.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn l3_work_cap_refuses_before_next_native_advance() -> TestResult {
    let index = index_with(&[&["a", "b", "c", "d"]])?;
    let searcher = index.reader()?.searcher();
    let query = MeasuredQuery::default();
    let resources = LexicalCollectionBudget::new(1, 1_000_000)?;
    let ledger = CollectionBudget::new(LexicalExecutionBudgetV1::new(100)?, resources.clone());
    let collector = GroupedPageCollector::new(ProjectionGroup::Path, 1.0, ledger.clone());
    let result = budgeted_collection(
        &searcher,
        &query,
        &collector,
        &RequestBudgetV1::unbounded(),
        ledger,
        "test:work",
    );
    assert!(matches!(
        result,
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexicalCollectionBudgetExceeded,
            ..
        })
    ));
    assert_eq!(query.scored.load(Ordering::Relaxed), 1);
    assert_eq!(query.advanced.load(Ordering::Relaxed), 0);
    assert_eq!(resources.used_work(), 1);
    assert_eq!(resources.resident_bytes(), 0);
    Ok(())
}

#[test]
fn l3_merge_work_exhaustion_is_typed_refusal_not_partial_success() -> TestResult {
    let index = index_with(&[&["a", "b", "c"]])?;
    let searcher = index.reader()?.searcher();
    let query = MeasuredQuery::default();
    // Three native documents + terminal advance + three segment representatives.
    // No work remains for the global merge's first representative.
    let resources = LexicalCollectionBudget::new(7, 1_000_000)?;
    let ledger = CollectionBudget::new(LexicalExecutionBudgetV1::new(100)?, resources.clone());
    let collector = GroupedPageCollector::new(ProjectionGroup::Path, 1.0, ledger.clone());
    let result = budgeted_collection(
        &searcher,
        &query,
        &collector,
        &RequestBudgetV1::unbounded(),
        ledger,
        "test:merge",
    );
    assert!(matches!(
        result,
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexicalCollectionBudgetExceeded,
            ..
        })
    ));
    assert_eq!(query.scored.load(Ordering::Relaxed), 3);
    assert_eq!(resources.used_work(), 7);
    assert_eq!(resources.resident_bytes(), 0);
    Ok(())
}

#[test]
fn l3_fruit_buffer_bytes_are_reserved_before_native_collection() -> TestResult {
    let index = index_with(&[&["a", "b"]])?;
    let searcher = index.reader()?.searcher();
    let query = MeasuredQuery::default();
    let resources = LexicalCollectionBudget::new(100, 1)?;
    let ledger = CollectionBudget::new(LexicalExecutionBudgetV1::new(100)?, resources.clone());
    let collector = GroupedPageCollector::new(ProjectionGroup::Path, 1.0, ledger.clone());
    let result = budgeted_collection(
        &searcher,
        &query,
        &collector,
        &RequestBudgetV1::unbounded(),
        ledger,
        "test:bytes",
    );
    assert!(matches!(
        result,
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexicalCollectionBudgetExceeded,
            ..
        })
    ));
    assert_eq!(query.opened.load(Ordering::Relaxed), 0);
    assert_eq!(
        (
            resources.used_work(),
            resources.resident_bytes(),
            resources.peak_bytes()
        ),
        (0, 0, 0)
    );
    Ok(())
}

#[test]
fn l3_group_map_bytes_refuse_before_next_document() -> TestResult {
    let index = index_with(&[&["a", "b", "c"]])?;
    let searcher = index.reader()?.searcher();
    let query = MeasuredQuery::default();
    // Enough for the outer fruit record, too small for an admitted BTree node.
    let resources = LexicalCollectionBudget::new(100, 128)?;
    let ledger = CollectionBudget::new(LexicalExecutionBudgetV1::new(100)?, resources.clone());
    let collector = GroupedPageCollector::new(ProjectionGroup::Path, 1.0, ledger.clone());
    let result = budgeted_collection(
        &searcher,
        &query,
        &collector,
        &RequestBudgetV1::unbounded(),
        ledger,
        "test:map-bytes",
    );
    assert!(matches!(
        result,
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexicalCollectionBudgetExceeded,
            ..
        })
    ));
    assert_eq!(query.scored.load(Ordering::Relaxed), 1);
    assert_eq!(query.advanced.load(Ordering::Relaxed), 0);
    assert!(resources.peak_bytes() <= 128);
    assert_eq!(resources.resident_bytes(), 0);
    Ok(())
}

#[test]
fn l3_returned_row_buffer_stays_reserved_until_iterator_drops() -> TestResult {
    let index = index_with(&[&["a", "b"]])?;
    let searcher = index.reader()?.searcher();
    let resources = LexicalCollectionBudget::new(100, 1_000_000)?;
    let ledger = CollectionBudget::new(LexicalExecutionBudgetV1::new(100)?, resources.clone());
    let collector = GroupedPageCollector::new(ProjectionGroup::Path, 1.0, ledger.clone());
    let fruit = budgeted_collection(
        &searcher,
        &MeasuredQuery::default(),
        &collector,
        &RequestBudgetV1::unbounded(),
        ledger,
        "test:retained-buffer",
    )?;
    assert_eq!(fruit.representatives.len(), 2);
    let retained = resources.resident_bytes();
    assert!(retained > 0);
    let mut rows = fruit.representatives.into_iter();
    let selected = rows.next();
    assert_eq!(
        resources.resident_bytes(),
        retained,
        "yielding one row must not release the iterator's allocation"
    );
    drop(selected);
    assert!(
        resources.resident_bytes() >= u64::try_from(2 * std::mem::size_of::<super::RankedRow>())?
    );
    drop(rows);
    assert_eq!(resources.resident_bytes(), 0);
    Ok(())
}

#[test]
fn l3_empty_index_reports_zero_without_consuming_work() -> TestResult {
    let index = index_with(&[])?;
    let searcher = index.reader()?.searcher();
    let query = MeasuredQuery::default();
    let resources = LexicalCollectionBudget::new(1, 1)?;
    let ledger = CollectionBudget::new(LexicalExecutionBudgetV1::new(1)?, resources.clone());
    let collector = GroupedPageCollector::new(ProjectionGroup::Path, 1.0, ledger.clone());
    let fruit = budgeted_collection(
        &searcher,
        &query,
        &collector,
        &RequestBudgetV1::unbounded(),
        ledger,
        "test:empty",
    )?;
    assert!(fruit.representatives.is_empty());
    assert_eq!(fruit.matched, 0);
    assert_eq!(query.opened.load(Ordering::Relaxed), 0);
    assert_eq!(
        (
            resources.used_work(),
            resources.resident_bytes(),
            resources.peak_bytes()
        ),
        (0, 0, 0)
    );
    assert!(resources.failure().is_none());
    Ok(())
}

#[test]
fn l3_generic_pruning_walk_stops_even_after_threshold_suppresses_callbacks() -> TestResult {
    let paths = vec!["same"; 64];
    let index = index_with(&[&paths])?;
    let searcher = index.reader()?.searcher();
    let query = MeasuredQuery {
        first_score: Some(10.0),
        ..MeasuredQuery::default()
    };
    let resources = LexicalCollectionBudget::new(3, 1_000_000)?;
    let ledger = CollectionBudget::new(LexicalExecutionBudgetV1::new(1)?, resources.clone());
    let collector =
        RankedPageCollector::new(1, None, 1.0, false).with_resource_budget(ledger.clone());
    let result = budgeted_collection(
        &searcher,
        &query,
        &collector,
        &RequestBudgetV1::unbounded(),
        ledger,
        "test:pruning-work",
    );
    assert!(matches!(
        result,
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexicalCollectionBudgetExceeded,
            ..
        })
    ));
    assert_eq!(query.scored.load(Ordering::Relaxed), 3);
    assert_eq!(query.advanced.load(Ordering::Relaxed), 2);
    assert_eq!(resources.resident_bytes(), 0);
    Ok(())
}

#[test]
fn l3_streaming_count_uses_resource_budget_without_candidate_materialization_cap() -> TestResult {
    let index = index_with(&[&["a", "b", "c", "d", "e"]])?;
    let searcher = index.reader()?.searcher();
    let resources = LexicalCollectionBudget::new(100, 1_000_000)?;
    let ledger = CollectionBudget::new(LexicalExecutionBudgetV1::new(2)?, resources.clone());
    let collector =
        RankedPageCollector::new(2, None, 1.0, true).with_resource_budget(ledger.clone());
    let fruit = budgeted_collection(
        &searcher,
        &MeasuredQuery::default(),
        &collector,
        &RequestBudgetV1::unbounded(),
        ledger,
        "test:stream-count",
    )?;
    assert_eq!(fruit.matched, 5);
    assert_eq!(fruit.rows.len(), 2);
    assert_eq!(resources.used_work(), 8); // Initial + five advances + two merge rows.
    drop(fruit);
    assert_eq!(resources.resident_bytes(), 0);
    Ok(())
}

#[test]
fn l3_source_repo_groups_and_cursor_walk_match_fixed_oracle_and_manual_path() -> TestResult {
    use quanta_index_contract::{
        LexicalCandidate, LexicalCursor, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
        SourceFileKey, SourceFileRevision,
    };
    let fixtures = [
        ("b", "same.rs", "same"),
        ("a", "z.rs", "z"),
        ("a", "same.rs", "same"),
        ("b", "same.rs", "worse-id"),
    ];
    let mut builder = Schema::builder();
    let repo = builder.add_text_field("repo_id", STRING | FAST);
    let path = builder.add_text_field("repo_relative_path", STRING | FAST);
    let id = builder.add_text_field("candidate_id", STRING | FAST);
    let start = builder.add_u64_field("start_line", FAST);
    let end = builder.add_u64_field("end_line", FAST);
    let index = Index::create_in_ram(builder.build());
    let mut writer: IndexWriter<TantivyDocument> = index.writer_with_num_threads(1, 15_000_000)?;
    writer.set_merge_policy(Box::new(NoMergePolicy));
    let mut manual = Vec::new();
    for (owner, file, candidate) in fixtures {
        let _op = writer.add_document(doc!(repo => owner, path => file, id => candidate,
            start => 1_u64, end => 1_u64))?;
        let _commit = writer.commit()?;
        manual.push(LexicalCandidate {
            source_repo_id: RepoId::new(owner)?,
            source: Some(SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new(owner)?,
                    repo_relative_path: RepoRelativePath::new(file),
                },
                revision_id: RevisionId::new("source-rev")?,
                source_sha256: [1; 32],
            }),
            preview: None,
            candidate_id: candidate.into(),
            repo_id: RepoId::new("container")?,
            revision_id: RevisionId::new("snapshot")?,
            manifest_generation: ManifestGeneration::new(1),
            repo_relative_path: RepoRelativePath::new(file),
            start_line: 1,
            end_line: 1,
            score: 1.0,
            snippet: String::new(),
            snippet_hit_offset: None,
            highlights: Vec::new(),
        });
    }
    writer.wait_merging_threads()?;
    let searcher = index.reader()?.searcher();
    for (group, expected) in [
        (
            ProjectionGroup::Path,
            vec![
                ("a", "same.rs", "same"),
                ("a", "z.rs", "z"),
                ("b", "same.rs", "same"),
            ],
        ),
        (
            ProjectionGroup::Repo,
            vec![("a", "same.rs", "same"), ("b", "same.rs", "same")],
        ),
    ] {
        let ledger = collection(20, searcher.segment_readers().len())?;
        let collector = GroupedPageCollector::new(group, 1.0, ledger.clone());
        let fruit = budgeted_collection(
            &searcher,
            &MeasuredQuery::default(),
            &collector,
            &RequestBudgetV1::unbounded(),
            ledger,
            "test:source-group",
        )?;
        let actual: Vec<_> = fruit
            .representatives
            .iter()
            .map(|row| {
                (
                    row.key.source_repo_id.as_str(),
                    row.key.repo_relative_path.as_str(),
                    row.key.candidate_id.as_str(),
                )
            })
            .collect();
        assert_eq!(actual, expected);
        let manual_rows =
            super::rank_in_memory(super::group_in_memory(manual.clone(), group), None);
        let actual: Vec<_> = manual_rows
            .iter()
            .map(|row| {
                let key = row.order_key();
                (key.source_repo_id, key.repo_relative_path, key.candidate_id)
            })
            .collect();
        assert_eq!(actual, expected);
    }
    for count in [false, true] {
        let mut after = None;
        let mut walked = Vec::new();
        for _page in 0..5 {
            let ledger = collection(20, searcher.segment_readers().len())?;
            let collector = RankedPageCollector::new(1, after.clone(), 1.0, count)
                .with_resource_budget(ledger.clone());
            let fruit = budgeted_collection(
                &searcher,
                &MeasuredQuery::default(),
                &collector,
                &RequestBudgetV1::unbounded(),
                ledger,
                "test:source-cursor",
            )?;
            let Some(row) = fruit.rows.first() else {
                break;
            };
            walked.push((row.key.source_repo_id.clone(), row.key.candidate_id.clone()));
            after = Some(Arc::new(LexicalCursor::at(
                ManifestGeneration::new(1),
                row.key.order_key(),
            )));
        }
        assert_eq!(
            walked,
            [("a", "same"), ("a", "z"), ("b", "same"), ("b", "worse-id")]
                .map(|(repo, id)| (repo.to_string(), id.to_string()))
        );
    }
    Ok(())
}

#[test]
fn l3_best_hit_in_last_visited_segment_replaces_earlier_group_representative() -> TestResult {
    let index = index_with(&[&["same.rs"], &["same.rs"], &["same.rs"]])?;
    let searcher = index.reader()?.searcher();
    let last = searcher.segment_readers().last().expect("three segments");
    let query = MeasuredQuery {
        high_segment: Some(last.segment_id()),
        ..MeasuredQuery::default()
    };
    let ledger = collection(10, searcher.segment_readers().len())?;
    let collector = GroupedPageCollector::new(ProjectionGroup::Path, 1.0, ledger.clone());
    let fruit = budgeted_collection(
        &searcher,
        &query,
        &collector,
        &RequestBudgetV1::unbounded(),
        ledger,
        "test:late-best",
    )?;
    assert_eq!(fruit.matched, 3);
    assert_eq!(fruit.representatives.len(), 1);
    let row = fruit.representatives.first().expect("representative");
    assert_eq!(row.key.score, 10.0);
    assert_eq!(row.address.segment_ord, 2);
    assert_eq!(row.key.repo_relative_path, "same.rs");
    Ok(())
}

#[test]
fn l3_term_pruning_preserves_results_and_refuses_work_exhaustion() -> TestResult {
    use tantivy::Term;
    use tantivy::query::TermQuery;
    use tantivy::schema::IndexRecordOption;
    let index = index_with(&[&["same.rs", "same.rs", "other.rs"]])?;
    let searcher = index.reader()?.searcher();
    let path = index.schema().get_field("repo_relative_path")?;
    let query = TermQuery::new(
        Term::from_field_text(path, "same.rs"),
        IndexRecordOption::Basic,
    );
    let expected = searcher.search(&query, &RankedPageCollector::new(2, None, 1.0, false))?;
    let ledger = collection(10, searcher.segment_readers().len())?;
    let collector =
        RankedPageCollector::new(2, None, 1.0, false).with_resource_budget(ledger.clone());
    let actual = budgeted_collection(
        &searcher,
        &query,
        &collector,
        &RequestBudgetV1::unbounded(),
        ledger,
        "test:term-pruning",
    )?;
    assert_eq!(actual.rows.len(), 2);
    for (got, want) in actual.rows.iter().zip(expected.rows.iter()) {
        assert_eq!(
            got.key.order_key().order(&want.key.order_key()),
            std::cmp::Ordering::Equal
        );
    }
    let resources = LexicalCollectionBudget::new(1, 1_000_000)?;
    let ledger = CollectionBudget::new(LexicalExecutionBudgetV1::new(10)?, resources.clone());
    let collector =
        RankedPageCollector::new(2, None, 1.0, false).with_resource_budget(ledger.clone());
    let result = budgeted_collection(
        &searcher,
        &query,
        &collector,
        &RequestBudgetV1::unbounded(),
        ledger,
        "test:term-pruning-refusal",
    );
    assert!(matches!(
        result,
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexicalCollectionBudgetExceeded,
            ..
        })
    ));
    assert_eq!(
        resources.used_work(),
        0,
        "postings bound is refused before native work"
    );
    assert_eq!(resources.resident_bytes(), 0);
    Ok(())
}
