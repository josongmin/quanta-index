//! Repo and file predicates: the constraints they compile to and the sets they match.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::documents::{required_stored_text, stored_doc_kind};
use crate::metadata_normalize::{
    normalize_contributor_identity, normalize_language, normalize_owner_identity,
    normalize_repo_meta_pattern, normalize_repo_topic_value,
};
use crate::predicate_registry::{
    ContentPathScope, ContentPredicateArgError, ContentPredicateConstraint, ContentScalarArg,
    ContentScalarArgError, ContributorPattern, FileContributorArg, FileContributorArgError,
    FileOwnerArg, FileOwnerArgError, MetaPattern, PREDICATE_OWNER, RepoDescriptionArg,
    RepoDescriptionArgError, RepoFileArgError, RepoFileConstraint, RepoFileMatcher, RepoMetaArg,
    RepoMetaArgError, RepoTopicArg, RepoTopicArgError, TimerefScalarArgError,
    canonicalize_predicate_call, parse_content_predicate_constraint, parse_content_scalar_arg,
    parse_file_contributor_arg, parse_file_owner_arg, parse_repo_description_arg,
    parse_repo_file_matchers, parse_repo_meta_arg, parse_repo_topic_arg, parse_timeref_scalar_arg,
    unimplemented_predicate,
};
use crate::searcher::query_rewrite::content_leaf_from_scalar;
use crate::{TEXT_DOC_KIND, TantivySearcher};
use quanta_index_contract::{LqLeaf, LqOptions, LqPredicateArg};
use quanta_index_core::{CoreError, RequestBudgetV1, timeref::parse_search_timeref_ms};
use quanta_index_lq_regex::RegexExecutor;
use roaring::RoaringBitmap;
use std::collections::BTreeSet;
use tantivy::query::{AllQuery, BooleanQuery, Occur, Query};
use tantivy::schema::TantivyDocument;

impl TantivySearcher {
    pub(crate) fn repo_has_file_path_query(
        &self,
        constraint: &RepoFileConstraint,
        options: &LqOptions,
        budget: &RequestBudgetV1,
    ) -> Result<Box<dyn Query>, CoreError> {
        let mut clauses: Vec<(Occur, Box<dyn Query>)> =
            Vec::with_capacity(constraint.matchers.len());
        for matcher in &constraint.matchers {
            let query = match matcher {
                RepoFileMatcher::Path(pattern) => {
                    self.regex_text_query(self.fields.repo_relative_path, pattern)?
                }
                RepoFileMatcher::Name(pattern) => {
                    self.regex_text_query(self.fields.file_name, pattern)?
                }
                RepoFileMatcher::Content(value) => {
                    // SGX-01: AND the content clause onto the SAME per-document
                    // BooleanQuery as the path/name/lang clauses, so a repo
                    // matches only when ONE file satisfies path AND content.
                    // Lower through content_leaf_from_scalar so `/regex/` content
                    // stays a regex; compile_leaf builds the chunk_text query.
                    let leaf = content_leaf_from_scalar(&ContentScalarArg::Keyword(value.clone()));
                    self.compile_leaf(&leaf, options, false, budget)?
                }
                RepoFileMatcher::Language(value) => {
                    // normalize_language case-folds the value for the exact
                    // language-field match. The registry already rejects
                    // empty/whitespace lang values, so the `None` branch is
                    // unreachable on every path that builds a constraint through
                    // parse_repo_file_matchers; it is kept as a fail-closed guard
                    // (not a panic) against a future caller constructing
                    // RepoFileMatcher::Language directly.
                    let Some(normalized) = normalize_language(value) else {
                        return Err(unimplemented_predicate(format!(
                            "lexical: predicate leaf `repo.has.file` lang: argument cannot be empty (owner: {PREDICATE_OWNER})"
                        )));
                    };
                    self.exact_text_query(self.fields.language, &normalized)
                }
            };
            clauses.push((Occur::Must, query));
        }
        Ok(self.with_doc_kind(Box::new(BooleanQuery::new(clauses)), TEXT_DOC_KIND))
    }

