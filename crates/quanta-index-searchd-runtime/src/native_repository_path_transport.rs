//! Selected FileMap queries retain row and cursor custody through serialization.

use std::sync::Arc;

use quanta_index_contract::{
    index::{IndexInstanceId, ViewSelection},
    native_protocol::NativeRepositoryPathCursor,
};
use quanta_index_core::index::{IndexError, QueryCoverage, WorkBudget, effort::ReservationGuard};
use quanta_index_repomap::native::{RepositoryFilePathPage, RepositoryPathReadRequest};

use crate::native::NativeDispatcher;

impl NativeDispatcher {
    pub(super) fn query_repository_paths(
        &self,
        selection: ViewSelection,
        instance: IndexInstanceId,
        prefix: String,
        after: Option<NativeRepositoryPathCursor>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<RepositoryFilePathPage, IndexError> {
        if after.as_ref().is_some_and(|cursor| {
            selection != ViewSelection::Exact(cursor.view)
                || cursor.instance != instance
                || cursor.prefix != prefix
        }) {
            return Err(IndexError::InvalidInput(
                "repository path cursor differs from selected view".into(),
            ));
        }
        let request = RepositoryPathReadRequest {
            prefix,
            after,
            limit,
        };
        request.validate()?;
        let prepared = {
            let control = ReservationGuard::try_acquire(
                Arc::clone(&self.control_scheduler),
                self.control_need,
            )
            .ok_or_else(|| self.control_scheduler.denial())?;
            budget.bind_memory_scope(control.memory_scope());
            self.kernel.prepare_query(
                &self.repository_path_queries,
                selection,
                instance,
                request,
                budget,
            )?
        };
        let Some(reservation) = ReservationGuard::try_acquire(
            Arc::clone(&self.interactive_scheduler),
            prepared.required_resources(),
        ) else {
            let denial = self.interactive_scheduler.denial();
            prepared.abort()?;
            return Err(denial);
        };
        let outcome = prepared.execute(&reservation)?;
        if !matches!(outcome.coverage(), QueryCoverage::Core)
            || outcome.exhaustive() == outcome.result().has_more
        {
            return Err(IndexError::BackendViolation(
                "repository path reader returned an invalid page".into(),
            ));
        }
        Ok(outcome.into_result())
    }
}
