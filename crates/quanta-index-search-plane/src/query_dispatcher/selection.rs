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
        (Some(pin), Some(selected)) if pin != selected => Err(CoreError::InvalidContract(format!(
            "{plane}: explicit generation pin does not match generation selector resolution"
        ))),
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
