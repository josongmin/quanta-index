//! Request-bound Native `RepoMap` publication and rank client.

use std::{collections::BTreeSet, mem::size_of, path::Path};

use quanta_index_contract::{
    RepoId, RepoMapNodeRef, RevisionId,
    index::{
        Digest, IndexInstanceId, IngressRef, OwnerMutation, ProducerCompleteness,
        ProjectionBinding, ProjectionCertificate, ProjectionId, ProjectionPayloadDigestErrorV3,
        ProjectionTransition, SchemaId, SourceCut, SourceEvent, UpdateKind, UpdateRequest, ViewRef,
        ViewSelection,
    },
    native_protocol::{
        NativeImportedRepoMapQueryV1, NativeImportedRepoMapResultV1, NativeIndexRequest,
        NativeIndexRequestEnvelope, NativeIndexResponse, NativeRepoMapQuery, NativeRepoMapResult,
        native_repomap_rank_policy,
    },
};
use quanta_index_ipc::{MemoryLease, MemoryLifetime, MemoryScope, RequestBudgetV1};

use super::{
    MUTATION_PAGE_SIZE, NativeUdsClient, NativeUdsError, allocate_request_id, selection_matches,
    staged_reference,
};

/// Exact Source cut for a ranked RepoMap advance.
/// The caller supplies the complete affected domain and retains the prepared
/// request before sending it.
pub struct NativeRepoMapAdvanceV1 {
    pub expected_view: ViewRef,
    pub target_source: SourceCut,
    pub projection: ProjectionId,
    pub schema: SchemaId,
    pub prior: Option<ProjectionBinding>,
    pub dependency_context: Digest,
    pub event_predecessor: u64,
}

/// The bounded advance request and its Original memory lease.
pub struct AdmittedNativeRepoMapAdvanceV1 {
    request: UpdateRequest,
    _memory: MemoryLease,
}

impl AdmittedNativeRepoMapAdvanceV1 {
    pub fn request(&self) -> &UpdateRequest {
        &self.request
    }
}

impl<P: AsRef<Path>> NativeUdsClient<P> {
    /// Stage the affected-owner certificate and construct the exact Native
    /// operation. Submission and acknowledgement recovery remain separately
    /// keyed to that operation.
    pub fn prepare_ranked_repomap_advance_v1(
        &self,
        ingress: IngressRef,
        advance: NativeRepoMapAdvanceV1,
        mutations: impl IntoIterator<Item = Result<OwnerMutation, NativeUdsError>>,
        budget: &RequestBudgetV1,
    ) -> Result<UpdateRequest, NativeUdsError> {
        self.prepare_ranked_repomap_advance_scoped_v1(
            ingress,
            advance,
            mutations,
            budget,
            None,
            &mut |_| Ok(()),
        )
    }

    /// The same preparation under the caller's Original memory grant.
    pub fn prepare_ranked_repomap_advance_admitted_v1(
        &self,
        ingress: IngressRef,
        advance: NativeRepoMapAdvanceV1,
        mutations: impl IntoIterator<Item = Result<OwnerMutation, NativeUdsError>>,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
        charge_work: &mut impl FnMut(usize) -> Result<(), NativeUdsError>,
    ) -> Result<AdmittedNativeRepoMapAdvanceV1, NativeUdsError> {
        // One bounded mutation page plus the fixed certificate, event and
        // returned request. The transport meters its encoded/decoded buffers.
        const SCRATCH_BYTES: u64 = (MUTATION_PAGE_SIZE * size_of::<OwnerMutation>()) as u64 + 4096;
        let memory_lease = memory
            .reserve(SCRATCH_BYTES, MemoryLifetime::Temporary)
            .map_err(|cause| NativeUdsError::Protocol(cause.to_string()))?;
        let request = self.prepare_ranked_repomap_advance_scoped_v1(
            ingress,
            advance,
            mutations,
            budget,
            Some(memory),
            charge_work,
        )?;
        Ok(AdmittedNativeRepoMapAdvanceV1 {
            request,
            _memory: memory_lease,
        })
    }

