//! The request, the view, its acquisition and the trace attachment: the
//! declared dependency vector, acquired once between the logical plan
//! and execution.
//!
//! A route lowers its request, resolves its pin, declares the domains the
//! plan reads ([`declare_required_domains_v1`]) and asks for a view. The
//! view pins exactly the declared set — the lexical and semantic handles
//! from the snapshot registries, the history / runtime / structural
//! snapshots at the epochs a continuation names (or current), the history
//! epoch's text index when the order scores — and nothing else: an
//! undeclared domain is never opened, and a route that reaches for one is
//! refused typed rather than served late.
//!
//! Every ledger read of a request happens under one read guard here:
//! the serving boundary of each pinned track (the durable authority must
//! retain the exact sealed generation, else `UNKNOWN_GENERATION` /
//! `NOT_READY`, QI-BB-003), lexical readiness, semantic validation, the
//! auxiliary snapshots and the text index handle (acquired under the same
//! guard so a prune cannot race the open). The registries are consulted
//! after the guard is released, under the request's budget; they are
//! keyed by the immutable sealed generation. A required domain that is
//! absent or not ready is refused with that domain's typed code before
//! any lane executes — never an empty page, never another generation.
//! Every auxiliary snapshot the view holds is proven to belong to the
//! pin's generation; a mix is refused `READ_VIEW_GENERATION_MIX`.
//!
//! Different domains are supplied by different producers at different
//! times; the view never claims they are one instant. What it fixes is
//! one pin, one epoch per auxiliary domain, and the artifacts and
//! capability versions the response can name (`ReadIdentityV2`).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use quanta_index_contract::{
    AuxEpochV1, GenerationPin, LqQuery, PlannerStage, PlannerTraceEntry, SearchExplanation,
    SearchPlaneTrackKind,
};
use quanta_index_core::{
    AuxiliaryGenerationKeyV1, CoreError, DomainReadEvidenceV2, HistoryTextSearcher,
    LexicalSearcher, PinnedRepoMapSnapshot, QueryRouteV1, ReadDomainV1, ReadIdentityV2,
    ReadResourceGroupV2, ReadViewRefusedError, RepoMapSnapshotAcquireV1, RequestBudgetV1,
    RequiredDomainsV1, SemanticProfileV1, SemanticSearcher, StructuralError,
    declare_required_domains_v1,
};

use crate::Ledger;
use crate::history_text::HistoryTextClaim;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::errors::{history_absent_error, history_relevance_unavailable};
use crate::readiness::{
    AuxRead, HistoryAuthorityState, RuntimeMetadataState, StructuralAuthorityState,
};

/// The epochs a continuation names, one per auxiliary domain; `None`
/// reads the current snapshot.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct AuxEpochPinsV1 {
    pub(crate) history: Option<AuxEpochV1>,
    pub(crate) runtime: Option<AuxEpochV1>,
    pub(crate) structural: Option<AuxEpochV1>,
}

/// What a route asks the view to pin.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ReadViewRequestV1<'a> {
    /// The route name for messages (`lexical`, `hybrid seed`, ...).
    pub(crate) plane: &'a str,
    pub(crate) pin: &'a GenerationPin,
    pub(crate) domains: RequiredDomainsV1,
    /// The manifest digest an `Active` semantic selector resolved, which
    /// the sealed semantic generation must carry.
    pub(crate) semantic_manifest_digest: Option<&'a str>,
    pub(crate) epochs: AuxEpochPinsV1,
    /// Whether the history domain must include its epoch's text index
    /// (the `relevance` order).
    pub(crate) history_text: bool,
}

impl<'a> ReadViewRequestV1<'a> {
    /// A request for `domains` at `pin`, reading current auxiliary
    /// snapshots.
    pub(crate) const fn new(
        plane: &'a str,
        pin: &'a GenerationPin,
        domains: RequiredDomainsV1,
    ) -> Self {
        Self {
            plane,
            pin,
            domains,
            semantic_manifest_digest: None,
            epochs: AuxEpochPinsV1 {
                history: None,
                runtime: None,
                structural: None,
            },
            history_text: false,
        }
    }

