//! Preparing a query for execution: document kinds, projections and repo filters.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::{QueryDocKind, TantivySearcher};
use quanta_index_contract::{LqFilter, LqQuery, LqSelect, LqType, LqVisibility, LqYesNoOnly};
use quanta_index_core::{CoreError, timeref::is_rev_at_time_spec};

impl TantivySearcher {
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

    pub(crate) fn doc_kind_for_type(kind: LqType) -> Result<QueryDocKind, CoreError> {
        match kind {
            LqType::File | LqType::Path | LqType::Repo => Ok(QueryDocKind::Text),
            LqType::Symbol => Ok(QueryDocKind::Symbol),
            // The planner pre-flight surfaces this as typed
            // `HISTORY_PRODUCER_UNAVAILABLE` before `search()` reaches the
            // doc-kind routing path. The defensive arm here preserves the
            // same typed code for callers that bypass the planner (today
            // there are none on the live rail).
            LqType::Commit | LqType::Diff => Err(CoreError::Typed {
                code: crate::filters::codes::HISTORY_PRODUCER_UNAVAILABLE.to_string(),
                message: format!(
                    "lexical: type filter `{}` targets a surface with no producer on the lexical rail",
                    kind.as_str()
                ),
            }),
        }
    }

    pub(crate) fn doc_kind_for_select(dim: LqSelect) -> QueryDocKind {
        match dim {
            // `select:repo` collapses text hits to one representative row per
            // repo. The current lexical rail opens exactly one repo/revision
            // generation at a time, so execution still runs against text docs
            // and the projection collapse happens after recall.
            LqSelect::File
            | LqSelect::FileOwners
            | LqSelect::Path
            | LqSelect::Content
            | LqSelect::ContentMatch
            | LqSelect::Repo => QueryDocKind::Text,
            LqSelect::Symbol => QueryDocKind::Symbol,
        }
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

    pub(crate) fn merge_doc_kind(
        current: Option<QueryDocKind>,
        next: QueryDocKind,
        source: &str,
    ) -> Result<Option<QueryDocKind>, CoreError> {
        match current {
            Some(existing) if existing != next => Err(CoreError::InvalidContract(format!(
                "lexical: incompatible doc domain constraint from `{source}`"
            ))),
            Some(existing) => Ok(Some(existing)),
            None => Ok(Some(next)),
        }
    }

    pub(crate) fn prepare_query_for_doc_kind(
        &self,
        query: &LqQuery,
        default_doc_kind: QueryDocKind,
    ) -> Result<(LqQuery, QueryDocKind), CoreError> {
        let mut doc_kind: Option<QueryDocKind> = None;
        let mut filters: Vec<LqFilter> = Vec::with_capacity(query.filters.len());
        for filter in &query.filters {
            match filter {
                LqFilter::Type { kind } => {
                    let next = Self::doc_kind_for_type(*kind)?;
                    doc_kind = Self::merge_doc_kind(doc_kind, next, "type")?;
                }
                LqFilter::Select { dim } => {
                    let next = Self::doc_kind_for_select(*dim);
                    doc_kind = Self::merge_doc_kind(doc_kind, next, "select")?;
                }
                LqFilter::Rev { spec } => {
                    // Planner pre-flight surfaces this as
                    // `LEX_FILTER_REV_UNAVAILABLE` before reaching here on
                    // the live `search` path; this arm preserves the same
                    // typed code as a defense-in-depth for any future caller
                    // that bypasses the planner.
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::REV_UNAVAILABLE.to_string(),
                        message: if is_rev_at_time_spec(spec) {
                            "lexical: rev:at.time(...) requires revision-selection and pin rebinding before lexical execution".to_string()
                        } else {
                            "lexical: rev filter requires history producer".to_string()
                        },
                    });
                }
                LqFilter::Author { .. } => {
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::AUTHOR_UNAVAILABLE.to_string(),
                        message:
                            "lexical: author filter is not executable on the current adapter set"
                                .to_string(),
                    });
                }
                LqFilter::Committer { .. } => {
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::COMMITTER_UNAVAILABLE.to_string(),
                        message:
                            "lexical: committer filter is not executable on the current adapter set"
                                .to_string(),
                    });
                }
                LqFilter::Message { .. } => {
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::MESSAGE_UNAVAILABLE.to_string(),
                        message:
                            "lexical: message filter is not executable on the current adapter set"
                                .to_string(),
                    });
                }
                LqFilter::Dirty { .. } => {
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::DIRTY_UNAVAILABLE.to_string(),
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
                        code: crate::filters::codes::RUNTIME_CATALOG_UNAVAILABLE.to_string(),
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
                        code: crate::filters::codes::HISTORY_PRODUCER_UNAVAILABLE.to_string(),
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
        Ok((prepared, doc_kind.unwrap_or(default_doc_kind)))
    }
}