    fn prepare_ranked_repomap_advance_scoped_v1(
        &self,
        ingress: IngressRef,
        advance: NativeRepoMapAdvanceV1,
        mutations: impl IntoIterator<Item = Result<OwnerMutation, NativeUdsError>>,
        budget: &RequestBudgetV1,
        memory: Option<&MemoryScope>,
        charge_work: &mut impl FnMut(usize) -> Result<(), NativeUdsError>,
    ) -> Result<UpdateRequest, NativeUdsError> {
        let operation = ingress.operation;
        let policy = native_repomap_rank_policy();
        if operation.workspace != advance.expected_view.workspace
            || operation.epoch != advance.expected_view.epoch
            || operation.stream_epoch == 0
            || advance.prior.is_some_and(|prior| {
                prior.projection != advance.projection
                    || prior.producer != operation.producer
                    || prior.schema != advance.schema
                    || prior.producer_policy != policy
            })
        {
            return Err(NativeUdsError::Protocol(
                "Native RepoMap operation, owner, or projection binding differs".into(),
            ));
        }
        let sequence = advance.event_predecessor.checked_add(1).ok_or_else(|| {
            NativeUdsError::Protocol("Native RepoMap event sequence overflow".into())
        })?;

        let mutations = mutations.into_iter().map(|mutation| {
            charge_work(size_of::<OwnerMutation>())?;
            let mutation = mutation?;
            if mutation.owner.producer != operation.producer {
                return Err(NativeUdsError::Protocol(
                    "Native RepoMap mutation producer differs".into(),
                ));
            }
            Ok(mutation)
        });
        let affected = match memory {
            Some(scope) => self.stage_mutations_with_send(ingress, mutations, |request| {
                let reply = self.request_publication_admitted(request, budget, scope)?;
                match reply.response() {
                    NativeIndexResponse::MutationPageStaged(root) => {
                        Ok(NativeIndexResponse::MutationPageStaged(*root))
                    }
                    NativeIndexResponse::MutationsStaged(set) => {
                        Ok(NativeIndexResponse::MutationsStaged(*set))
                    }
                    NativeIndexResponse::Error(failure) => {
                        Err(NativeUdsError::Refused(failure.clone()))
                    }
                    _ => Err(NativeUdsError::Unknown(operation)),
                }
            })?,
            None => self.stage_mutations_fallible(ingress, mutations, budget)?,
        };
        let certificate = ProjectionCertificate {
            projection: advance.projection,
            producer: operation.producer,
            prior: advance.prior,
            target_source: advance.target_source,
            schema: advance.schema,
            producer_policy: policy,
            dependency_context: advance.dependency_context,
            affected,
        };
        let reference = match memory {
            Some(scope) => {
                let reply = self.request_publication_admitted(
                    NativeIndexRequest::StageProjectionCertificate {
                        ingress,
                        certificate,
                    },
                    budget,
                    scope,
                )?;
                match reply.response() {
                    NativeIndexResponse::Error(failure) => {
                        return Err(NativeUdsError::Refused(failure.clone()));
                    }
                    response => staged_reference(response, operation)?,
                }
            }
            None => self.stage_projection_certificate(ingress, certificate, budget)?,
        };
        let mut transition = ProjectionTransition {
            projection: advance.projection,
            producer: operation.producer,
            expected: advance.prior,
            target_source: advance.target_source,
            schema: advance.schema,
            producer_policy: policy,
            dependency_context: advance.dependency_context,
            completeness: ProducerCompleteness::CompleteAffectedDomain {
                certificate: reference,
            },
            owners: affected,
            events: Vec::new(),
        };
        let payload = transition
            .payload_digest_with_work_v3(charge_work)
            .map_err(|cause| match cause {
                ProjectionPayloadDigestErrorV3::Work(failure) => failure,
                cause => NativeUdsError::Protocol(cause.to_string()),
            })?;
        transition.events.push(SourceEvent {
            producer: operation.producer,
            stream: operation.stream,
            stream_epoch: operation.stream_epoch,
            predecessor: advance.event_predecessor,
            sequence,
            payload,
        });
        Ok(UpdateRequest {
            operation,
            expected_view: advance.expected_view,
            kind: UpdateKind::ProjectionAdvance(Box::new(transition)),
        })
    }

    /// Resolve the selected BOOT route and rank its exact imported RepoMap view.
    pub fn query_protected_repomap_rank_v1(
        &self,
        repo_id: RepoId,
        revision_id: RevisionId,
        query: NativeRepoMapQuery,
        budget: &RequestBudgetV1,
    ) -> Result<NativeRepoMapResult, NativeUdsError> {
        let resolved = self.resolve_protected_repomap_head_v1(repo_id, revision_id, budget)?;
        self.query_repomap(
            ViewSelection::Exact(resolved.root.view.identity),
            resolved.instance,
            query,
            budget,
        )
    }

    /// Resolve the selected BOOT route and query its exact protected graph.
    pub fn query_protected_imported_repomap_v1(
        &self,
        repo_id: RepoId,
        revision_id: RevisionId,
        query: NativeImportedRepoMapQueryV1,
        budget: &RequestBudgetV1,
    ) -> Result<NativeImportedRepoMapResultV1, NativeUdsError> {
        let resolved = self.resolve_protected_repomap_head_v1(repo_id, revision_id, budget)?;
        self.query_imported_repomap(
            ViewSelection::Exact(resolved.root.view.identity),
            resolved.instance,
            query,
            budget,
        )
    }

