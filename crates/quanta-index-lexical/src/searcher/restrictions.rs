//! Restriction queries: repo, path, candidate, text-authority and constraint filters.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::authority_doc_set::AuthorityDocSetQuery;
use crate::budgeted_search::budgeted_search;
use crate::documents::stored_text;
use crate::metadata_normalize::standard_pattern_options;
use crate::normalize::TextQueryError;
use crate::query_errors::{map_text_query_error, text_query_tokens};
use crate::{TEXT_DOC_KIND, TantivySearcher, normalize};
use quanta_index_contract::{
    LqExpr, LqFileScope, LqLeaf, LqOptions, LqPatternType, QueryConstraintSetV1,
};
use quanta_index_core::{CoreError, RequestBudgetV1};
use roaring::RoaringBitmap;
use std::collections::BTreeSet;
use std::sync::Arc;
use tantivy::Term;
use tantivy::collector::TopDocs;
use tantivy::query::{AllQuery, BooleanQuery, Occur, PhraseQuery, Query, RegexQuery, TermQuery};
use tantivy::schema::{Field, IndexRecordOption, TantivyDocument};

impl TantivySearcher {
    pub(crate) fn exact_text_query(&self, field: Field, value: &str) -> Box<dyn Query> {
        Box::new(TermQuery::new(
            Term::from_field_text(field, value),
            IndexRecordOption::Basic,
        ))
    }

    pub(crate) fn regex_text_query(
        &self,
        field: Field,
        pattern: &str,
    ) -> Result<Box<dyn Query>, CoreError> {
        let query = RegexQuery::from_pattern(pattern, field).map_err(|err| {
            CoreError::InvalidContract(format!("lexical: regex filter compile: {err}"))
        })?;
        Ok(Box::new(query))
    }

    pub(crate) fn compile_file_filter(
        &self,
        pattern: &str,
        scope: LqFileScope,
    ) -> Result<Box<dyn Query>, CoreError> {
        let path_query = self.regex_text_query(self.fields.repo_relative_path, pattern)?;
        let name_query = self.regex_text_query(self.fields.file_name, pattern)?;
        match scope {
            LqFileScope::PathOnly => Ok(path_query),
            LqFileScope::NameOnly => Ok(name_query),
            LqFileScope::NameAndPath => Ok(Box::new(BooleanQuery::new(vec![
                (Occur::Should, path_query),
                (Occur::Should, name_query),
            ]))),
        }
    }

    /// The fields a keyword is looked up in.
    ///
    /// Content, plus the tokenized path when the leaf surface allows path
    /// terms, in the case mode's analyzer.
    pub(crate) fn keyword_fields(
        &self,
        options: &LqOptions,
        include_path_terms: bool,
    ) -> Vec<Field> {
        let mut fields: Vec<Field> = Vec::with_capacity(if include_path_terms { 2 } else { 1 });
        if Self::is_case_sensitive(options) {
            fields.push(self.fields.chunk_text_case);
            if include_path_terms {
                fields.push(self.fields.repo_relative_path_case);
            }
        } else {
            fields.push(self.fields.chunk_text);
            if include_path_terms {
                fields.push(self.fields.repo_relative_path_query);
            }
        }
        fields
    }

    /// Lower a keyword leaf onto the inverted index.
    ///
    /// The literal is tokenized by the shared normalizer — never re-parsed
    /// by a second query grammar, so `-`, `:`, `^` and the like inside a
    /// keyword are boundaries, not operators. One token is a term query;
    /// several are a phrase query over the field's positions, which is what
    /// the position sidecar answers for the same text. A literal with no
    /// token, or with a run past the term cap, is refused typed.
    pub(crate) fn compile_keyword_leaf(
        &self,
        text: &str,
        options: &LqOptions,
        include_path_terms: bool,
    ) -> Result<Box<dyn Query>, CoreError> {
        let tokens = text_query_tokens(text, Self::case_mode(options))?;
        let mut per_field: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for field in self.keyword_fields(options, include_path_terms) {
            let query = Self::token_sequence_query(field, &tokens)
                .ok_or_else(|| map_text_query_error(&TextQueryError::NoTokens))?;
            per_field.push((Occur::Should, query));
        }
        if per_field.len() == 1
            && let Some((_, query)) = per_field.pop()
        {
            return Ok(query);
        }
        Ok(Box::new(BooleanQuery::new(per_field)))
    }