    /// A request for the domains `route` declares over `plan`, at `pin`.
    pub(crate) fn declare(
        plane: &'a str,
        route: QueryRouteV1,
        plan: Option<&LqQuery>,
        pin: &'a GenerationPin,
    ) -> Self {
        Self::new(plane, pin, declare_required_domains_v1(route, plan))
    }

    /// A request that pins one domain at `pin` for a plan-time selection
    /// (`rev:at.time(...)` walking the history authority of the requested
    /// generation) rather than for execution.
    pub(crate) const fn selection(
        plane: &'a str,
        domain: ReadDomainV1,
        pin: &'a GenerationPin,
    ) -> Self {
        Self::new(plane, pin, RequiredDomainsV1::of(domain))
    }

    pub(crate) const fn with_semantic_manifest_digest(mut self, digest: Option<&'a str>) -> Self {
        self.semantic_manifest_digest = digest;
        self
    }

    pub(crate) const fn with_epochs(mut self, epochs: AuxEpochPinsV1) -> Self {
        self.epochs = epochs;
        self
    }

    pub(crate) const fn with_history_text(mut self, history_text: bool) -> Self {
        self.history_text = history_text;
        self
    }
}

/// What the ledger supplied for the declared domains, read under one
/// guard.
struct LedgerParts {
    lexical_manifest_digest: Option<String>,
    semantic_manifest_digest: Option<String>,
    history: Option<AuxRead<HistoryAuthorityState>>,
    /// The epoch's text index, claimed under the guard and landed after
    /// it (QI-BB-020): no open runs under the ledger lock.
    history_text: Option<HistoryTextClaim>,
    runtime: Option<AuxRead<RuntimeMetadataState>>,
    structural: Option<AuxRead<StructuralAuthorityState>>,
}

/// The dependency vector one request executes against.
///
/// Holds a handle or snapshot for exactly the declared domains; the
/// accessors refuse typed for any other, so a route cannot widen its
/// declaration by reaching past it.
pub(crate) struct QueryReadViewV2 {
    identity: ReadIdentityV2,
    lexical: Option<Arc<dyn LexicalSearcher>>,
    semantic: Option<Arc<dyn SemanticSearcher>>,
    history: Option<AuxRead<HistoryAuthorityState>>,
    history_text: Option<Arc<dyn HistoryTextSearcher>>,
    runtime: Option<AuxRead<RuntimeMetadataState>>,
    structural: Option<AuxRead<StructuralAuthorityState>>,
    repo_map: Option<Box<dyn PinnedRepoMapSnapshot>>,
}

impl QueryReadViewV2 {
    pub(crate) const fn identity(&self) -> &ReadIdentityV2 {
        &self.identity
    }

    pub(crate) const fn domains(&self) -> RequiredDomainsV1 {
        self.identity.domains
    }

    fn undeclared(&self, domain: ReadDomainV1) -> CoreError {
        ReadViewRefusedError::DomainUndeclared {
            domain,
            pin: self.identity.pin.clone(),
        }
        .into()
    }

    /// The lexical handle of the pinned generation.
    pub(crate) fn lexical(&self) -> Result<&Arc<dyn LexicalSearcher>, CoreError> {
        self.lexical
            .as_ref()
            .ok_or_else(|| self.undeclared(ReadDomainV1::LexicalTrack))
    }

    /// The semantic handle of the pinned generation.
    pub(crate) fn semantic(&self) -> Result<&Arc<dyn SemanticSearcher>, CoreError> {
        self.semantic
            .as_ref()
            .ok_or_else(|| self.undeclared(ReadDomainV1::SemanticTrack))
    }

    /// The sealed manifest digest of the pinned semantic generation.
    pub(crate) fn semantic_manifest_digest(&self) -> Result<&str, CoreError> {
        self.identity
            .semantic_artifact
            .as_deref()
            .ok_or_else(|| self.undeclared(ReadDomainV1::SemanticTrack))
    }

