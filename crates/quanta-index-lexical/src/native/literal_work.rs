//! Shared source-object and work boundaries for the Native Literal formats.

use quanta_index_core::index::{
    AttemptContext, IndexError, OwnerKey, SourceRecord, StoredObject, WorkBudget,
    ports::ImmutableObjectPort,
};
use quanta_index_core::{DiagnosticLaneV1, DiagnosticStageV1, DiagnosticTerminalV1};

pub(super) fn load_literal_body(
    record: &SourceRecord,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<StoredObject, IndexError> {
    budget.checkpoint()?;
    let body = objects.get(record.bytes, budget)?;
    budget.checkpoint()?;
    if body.registration.reference != record.bytes
        || !body.registration.children.is_empty()
        || u64::try_from(body.payload.len()).map_err(|_error| IndexError::ResourceLimited)?
            != record.byte_len
    {
        return Err(IndexError::CorruptObject(
            "literal source body identity, edges or length mismatch".into(),
        ));
    }
    Ok(body)
}

/// Infallible by construction because both owner key components have fixed widths.
pub(super) fn owner_key(owner: OwnerKey) -> [u8; 32] {
    let mut key = [0; 32];
    for (target, byte) in key
        .iter_mut()
        .zip(owner.producer.0.iter().chain(owner.stable_id.0.iter()))
    {
        *target = *byte;
    }
    key
}

#[expect(
    clippy::manual_ok_err,
    reason = "Core enter failures invalidate the trace through health events but must not change product work; keep discarded observation explicit"
)]
pub(super) fn observe_member_work<T>(
    context: &mut AttemptContext<'_>,
    lane: u16,
    work: impl FnOnce(&mut AttemptContext<'_>) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    let parent = context.budget.request().diagnostic_context().cloned();
    let span = parent.as_ref().and_then(|parent| {
        match parent.enter(DiagnosticStageV1::BuildPrepare, DiagnosticLaneV1(lane)) {
            Ok(span) => Some(span),
            Err(_diagnostic) => None,
        }
    });
    let before = span.as_ref().map(|_| context.budget.usage());
    let result = {
        let child = parent
            .as_ref()
            .zip(span.as_ref())
            .map(|(parent, span)| parent.child_context(span));
        let mut budget = context.budget.scoped_diagnostic_context(child);
        let mut phase = AttemptContext {
            attempt: context.attempt,
            objects: context.objects,
            maps: context.maps,
            custody: context.custody,
            scratch: context.scratch,
            budget: &mut budget,
        };
        work(&mut phase)
    };
    if let (Some(span), Some(before)) = (span.as_ref(), before) {
        context.budget.record_diagnostic_usage_delta(before, span);
    }
    if let Some(span) = span {
        let _finished = span.finish(DiagnosticTerminalV1::from_result(&result));
    }
    result
}
