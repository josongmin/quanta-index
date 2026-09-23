//! Generation selection: resolve explicit pins and `GenerationSelector`s against
//! the activation catalog for lexical and semantic tracks.

use quanta_index_contract::{
    GenerationPin, GenerationSelector, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneTrackKind, TextQueryRequest,
};
use quanta_index_core::CoreError;

use crate::{ActivationCatalog, ActiveGenerationRecord};

#[derive(Clone, Debug)]
pub(super) struct SemanticSelection {
    pub(super) pin: GenerationPin,
    pub(super) expected_manifest_digest: Option<String>,
}

pub(super) fn selection_mismatch_error(
    left: (&GenerationPin, Option<&GenerationSelector>),
    right: (&GenerationPin, Option<&GenerationSelector>),
    message: String,
) -> CoreError {
    if same_generation_scope(left.0, right.0)
        && (matches!(left.1, Some(GenerationSelector::Active { .. }))
            || matches!(right.1, Some(GenerationSelector::Active { .. })))
    {
        CoreError::NotReady(message)
    } else {
        CoreError::InvalidContract(message)
    }
}

fn same_generation_scope(left: &GenerationPin, right: &GenerationPin) -> bool {
    left.repo_id == right.repo_id && left.revision_id == right.revision_id
}

/// Reject contradictory repository/revision declarations before consulting
/// activation state. No future activation can change a selector's identity.
pub(super) fn validate_generation_scope(
    pins: &[Option<&GenerationPin>],
    selectors: &[Option<&GenerationSelector>],
    plane: &str,
) -> Result<(), CoreError> {
    let mut scope: Option<(&RepoId, &RevisionId)> = None;
    for pin in pins.iter().flatten() {
        let candidate = (&pin.repo_id, &pin.revision_id);
        if scope.is_some_and(|current| current != candidate) {
            return Err(CoreError::InvalidContract(format!(
                "{plane}: generation declarations name different repositories or revisions"
            )));
        }
        scope = Some(candidate);
    }
    for selector in selectors.iter().flatten() {
        let candidate = match selector {
            GenerationSelector::Active {
                repo_id,
                revision_id,
            } => (repo_id, revision_id),
            GenerationSelector::Pinned(pin) => (&pin.repo_id, &pin.revision_id),
        };
        if scope.is_some_and(|current| current != candidate) {
            return Err(CoreError::InvalidContract(format!(
                "{plane}: generation declarations name different repositories or revisions"
            )));
        }
        scope = Some(candidate);
    }
    Ok(())
}

/// Classify each constraint that disagrees with an explicit pin. A fixed
/// disagreement cannot become valid by retrying, even if another constraint
/// happens to resolve through `Active`.
pub(super) fn explicit_pin_mismatch_error(
    explicit: &GenerationPin,
    constraints: &[(&GenerationPin, Option<&GenerationSelector>)],
    message: String,
) -> CoreError {
    let active_drift_only = constraints
        .iter()
        .filter(|(selected, _)| *selected != explicit)
        .all(|(selected, selector)| {
            same_generation_scope(explicit, selected)
                && matches!(selector, Some(GenerationSelector::Active { .. }))
        });
    if active_drift_only {
        CoreError::NotReady(message)
    } else {
        CoreError::InvalidContract(message)
    }
}

fn resolve_generation_selector_pin(
    activation_catalog: &ActivationCatalog,
    selector: &GenerationSelector,
    track: SearchPlaneTrackKind,
    plane: &str,
) -> Result<GenerationPin, CoreError> {
    match selector {
        GenerationSelector::Active {
            repo_id,
            revision_id,
        } => activation_catalog
            .resolve(repo_id, revision_id, track)
            .map_err(|err| match err {
                CoreError::NotReady(msg) => {
                    CoreError::NotReady(format!("{plane}: active generation unresolved: {msg}"))
                }
                err @ (CoreError::InvalidContract(_)
                | CoreError::Typed { .. }
                | CoreError::NotImplemented(_)
                | CoreError::NotFound(_)
                | CoreError::Storage(_)) => err,
            }),
        GenerationSelector::Pinned(pin) => Ok(pin.clone()),
    }
}

pub(super) fn resolve_optional_selection(
    activation_catalog: &ActivationCatalog,
    generation: Option<GenerationPin>,
    generation_selector: Option<&GenerationSelector>,
    track: SearchPlaneTrackKind,
    plane: &str,
) -> Result<Option<GenerationPin>, CoreError> {
    validate_generation_scope(&[generation.as_ref()], &[generation_selector], plane)?;
    let selector_pin = match generation_selector {
        Some(selector) => Some(resolve_generation_selector_pin(
            activation_catalog,
            selector,
            track,
            plane,
        )?),
        None => None,
    };
    match (generation, selector_pin) {
        (Some(pin), Some(selected)) if pin != selected => Err(selection_mismatch_error(
            (&pin, None),
            (&selected, generation_selector),
            format!(
                "{plane}: explicit generation pin does not match generation selector resolution"
            ),
        )),
        (Some(pin), _) | (None, Some(pin)) => Ok(Some(pin)),
        (None, None) => Ok(None),
    }
}

