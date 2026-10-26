//! Request-local Text eligibility and Semantic ranking under one selected ticket.

use std::sync::Arc;

use quanta_index_contract::{
    SemanticQueryRequest,
    index::{IndexInstanceId, ViewSelection},
};
use quanta_index_core::index::{IndexError, WorkBudget, effort::ReservationGuard};
use quanta_index_semantic::native::SemanticReadRequest;

use crate::{
    native::{
        NativeDispatcher, ScopedNativeSemanticResult, native_semantic_result, plan_native_text,
    },
    native_lexical_scope,
    native_provider_custody::NativeProviderExecutionCustody,
};

impl NativeDispatcher {
    pub(super) fn search_semantic_scoped(
        &self,
        selection: ViewSelection,
        instance: IndexInstanceId,
        query: SemanticQueryRequest,
        budget: &mut WorkBudget,
    ) -> Result<ScopedNativeSemanticResult, IndexError> {
        let scope = query
            .lexical_scope
            .as_ref()
            .ok_or_else(|| IndexError::InvalidInput("semantic scope is absent".into()))?;
        if scope.constraints != query.constraints {
            return Err(IndexError::InvalidInput(
                "semantic and lexical scope constraints differ".into(),
            ));
        }
        let semantic_top_k = query.top_k;
        let (mut session, planned, scope_limit) = {
            let control = ReservationGuard::try_acquire(
                Arc::clone(&self.control_scheduler),
                self.control_need,
            )
            .ok_or_else(|| self.control_scheduler.denial())?;
            budget.bind_memory_scope(control.memory_scope());
            // Plan while the actual control grant is live. The resulting plan
            // keeps retained DATA custody after the control stage ends.
            let planned = plan_native_text(scope, |bytes, lifetime| {
                budget.reserve_memory(bytes, lifetime)
            })?;
            let scope_limit = planned.result_limit()?;
            let session = self.kernel.begin_query_session(selection, budget)?;
            (session, planned, scope_limit)
        };
        let result = (|| {
            let view = session.view();
            let text = {
                let _control = ReservationGuard::try_acquire(
                    Arc::clone(&self.control_scheduler),
                    self.control_need,
                )
                .ok_or_else(|| self.control_scheduler.denial())?;
                let text_instance = session.unique_compatible_instance(&self.text_queries)?;
                session.prepare_stage(&self.text_queries, text_instance, planned)?
            };
            let need = text.required_resources();
            let permit =
                ReservationGuard::try_acquire(Arc::clone(&self.interactive_scheduler), need)
                    .ok_or_else(|| self.interactive_scheduler.denial())?;
            let outcome = text.execute(&permit)?;
            let eligible = native_lexical_scope::lexical_eligibility_from_raw(
                &outcome,
                view,
                scope_limit,
                None,
                |bytes, lifetime| session.reserve_query_memory(bytes, lifetime),
            )?;
            drop(outcome);
            drop(permit);
            let mut request = SemanticReadRequest::from(query);
            request.lexical_eligibility = Some(eligible);
            let semantic = {
                let _control = ReservationGuard::try_acquire(
                    Arc::clone(&self.control_scheduler),
                    self.control_need,
                )
                .ok_or_else(|| self.control_scheduler.denial())?;
                session.prepare_stage(&self.semantic_queries, instance, request)?
            };
            let need = semantic.required_resources();
            let permit =
                ReservationGuard::try_acquire(Arc::clone(&self.interactive_scheduler), need)
                    .ok_or_else(|| self.interactive_scheduler.denial())?;
            if need.provider_units > 0 {
                let custody = NativeProviderExecutionCustody::new_with_owner(
                    permit,
                    self.root_lease.clone(),
                )?;
                let outcome = semantic
                    .execute_with_provider_custody(custody.reservation(), &custody.port())?;
                // Conversion remains inside the producing stage's grant.
                return native_semantic_result(
                    outcome,
                    instance,
                    semantic_top_k,
                    |bytes, lifetime| session.reserve_query_memory(bytes, lifetime),
                );
            }
            let outcome = semantic.execute(&permit)?;
            native_semantic_result(outcome, instance, semantic_top_k, |bytes, lifetime| {
                session.reserve_query_memory(bytes, lifetime)
            })
        })();
        // Cleanup executes on every branch. Preserve the original typed cause;
        // a cleanup failure refuses an otherwise successful result.
        let released = session.close();
        let result = result.and_then(|value| released.map(|()| value))?;
        budget.checkpoint()?;
        Ok(result)
    }
}
