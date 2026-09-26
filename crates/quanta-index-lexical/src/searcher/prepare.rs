//! Preparing a query for execution: document kinds, projections and repo filters.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::metadata_normalize::normalize_language;
use crate::{QueryDocKind, TantivySearcher};
use quanta_index_contract::{
    LqFileScope, LqFilter, LqQuery, LqSelect, LqType, LqVisibility, LqYesNoOnly,
};
use quanta_index_core::{
    CoreError, RequestBudgetV1, ValidatedLexicalPlan, timeref::is_rev_at_time_spec,
};
use quanta_index_lq_regex::RegexExecutor;

enum CoverageScopeFilter {
    Repo(RegexExecutor),
    File(RegexExecutor, LqFileScope),
    Language(String),
}

impl TantivySearcher {
    /// Admit the full source-file universe before any content/name/result
    /// predicate can remove rows. This consumes the same immutable read handle
    /// as execution and the same repo/path/language matching owners.
    pub(crate) fn validate_symbol_coverage_for_plan(
        &self,
        plan: &ValidatedLexicalPlan,
        budget: &RequestBudgetV1,
    ) -> Result<(), CoreError> {
        if !plan.uses_symbol_authority() {
            return Ok(());
        }
        let mut scope_filters = Vec::new();
        for filter in &plan.query().filters {
            budget.checkpoint("symbol coverage scope preparation")?;
            #[expect(
                clippy::wildcard_enum_match_arm,
                reason = "only explicit repo/path/language scope narrows capability; every other filter conservatively retains the full admitted universe"
            )]
            match filter {
                LqFilter::Repo { pattern, revs } => {
                    if !revs.is_empty() {
                        return Err(CoreError::Typed {
                            code: crate::filters::codes::REV_UNAVAILABLE,
                            message: "lexical: repo filter revisions require a history producer"
                                .into(),
                        });
                    }
                    scope_filters.push(CoverageScopeFilter::Repo(
                        self.manual_filter_regex(pattern, "repo")?,
                    ));
                }
                LqFilter::File { pattern, scope } => {
                    scope_filters.push(CoverageScopeFilter::File(
                        self.manual_filter_regex(pattern, "file")?,
                        *scope,
                    ));
                }
                LqFilter::Lang { id } => {
                    let language = normalize_language(id).ok_or_else(|| {
                        CoreError::InvalidContract(
                            "lexical: lang filter value cannot be empty".into(),
                        )
                    })?;
                    scope_filters.push(CoverageScopeFilter::Language(language));
                }
                // Only explicit request scope may narrow coverage. Result
                // predicates and projections do not establish source capability.
                _ => {}
            }
        }
        let constraints = plan.constraints();
        quanta_index_core::domains::lexical::require_complete_symbol_coverage(
            self.source_coverage.as_ref(),
            |entry| {
                let path = entry.source.file.repo_relative_path.as_str();
                if !Self::manual_exact_path_allows(path, constraints)
                    || (!constraints.language_any_of.is_empty()
                        && !constraints.language_any_of.contains(&entry.language))
                {
                    return Ok(false);
                }
                for filter in &scope_filters {
                    let matches = match filter {
                        CoverageScopeFilter::Repo(executor) => {
                            executor.verify(entry.source.file.source_repo_id.as_str().as_bytes())
                        }
                        CoverageScopeFilter::File(executor, scope) => {
                            Self::file_filter_scope_matches(executor, *scope, path)
                        }
                        CoverageScopeFilter::Language(language) => {
                            entry.language.as_str() == language
                        }
                    };
                    if !matches {
                        return Ok(false);
                    }
                }
                Ok(true)
            },
            budget,
        )
    }

    pub(crate) fn repo_filter_matches(&self, filter: &LqFilter) -> Result<Option<bool>, CoreError> {
        match filter {
            LqFilter::Fork { mode } => {
                if *mode == LqYesNoOnly::Yes {
                    return Ok(Some(true));
                }
                let Some(metadata) = self.repo_metadata.as_ref() else {
                    return Err(Self::missing_repo_metadata_error("fork"));
                };
                let matched = match mode {
                    LqYesNoOnly::Yes => true,
                    LqYesNoOnly::No => !metadata.fork,
                    LqYesNoOnly::Only => metadata.fork,
                };
                Ok(Some(matched))
            }
            LqFilter::Archived { mode } => {
                if *mode == LqYesNoOnly::Yes {
                    return Ok(Some(true));
                }
                let Some(metadata) = self.repo_metadata.as_ref() else {
                    return Err(Self::missing_repo_metadata_error("archived"));
                };
                let matched = match mode {
                    LqYesNoOnly::Yes => true,
                    LqYesNoOnly::No => !metadata.archived,
                    LqYesNoOnly::Only => metadata.archived,
                };
                Ok(Some(matched))
            }
            LqFilter::Visibility { mode } => {
                if *mode == LqVisibility::Any {
                    return Ok(Some(true));
                }
                let Some(metadata) = self.repo_metadata.as_ref() else {
                    return Err(Self::missing_repo_metadata_error("visibility"));
                };
                let matched = match mode {
                    LqVisibility::Any => true,
                    LqVisibility::Public => metadata.visibility == LqVisibility::Public,
                    LqVisibility::Private => metadata.visibility == LqVisibility::Private,
                };
                Ok(Some(matched))
            }
            LqFilter::Context { name } => {
                let Some(metadata) = self.repo_metadata.as_ref() else {
                    return Err(Self::missing_repo_metadata_error("context"));
                };
                let matched = metadata.contexts.iter().any(|context| context == name);
                Ok(Some(matched))
            }
            LqFilter::Repo { .. }
            | LqFilter::File { .. }
            | LqFilter::Lang { .. }
            | LqFilter::Rev { .. }
            | LqFilter::Author { .. }
            | LqFilter::Committer { .. }
            | LqFilter::Message { .. }
            | LqFilter::Before { .. }
            | LqFilter::After { .. }
            | LqFilter::Since { .. }
            | LqFilter::Until { .. }
            | LqFilter::DiffAdded { .. }
            | LqFilter::DiffRemoved { .. }
            | LqFilter::DiffTouched { .. }
            | LqFilter::Dirty { .. }
            | LqFilter::Changed { .. }
            | LqFilter::Stale { .. }
            | LqFilter::Snapshot { .. }
            | LqFilter::MetaOwner { .. }
            | LqFilter::MetaService { .. }
            | LqFilter::MetaLayer { .. }
            | LqFilter::MetaSurface { .. }
            | LqFilter::Affected { .. }
            | LqFilter::InvalidatedBy { .. }
            | LqFilter::Type { .. }
            | LqFilter::Select { .. }
            | LqFilter::Content { .. } => Ok(None),
        }
    }

    pub(crate) fn repo_filters_allow(&self, query: &LqQuery) -> Result<bool, CoreError> {
        for filter in &query.filters {
            if let Some(matched) = self.repo_filter_matches(filter)?
                && !matched
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn projects_repo_surface(query: &LqQuery) -> bool {
        query.filters.iter().any(|filter| {
            matches!(
                filter,
                LqFilter::Select {
                    dim: LqSelect::Repo
                } | LqFilter::Type { kind: LqType::Repo }
            )
        })
    }

    pub(crate) fn projects_path_surface(query: &LqQuery) -> bool {
        query.filters.iter().any(|filter| {
            matches!(
                filter,
                LqFilter::Select {
                    dim: LqSelect::Path
                } | LqFilter::Type { kind: LqType::Path }
            )
        })
    }

    pub(crate) fn selects_file_projection(query: &LqQuery) -> bool {
        query.filters.iter().any(|filter| {
            matches!(
                filter,
                LqFilter::Select {
                    dim: LqSelect::File
                }
            )
        })
    }

    pub(crate) fn selects_file_owner_projection(query: &LqQuery) -> bool {
        query.filters.iter().any(|filter| {
            matches!(
                filter,
                LqFilter::Select {
                    dim: LqSelect::FileOwners
                }
            )
        })
    }

    pub(crate) fn prepare_query_for_plan(
        &self,
        plan: &ValidatedLexicalPlan,
    ) -> Result<(LqQuery, QueryDocKind), CoreError> {
        let query = plan.query();
        let doc_kind = if plan.executes_symbol_domain() {
            QueryDocKind::Symbol
        } else {
            QueryDocKind::Text
        };
        let mut filters: Vec<LqFilter> = Vec::with_capacity(query.filters.len());
        for filter in &query.filters {
            match filter {
                LqFilter::Type { .. } | LqFilter::Select { .. } => {}
                LqFilter::Rev { spec } => {
                    // Planner pre-flight surfaces this as
                    // `LEX_FILTER_REV_UNAVAILABLE` before reaching here on
                    // the live `search` path; this arm preserves the same
                    // typed code as a defense-in-depth for any future caller
                    // that bypasses the planner.
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::REV_UNAVAILABLE,
                        message: if is_rev_at_time_spec(spec) {
                            "lexical: rev:at.time(...) requires revision-selection and pin rebinding before lexical execution".to_string()
                        } else {
                            "lexical: rev filter requires history producer".to_string()
                        },
                    });
                }
                LqFilter::Author { .. } => {
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::AUTHOR_UNAVAILABLE,
                        message:
                            "lexical: author filter is not executable on the current adapter set"
                                .to_string(),
                    });
                }
                LqFilter::Committer { .. } => {
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::COMMITTER_UNAVAILABLE,
                        message:
                            "lexical: committer filter is not executable on the current adapter set"
                                .to_string(),
                    });
                }
                LqFilter::Message { .. } => {
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::MESSAGE_UNAVAILABLE,
                        message:
                            "lexical: message filter is not executable on the current adapter set"
                                .to_string(),
                    });
                }
                LqFilter::Dirty { .. } => {
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::DIRTY_UNAVAILABLE,
                        message:
                            "lexical: dirty filter is not executable on the current adapter set"
                                .to_string(),
                    });
                }
                LqFilter::Changed { .. }
                | LqFilter::Stale { .. }
                | LqFilter::Snapshot { .. }
                | LqFilter::MetaOwner { .. }
                | LqFilter::MetaService { .. }
                | LqFilter::MetaLayer { .. }
                | LqFilter::MetaSurface { .. }
                | LqFilter::Affected { .. }
                | LqFilter::InvalidatedBy { .. } => {
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::RUNTIME_CATALOG_UNAVAILABLE,
                        message:
                            "lexical: runtime catalog filters are not executable on the current adapter set"
                                .to_string(),
                    });
                }
                LqFilter::Before { .. }
                | LqFilter::After { .. }
                | LqFilter::Since { .. }
                | LqFilter::Until { .. }
                | LqFilter::DiffAdded { .. }
                | LqFilter::DiffRemoved { .. }
                | LqFilter::DiffTouched { .. } => {
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::HISTORY_PRODUCER_UNAVAILABLE,
                        message: "lexical: history date/diff filters require history producer"
                            .to_string(),
                    });
                }
                other @ (LqFilter::Repo { .. }
                | LqFilter::File { .. }
                | LqFilter::Lang { .. }
                | LqFilter::Content { .. }
                | LqFilter::Fork { .. }
                | LqFilter::Archived { .. }
                | LqFilter::Visibility { .. }
                | LqFilter::Context { .. }) => filters.push(other.clone()),
            }
        }
        let mut prepared = query.clone();
        prepared.filters = filters;
        Ok((prepared, doc_kind))
    }
}