    /// The history snapshot at the pinned epoch.
    pub(crate) fn history(&self) -> Result<&AuxRead<HistoryAuthorityState>, CoreError> {
        self.history
            .as_ref()
            .ok_or_else(|| self.undeclared(ReadDomainV1::History))
    }

    /// The history text index of the pinned epoch, when the request asked
    /// for it.
    pub(crate) fn history_text(&self) -> Result<&Arc<dyn HistoryTextSearcher>, CoreError> {
        self.history_text
            .as_ref()
            .ok_or_else(|| self.undeclared(ReadDomainV1::History))
    }

    /// The runtime-metadata snapshot at the pinned epoch.
    pub(crate) fn runtime(&self) -> Result<&AuxRead<RuntimeMetadataState>, CoreError> {
        self.runtime
            .as_ref()
            .ok_or_else(|| self.undeclared(ReadDomainV1::RuntimeOverlay))
    }

    /// The pinned `RepoMap` snapshot of the declared `RepoMap` domain:
    /// the only way a route reaches the `RepoMap` store. Executing the
    /// query cannot re-enter any store, registry or ledger — the handle
    /// was acquired once, with the view.
    pub(crate) fn repo_map(&self) -> Result<&dyn PinnedRepoMapSnapshot, CoreError> {
        self.repo_map
            .as_deref()
            .ok_or_else(|| self.undeclared(ReadDomainV1::RepoMap))
    }

    /// The structural snapshot at the pinned epoch.
    pub(crate) fn structural(&self) -> Result<&AuxRead<StructuralAuthorityState>, CoreError> {
        self.structural
            .as_ref()
            .ok_or_else(|| self.undeclared(ReadDomainV1::StructuralChunkUniverse))
    }