pub(super) fn resolve_semantic_selector_selection(
    activation_catalog: &ActivationCatalog,
    selector: &GenerationSelector,
    plane: &str,
) -> Result<SemanticSelection, CoreError> {
    match selector {
        GenerationSelector::Active {
            repo_id,
            revision_id,
        } => resolve_active_semantic_selection(activation_catalog, repo_id, revision_id, plane),
        GenerationSelector::Pinned(pin) => Ok(SemanticSelection {
            pin: pin.clone(),
            expected_manifest_digest: None,
        }),
    }
}

/// Resolve two active selectors from one composite catalog snapshot. A pair
/// activated between independent lexical and semantic lookups must not be
/// observable as one successful mixed selection.
pub(super) fn resolve_joint_active_selection(
    activation_catalog: &ActivationCatalog,
    lexical_selector: Option<&GenerationSelector>,
    semantic_selector: Option<&GenerationSelector>,
    lexical_generation: Option<&GenerationPin>,
    plane: &str,
) -> Result<Option<SemanticSelection>, CoreError> {
    validate_generation_scope(
        &[lexical_generation],
        &[lexical_selector, semantic_selector],
        plane,
    )?;
    let (
        Some(GenerationSelector::Active {
            repo_id: lexical_repo,
            revision_id: lexical_revision,
        }),
        Some(GenerationSelector::Active {
            repo_id: semantic_repo,
            revision_id: semantic_revision,
        }),
    ) = (lexical_selector, semantic_selector)
    else {
        return Ok(None);
    };
    if lexical_repo != semantic_repo || lexical_revision != semantic_revision {
        return Err(CoreError::InvalidContract(format!(
            "{plane}: lexical and semantic active selectors name different repositories or revisions"
        )));
    }
    let generation = activation_catalog
        .active_search_corpus_v1(lexical_repo, lexical_revision)?
        .ok_or_else(|| {
            CoreError::NotReady(format!(
                "{plane}: active composite generation unresolved for repo={} revision={}",
                lexical_repo.as_str(),
                lexical_revision.as_str()
            ))
        })?;
    let pin = GenerationPin::new(
        lexical_repo.clone(),
        lexical_revision.clone(),
        generation.manifest_generation(),
    );
    if let Some(explicit) = lexical_generation.filter(|explicit| *explicit != &pin) {
        let message = format!(
            "{plane}: explicit lexical generation pin does not match active composite resolution"
        );
        return Err(if same_generation_scope(explicit, &pin) {
            CoreError::NotReady(message)
        } else {
            CoreError::InvalidContract(message)
        });
    }
    Ok(Some(SemanticSelection {
        pin,
        expected_manifest_digest: Some(generation.manifest_digest().to_string()),
    }))
}

fn resolve_active_semantic_selection(
    activation_catalog: &ActivationCatalog,
    repo_id: &RepoId,
    revision_id: &RevisionId,
    plane: &str,
) -> Result<SemanticSelection, CoreError> {
    let record = activation_catalog
        .resolve_record(repo_id, revision_id, SearchPlaneTrackKind::Semantic)
        .map_err(|err| match err {
            CoreError::NotReady(msg) => {
                CoreError::NotReady(format!("{plane}: active generation unresolved: {msg}"))
            }
            err @ (CoreError::InvalidContract(_)
            | CoreError::Typed { .. }
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_)
            | CoreError::Storage(_)) => err,
        })?;
    Ok(selection_from_active_semantic_record(record))
}

fn selection_from_active_semantic_record(record: ActiveGenerationRecord) -> SemanticSelection {
    let pin = GenerationPin::new(
        record.repo_id.clone(),
        record.revision_id.clone(),
        record.manifest_generation,
    );
    SemanticSelection {
        pin,
        expected_manifest_digest: Some(record.manifest_digest),
    }
}

pub(super) fn resolve_lexical_request_pin(
    activation_catalog: &ActivationCatalog,
    request: &TextQueryRequest,
    track: SearchPlaneTrackKind,
    plane: &str,
) -> Result<GenerationPin, CoreError> {
    resolve_optional_selection(
        activation_catalog,
        request.generation.clone(),
        request.generation_selector.as_ref(),
        track,
        plane,
    )?
    .ok_or_else(|| CoreError::InvalidContract(format!("{plane}: generation pin required")))
}

/// Construct a [`GenerationPin`] from primitives. Public helper used in tests.
#[must_use]
pub fn make_pin(
    repo_id: RepoId,
    revision_id: RevisionId,
    manifest_generation: ManifestGeneration,
) -> GenerationPin {
    GenerationPin::new(repo_id, revision_id, manifest_generation)
}
