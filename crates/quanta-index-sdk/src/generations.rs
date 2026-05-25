use quanta_index_contract::{
    ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
    ipc::{
        CurrentGenerationRequest, GenerationSnapshot, GenerationStatusReport,
        GenerationStatusRequest, SearchPlaneActivateGenerationRequest, SearchPlaneActivationAck,
        SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse,
    },
};

use crate::{QuantaIndex, SdkError};

pub struct GenerationNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> GenerationNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    #[must_use]
    pub fn activate(&self) -> ActivationBuilder<'a> {
        ActivationBuilder::new(self.client)
    }

    pub fn commit(
        &self,
        request: SearchPlaneActivateGenerationRequest,
    ) -> Result<SearchPlaneActivationAck, SdkError> {
        let response = self
            .client
            .dispatch_control(SearchPlaneControlIpcRequest::ActivateGeneration(request))?;
        match response {
            SearchPlaneControlIpcResponse::ActivationAck(ack) => Ok(ack),
            other @ (SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::Error(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)) => {
                Err(SdkError::Protocol(format!(
                    "expected activation ack, got {}",
                    QuantaIndex::control_response_kind(&other)
                )))
            }
        }
    }

    /// QI-ACT-01: look up the active generation for one
    /// `(repo, revision, track)` triple. Fails with [`SdkError::Remote`]
    /// when no entry exists (typed code `NOT_READY`); the activation
    /// catalog is the single source of truth — no client-side default.
    pub fn current(
        &self,
        repo_id: RepoId,
        revision_id: RevisionId,
        track: SearchPlaneTrackKind,
    ) -> Result<GenerationSnapshot, SdkError> {
        let response =
            self.client
                .dispatch_control(SearchPlaneControlIpcRequest::CurrentGeneration(
                    CurrentGenerationRequest {
                        repo_id,
                        revision_id,
                        track,
                    },
                ))?;
        match response {
            SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(snapshot) => Ok(snapshot),
            other @ (SearchPlaneControlIpcResponse::ActivationAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::Error(_)) => Err(SdkError::Protocol(format!(
                "expected current generation snapshot, got {}",
                QuantaIndex::control_response_kind(&other)
            ))),
        }
    }

    /// QI-ACT-01: return every active track for one `(repo, revision)`
    /// pair. Empty `tracks` vec means nothing is activated yet — distinct
    /// from a per-track `NOT_READY` from [`Self::current`].
    pub fn status(
        &self,
        repo_id: RepoId,
        revision_id: RevisionId,
    ) -> Result<GenerationStatusReport, SdkError> {
        let response =
            self.client
                .dispatch_control(SearchPlaneControlIpcRequest::GenerationStatus(
                    GenerationStatusRequest {
                        repo_id,
                        revision_id,
                    },
                ))?;
        match response {
            SearchPlaneControlIpcResponse::GenerationStatusReport(report) => Ok(report),
            other @ (SearchPlaneControlIpcResponse::ActivationAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::Error(_)) => Err(SdkError::Protocol(format!(
                "expected generation status report, got {}",
                QuantaIndex::control_response_kind(&other)
            ))),
        }
    }
}

pub struct ActivationBuilder<'a> {
    client: &'a QuantaIndex,
    repo_id: Option<RepoId>,
    revision_id: Option<RevisionId>,
    generation: Option<ManifestGeneration>,
    manifest_digest: Option<String>,
    tracks: Vec<SearchPlaneTrackKind>,
}

impl<'a> ActivationBuilder<'a> {
    const fn new(client: &'a QuantaIndex) -> Self {
        Self {
            client,
            repo_id: None,
            revision_id: None,
            generation: None,
            manifest_digest: None,
            tracks: Vec::new(),
        }
    }

    #[must_use]
    pub fn repo(mut self, repo_id: RepoId) -> Self {
        self.repo_id = Some(repo_id);
        self
    }

    #[must_use]
    pub fn revision(mut self, revision_id: RevisionId) -> Self {
        self.revision_id = Some(revision_id);
        self
    }

    #[must_use]
    pub fn generation(mut self, generation: ManifestGeneration) -> Self {
        self.generation = Some(generation);
        self
    }

    #[must_use]
    pub fn manifest_digest(mut self, manifest_digest: impl Into<String>) -> Self {
        self.manifest_digest = Some(manifest_digest.into());
        self
    }

    #[must_use]
    pub fn track(mut self, track: SearchPlaneTrackKind) -> Self {
        if !self.tracks.contains(&track) {
            self.tracks.push(track);
        }
        self
    }

    #[must_use]
    pub fn tracks<I>(mut self, tracks: I) -> Self
    where
        I: IntoIterator<Item = SearchPlaneTrackKind>,
    {
        for track in tracks {
            if !self.tracks.contains(&track) {
                self.tracks.push(track);
            }
        }
        self
    }

    pub fn commit(self) -> Result<SearchPlaneActivationAck, SdkError> {
        let repo_id = self
            .repo_id
            .ok_or_else(|| SdkError::Usage("activation repo_id is required".to_string()))?;
        let revision_id = self
            .revision_id
            .ok_or_else(|| SdkError::Usage("activation revision_id is required".to_string()))?;
        let manifest_generation = self
            .generation
            .ok_or_else(|| SdkError::Usage("activation generation is required".to_string()))?;
        let manifest_digest = self
            .manifest_digest
            .ok_or_else(|| SdkError::Usage("activation manifest_digest is required".to_string()))?;
        if self.tracks.is_empty() {
            return Err(SdkError::Usage(
                "activation requires at least one track".to_string(),
            ));
        }
        self.client
            .generations()
            .commit(SearchPlaneActivateGenerationRequest {
                repo_id,
                revision_id,
                manifest_generation,
                manifest_digest,
                tracks: self.tracks,
            })
    }
}