    /// Assemble the view from its parts, proving every auxiliary snapshot
    /// belongs to the pin and every declared repo-metadata authority is
    /// held by the lexical handle.
    fn assemble(
        request: &ReadViewRequestV1<'_>,
        parts: LedgerParts,
        history_text: Option<Arc<dyn HistoryTextSearcher>>,
        lexical: Option<Arc<dyn LexicalSearcher>>,
        semantic: Option<Arc<dyn SemanticSearcher>>,
        repo_map: Option<Box<dyn PinnedRepoMapSnapshot>>,
    ) -> Result<Self, CoreError> {
        let pin = request.pin.clone();
        let mut aux_epochs = BTreeMap::new();
        if let Some(read) = &parts.history {
            ensure_same_generation(&pin, ReadDomainV1::History, &read.generation)?;
            let _previous = aux_epochs.insert(ReadDomainV1::History, read.epoch);
        }
        if let Some(read) = &parts.runtime {
            ensure_same_generation(&pin, ReadDomainV1::RuntimeOverlay, &read.generation)?;
            let _previous = aux_epochs.insert(ReadDomainV1::RuntimeOverlay, read.epoch);
        }
        if let Some(read) = &parts.structural {
            ensure_same_generation(
                &pin,
                ReadDomainV1::StructuralChunkUniverse,
                &read.generation,
            )?;
            let _previous = aux_epochs.insert(ReadDomainV1::StructuralChunkUniverse, read.epoch);
        }
        let lexical_identity = lexical.as_ref().map(|handle| handle.artifact_identity());
        if let Some(identity) = &lexical_identity {
            let expected = parts.lexical_manifest_digest.as_deref().ok_or_else(|| {
                CoreError::InvalidContract(
                    "lexical read view acquired a handle without ledger identity".into(),
                )
            })?;
            if identity.manifest_digest != expected {
                return Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch,
                    message: format!(
                        "{}: opened lexical manifest digest mismatch for repo={} revision={} generation={}: expected={}, observed={}",
                        request.plane,
                        pin.repo_id.as_str(),
                        pin.revision_id.as_str(),
                        pin.manifest_generation.get(),
                        expected,
                        identity.manifest_digest,
                    ),
                });
            }
            for authority in request.domains.repo_metadata() {
                if !identity.repo_metadata.contains(authority) {
                    return Err(
                        ReadViewRefusedError::RepoMetadataUnavailable { authority, pin }.into(),
                    );
                }
            }
        }
        if let Some(handle) = semantic.as_ref() {
            let expected = parts.semantic_manifest_digest.as_deref().ok_or_else(|| {
                CoreError::InvalidContract(
                    "semantic read view acquired a handle without ledger identity".into(),
                )
            })?;
            if handle.manifest_digest() != expected {
                return Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::SemanticManifestDigestMismatch,
                    message: format!(
                        "{}: opened semantic manifest digest mismatch for repo={} revision={} generation={}: expected={}, observed={}",
                        request.plane,
                        pin.repo_id.as_str(),
                        pin.revision_id.as_str(),
                        pin.manifest_generation.get(),
                        expected,
                        handle.manifest_digest(),
                    ),
                });
            }
        }
        let profile = semantic.as_ref().map(|handle| SemanticProfileV1 {
            model_id: handle.index_model_id().to_string(),
            model_revision: handle.index_model_revision().map(str::to_string),
        });
        let repomap_evidence = repo_map.as_ref().map(|handle| handle.evidence().clone());
        let mut evidence = BTreeMap::new();
        for domain in request.domains.iter() {
            let entry = match domain {
                ReadDomainV1::LexicalTrack | ReadDomainV1::RepoMetadata(_) => {
                    DomainReadEvidenceV2 {
                        domain,
                        resource_group: ReadResourceGroupV2::LexicalTrack,
                        artifact: lexical_identity
                            .as_ref()
                            .map(|identity| identity.manifest_digest.clone()),
                        activation_epoch: None,
                        aux_epoch: None,
                    }
                }
                ReadDomainV1::SemanticTrack => DomainReadEvidenceV2 {
                    domain,
                    resource_group: ReadResourceGroupV2::SemanticTrack,
                    artifact: parts.semantic_manifest_digest.clone(),
                    activation_epoch: None,
                    aux_epoch: None,
                },
                ReadDomainV1::History => DomainReadEvidenceV2 {
                    domain,
                    resource_group: ReadResourceGroupV2::HistoryEpoch,
                    artifact: None,
                    activation_epoch: None,
                    aux_epoch: aux_epochs.get(&domain).copied(),
                },
                ReadDomainV1::RuntimeOverlay => DomainReadEvidenceV2 {
                    domain,
                    resource_group: ReadResourceGroupV2::RuntimeEpoch,
                    artifact: None,
                    activation_epoch: None,
                    aux_epoch: aux_epochs.get(&domain).copied(),
                },
                ReadDomainV1::StructuralChunkUniverse => DomainReadEvidenceV2 {
                    domain,
                    resource_group: ReadResourceGroupV2::StructuralEpoch,
                    artifact: None,
                    activation_epoch: None,
                    aux_epoch: aux_epochs.get(&domain).copied(),
                },
                ReadDomainV1::RepoMap => DomainReadEvidenceV2 {
                    domain,
                    resource_group: ReadResourceGroupV2::RepoMapSnapshot,
                    artifact: repomap_evidence
                        .as_ref()
                        .map(|evidence| evidence.candidate_commitment.clone()),
                    activation_epoch: repomap_evidence
                        .as_ref()
                        .map(|evidence| evidence.activation_epoch),
                    aux_epoch: None,
                },
            };
            let _prior = evidence.insert(domain, entry);
        }
        let identity = ReadIdentityV2 {
            pin,
            domains: request.domains,
            lexical_artifact: lexical_identity
                .as_ref()
                .map(|identity| identity.manifest_digest.clone()),
            semantic_artifact: parts.semantic_manifest_digest,
            aux_epochs,
            normalizer_version: lexical_identity
                .as_ref()
                .map(|identity| identity.normalizer),
            profile,
            repomap_commitment: repomap_evidence
                .as_ref()
                .map(|evidence| evidence.candidate_commitment.clone()),
            repomap_activation_epoch: repomap_evidence
                .as_ref()
                .map(|evidence| evidence.activation_epoch),
            evidence,
        };
        Ok(Self {
            identity,
            lexical,
            semantic,
            history: parts.history,
            history_text,
            runtime: parts.runtime,
            structural: parts.structural,
            repo_map,
        })
    }
}

