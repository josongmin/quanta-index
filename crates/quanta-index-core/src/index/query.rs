//! Typed, ticket-bound native query capability. Request IR remains in the
//! existing query contract; concrete adapters choose their own typed result.

use super::{
    CoverageBinding, CoverageStatus, GroupActivation, IndexError, InstanceSpec, ObjectRef,
    TicketRecord, ViewRef, WorkBudget,
    effort::ResourceNeed,
    ports::{ImmutableObjectPort, PersistentMapPort},
};
pub use quanta_index_contract::{FiniteScore, FiniteScoreError};

/// Authenticated origin of a native Text hit shared by lexical and semantic readers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeTextHitSource {
    Durable(ObjectRef),
    Overlay { revision: u64 },
}

/// Coverage is independent of whether the selected scope was fully scanned.
/// `Core` means the synchronous core member; async status stays explicit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueryCoverage {
    Core,
    Async(Box<CoverageBinding>),
}

/// A result can be exhaustive only when an async group's desired input is
/// completely covered. A scan cap may still make a Complete group nonexhaustive.
#[derive(Debug, PartialEq)]
pub struct QueryOutcome<R> {
    view: ViewRef,
    coverage: QueryCoverage,
    exhaustive: bool,
    result: R,
}

impl<R> QueryOutcome<R> {
    pub fn core_scanned(
        ticket: &TicketRecord,
        instance: &InstanceSpec,
        exhaustive: bool,
        result: R,
    ) -> Result<Self, IndexError> {
        if instance.activation != GroupActivation::CoreRequired {
            return Err(IndexError::BackendViolation(
                "core query used an async index instance".into(),
            ));
        }
        Ok(Self {
            view: ticket.view.identity,
            coverage: QueryCoverage::Core,
            exhaustive,
            result,
        })
    }

    pub fn async_ranked(
        ticket: &TicketRecord,
        instance: &InstanceSpec,
        coverage: &CoverageBinding,
        exhaustive: bool,
        result: R,
    ) -> Result<Self, IndexError> {
        if instance.activation != GroupActivation::AsyncCoverage
            || coverage.instance != instance.instance
            || coverage.desired.schema != instance.schema
            || coverage.desired.policy != instance.policy
        {
            return Err(IndexError::BackendViolation(
                "query coverage differs from selected instance".into(),
            ));
        }
        let complete = matches!(coverage.status, CoverageStatus::Complete);
        if complete && coverage.covered != Some(coverage.desired) {
            return Err(IndexError::CorruptObject(
                "complete query coverage differs from desired input".into(),
            ));
        }
        if exhaustive && !complete {
            return Err(IndexError::BackendViolation(
                "incomplete query coverage cannot be exhaustive".into(),
            ));
        }
        Ok(Self {
            view: ticket.view.identity,
            coverage: QueryCoverage::Async(Box::new(*coverage)),
            exhaustive,
            result,
        })
    }

    #[must_use]
    pub const fn view(&self) -> ViewRef {
        self.view
    }

    #[must_use]
    pub const fn coverage(&self) -> &QueryCoverage {
        &self.coverage
    }

    #[must_use]
    pub const fn exhaustive(&self) -> bool {
        self.exhaustive
    }

    #[must_use]
    pub const fn result(&self) -> &R {
        &self.result
    }

    #[must_use]
    pub fn into_result(self) -> R {
        self.result
    }
}

/// The caller owns the read ticket and validates it before and after adapter
/// execution.
///
/// `instance` must be resolved from that ticket's stored profile;
/// neither a client-supplied descriptor nor a newer profile is authoritative.
pub struct QueryReadContext<'a> {
    pub ticket: &'a TicketRecord,
    pub instance: &'a InstanceSpec,
    pub objects: &'a dyn ImmutableObjectPort,
    pub maps: &'a dyn PersistentMapPort,
    pub budget: &'a mut WorkBudget,
}

/// Generic over the existing typed public request and the adapter's result.
///
/// An erased registry bridge may be built per concrete `(Q, R)` capability;
/// this port does not introduce an alternate query AST or opaque payload.
pub trait IndexQueryPort<Q, R>: Send + Sync {
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError>;
    /// Verify the selected immutable member is searchable without issuing a
    /// query. Unsupported readers refuse; source publication alone is not a
    /// searchable frontier. The caller holds the original IO/memory grant.
    fn validate_searchable(
        &self,
        _owner: super::OwnerKey,
        _source: Option<super::ObjectRef>,
        _context: &mut QueryReadContext<'_>,
    ) -> Result<(), IndexError> {
        Err(IndexError::SnapshotUnavailable)
    }
    /// Exact logical reservation required before calling `query`. The service
    /// validates this against its shared scheduler; a provider-backed reader
    /// must declare its provider unit here.
    fn reservation(&self, request: &Q) -> Result<ResourceNeed, IndexError>;
    fn query(
        &self,
        request: &Q,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<R>, IndexError>;
}

#[cfg(test)]
mod tests {
    use super::{FiniteScore, QueryCoverage, QueryOutcome};
    use crate::index::{
        CatalogView, CollectionId, CoverageBinding, CoverageStatus, Digest, GroupActivation,
        IndexInstanceId, IndexKind, InputBinding, InputDomain, InstanceSpec, MapRoot, ObjectRef,
        PublicationGroupId, ReadSessionRef, ReadTicketLease, SchemaId, SlotId, SourceCut,
        StorageFormatId, TenantId, TicketId, TicketRecord, ViewRef, WorkspaceKey, WorktreeId,
    };

