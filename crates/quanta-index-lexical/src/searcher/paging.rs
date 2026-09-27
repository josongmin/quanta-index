//! Ranked pages, projections and the whole-set collect: how a query becomes rows.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::TantivySearcher;
use crate::budgeted_search::{CollectionBudget, budgeted_collection};
use crate::channel_payloads::count_from_len;
use crate::normalize::CaseMode;
use crate::ranked_page::{
    GroupedPageCollector, ProjectionGroup, RankedPageCollector, RankedPageFruit, RankedRowView,
    RankedRows,
};
use quanta_index_contract::{LexicalCursor, LqOptions, LqQuery, LqYesNoOnly};
use quanta_index_core::{CoreError, LexicalPageSpec, RequestBudgetV1};
use std::sync::Arc;
use tantivy::query::Query;
use tantivy::schema::TantivyDocument;

impl TantivySearcher {
    /// The DSL's one case default (`LqOptions::case_mode`), read here.
    pub(crate) fn is_case_sensitive(options: &LqOptions) -> bool {
        Self::case_mode(options) == CaseMode::Sensitive
    }

    pub(crate) fn case_mode(options: &LqOptions) -> CaseMode {
        options.case_mode()
    }

    /// Candidates a page needs: `top_k`, or `min(top_k, N)` under `count:N`.
    ///
    /// `count:all` no longer widens the page (QI-BB-005): rows are the
    /// caller's `top_k` and the total is reported through the count
    /// collector instead of by materializing every match.
    pub(crate) fn page_limit(query: &LqQuery, requested: usize) -> usize {
        match query.options.count {
            Some(quanta_index_contract::LqCountBound::Bounded(bound)) => {
                usize::try_from(bound).map_or(requested, |bound| requested.min(bound))
            }
            Some(quanta_index_contract::LqCountBound::All) | None => requested,
        }
    }

    pub(crate) fn wants_exact_total(query: &LqQuery) -> bool {
        query.options.count.is_some()
    }

    pub(crate) fn corpus_docs(
        searcher: &tantivy::Searcher,
        surface: &str,
    ) -> Result<usize, CoreError> {
        usize::try_from(searcher.num_docs()).map_err(|err| {
            CoreError::InvalidContract(format!("lexical: num_docs overflow in {surface}: {err}"))
        })
    }

    /// The projection a text query's `select:`/`type:` filter asks for.
    ///
    /// `select:repo` keeps one row per source repository; a path
    /// or file projection (`select:path`, `select:file`, `select:file.owners`,
    /// `type:path`) keeps one row per (source repository, path). With both,
    /// the coarser projection wins.
    pub(crate) fn projection_group(query: &LqQuery) -> Option<ProjectionGroup> {
        if Self::projects_repo_surface(query) {
            Some(ProjectionGroup::Repo)
        } else if Self::projects_path_surface(query)
            || Self::selects_file_projection(query)
            || Self::selects_file_owner_projection(query)
        {
            Some(ProjectionGroup::Path)
        } else {
            None
        }
    }