/// Put the view's identity at the head of a response's planner trace.
///
/// The entries are plan-stage (`read_view.domains=…`, `read_view.epochs=…`,
/// the pin and the pinned artifacts): the dependency vector the plan ran
/// against comes before what the plan did with it.
pub(crate) fn attach_read_view_trace(
    explanation: &mut SearchExplanation,
    identity: &ReadIdentityV2,
) {
    let entries: Vec<PlannerTraceEntry> = identity
        .trace_details()
        .into_iter()
        .map(|detail| PlannerTraceEntry {
            stage: PlannerStage::Plan,
            detail,
        })
        .collect();
    let _replaced: Vec<PlannerTraceEntry> =
        explanation.planner_trace.splice(0..0, entries).collect();
}

fn ensure_same_generation(
    pin: &GenerationPin,
    domain: ReadDomainV1,
    offered: &AuxiliaryGenerationKeyV1,
) -> Result<(), CoreError> {
    let pinned = AuxiliaryGenerationKeyV1 {
        repo_id: pin.repo_id.clone(),
        revision_id: pin.revision_id.clone(),
        generation: pin.manifest_generation,
    };
    if *offered == pinned {
        return Ok(());
    }
    Err(ReadViewRefusedError::GenerationMix {
        domain,
        pin: pin.clone(),
        offered: offered.clone(),
    }
    .into())
}

impl SearchPlaneDispatcher {
    /// Acquire the view one request executes against: exactly the
    /// declared domains at the pin, each refused typed when absent. The
    /// track handles are acquired under `budget`: a wait on another
    /// request's cold open ends with this request's own interruption.
    pub(crate) fn acquire_read_view(
        &self,
        request: &ReadViewRequestV1<'_>,
        budget: &RequestBudgetV1,
    ) -> Result<QueryReadViewV2, CoreError> {
        let mut parts = {
            let guard = self
                .ledger
                .read()
                .map_err(|err| CoreError::Storage(format!("ledger poisoned: {err}")))?;
            self.ledger_parts(&guard, request, Instant::now())?
        };
        let history_text = parts
            .history_text
            .take()
            .map(|claim| self.land_history_text(claim, budget))
            .transpose()?;
        let pin = request.pin;
        let lexical = if request.domains.contains(ReadDomainV1::LexicalTrack) {
            Some(self.acquire_lexical(
                &pin.repo_id,
                &pin.revision_id,
                pin.manifest_generation,
                budget,
            )?)
        } else {
            None
        };
        let semantic = if request.domains.contains(ReadDomainV1::SemanticTrack) {
            Some(self.acquire_semantic(
                &pin.repo_id,
                &pin.revision_id,
                pin.manifest_generation,
                budget,
            )?)
        } else {
            None
        };
        // The RepoMap handle is acquired after the ledger guard is
        // released: the store's own critical section resolves the active
        // identity, commitment, epoch and artifact together, and no
        // ledger or catalog guard is ever held across it.
        let repo_map = if request.domains.contains(ReadDomainV1::RepoMap) {
            Some(self.repo_map_snapshots.acquire(RepoMapSnapshotAcquireV1 {
                repo_id: pin.repo_id.clone(),
                revision_id: pin.revision_id.clone(),
                manifest_generation: pin.manifest_generation,
            })?)
        } else {
            None
        };
        QueryReadViewV2::assemble(request, parts, history_text, lexical, semantic, repo_map)
    }

