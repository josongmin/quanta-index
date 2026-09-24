//! Generation selection: resolve explicit pins and `GenerationSelector`s against
//! the activation catalog for lexical and semantic tracks.

use quanta_index_contract::{
    GenerationPin, GenerationSelector, ManifestGeneration, RepoId, RevisionId,
    SearchCorpusActivationTokenV1, SearchPlaneTrackKind, TextQueryRequest,
};
use quanta_index_core::CoreError;

use crate::{ActivationCatalog, SearchCorpusGenerationV1};

#[derive(Clone, Debug)]
pub(super) struct SemanticSelection {
    pub(super) pin: GenerationPin,
    pub(super) expected_manifest_digest: Option<String>,
}

fn active_selector_parts(
    selector: &GenerationSelector,
) -> Option<(&RepoId, &RevisionId, Option<SearchCorpusActivationTokenV1>)> {
    match selector {
        GenerationSelector::Active {
            repo_id,
            revision_id,
        } => Some((repo_id, revision_id, None)),
        GenerationSelector::ResolvedActive {
            repo_id,
            revision_id,
            activation_token,
        } => Some((repo_id, revision_id, Some(*activation_token))),
        GenerationSelector::Pinned(_) => None,
    }
}

fn resolve_active_head(
    catalog: &ActivationCatalog,
    repo_id: &RepoId,
    revision_id: &RevisionId,
    expected_token: Option<SearchCorpusActivationTokenV1>,
    plane: &str,
) -> Result<SearchCorpusGenerationV1, CoreError> {
    let (generation, observed_token) = catalog
        .active_search_corpus_with_token_v1(repo_id, revision_id)?
        .ok_or_else(|| {
            CoreError::NotReady(format!(
                "{plane}: active composite generation unresolved for repo={} revision={}",
                repo_id.as_str(),
                revision_id.as_str()
            ))
        })?;
    if expected_token.is_some_and(|expected| expected != observed_token) {
        return Err(CoreError::NotReady(format!(
            "{plane}: active composite activation token changed for repo={} revision={}",
            repo_id.as_str(),
            revision_id.as_str()
        )));
    }
    Ok(generation)
}

fn is_active_selector(selector: Option<&GenerationSelector>) -> bool {
    selector.is_some_and(|selector| active_selector_parts(selector).is_some())
}

pub(super) fn selection_mismatch_error(
    left: (&GenerationPin, Option<&GenerationSelector>),
    right: (&GenerationPin, Option<&GenerationSelector>),
    message: String,
) -> CoreError {
    if same_generation_scope(left.0, right.0)
        && (is_active_selector(left.1) || is_active_selector(right.1))
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
            }
            | GenerationSelector::ResolvedActive {
                repo_id,
                revision_id,
                ..
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
            same_generation_scope(explicit, selected) && is_active_selector(*selector)
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
        GenerationSelector::Active { .. } | GenerationSelector::ResolvedActive { .. } => {
            let Some((repo_id, revision_id, expected_token)) = active_selector_parts(selector)
            else {
                return Err(CoreError::InvalidContract(format!(
                    "{plane}: active selector did not carry an active domain"
                )));
            };
            if track == SearchPlaneTrackKind::Structural {
                return Err(CoreError::NotReady(format!(
                    "{plane}: structural track has no composite active head"
                )));
            }
            let generation = resolve_active_head(
                activation_catalog,
                repo_id,
                revision_id,
                expected_token,
                plane,
            )?;
            Ok(GenerationPin::new(
                repo_id.clone(),
                revision_id.clone(),
                generation.manifest_generation(),
            ))
        }
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
        GenerationSelector::Active { .. } | GenerationSelector::ResolvedActive { .. } => {
            let Some((repo_id, revision_id, expected_token)) = active_selector_parts(selector)
            else {
                return Err(CoreError::InvalidContract(format!(
                    "{plane}: active selector did not carry an active domain"
                )));
            };
            resolve_active_semantic_selection(
                activation_catalog,
                repo_id,
                revision_id,
                expected_token,
                plane,
            )
        }
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
    let (Some(lexical_selector), Some(semantic_selector)) = (lexical_selector, semantic_selector)
    else {
        return Ok(None);
    };
    let (
        Some((lexical_repo, lexical_revision, lexical_token)),
        Some((semantic_repo, semantic_revision, semantic_token)),
    ) = (
        active_selector_parts(lexical_selector),
        active_selector_parts(semantic_selector),
    )
    else {
        return Ok(None);
    };
    if lexical_repo != semantic_repo || lexical_revision != semantic_revision {
        return Err(CoreError::InvalidContract(format!(
            "{plane}: lexical and semantic active selectors name different repositories or revisions"
        )));
    }
    if let (Some(lexical_token), Some(semantic_token)) = (lexical_token, semantic_token)
        && lexical_token != semantic_token
    {
        return Err(CoreError::NotReady(format!(
            "{plane}: lexical and semantic active resolutions name different activations"
        )));
    }
    let generation = resolve_active_head(
        activation_catalog,
        lexical_repo,
        lexical_revision,
        lexical_token.or(semantic_token),
        plane,
    )?;
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
    expected_token: Option<SearchCorpusActivationTokenV1>,
    plane: &str,
) -> Result<SemanticSelection, CoreError> {
    let generation = resolve_active_head(
        activation_catalog,
        repo_id,
        revision_id,
        expected_token,
        plane,
    )?;
    let pin = GenerationPin::new(
        repo_id.clone(),
        revision_id.clone(),
        generation.manifest_generation(),
    );
    Ok(SemanticSelection {
        pin,
        expected_manifest_digest: Some(generation.manifest_digest().to_string()),
    })
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
