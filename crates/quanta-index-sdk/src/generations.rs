use quanta_index_contract::{
    ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
    ipc::{
        CurrentGenerationRequest, GenerationSnapshot, GenerationStatusReport,
        GenerationStatusRequest, SearchPlaneActivateGenerationRequest, SearchPlaneActivationAck,
        SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse,
        SearchPlaneRollbackGenerationAck, SearchPlaneRollbackGenerationRequest,
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
            | SearchPlaneControlIpcResponse::RollbackAck(_)
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

    /// Apply an explicit semantic rollback guarded by the expected active
    /// generation and digest. This is separate from monotonic activation.
    pub fn rollback(
        &self,
        request: SearchPlaneRollbackGenerationRequest,
    ) -> Result<SearchPlaneRollbackGenerationAck, SdkError> {
        let response = self
            .client
            .dispatch_control(SearchPlaneControlIpcRequest::RollbackGeneration(request))?;
        match response {
            SearchPlaneControlIpcResponse::RollbackAck(ack) => Ok(ack),
            other @ (SearchPlaneControlIpcResponse::ActivationAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::Error(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)) => {
                Err(SdkError::Protocol(format!(
                    "expected rollback ack, got {}",
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
            | SearchPlaneControlIpcResponse::RollbackAck(_)
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
            | SearchPlaneControlIpcResponse::RollbackAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::Error(_)) => Err(SdkError::Protocol(format!(
                "expected generation status report, got {}",
                QuantaIndex::control_response_kind(&other)
            ))),
        }
    }
}

pub struct ActivationBuilder<
    'a,
    const HAS_REPO: bool = false,
    const HAS_REVISION: bool = false,
    const HAS_GENERATION: bool = false,
    const HAS_MANIFEST_DIGEST: bool = false,
    const HAS_TRACKS: bool = false,
> {
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
}

impl<
    'a,
    const HAS_REPO: bool,
    const HAS_REVISION: bool,
    const HAS_GENERATION: bool,
    const HAS_MANIFEST_DIGEST: bool,
    const HAS_TRACKS: bool,
> ActivationBuilder<'a, HAS_REPO, HAS_REVISION, HAS_GENERATION, HAS_MANIFEST_DIGEST, HAS_TRACKS>
{
    fn transition<
        const NEXT_REPO: bool,
        const NEXT_REVISION: bool,
        const NEXT_GENERATION: bool,
        const NEXT_MANIFEST_DIGEST: bool,
        const NEXT_TRACKS: bool,
    >(
        mut self,
        update: impl FnOnce(&mut Self),
    ) -> ActivationBuilder<
        'a,
        NEXT_REPO,
        NEXT_REVISION,
        NEXT_GENERATION,
        NEXT_MANIFEST_DIGEST,
        NEXT_TRACKS,
    > {
        update(&mut self);
        ActivationBuilder {
            client: self.client,
            repo_id: self.repo_id,
            revision_id: self.revision_id,
            generation: self.generation,
            manifest_digest: self.manifest_digest,
            tracks: self.tracks,
        }
    }

    #[must_use]
    pub fn repo(
        self,
        repo_id: RepoId,
    ) -> ActivationBuilder<'a, true, HAS_REVISION, HAS_GENERATION, HAS_MANIFEST_DIGEST, HAS_TRACKS>
    {
        self.transition(|builder| {
            builder.repo_id = Some(repo_id);
        })
    }

    #[must_use]
    pub fn revision(
        self,
        revision_id: RevisionId,
    ) -> ActivationBuilder<'a, HAS_REPO, true, HAS_GENERATION, HAS_MANIFEST_DIGEST, HAS_TRACKS>
    {
        self.transition(|builder| {
            builder.revision_id = Some(revision_id);
        })
    }

    #[must_use]
    pub fn generation(
        self,
        generation: ManifestGeneration,
    ) -> ActivationBuilder<'a, HAS_REPO, HAS_REVISION, true, HAS_MANIFEST_DIGEST, HAS_TRACKS> {
        self.transition(|builder| {
            builder.generation = Some(generation);
        })
    }

    #[must_use]
    pub fn manifest_digest(
        self,
        manifest_digest: impl Into<String>,
    ) -> ActivationBuilder<'a, HAS_REPO, HAS_REVISION, HAS_GENERATION, true, HAS_TRACKS> {
        self.transition(|builder| {
            builder.manifest_digest = Some(manifest_digest.into());
        })
    }

    #[must_use]
    pub fn track(
        self,
        track: SearchPlaneTrackKind,
    ) -> ActivationBuilder<'a, HAS_REPO, HAS_REVISION, HAS_GENERATION, HAS_MANIFEST_DIGEST, true>
    {
        self.transition(|builder| {
            if !builder.tracks.contains(&track) {
                builder.tracks.push(track);
            }
        })
    }

    pub fn tracks<I>(
        self,
        tracks: I,
    ) -> Result<
        ActivationBuilder<'a, HAS_REPO, HAS_REVISION, HAS_GENERATION, HAS_MANIFEST_DIGEST, true>,
        SdkError,
    >
    where
        I: IntoIterator<Item = SearchPlaneTrackKind>,
    {
        let next = self.transition(|builder| {
            for track in tracks {
                if !builder.tracks.contains(&track) {
                    builder.tracks.push(track);
                }
            }
        });
        if next.tracks.is_empty() {
            return Err(SdkError::Usage(
                "activation requires at least one track".to_string(),
            ));
        }
        Ok(next)
    }
}

impl ActivationBuilder<'_, true, true, true, true, true> {
    pub fn commit(self) -> Result<SearchPlaneActivationAck, SdkError> {
        let Some(repo_id) = self.repo_id else {
            return Err(SdkError::Protocol(
                "activation builder lost repo_id invariant".to_string(),
            ));
        };
        let Some(revision_id) = self.revision_id else {
            return Err(SdkError::Protocol(
                "activation builder lost revision_id invariant".to_string(),
            ));
        };
        let Some(manifest_generation) = self.generation else {
            return Err(SdkError::Protocol(
                "activation builder lost generation invariant".to_string(),
            ));
        };
        let Some(manifest_digest) = self.manifest_digest else {
            return Err(SdkError::Protocol(
                "activation builder lost manifest_digest invariant".to_string(),
            ));
        };
        if self.tracks.is_empty() {
            return Err(SdkError::Protocol(
                "activation builder lost tracks invariant".to_string(),
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