    /// The history snapshot of `pin` at `epoch` (current when `None`);
    /// a generation with no history authority is refused with the history
    /// domain's own codes.
    fn history_read(
        ledger: &Ledger,
        pin: &GenerationPin,
        epoch: Option<AuxEpochV1>,
        now: Instant,
    ) -> Result<AuxRead<HistoryAuthorityState>, CoreError> {
        ledger
            .history_read_at(
                &pin.repo_id,
                &pin.revision_id,
                pin.manifest_generation,
                epoch,
                now,
            )?
            .ok_or_else(|| {
                let lexical_materialized = ledger.track_materialized(
                    &pin.repo_id,
                    &pin.revision_id,
                    SearchPlaneTrackKind::Lexical,
                );
                history_absent_error(pin, lexical_materialized)
            })
    }

    /// Every ledger read of the request, under the guard the caller holds.
    fn ledger_parts(
        &self,
        ledger: &Ledger,
        request: &ReadViewRequestV1<'_>,
        now: Instant,
    ) -> Result<LedgerParts, CoreError> {
        let pin = request.pin;
        let domains = request.domains;
        // The serving boundary first (QI-BB-003): a track pin the durable
        // authority does not retain is refused here, before any lane, so a
        // reaped or orphaned generation reads as `UNKNOWN_GENERATION` on
        // every route and never reaches an open.
        let lexical_manifest_digest = if domains.contains(ReadDomainV1::LexicalTrack) {
            ledger.validate_pinned_track_generation(
                pin,
                SearchPlaneTrackKind::Lexical,
                request.plane,
            )?;
            Some(
                ledger
                    .sealed_track_identity_digest(
                        &pin.repo_id,
                        &pin.revision_id,
                        SearchPlaneTrackKind::Lexical,
                        pin.manifest_generation,
                    )
                    .ok_or_else(|| {
                        CoreError::InvalidContract(
                            "validated lexical generation has no ledger seal identity".into(),
                        )
                    })?,
            )
        } else {
            None
        };
        let semantic_manifest_digest = if domains.contains(ReadDomainV1::SemanticTrack) {
            Self::validate_semantic_pin(ledger, request)?;
            let digest = ledger
                .semantic_generation_state(&pin.repo_id, &pin.revision_id, pin.manifest_generation)
                .map(|state| state.manifest_digest().to_string())
                .ok_or_else(|| {
                    CoreError::NotReady(format!(
                        "{}: semantic generation {} manifest authority disappeared after validation",
                        request.plane,
                        pin.manifest_generation.get()
                    ))
                })?;
            Some(digest)
        } else {
            None
        };
        let history = if domains.contains(ReadDomainV1::History) {
            Some(Self::history_read(
                ledger,
                pin,
                request.epochs.history,
                now,
            )?)
        } else {
            None
        };
        let history_text = match (&history, request.history_text) {
            (Some(read), true) => Some(self.claim_history_text(pin, read.epoch)?),
            (Some(_) | None, false) => None,
            (None, true) => {
                return Err(ReadViewRefusedError::DomainUndeclared {
                    domain: ReadDomainV1::History,
                    pin: pin.clone(),
                }
                .into());
            }
        };
        let runtime = if domains.contains(ReadDomainV1::RuntimeOverlay) {
            Some(
                ledger
                    .runtime_read_at(
                        &pin.repo_id,
                        &pin.revision_id,
                        pin.manifest_generation,
                        request.epochs.runtime,
                        now,
                    )?
                    .ok_or_else(|| ReadViewRefusedError::RuntimeNotReady { pin: pin.clone() })?,
            )
        } else {
            None
        };
        let structural = if domains.contains(ReadDomainV1::StructuralChunkUniverse) {
            Some(
                ledger
                    .structural_read_at(
                        &pin.repo_id,
                        &pin.revision_id,
                        pin.manifest_generation,
                        request.epochs.structural,
                        now,
                    )?
                    .ok_or_else(|| {
                        let not_ready = StructuralError::GenerationNotReady;
                        CoreError::Typed {
                            code: not_ready.code(),
                            message: format!(
                                "{not_ready}: generation {} has no structural authority to pin",
                                pin.manifest_generation.get()
                            ),
                        }
                    })?,
            )
        } else {
            None
        };
        Ok(LedgerParts {
            lexical_manifest_digest,
            semantic_manifest_digest,
            history,
            history_text,
            runtime,
            structural,
        })
    }