    // Preserve single evaluation and borrowed operands while reporting test failures as errors.
    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(format!("assertion failed: {}", stringify!($condition)).into());
            }
        }};
    }
    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) => {
                    if left != right {
                        return Err(format!(
                            "assertion failed: {} == {}; left: {left:?}, right: {right:?}",
                            stringify!($left),
                            stringify!($right)
                        )
                        .into());
                    }
                }
            }
        }};
    }

    fn ticket() -> Result<TicketRecord, Box<dyn std::error::Error>> {
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        };
        let mut view = CatalogView {
            identity: ViewRef {
                workspace,
                epoch: 1,
                sequence: 7,
                digest: Digest([0; 32]),
            },
            profile: ObjectRef(Digest([5; 32])),
            source: SourceCut {
                owners: MapRoot(None),
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            },
            projections: MapRoot(None),
            members: MapRoot(None),
            coverage: MapRoot(None),
        };
        view.identity.digest = view.identity_digest()?;
        Ok(TicketRecord {
            lease: ReadTicketLease {
                id: TicketId([6; 16]),
                session: ReadSessionRef { id: 1, fence: 1 },
                fence: 1,
                expires_at_ms: 10_000,
            },
            view,
        })
    }

    fn instance(activation: GroupActivation) -> InstanceSpec {
        InstanceSpec {
            instance: IndexInstanceId([7; 16]),
            kind: IndexKind([8; 16]),
            group: PublicationGroupId([9; 16]),
            activation,
            input_domain: InputDomain::SourceLocal,
            schema: SchemaId([10; 16]),
            policy: Digest([11; 32]),
            format: StorageFormatId([12; 16]),
        }
    }

    fn coverage(spec: &InstanceSpec, status: CoverageStatus) -> CoverageBinding {
        let desired = InputBinding {
            root: MapRoot(None),
            schema: spec.schema,
            policy: spec.policy,
            context: Digest([13; 32]),
        };
        CoverageBinding {
            instance: spec.instance,
            desired,
            covered: Some(desired),
            searchable: None,
            status,
            owners: MapRoot(None),
        }
    }

    #[test]
    fn finite_score_rejects_nonfinite_and_roundtrips_without_eq_drift()
    -> Result<(), Box<dyn std::error::Error>> {
        check!(FiniteScore::new(f64::NAN).is_err());
        check!(FiniteScore::new(f64::INFINITY).is_err());
        check!(FiniteScore::new(f64::NEG_INFINITY).is_err());
        let negative_zero = FiniteScore::new(-0.0)?;
        check_eq!(negative_zero, FiniteScore::new(0.0)?);
        let score = FiniteScore::new(1.234_567_890_123)?;
        let wire = serde_json::to_string(&score)?;
        let restored: FiniteScore = serde_json::from_str(&wire)?;
        check_eq!(restored, score);
        check_eq!(restored.get().to_bits(), score.get().to_bits());
        check!(serde_json::from_str::<FiniteScore>("null").is_err());
        Ok(())
    }

    #[test]
    fn exact_core_result_uses_selected_ticket_view_and_rejects_async_instance()
    -> Result<(), Box<dyn std::error::Error>> {
        let selected = ticket()?;
        let outcome = QueryOutcome::core_scanned(
            &selected,
            &instance(GroupActivation::CoreRequired),
            true,
            vec![1_u8],
        )?;
        check_eq!(outcome.view(), selected.view.identity);
        check!(matches!(outcome.coverage(), QueryCoverage::Core));
        check!(outcome.exhaustive());
        check_eq!(outcome.result().as_slice(), &[1]);
        check!(
            QueryOutcome::core_scanned(
                &selected,
                &instance(GroupActivation::AsyncCoverage),
                true,
                vec![1_u8]
            )
            .is_err()
        );
        let capped = QueryOutcome::core_scanned(
            &selected,
            &instance(GroupActivation::CoreRequired),
            false,
            vec![1_u8],
        )?;
        check!(!capped.exhaustive());
        Ok(())
    }

    #[test]
    fn async_result_cannot_promote_pending_or_inconsistent_coverage_to_exhaustive()
    -> Result<(), Box<dyn std::error::Error>> {
        let selected = ticket()?;
        let semantic = instance(GroupActivation::AsyncCoverage);
        let pending = coverage(
            &semantic,
            CoverageStatus::Pending {
                previous_missing: MapRoot(None),
            },
        );
        check!(
            QueryOutcome::async_ranked(&selected, &semantic, &pending, true, vec![1_u8]).is_err()
        );
        let incomplete =
            QueryOutcome::async_ranked(&selected, &semantic, &pending, false, vec![1_u8])?;
        check_eq!(incomplete.view(), selected.view.identity);
        check!(!incomplete.exhaustive());
        check!(
            matches!(incomplete.coverage(), QueryCoverage::Async(binding)
            if binding.status == pending.status)
        );
        let mut false_complete = coverage(&semantic, CoverageStatus::Complete);
        false_complete.covered = None;
        check!(
            QueryOutcome::async_ranked(&selected, &semantic, &false_complete, true, vec![1_u8])
                .is_err()
        );
        let complete = coverage(&semantic, CoverageStatus::Complete);
        let capped =
            QueryOutcome::async_ranked(&selected, &semantic, &complete, false, vec![1_u8])?;
        check!(!capped.exhaustive());
        check!(
            QueryOutcome::async_ranked(
                &selected,
                &instance(GroupActivation::CoreRequired),
                &complete,
                true,
                vec![1_u8]
            )
            .is_err()
        );
        Ok(())
    }
}