    /// The page's boundary, refused typed when it was cut from another
    /// generation: a score only compares within the ranking that made it.
    pub(crate) fn page_boundary(
        &self,
        page: &LexicalPageSpec,
    ) -> Result<Option<Arc<LexicalCursor>>, CoreError> {
        match &page.after {
            None => Ok(None),
            Some(cursor) if cursor.manifest_generation == self.generation => {
                Ok(Some(Arc::new(cursor.clone())))
            }
            Some(cursor) => Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::QueryCursorGenerationMismatch,
                message: format!(
                    "lexical: the cursor was cut from generation {} but this page reads generation {}",
                    cursor.manifest_generation.get(),
                    self.generation.get()
                ),
            }),
        }
    }

    /// One ranked page: the first `limit` rows after the boundary in exact
    /// page order, and — when `count` — every row after it counted.
    ///
    /// A page wider than the examined budget is refused rather than cut:
    /// a silently shorter page would read as the end of the matches.
    #[expect(
        clippy::too_many_arguments,
        reason = "one collect's inputs: where, what, how many, after what, how scored, whether counted, named how, under which budget"
    )]
    pub(crate) fn collect_ranked_page(
        &self,
        searcher: &tantivy::Searcher,
        compiled: &dyn Query,
        limit: usize,
        after: Option<Arc<LexicalCursor>>,
        boost: f32,
        count: bool,
        surface: &str,
        budget: &RequestBudgetV1,
    ) -> Result<RankedPageFruit, CoreError> {
        if limit > self.execution_budget.max_examined_candidates() {
            return Err(self.execution_budget.exceeded(surface));
        }
        let collection = CollectionBudget::new(
            self.execution_budget,
            self.execution_budget
                .collection_budget(searcher.segment_readers().len())?,
        );
        let collector =
            RankedPageCollector::new(Arc::clone(&self.ranked_keys), limit, after, boost, count)
                .with_resource_budget(collection.clone());
        budgeted_collection(
            searcher,
            compiled,
            &collector,
            budget,
            collection,
            "lexical:collect",
        )
    }

    /// Every match of a whole-set execution, in page order, refused past the
    /// examined budget (structural scope materialization).
    pub(crate) fn collect_whole_set(
        &self,
        searcher: &tantivy::Searcher,
        compiled: &dyn Query,
        boost: f32,
        surface: &str,
        budget: &RequestBudgetV1,
    ) -> Result<RankedRows, CoreError> {
        let examined = self.execution_budget.max_examined_candidates();
        let collection = CollectionBudget::new(
            self.execution_budget,
            self.execution_budget
                .collection_budget(searcher.segment_readers().len())?,
        );
        let collector =
            RankedPageCollector::new(Arc::clone(&self.ranked_keys), examined, None, boost, true)
                .with_collection_budget(collection.clone());
        let fruit = budgeted_collection(
            searcher,
            compiled,
            &collector,
            budget,
            collection,
            "lexical:collect",
        )?;
        if fruit.matched > count_from_len(examined)? {
            return Err(self.execution_budget.exceeded(surface));
        }
        Ok(fruit.rows)
    }

    /// One projected page: every group's first row, those after the
    /// boundary counted exactly, the first `limit` of them returned.
    ///
    /// Memory is the groups; the examined budget still bounds the matches
    /// the projection walks.
    #[expect(
        clippy::too_many_arguments,
        reason = "one grouped collect's inputs: where, what, grouped how, after what, how scored, how many, named how, under which budget"
    )]
    pub(crate) fn collect_projection(
        &self,
        searcher: &tantivy::Searcher,
        compiled: &dyn Query,
        group: ProjectionGroup,
        after: Option<&LexicalCursor>,
        boost: f32,
        limit: usize,
        surface: &str,
        budget: &RequestBudgetV1,
    ) -> Result<(RankedRows, u64), CoreError> {
        let collection = CollectionBudget::new(
            self.execution_budget,
            self.execution_budget
                .collection_budget(searcher.segment_readers().len())?,
        );
        let fruit = budgeted_collection(
            searcher,
            compiled,
            &GroupedPageCollector::new(
                Arc::clone(&self.ranked_keys),
                group,
                boost,
                collection.clone(),
            ),
            budget,
            collection,
            "lexical:collect",
        )?;
        let examined = count_from_len(self.execution_budget.max_examined_candidates())?;
        if fruit.matched > examined {
            return Err(self.execution_budget.exceeded(surface));
        }
        let mut rows = fruit.representatives;
        rows.retain(|row| after.is_none_or(|cursor| cursor.admits(&row.key.order_key())));
        let total = count_from_len(rows.len())?;
        rows.truncate(limit);
        Ok((rows, total))
    }

    /// The page's rows as candidates: one stored document per row, whose
    /// fields must agree with the order columns the row was ranked by.
    pub(crate) fn rows_to_candidates<T: RankedRowView>(
        &self,
        searcher: &tantivy::Searcher,
        rows: RankedRows,
        context: &mut crate::searcher::candidates::SelectedPreviewContext<'_>,
        convert: fn(
            &Self,
            &TantivyDocument,
            f32,
            &mut crate::searcher::candidates::SelectedPreviewContext<'_>,
        ) -> Result<T, CoreError>,
    ) -> Result<Vec<T>, CoreError> {
        let candidates = rows
            .into_iter()
            .map(|row| {
                let doc: TantivyDocument = searcher.doc(row.address).map_err(|err| {
                    CoreError::Storage(format!("lexical: fetch doc {:?}: {err}", row.address))
                })?;
                let candidate = convert(self, &doc, row.key.score, context)?;
                if candidate.ranked_key().order(&row.key.order_key()) != std::cmp::Ordering::Equal {
                    return Err(CoreError::Storage(format!(
                        "lexical: document {:?} stores {:?} but its order columns say {:?}",
                        row.address,
                        candidate.ranked_key(),
                        row.key.order_key()
                    )));
                }
                Ok(candidate)
            })
            .collect::<Result<Vec<T>, CoreError>>()?;
        context.retain_output_for_request()?;
        Ok(candidates)
    }

    pub(crate) fn uses_unindexed_scan(options: &LqOptions) -> bool {
        matches!(options.index_mode, Some(LqYesNoOnly::No))
    }

    #[expect(
        clippy::as_conversions,
        clippy::cast_precision_loss,
        reason = "boost_millis is a small score multiplier in milli-units; the u32->f32 widen then /1000.0 scales it back into a score factor with negligible precision impact for real boost magnitudes"
    )]
    pub(crate) fn boost_factor(options: &LqOptions) -> f32 {
        options
            .boost_millis
            .map_or(1.0, |millis| millis as f32 / 1_000.0)
    }

    pub(crate) fn apply_query_boost_score(score: f32, options: &LqOptions) -> f32 {
        score * Self::boost_factor(options)
    }
}