    /// The semantic track's serving boundary and readiness, in the order
    /// their codes are owed: a generation the authority does not retain
    /// is `UNKNOWN_GENERATION`; one it may still come to retain (the head
    /// being built, or beyond it) is refused with the semantic track's
    /// exact readiness code (`SEMANTIC_GENERATION_NOT_MATERIALIZED` /
    /// `NOT_SEALED`) when that check has one, and `NOT_READY` otherwise;
    /// a retained one must still carry the digest an `Active` selector
    /// resolved.
    fn validate_semantic_pin(
        ledger: &Ledger,
        request: &ReadViewRequestV1<'_>,
    ) -> Result<(), CoreError> {
        let pin = request.pin;
        let readiness = || {
            ledger.validate_semantic_generation(
                &pin.repo_id,
                &pin.revision_id,
                pin.manifest_generation,
                request.semantic_manifest_digest,
                true,
                request.plane,
            )
        };
        match ledger.validate_pinned_track_generation(
            pin,
            SearchPlaneTrackKind::Semantic,
            request.plane,
        ) {
            Ok(()) => readiness(),
            Err(not_ready @ CoreError::NotReady(_)) => {
                readiness()?;
                Err(not_ready)
            }
            Err(unknown) => Err(unknown),
        }
    }

    /// Claim the epoch's text index from the registry the composition
    /// root wired; refused typed when it wired none.
    ///
    /// Called under the ledger read guard so a mutation pruning the epoch
    /// — which takes the write lock only after this read releases — finds
    /// the claim and defers the discard instead of removing the index
    /// between the read that found the epoch retained and the open. The
    /// claim is a lookup; the open runs in [`Self::land_history_text`].
    fn claim_history_text(
        &self,
        pin: &GenerationPin,
        epoch: AuxEpochV1,
    ) -> Result<HistoryTextClaim, CoreError> {
        let parts = self
            .history_text
            .as_ref()
            .ok_or_else(history_relevance_unavailable)?;
        parts.claim(
            &AuxiliaryGenerationKeyV1 {
                repo_id: pin.repo_id.clone(),
                revision_id: pin.revision_id.clone(),
                generation: pin.manifest_generation,
            },
            epoch,
        )
    }

    /// Land a history text claim after the ledger guard is released: the
    /// cold open, if the claim reserved one, runs here under `budget`.
    fn land_history_text(
        &self,
        claim: HistoryTextClaim,
        budget: &RequestBudgetV1,
    ) -> Result<Arc<dyn HistoryTextSearcher>, CoreError> {
        self.history_text
            .as_ref()
            .ok_or_else(history_relevance_unavailable)?
            .land(claim, budget)
    }
}

#[cfg(test)]
pub(crate) fn assemble_for_test(
    request: &ReadViewRequestV1<'_>,
    history: Option<AuxRead<HistoryAuthorityState>>,
    runtime: Option<AuxRead<RuntimeMetadataState>>,
    structural: Option<AuxRead<StructuralAuthorityState>>,
    lexical: Option<Arc<dyn LexicalSearcher>>,
) -> Result<QueryReadViewV2, CoreError> {
    let lexical_manifest_digest = lexical
        .as_ref()
        .map(|handle| handle.artifact_identity().manifest_digest);
    QueryReadViewV2::assemble(
        request,
        LedgerParts {
            lexical_manifest_digest,
            semantic_manifest_digest: None,
            history,
            history_text: None,
            runtime,
            structural,
        },
        None,
        lexical,
        None,
        None,
    )
}