    pub(crate) fn collect_repo_ids_for_repo_has_file(
        &self,
        constraint: &RepoFileConstraint,
        options: &LqOptions,
        budget: &RequestBudgetV1,
    ) -> Result<BTreeSet<String>, CoreError> {
        let compiled = self.repo_has_file_path_query(constraint, options, budget)?;
        let searcher = self.reader.searcher();
        let rows =
            self.collect_whole_set(&searcher, &*compiled, 1.0, "repo.has.file scope", budget)?;
        let mut out: BTreeSet<String> = BTreeSet::new();
        for row in rows {
            budget.checkpoint("lexical:scope-decode")?;
            let doc_address = row.address;
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            let repo_id = required_stored_text(&doc, self.fields.repo_id, "repo_id")?;
            let _inserted: bool = out.insert(repo_id.to_owned());
        }
        Ok(out)
    }

    pub(crate) fn canonicalize_predicate_call(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<Option<(String, Vec<LqPredicateArg>)>, CoreError> {
        canonicalize_predicate_call(name, args)
            .map(|call| call.map(|canonical| (canonical.name.to_string(), canonical.args)))
            .map_err(|_err| {
                unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` does not admit this alias argument shape (owner: {PREDICATE_OWNER})"
                ))
            })
    }

    pub(crate) fn content_predicate_constraint(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<ContentPredicateConstraint, CoreError> {
        parse_content_predicate_constraint(args).map_err(|err| match err {
            ContentPredicateArgError::MissingContentScalar => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` requires exactly one content scalar argument (owner: {PREDICATE_OWNER})"
            )),
            ContentPredicateArgError::MultipleContentScalars => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` only supports one content scalar argument (owner: {PREDICATE_OWNER})"
            )),
            ContentPredicateArgError::UnsupportedFilter => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` only supports one content scalar plus optional file:/path:/name:/lang: scope filters (owner: {PREDICATE_OWNER})"
            )),
            ContentPredicateArgError::DuplicatePathScope => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` only supports one file:/path:/name: scope filter (owner: {PREDICATE_OWNER})"
            )),
            ContentPredicateArgError::DuplicateLanguage => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` only supports one lang: scope filter (owner: {PREDICATE_OWNER})"
            )),
            ContentPredicateArgError::EmptyLanguageValue => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` lang: argument cannot be empty (owner: {PREDICATE_OWNER})"
            )),
        })
    }

    /// Validate `repo.has.content(...)` and lower it to its content leaf.
    pub(crate) fn repo_content_constraint(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<LqLeaf, CoreError> {
        let arg = parse_content_scalar_arg(args).map_err(|err| match err {
            ContentScalarArgError::WrongArity => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` requires exactly one content scalar argument (owner: {PREDICATE_OWNER})"
            )),
            ContentScalarArgError::UnsupportedArg => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` only supports a keyword/phrase/raw-string/number content argument (owner: {PREDICATE_OWNER})"
            )),
        })?;
        Ok(content_leaf_from_scalar(&arg))
    }

    pub(crate) fn repo_commit_after_constraint(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<String, CoreError> {
        parse_timeref_scalar_arg(args)
            .map(|arg| arg.value)
            .map_err(|err| match err {
                TimerefScalarArgError::WrongArity => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` requires exactly one timeref scalar argument (owner: {PREDICATE_OWNER})"
                )),
                TimerefScalarArgError::UnsupportedArg => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` only supports one keyword/phrase/raw-string/number timeref argument (owner: {PREDICATE_OWNER})"
                )),
            })
    }

    /// Compile a `repo.has.content` leaf into its repo-scope content query.
    ///
    /// Unlike `file.contains` path-discovery (which uses `standard_pattern_options`
    /// as a prelude), this collection IS the user-visible evaluation, so it
    /// preserves the caller's `case` / `patterntype` options.
    pub(crate) fn repo_has_content_query(
        &self,
        leaf: &LqLeaf,
        options: &LqOptions,
        budget: &RequestBudgetV1,
    ) -> Result<Box<dyn Query>, CoreError> {
        Ok(self.with_doc_kind(
            self.compile_leaf(leaf, options, false, budget)?,
            TEXT_DOC_KIND,
        ))
    }

    pub(crate) fn collect_repo_ids_for_repo_has_content(
        &self,
        leaf: &LqLeaf,
        options: &LqOptions,
        budget: &RequestBudgetV1,
    ) -> Result<BTreeSet<String>, CoreError> {
        let compiled = self.repo_has_content_query(leaf, options, budget)?;
        let searcher = self.reader.searcher();
        let rows =
            self.collect_whole_set(&searcher, &*compiled, 1.0, "repo.has.content scope", budget)?;
        let mut out: BTreeSet<String> = BTreeSet::new();
        for row in rows {
            budget.checkpoint("lexical:scope-decode")?;
            let doc_address = row.address;
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            let repo_id = required_stored_text(&doc, self.fields.repo_id, "repo_id")?;
            let _inserted: bool = out.insert(repo_id.to_owned());
        }
        Ok(out)
    }

    pub(crate) fn collect_repo_ids_for_repo_has_commit_after(
        &self,
        timeref: &str,
    ) -> Result<BTreeSet<String>, CoreError> {
        let Some(boundary_ms) = parse_search_timeref_ms(timeref) else {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::HistoryInvalidTimeref,
                message: format!(
                    "history: timeref `{timeref}` is not a valid RFC3339 timestamp, date-only value, duration, or supported human timeref"
                ),
            });
        };
        let authority = self.repo_commit_recency_authority()?;
        Ok(authority
            .latest_committer_time_ms_by_repo_id
            .iter()
            .filter(|&(_, latest_committer_time_ms)| *latest_committer_time_ms > boundary_ms)
            .map(|(repo_id, _)| repo_id.clone())
            .collect())
    }

    pub(crate) fn repo_meta_constraint(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<RepoMetaArg, CoreError> {
        parse_repo_meta_arg(args)
            .and_then(|arg| {
                let key = normalize_repo_meta_pattern(arg.key)?;
                let value = arg.value.map(normalize_repo_meta_pattern).transpose()?;
                Ok(RepoMetaArg {
                    key,
                    value,
                })
            })
            .map_err(|err| match err {
                RepoMetaArgError::WrongArity => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` requires exactly one metadata argument (owner: {PREDICATE_OWNER})"
                )),
                RepoMetaArgError::UnsupportedArg => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` only supports `key:value`, bare `key`, `tag:`, or slash-delimited regex key/value metadata shapes (owner: {PREDICATE_OWNER})"
                )),
                RepoMetaArgError::EmptyKey => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` requires a non-empty metadata key (owner: {PREDICATE_OWNER})"
                )),
            })
    }

    pub(crate) fn compile_repo_meta_pattern(
        &self,
        field_name: &str,
        pattern: &MetaPattern,
    ) -> Result<Option<RegexExecutor>, CoreError> {
        let MetaPattern::Regex(source) = pattern else {
            return Ok(None);
        };
        RegexExecutor::compile(source)
            .map(Some)
            .map_err(|err| CoreError::Typed {
                code: crate::query_errors::regex_wire_code(err.code),
                message: format!(
                    "lexical: repo.has.meta {field_name} regex {source:?} failed to compile: {err}"
                ),
            })
    }

    pub(crate) fn repo_meta_pattern_matches(
        &self,
        pattern: &MetaPattern,
        executor: Option<&RegexExecutor>,
        candidate: &str,
    ) -> bool {
        match (pattern, executor) {
            (MetaPattern::Exact(expected), _) => candidate == expected,
            (MetaPattern::Regex(_), Some(executor)) => executor.verify(candidate.as_bytes()),
            (MetaPattern::Regex(_), None) => false,
        }
    }

    pub(crate) fn collect_repo_ids_for_repo_has_meta(
        &self,
        arg: &RepoMetaArg,
    ) -> Result<BTreeSet<String>, CoreError> {
        let authority = self.repo_meta_authority()?;
        let key_executor = self.compile_repo_meta_pattern("key", &arg.key)?;
        let value_executor = arg
            .value
            .as_ref()
            .map(|pattern| self.compile_repo_meta_pattern("value", pattern))
            .transpose()?
            .flatten();
        Ok(authority
            .meta_by_repo_id
            .iter()
            .filter(|(_, by_key)| {
                by_key.iter().any(|(key, value)| {
                    self.repo_meta_pattern_matches(&arg.key, key_executor.as_ref(), key)
                        && arg.value.as_ref().is_none_or(|pattern| {
                            self.repo_meta_pattern_matches(pattern, value_executor.as_ref(), value)
                        })
                })
            })
            .map(|(repo_id, _)| repo_id.clone())
            .collect())
    }

    pub(crate) fn repo_topic_constraint(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<RepoTopicArg, CoreError> {
        parse_repo_topic_arg(args)
            .and_then(|arg| {
                let Some(topic) = normalize_repo_topic_value(&arg.topic) else {
                    return Err(RepoTopicArgError::EmptyTopic);
                };
                Ok(RepoTopicArg { topic })
            })
            .map_err(|err| match err {
                RepoTopicArgError::WrongArity => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` requires exactly one topic scalar argument (owner: {PREDICATE_OWNER})"
                )),
                RepoTopicArgError::UnsupportedArg => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` only supports one keyword/phrase/raw-string topic argument (owner: {PREDICATE_OWNER})"
                )),
                RepoTopicArgError::EmptyTopic => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` topic cannot be empty (owner: {PREDICATE_OWNER})"
                )),
            })
    }

    pub(crate) fn collect_repo_ids_for_repo_has_topic(
        &self,
        arg: &RepoTopicArg,
    ) -> Result<BTreeSet<String>, CoreError> {
        let authority = self.repo_topic_authority()?;
        Ok(authority
            .topics_by_repo_id
            .iter()
            .filter(|(_, topics)| topics.contains(&arg.topic))
            .map(|(repo_id, _)| repo_id.clone())
            .collect())
    }

    pub(crate) fn repo_description_constraint(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<RepoDescriptionArg, CoreError> {
        parse_repo_description_arg(args).map_err(|err| match err {
            RepoDescriptionArgError::WrongArity => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` requires exactly one description pattern scalar argument (owner: {PREDICATE_OWNER})"
            )),
            RepoDescriptionArgError::UnsupportedArg => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` only supports one keyword/phrase/raw-string description pattern argument (owner: {PREDICATE_OWNER})"
            )),
            RepoDescriptionArgError::EmptyPattern => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` description pattern cannot be empty (owner: {PREDICATE_OWNER})"
            )),
        })
    }

    pub(crate) fn collect_repo_ids_for_repo_has_description(
        &self,
        arg: &RepoDescriptionArg,
    ) -> Result<BTreeSet<String>, CoreError> {
        let authority = self.repo_description_authority()?;
        // SG `repo:has.description(<pattern>)` matches the repo description as a
        // regex. Compile the producer-published pattern once and verify it
        // against each repo's verbatim description. A malformed pattern is a
        // typed query error, not a silent empty result.
        //
        // We compile directly via `RegexExecutor::compile` (which applies the
        // upstream structural charge and engine byte ceilings) rather than
        // `crate::regex::plan_regex`:
        // `plan_regex` exists to drive the trigram pre-filter over the indexed
        // content corpus and enforces its literal policy. Description matching
        // verifies verbatim metadata without the content trigram prefilter.
        // These compile gates do not establish aggregate heap admission.
        let executor = RegexExecutor::compile(&arg.pattern).map_err(|err| CoreError::Typed {
            code: crate::query_errors::regex_wire_code(err.code),
            message: format!(
                "lexical: repo.has.description pattern {:?} failed to compile: {err}",
                arg.pattern
            ),
        })?;
        Ok(authority
            .descriptions_by_repo_id
            .iter()
            .filter(|(_, description)| executor.verify(description.as_bytes()))
            .map(|(repo_id, _)| repo_id.clone())
            .collect())
    }

    pub(crate) fn file_owner_constraint(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<FileOwnerArg, CoreError> {
        parse_file_owner_arg(args)
            .and_then(|arg| match arg.owner {
                Some(owner) => {
                    let Some(owner) = normalize_owner_identity(&owner) else {
                        return Err(FileOwnerArgError::EmptyOwner);
                    };
                    Ok(FileOwnerArg { owner: Some(owner) })
                }
                None => Ok(arg),
            })
            .map_err(|err| match err {
                FileOwnerArgError::WrongArity => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` supports zero args (`any owner`) or exactly one owner identity argument (owner: {PREDICATE_OWNER})"
                )),
                FileOwnerArgError::UnsupportedArg => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` only supports zero args or one keyword/phrase/raw-string owner identity (owner: {PREDICATE_OWNER})"
                )),
                FileOwnerArgError::EmptyOwner => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` owner identity cannot be empty (owner: {PREDICATE_OWNER})"
                )),
            })
    }

    /// The text documents whose file has an owner matching `arg`, as
    /// authority doc ids.
    pub(crate) fn file_owner_match_set(
        &self,
        arg: &FileOwnerArg,
        budget: &RequestBudgetV1,
    ) -> Result<RoaringBitmap, CoreError> {
        let authority = self.file_ownership_authority()?;
        let searcher = self.reader.searcher();
        let compiled = self.with_doc_kind(Box::new(AllQuery), TEXT_DOC_KIND);
        let mut out = RoaringBitmap::new();
        let rows = self.collect_whole_set(
            &searcher,
            &*compiled,
            1.0,
            "file ownership candidates",
            budget,
        )?;
        for row in rows {
            budget.checkpoint("lexical:scope-decode")?;
            let doc_address = row.address;
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            if stored_doc_kind(&doc, self.fields.doc_kind)? != TEXT_DOC_KIND {
                return Err(CoreError::Storage(
                    "lexical: indexed text document has non-text stored doc_kind".to_string(),
                ));
            }
            let source_repo_id = required_stored_text(&doc, self.fields.repo_id, "repo_id")?;
            let repo_relative_path =
                required_stored_text(&doc, self.fields.repo_relative_path, "repo_relative_path")?;
            let candidate_id =
                required_stored_text(&doc, self.fields.candidate_id, "candidate_id")?;
            let Some(owners) = authority
                .owners_by_repo_id
                .get(source_repo_id)
                .and_then(|by_path| by_path.get(repo_relative_path))
            else {
                continue;
            };
            let matches = arg
                .owner
                .as_ref()
                .map_or(!owners.is_empty(), |owner| owners.contains(owner));
            if matches {
                let _inserted: bool =
                    out.insert(self.stored_text_member(&doc, candidate_id, "file.has.owner")?);
            }
        }
        Ok(out)
    }

    pub(crate) fn file_contributor_constraint(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<FileContributorArg, CoreError> {
        parse_file_contributor_arg(args)
            .and_then(|arg| match arg.contributor {
                ContributorPattern::Exact(contributor) => {
                    let Some(contributor) = normalize_contributor_identity(&contributor) else {
                        return Err(FileContributorArgError::EmptyContributor);
                    };
                    Ok(FileContributorArg {
                        contributor: ContributorPattern::Exact(contributor),
                    })
                }
                ContributorPattern::Regex(source) => Ok(FileContributorArg {
                    contributor: ContributorPattern::Regex(source.to_ascii_lowercase()),
                }),
            })
            .map_err(|err| match err {
                FileContributorArgError::WrongArity => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` requires exactly one contributor identity argument (owner: {PREDICATE_OWNER})"
                )),
                FileContributorArgError::UnsupportedArg => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` only supports one keyword/phrase/raw-string contributor identity or `/.../` regex contributor pattern (owner: {PREDICATE_OWNER})"
                )),
                FileContributorArgError::EmptyContributor => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` contributor identity cannot be empty (owner: {PREDICATE_OWNER})"
                )),
            })
    }

    /// The text documents whose file has a contributor matching `arg`, as
    /// authority doc ids.
    pub(crate) fn file_contributor_match_set(
        &self,
        arg: &FileContributorArg,
        budget: &RequestBudgetV1,
    ) -> Result<RoaringBitmap, CoreError> {
        let authority = self.file_contributor_authority()?;
        let searcher = self.reader.searcher();
        let compiled = self.with_doc_kind(Box::new(AllQuery), TEXT_DOC_KIND);
        let mut out = RoaringBitmap::new();
        let contributor_regex = match &arg.contributor {
            ContributorPattern::Regex(source) => Some(RegexExecutor::compile(source).map_err(
                |err| CoreError::Typed {
                    code: crate::query_errors::regex_wire_code(err.code),
                    message: format!(
                        "lexical: file.has.contributor regex {source:?} failed to compile: {err}"
                    ),
                },
            )?),
            ContributorPattern::Exact(_) => None,
        };
        let rows = self.collect_whole_set(
            &searcher,
            &*compiled,
            1.0,
            "file contributor candidates",
            budget,
        )?;
        for row in rows {
            budget.checkpoint("lexical:scope-decode")?;
            let doc_address = row.address;
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            if stored_doc_kind(&doc, self.fields.doc_kind)? != TEXT_DOC_KIND {
                return Err(CoreError::Storage(
                    "lexical: indexed text document has non-text stored doc_kind".to_string(),
                ));
            }
            let source_repo_id = required_stored_text(&doc, self.fields.repo_id, "repo_id")?;
            let repo_relative_path =
                required_stored_text(&doc, self.fields.repo_relative_path, "repo_relative_path")?;
            let candidate_id =
                required_stored_text(&doc, self.fields.candidate_id, "candidate_id")?;
            let Some(contributors) = authority
                .contributors_by_repo_id
                .get(source_repo_id)
                .and_then(|by_path| by_path.get(repo_relative_path))
            else {
                continue;
            };
            let matches = match &arg.contributor {
                ContributorPattern::Exact(contributor) => contributors
                    .iter()
                    .any(|identity| identity.canonical == *contributor),
                ContributorPattern::Regex(_) => {
                    let Some(executor) = contributor_regex.as_ref() else {
                        return Err(CoreError::Storage(
                            "lexical: contributor regex executor missing for a compiled regex pattern"
                                .to_string(),
                        ));
                    };
                    contributors.iter().any(|identity| {
                        identity
                            .name
                            .as_deref()
                            .is_some_and(|name| executor.verify(name.as_bytes()))
                            || identity
                                .email
                                .as_deref()
                                .is_some_and(|email| executor.verify(email.as_bytes()))
                    })
                }
            };
            if matches {
                let _inserted: bool = out.insert(self.stored_text_member(
                    &doc,
                    candidate_id,
                    "file.has.contributor",
                )?);
            }
        }
        Ok(out)
    }

    pub(crate) fn predicate_content_leaf_from_constraint(
        &self,
        constraint: &ContentPredicateConstraint,
    ) -> LqLeaf {
        content_leaf_from_scalar(&constraint.content)
    }

    pub(crate) fn collect_matching_files_for_content_scope(
        &self,
        constraint: &ContentPredicateConstraint,
        budget: &RequestBudgetV1,
    ) -> Result<Option<BTreeSet<quanta_index_contract::SourceFileKey>>, CoreError> {
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        if let Some(ContentPathScope { pattern, scope }) = constraint.path_scope.as_ref() {
            clauses.push((Occur::Must, self.compile_file_filter(pattern, *scope)?));
        }
        if let Some(language) = constraint.language.as_ref() {
            let Some(normalized) = normalize_language(language) else {
                return Err(CoreError::InvalidContract(
                    "lexical: scoped content predicate escaped with an empty lang value"
                        .to_string(),
                ));
            };
            clauses.push((
                Occur::Must,
                self.exact_text_query(self.fields.language, &normalized),
            ));
        }
        if clauses.is_empty() {
            return Ok(None);
        }
        let compiled = self.with_doc_kind(Box::new(BooleanQuery::new(clauses)), TEXT_DOC_KIND);
        let searcher = self.reader.searcher();
        let rows =
            self.collect_whole_set(&searcher, &*compiled, 1.0, "scoped content files", budget)?;
        let mut out: BTreeSet<quanta_index_contract::SourceFileKey> = BTreeSet::new();
        for row in rows {
            budget.checkpoint("lexical:scope-decode")?;
            let doc_address = row.address;
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            let _inserted: bool = out.insert(crate::documents::stored_source_file_key(
                &doc,
                &self.fields,
            )?);
        }
        Ok(Some(out))
    }

    /// The text documents at `files`, as authority doc ids.
    pub(crate) fn source_file_match_set(
        &self,
        files: &BTreeSet<quanta_index_contract::SourceFileKey>,
        budget: &RequestBudgetV1,
    ) -> Result<RoaringBitmap, CoreError> {
        let mut out = RoaringBitmap::new();
        if files.is_empty() {
            return Ok(out);
        }
        let compiled = self.with_doc_kind(self.source_file_restriction_query(files), TEXT_DOC_KIND);
        let searcher = self.reader.searcher();
        let rows = self.collect_whole_set(
            &searcher,
            &*compiled,
            1.0,
            "scoped content candidate ids",
            budget,
        )?;
        for row in rows {
            budget.checkpoint("lexical:scope-decode")?;
            let doc_address = row.address;
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            let candidate_id =
                required_stored_text(&doc, self.fields.candidate_id, "candidate_id")?;
            let _inserted: bool =
                out.insert(self.stored_text_member(&doc, candidate_id, "content scope")?);
        }
        Ok(out)
    }

    pub(crate) fn allowed_files_for_content_predicate(
        &self,
        constraint: &ContentPredicateConstraint,
        budget: &RequestBudgetV1,
    ) -> Result<BTreeSet<quanta_index_contract::SourceFileKey>, CoreError> {
        let content_leaf = self.predicate_content_leaf_from_constraint(constraint);
        let content_files = self.collect_matching_files_for_leaf(&content_leaf, budget)?;
        let Some(scope_files) =
            self.collect_matching_files_for_content_scope(constraint, budget)?
        else {
            return Ok(content_files);
        };
        Ok(content_files.intersection(&scope_files).cloned().collect())
    }

    /// The text documents a scoped content predicate admits, as authority
    /// doc ids.
    pub(crate) fn content_predicate_match_set(
        &self,
        constraint: &ContentPredicateConstraint,
        budget: &RequestBudgetV1,
    ) -> Result<RoaringBitmap, CoreError> {
        let allowed_files = self.allowed_files_for_content_predicate(constraint, budget)?;
        self.source_file_match_set(&allowed_files, budget)
    }

    pub(crate) fn repo_has_file_constraint(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<RepoFileConstraint, CoreError> {
        parse_repo_file_matchers(args).map_err(|err| match err {
            RepoFileArgError::UnsupportedArg => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` only supports one scalar path argument or path:/name:/lang:/content: filter arguments (owner: {PREDICATE_OWNER})"
            )),
            RepoFileArgError::NoMatcher => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` requires either one scalar path argument or at least one path:/name:/lang:/content: argument (owner: {PREDICATE_OWNER})"
            )),
            RepoFileArgError::EmptyLanguageValue => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` lang: argument cannot be empty (owner: {PREDICATE_OWNER})"
            )),
            RepoFileArgError::EmptyContentValue => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` content: argument cannot be empty (owner: {PREDICATE_OWNER})"
            )),
        })
    }
}
