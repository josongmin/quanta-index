//! Executable text-plane policy shared by the history and runtime-metadata
//! routes: filter/leaf admissibility and in-memory boolean text matching.

use quanta_index_contract::{LqCase, LqExpr, LqFilter, LqLeaf, LqOptions, LqQuery, LqType};
use quanta_index_core::CoreError;

use crate::query_dispatcher::timeref::{
    parse_runtime_changed_scope_ms, parse_runtime_stale_scope_ms,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ExecutableTextPlaneValidationState {
    saw_runtime_authority_filter: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ExecutableTextPlanePolicy {
    History,
    RuntimeMetadata,
}

impl ExecutableTextPlanePolicy {
    const fn plane_name(self) -> &'static str {
        match self {
            Self::History => "history",
            Self::RuntimeMetadata => "runtime metadata",
        }
    }

    fn validate_filter(
        self,
        filter: &LqFilter,
        state: &mut ExecutableTextPlaneValidationState,
    ) -> Result<(), CoreError> {
        match self {
            Self::History => match filter {
                LqFilter::Type { kind } => match kind {
                    LqType::Commit | LqType::Diff => Ok(()),
                    LqType::File | LqType::Path | LqType::Symbol | LqType::Repo => {
                        Err(CoreError::NotImplemented(format!(
                            "history: type filter `{}` is not executable on the current adapter set",
                            kind.as_str()
                        )))
                    }
                },
                LqFilter::File { .. }
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
                | LqFilter::Content { .. } => Ok(()),
                LqFilter::Repo { .. }
                | LqFilter::Lang { .. }
                | LqFilter::Select { .. }
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
                | LqFilter::Fork { .. }
                | LqFilter::Archived { .. }
                | LqFilter::Visibility { .. }
                | LqFilter::Context { .. } => Err(CoreError::NotImplemented(
                    "history: one or more filters are not executable on the current adapter set"
                        .to_string(),
                )),
            },
            Self::RuntimeMetadata => match filter {
                LqFilter::Changed { scope } => {
                    state.saw_runtime_authority_filter = true;
                    let _: u64 = parse_runtime_changed_scope_ms(scope)?;
                    Ok(())
                }
                LqFilter::Stale { scope } => {
                    state.saw_runtime_authority_filter = true;
                    let _: u64 = parse_runtime_stale_scope_ms(scope)?;
                    Ok(())
                }
                // `Dirty` carries a yes/no/only mode but, like the snapshot /
                // meta / edge authority filters, only needs to record that a
                // runtime-authority filter was seen at planning time.
                LqFilter::Dirty { .. }
                | LqFilter::Snapshot { .. }
                | LqFilter::MetaOwner { .. }
                | LqFilter::MetaService { .. }
                | LqFilter::MetaLayer { .. }
                | LqFilter::MetaSurface { .. }
                | LqFilter::Affected { .. }
                | LqFilter::InvalidatedBy { .. } => {
                    state.saw_runtime_authority_filter = true;
                    Ok(())
                }
                LqFilter::File { .. } | LqFilter::Lang { .. } | LqFilter::Content { .. } => {
                    Ok(())
                }
                LqFilter::Repo { .. }
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
                | LqFilter::Type { .. }
                | LqFilter::Select { .. }
                | LqFilter::Fork { .. }
                | LqFilter::Archived { .. }
                | LqFilter::Visibility { .. }
                | LqFilter::Context { .. } => Err(CoreError::NotImplemented(
                    "runtime metadata: one or more filters are not executable on the current adapter set"
                        .to_string(),
                )),
            },
        }
    }

    fn finalize(self, state: ExecutableTextPlaneValidationState) -> Result<(), CoreError> {
        match self {
            Self::History => Ok(()),
            Self::RuntimeMetadata => {
                if !state.saw_runtime_authority_filter {
                    return Err(CoreError::InvalidContract(
                        "runtime metadata: at least one runtime authority filter is required (dirty/changed/stale/snapshot/meta.*/affected/invalidated_by)"
                            .to_string(),
                    ));
                }
                Ok(())
            }
        }
    }
}

pub(super) fn validate_executable_text_query(
    query: &LqQuery,
    policy: ExecutableTextPlanePolicy,
) -> Result<(), CoreError> {
    if query.options.timeout_ms.is_some() {
        return Err(CoreError::InvalidContract(format!(
            "{}: timeout option is not executable on the current adapter set",
            policy.plane_name()
        )));
    }
    let mut state = ExecutableTextPlaneValidationState::default();
    for filter in &query.filters {
        policy.validate_filter(filter, &mut state)?;
    }
    policy.finalize(state)?;
    validate_executable_text_surface(&query.expr, policy)?;
    for filter in &query.filters {
        if let LqFilter::Content { leaf } = filter {
            validate_leaf_surface(leaf, policy)?;
        }
    }
    Ok(())
}

fn validate_executable_text_surface(
    expr: &LqExpr,
    policy: ExecutableTextPlanePolicy,
) -> Result<(), CoreError> {
    match expr {
        LqExpr::Empty => Ok(()),
        LqExpr::Leaf(leaf) => validate_leaf_surface(leaf, policy),
        LqExpr::Not(inner) => validate_executable_text_surface(inner, policy),
        LqExpr::All(children) | LqExpr::Any(children) => {
            for child in children {
                validate_executable_text_surface(child, policy)?;
            }
            Ok(())
        }
    }
}

fn validate_leaf_surface(
    leaf: &LqLeaf,
    policy: ExecutableTextPlanePolicy,
) -> Result<(), CoreError> {
    let plane = policy.plane_name();
    match leaf {
        LqLeaf::Keyword(_) | LqLeaf::Phrase(_) | LqLeaf::RawString(_) => Ok(()),
        LqLeaf::Regex(_) => Err(CoreError::NotImplemented(format!(
            "{plane}: regex leaves are not executable on the current adapter set"
        ))),
        LqLeaf::StructuralBlock(_) => Err(CoreError::NotImplemented(format!(
            "{plane}: structural leaves are not executable on this route"
        ))),
        LqLeaf::Predicate { .. } => Err(CoreError::NotImplemented(format!(
            "{plane}: predicate leaves are not executable on this route"
        ))),
    }
}

pub(super) fn expr_matches<F>(expr: &LqExpr, leaf_matches: &mut F) -> Result<bool, CoreError>
where
    F: FnMut(&LqLeaf) -> Result<bool, CoreError>,
{
    match expr {
        LqExpr::Empty => Ok(true),
        LqExpr::Leaf(leaf) => leaf_matches(leaf),
        LqExpr::Not(inner) => Ok(!expr_matches(inner, leaf_matches)?),
        LqExpr::All(children) => {
            for child in children {
                if !expr_matches(child, leaf_matches)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        LqExpr::Any(children) => {
            for child in children {
                if expr_matches(child, leaf_matches)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}

pub(super) fn leaf_matches_text(
    plane: &str,
    leaf: &LqLeaf,
    text: &str,
    options: &LqOptions,
) -> Result<bool, CoreError> {
    match leaf {
        LqLeaf::Keyword(value) | LqLeaf::Phrase(value) | LqLeaf::RawString(value) => {
            Ok(matches_text(value, text, options))
        }
        LqLeaf::Regex(_) => Err(CoreError::NotImplemented(format!(
            "{plane}: regex leaves are not executable on the current adapter set"
        ))),
        LqLeaf::StructuralBlock(_) => Err(CoreError::NotImplemented(format!(
            "{plane}: structural leaves are not executable on this route"
        ))),
        LqLeaf::Predicate { .. } => Err(CoreError::NotImplemented(format!(
            "{plane}: predicate leaves are not executable on this route"
        ))),
    }
}

pub(super) fn matches_text(needle: &str, haystack: &str, options: &LqOptions) -> bool {
    if matches!(options.case, Some(LqCase::Insensitive)) {
        haystack
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
    } else {
        haystack.contains(needle)
    }
}
