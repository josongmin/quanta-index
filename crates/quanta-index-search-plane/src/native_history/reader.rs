//! Existing typed query-port bridge; no new read-context or ticket authority.
use super::*;
use quanta_index_contract::{
    HistoryCursor, HistoryOrderV1, HistoryQueryRequest, LqQuery, index::IndexInstanceId,
    native_protocol::NativeHistoryCursor,
};
use quanta_index_core::index::{IndexQueryPort, QueryOutcome, effort::ResourceNeed};

/// The Runtime admits and lowers the public request under its original DATA
/// grant before moving it here. The grant follows this plan until it is dropped.
pub struct PlannedNativeHistoryReadV1 {
    query: LqQuery,
    order: HistoryOrderV1,
    top_k: u32,
    after: Option<(ObjectRef, HistoryCursor)>,
    _memory: Option<MemoryLease>,
}

impl PlannedNativeHistoryReadV1 {
    pub fn new(
        query: LqQuery,
        order: HistoryOrderV1,
        top_k: u32,
        after: Option<(ObjectRef, HistoryCursor)>,
        memory: Option<MemoryLease>,
    ) -> Result<Self, CoreError> {
        let top_k = quanta_index_core::validate_query_top_k(top_k)?;
        crate::query_dispatcher::routes::history::validate_history_query(&query)?;
        Ok(Self {
            query,
            order,
            top_k,
            after,
            _memory: memory,
        })
    }
}

/// Public request lowering through the existing parser/IR and original budget.
/// Cursor hashing follows bounded lowering under the same Control grant; the
/// reader validates its member and actual rank afterward.
pub fn plan_native_history_read_v1(
    request: &HistoryQueryRequest,
    after: Option<(ViewRef, IndexInstanceId, NativeHistoryCursor)>,
    budget: &mut WorkBudget,
) -> Result<PlannedNativeHistoryReadV1, CoreError> {
    quanta_index_contract::native_protocol::validate_native_history_query(request)
        .map_err(|error| invalid(&error))?;
    if !request.text_query.constraints.language_any_of.is_empty()
        || request
            .text_query
            .constraints
            .repo_relative_path_exact
            .is_some()
    {
        return Err(CoreError::NotImplemented(
            "History has no current-file language/exact-path constraint surface; use its explicit diff file filter".into(),
        ));
    }
    crate::lowering::reject_code_search_on_nonlexical_route(request.text_query.syntax, "history")?;
    let length = cbor_payload_len_with_progress_v1(request, &mut |bytes| {
        budget.charge(WorkUsage {
            bytes: u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })
    })
    .map_err(|error| match error {
        CborPayloadLengthProgressErrorV1::Progress(error) => history_work_error(budget, error),
        CborPayloadLengthProgressErrorV1::Codec(error) => invalid(&error.to_string()),
    })?;
    let boundary_bytes = after
        .as_ref()
        .and_then(|(_, _, cursor)| cursor.boundary.file_path.as_ref())
        .map_or(0, String::len);
    let bound = length
        .checked_mul(128)
        .and_then(|bytes| bytes.checked_add(u64::try_from(boundary_bytes).ok()?))
        .and_then(|bytes| bytes.checked_add(65_536))
        .ok_or_else(|| invalid("History lowering allocation bound overflow"))?;
    let memory = budget
        .reserve_memory(bound, MemoryLifetime::Retained)
        .map_err(|error| history_work_error(budget, error))?;
    let lowered = crate::lower_lexical_text_query(&request.text_query)?;
    budget
        .checkpoint()
        .map_err(|error| history_work_error(budget, error))?;
    let after = if let Some((view, instance, cursor)) = after {
        cursor
            .validate_request(view, instance, request)
            .map_err(|error| invalid(&error))?;
        Some((cursor.member, cursor.boundary))
    } else {
        None
    };
    budget
        .checkpoint()
        .map_err(|error| history_work_error(budget, error))?;
    PlannedNativeHistoryReadV1::new(
        lowered,
        request.order,
        request.text_query.top_k,
        after,
        memory,
    )
}

/// Reservation is the registered service demand, never an invented local
/// quota. History has no provider and keeps the caller's original budget.
pub struct NativeHistoryReaderV1 {
    demand: ResourceNeed,
}

impl NativeHistoryReaderV1 {
    pub fn new(demand: ResourceNeed) -> Result<Self, IndexError> {
        if demand.provider_units != 0 || demand.cpu_units == 0 || demand.memory_bytes == 0 {
            return Err(IndexError::InvalidInput(
                "History reader needs admitted CPU/memory and no provider".into(),
            ));
        }
        Ok(Self { demand })
    }
}