    /// A term query for one token, a phrase query for a sequence, nothing
    /// for an empty sequence.
    pub(crate) fn token_sequence_query(
        field: Field,
        tokens: &[normalize::Token],
    ) -> Option<Box<dyn Query>> {
        let (first, rest) = tokens.split_first()?;
        let term = |token: &normalize::Token| Term::from_field_text(field, &token.text);
        if rest.is_empty() {
            return Some(Box::new(TermQuery::new(
                term(first),
                IndexRecordOption::WithFreqs,
            )));
        }
        Some(Box::new(PhraseQuery::new(
            tokens.iter().map(term).collect(),
        )))
    }

    pub(crate) fn enables_path_term_surface(expr: &LqExpr, options: &LqOptions) -> bool {
        options.pattern_type != LqPatternType::Regexp
            && matches!(expr, LqExpr::Leaf(LqLeaf::Keyword(_)))
    }

    pub(crate) fn repo_id_restriction_query(&self, repo_ids: &BTreeSet<String>) -> Box<dyn Query> {
        if repo_ids.len() == 1
            && let Some(repo_id) = repo_ids.iter().next()
        {
            return self.exact_text_query(self.fields.repo_id, repo_id);
        }
        Box::new(BooleanQuery::new(
            repo_ids
                .iter()
                .map(|repo_id| {
                    (
                        Occur::Should,
                        self.exact_text_query(self.fields.repo_id, repo_id),
                    )
                })
                .collect(),
        ))
    }

    pub(crate) fn path_restriction_query(&self, paths: &BTreeSet<String>) -> Box<dyn Query> {
        if paths.len() == 1
            && let Some(path) = paths.iter().next()
        {
            return self.exact_text_query(self.fields.repo_relative_path, path);
        }
        Box::new(BooleanQuery::new(
            paths
                .iter()
                .map(|path| {
                    (
                        Occur::Should,
                        self.exact_text_query(self.fields.repo_relative_path, path),
                    )
                })
                .collect(),
        ))
    }

    /// The text documents whose text-authority doc id is a member of
    /// `members`: one query over the shared set, nothing built per member
    /// (QI-BB-024). An empty set matches nothing.
    pub(crate) fn authority_restriction_query(
        &self,
        members: Arc<RoaringBitmap>,
    ) -> Box<dyn Query> {
        if members.is_empty() {
            return self.match_none_query();
        }
        Box::new(AuthorityDocSetQuery::new(
            self.fields.text_authority_doc_id,
            members,
        ))
    }

    /// The documents of candidates named from outside the index: the dense
    /// lane's candidates at admission, as many as the dense top-k, text or
    /// symbol. Every set the index derives itself restricts through
    /// [`Self::authority_restriction_query`] instead.
    pub(crate) fn candidate_restriction_query(
        &self,
        candidate_ids: &BTreeSet<String>,
    ) -> Box<dyn Query> {
        if candidate_ids.len() == 1
            && let Some(candidate_id) = candidate_ids.iter().next()
        {
            return self.exact_text_query(self.fields.candidate_id, candidate_id);
        }
        Box::new(BooleanQuery::new(
            candidate_ids
                .iter()
                .map(|candidate_id| {
                    (
                        Occur::Should,
                        self.exact_text_query(self.fields.candidate_id, candidate_id),
                    )
                })
                .collect(),
        ))
    }

    pub(crate) fn match_none_query(&self) -> Box<dyn Query> {
        Box::new(BooleanQuery::new(vec![
            (Occur::Must, Box::new(AllQuery)),
            (Occur::MustNot, Box::new(AllQuery)),
        ]))
    }