    /// Query one imported, exact Legacy compiler graph member in a pinned view.
    pub fn query_imported_repomap(
        &self,
        selection: ViewSelection,
        instance: IndexInstanceId,
        query: NativeImportedRepoMapQueryV1,
        budget: &RequestBudgetV1,
    ) -> Result<NativeImportedRepoMapResultV1, NativeUdsError> {
        if !query.valid_v1() {
            return Err(NativeUdsError::Protocol(
                "imported RepoMap query exceeds request bounds".into(),
            ));
        }
        let request_id = allocate_request_id(&self.next_request_id)?;
        let envelope = NativeIndexRequestEnvelope {
            request_id,
            payload: NativeIndexRequest::QueryImportedRepoMap {
                selection,
                instance,
                query,
            },
        };
        let NativeIndexResponse::ImportedRepoMap(result) =
            self.request_wire(request_id, &envelope, budget, Some(&envelope.payload))?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected imported RepoMap reply".into(),
            ));
        };
        let NativeIndexRequest::QueryImportedRepoMap { query, .. } = &envelope.payload else {
            return Err(NativeUdsError::Protocol(
                "imported RepoMap request context changed".into(),
            ));
        };
        if !selection_matches(selection, result.view)
            || result.instance != instance
            || result.source != query.source
            || result.direction != query.direction
            || usize::try_from(query.max_edges).map_or(true, |max| result.edges.len() > max)
            || result.next.is_some() == result.exhaustive
            || result.next.is_some_and(|next| {
                next.direction != query.direction
                    || result.edges.last().map(|edge| edge.cursor_start) != Some(next.start)
                    || query.after.is_some_and(|after| {
                        next.member != after.member
                            || next.payload_sha256 != after.payload_sha256
                            || next.source_digest != after.source_digest
                            || next.start <= after.start
                    })
            })
            || result
                .edges
                .windows(2)
                .any(|pair| pair[0].cursor_start >= pair[1].cursor_start)
            || result
                .node
                .as_ref()
                .is_some_and(|node| node.identity != query.source)
            || result.edges.iter().any(|edge| match query.direction {
                quanta_index_contract::native_protocol::NativeGraphDirection::Outgoing => {
                    edge.source != query.source
                }
                quanta_index_contract::native_protocol::NativeGraphDirection::Incoming => {
                    edge.target != query.source
                }
            })
        {
            return Err(NativeUdsError::Protocol(
                "imported RepoMap reply differs from selected request".into(),
            ));
        }
        Ok(result)
    }

    /// Return ranked `RepoMap` hits from one selected, certified Native view.
    pub fn query_repomap(
        &self,
        selection: ViewSelection,
        instance: IndexInstanceId,
        query: NativeRepoMapQuery,
        budget: &RequestBudgetV1,
    ) -> Result<NativeRepoMapResult, NativeUdsError> {
        if !query.valid_native_rank_query() {
            return Err(NativeUdsError::Protocol(
                "RepoMap rank query exceeds request bounds".into(),
            ));
        }
        let top_k = query.top_k;
        let request_id = allocate_request_id(&self.next_request_id)?;
        let envelope = NativeIndexRequestEnvelope {
            request_id,
            payload: NativeIndexRequest::QueryRepoMap {
                selection,
                instance,
                query,
            },
        };
        let NativeIndexResponse::RepoMapRanked(result) =
            self.request_wire(request_id, &envelope, budget, Some(&envelope.payload))?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected RepoMap rank reply".into(),
            ));
        };
        if !selection_matches(selection, result.view)
            || result.instance != instance
            || !result.exhaustive
            || usize::try_from(top_k).map_or(true, |limit| result.hits.len() > limit)
        {
            return Err(NativeUdsError::Protocol(
                "RepoMap rank reply differs from selected request".into(),
            ));
        }
        let mut identities = BTreeSet::new();
        for hit in &result.hits {
            let raw_identity = match &hit.subject {
                RepoMapNodeRef::File(id) => id.as_str(),
                RepoMapNodeRef::Symbol(id) => id.as_str(),
                RepoMapNodeRef::Module(_) | RepoMapNodeRef::Chunk(_) => {
                    return Err(NativeUdsError::Protocol(
                        "RepoMap rank reply contains an unsupported subject".into(),
                    ));
                }
            };
            if !identities.insert(raw_identity) {
                return Err(NativeUdsError::Protocol(
                    "RepoMap rank reply repeats a raw subject identity".into(),
                ));
            }
        }
        Ok(result)
    }
}