impl IndexQueryPort<PlannedNativeHistoryReadV1, NativeHistoryQueryResultV1>
    for NativeHistoryReaderV1
{
    fn validate_instance(
        &self,
        instance: &quanta_index_contract::index::InstanceSpec,
    ) -> Result<(), IndexError> {
        super::validate_instance(instance)
            .map_err(|error| IndexError::InvalidInput(error.to_string()))
    }
    fn reservation(&self, _: &PlannedNativeHistoryReadV1) -> Result<ResourceNeed, IndexError> {
        Ok(self.demand)
    }
    fn query(
        &self,
        request: &PlannedNativeHistoryReadV1,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<NativeHistoryQueryResultV1>, IndexError> {
        let read = NativeHistoryReadV1::open(context)
            .map_err(|error| index_error(error, context.budget))?;
        if request
            .after
            .as_ref()
            .is_some_and(|(artifact, _)| *artifact != read.artifact)
        {
            return Err(IndexError::InvalidInput(
                "History boundary member differs from retained ticket".into(),
            ));
        }
        let result = read
            .query(
                context.ticket.view.identity,
                &request.query,
                request.order,
                request.top_k,
                request.after.as_ref().map(|(_, boundary)| boundary),
                context.budget,
            )
            .map_err(|error| index_error(error, context.budget))?;
        QueryOutcome::core_scanned(context.ticket, context.instance, true, result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::{
        HistoryCursorOrderV1, LqFilter, LqType, QueryConstraintSetV1, SearchPlaneErrorCodeV2,
        TextQueryRequest, TextQuerySyntax,
        index::{CollectionId, SlotId, TenantId, WorkspaceKey, WorktreeId},
        lex::CommitSha,
        native_protocol::native_history_query_digest,
    };
    use quanta_index_core::index::{WorkBudgetDenial, WorkLimits};
    use std::time::{Duration, Instant};

    fn request() -> HistoryQueryRequest {
        HistoryQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "type:commit needle author:alice".into(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: None,
                generation_selector: None,
                top_k: 2,
                cursor: None,
            },
            order: HistoryOrderV1::Recency,
            cursor: None,
        }
    }
    fn budget(bytes: u64) -> WorkBudget {
        WorkBudget::new(
            WorkLimits {
                reads: 1_000_000,
                writes: 1_000_000,
                mutations: 1_000_000,
                bytes,
            },
            Instant::now() + Duration::from_secs(60),
        )
    }

    #[test]
    fn native_history_cursor_digest_follows_budgeted_bounded_lowering() -> Result<(), CoreError> {
        let request = request();
        let view = ViewRef {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
            sequence: 9,
            digest: Digest([5; 32]),
        };
        let instance = IndexInstanceId([8; 16]);
        let boundary = HistoryCursor {
            order: HistoryCursorOrderV1::Recency,
            committer_time_ms: 1000,
            sha: CommitSha::from_bytes([1; 20]),
            file_path: None,
            aux_epoch: AuxEpochV1::new(3),
        };
        let mut cursor = NativeHistoryCursor {
            view,
            instance,
            member: ObjectRef(Digest([7; 32])),
            query: Digest([9; 32]),
            boundary: boundary.clone(),
        };
        let mut exhausted = budget(0);
        let error = plan_native_history_read_v1(
            &request,
            Some((view, instance, cursor.clone())),
            &mut exhausted,
        )
        .err()
        .expect("byte refusal precedes cursor digest mismatch");
        assert!(matches!(
            error,
            CoreError::Typed {
                code: SearchPlaneErrorCodeV2::ServerOverloaded,
                ..
            }
        ));
        assert_eq!(exhausted.first_denial(), Some(WorkBudgetDenial::Bytes));

        let error = plan_native_history_read_v1(
            &request,
            Some((view, instance, cursor.clone())),
            &mut budget(512 * 1024 * 1024),
        )
        .err()
        .expect("funded cursor digest mismatch is invalid input");
        assert!(matches!(error, CoreError::InvalidContract(_)));

        cursor.query =
            native_history_query_digest(instance, &request).map_err(|error| invalid(&error))?;
        let plan = plan_native_history_read_v1(
            &request,
            Some((view, instance, cursor.clone())),
            &mut budget(512 * 1024 * 1024),
        )?;
        assert_eq!(plan.after, Some((cursor.member, boundary)));
        Ok(())
    }

    #[test]
    fn native_history_public_plan_reuses_canonical_lowering_and_rejects_wrong_kind()
    -> Result<(), CoreError> {
        let mut request = request();
        let plan = plan_native_history_read_v1(&request, None, &mut budget(512 * 1024 * 1024))?;
        assert_eq!(plan.top_k, 2);
        assert!(plan.query.filters.iter().any(|filter| matches!(
            filter,
            LqFilter::Type {
                kind: LqType::Commit
            }
        )));
        assert!(
            plan.query
                .filters
                .iter()
                .any(|filter| matches!(filter, LqFilter::Author { pattern } if pattern == "alice"))
        );
        request.text_query.query_text = "type:file needle".into();
        assert!(
            plan_native_history_read_v1(&request, None, &mut budget(512 * 1024 * 1024)).is_err()
        );
        Ok(())
    }

    #[test]
    fn native_history_public_plan_retains_original_byte_and_cancel_refusal() {
        let mut exhausted = budget(0);
        assert!(plan_native_history_read_v1(&request(), None, &mut exhausted).is_err());
        assert_eq!(exhausted.first_denial(), Some(WorkBudgetDenial::Bytes));
        let mut cancelled = budget(512 * 1024 * 1024);
        cancelled.request().cancel_handle().cancel();
        assert!(plan_native_history_read_v1(&request(), None, &mut cancelled).is_err());
        assert_eq!(cancelled.first_denial(), Some(WorkBudgetDenial::Cancelled));
    }
}