    pub(crate) fn with_doc_kind(&self, query: Box<dyn Query>, doc_kind: &str) -> Box<dyn Query> {
        let doc_kind_term = Term::from_field_text(self.fields.doc_kind, doc_kind);
        Box::new(BooleanQuery::new(vec![
            (Occur::Must, query),
            (
                Occur::Must,
                Box::new(TermQuery::new(doc_kind_term, IndexRecordOption::Basic)),
            ),
        ]))
    }

    pub(crate) fn language_any_of_query(
        &self,
        constraints: &QueryConstraintSetV1,
    ) -> Box<dyn Query> {
        let mut clauses = Vec::with_capacity(constraints.language_any_of.len());
        for language in &constraints.language_any_of {
            clauses.push((
                Occur::Should,
                self.exact_text_query(self.fields.language, language.as_str()),
            ));
        }
        Box::new(BooleanQuery::new(clauses))
    }

    pub(crate) fn manual_exact_path_allows(
        repo_relative_path: &str,
        constraints: &QueryConstraintSetV1,
    ) -> bool {
        constraints
            .repo_relative_path_exact
            .as_ref()
            .is_none_or(|expected| expected.as_str() == repo_relative_path)
    }

    pub(crate) fn ensure_manual_scan_supports_constraints(
        constraints: &QueryConstraintSetV1,
        surface: &str,
    ) -> Result<(), CoreError> {
        if constraints.language_any_of.is_empty() {
            return Ok(());
        }
        Err(CoreError::NotImplemented(format!(
            "{surface}: typed language constraints require indexed execution; index:no cannot read the non-stored language field"
        )))
    }

    pub(crate) fn with_doc_kind_and_constraints(
        &self,
        query: Box<dyn Query>,
        doc_kind: &str,
        constraints: &QueryConstraintSetV1,
    ) -> Box<dyn Query> {
        let typed = self.with_doc_kind(query, doc_kind);
        if constraints.is_unconstrained() {
            return typed;
        }
        let mut clauses = vec![(Occur::Must, typed)];
        if !constraints.language_any_of.is_empty() {
            clauses.push((Occur::Must, self.language_any_of_query(constraints)));
        }
        if let Some(path) = &constraints.repo_relative_path_exact {
            clauses.push((
                Occur::Must,
                self.exact_text_query(self.fields.repo_relative_path, path.as_str()),
            ));
        }
        Box::new(BooleanQuery::new(clauses))
    }

    pub(crate) fn collect_matching_paths_for_leaf(
        &self,
        leaf: &LqLeaf,
        budget: &RequestBudgetV1,
    ) -> Result<BTreeSet<String>, CoreError> {
        // Predicate scope collection (`file.contains` / `file.has.content`)
        // intentionally compiles with `LqPatternType::Standard` regardless of
        // the caller's pattern type: it is a path-discovery prelude, not a
        // user-facing leaf evaluation. The full caller options carry through
        // to the user-facing executor pass downstream.
        let scope_options = standard_pattern_options();
        let compiled = self.with_doc_kind(
            self.compile_leaf(leaf, &scope_options, false, budget)?,
            TEXT_DOC_KIND,
        );
        let searcher = self.reader.searcher();
        let limit = usize::try_from(searcher.num_docs()).map_err(|err| {
            CoreError::InvalidContract(format!(
                "lexical: num_docs overflow while collecting predicate scope: {err}"
            ))
        })?;
        if limit == 0 {
            return Ok(BTreeSet::new());
        }
        let hits = budgeted_search(
            &searcher,
            &*compiled,
            &TopDocs::with_limit(limit),
            budget,
            "lexical:predicate-scope",
        )?;
        let mut out: BTreeSet<String> = BTreeSet::new();
        for (_, doc_address) in hits {
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            if let Some(path) = stored_text(&doc, self.fields.repo_relative_path) {
                let _inserted: bool = out.insert(path);
            }
        }
        Ok(out)
    }
}
