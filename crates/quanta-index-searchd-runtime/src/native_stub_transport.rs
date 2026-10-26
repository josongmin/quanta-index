//! Source-bound, saved-revision Stub certification and action preflight.
//!
//! The AST certificate authority is configured by the product. This route
//! never treats a caller's item list, a rendered row, or overlay text as an
//! authority for source content or editor revision.

use std::{ops::Deref, str, sync::Arc};

use quanta_index_contract::{
    index::ViewSelection,
    native_protocol::{
        NativeStubBinding, NativeStubByteEdit, NativeStubCoordinateUnit, NativeStubEdit,
        NativeStubItem, NativeStubReply, NativeStubRequest,
    },
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{
        IndexError, MemoryLease, MemoryLifetime, ObjectRef, SourceRecordOwner, StoredObject,
        WorkBudget, WorkUsage,
        effort::ReservationGuard,
        owner_map_key,
        ports::{ImmutableObjectPort, PersistentMapPort},
    },
};
use quanta_index_ipc::DispatchContextV1;
use quanta_index_lexical::native::stub::{
    ByteRange, CoordinateUnit, ProposedEdit, StubAstCertificatePort, StubContext, StubItem,
    StubRefusal, TextPosition, TextRange, certify_manifest, preflight_action,
};

use crate::{native::NativeDispatcher, native_overlay::OverlayDocumentKey};

const MAX_STUB_ITEMS: usize = 128;
const MAX_STUB_EDITS: usize = 64;
const MAX_STUB_SOURCE_BYTES: usize = 1 << 20;
// A 4091-byte canonical path may JSON-escape to six bytes per byte.
const MAX_STUB_RECORD_BYTES: usize = 32 << 10;
const MAX_STUB_REPLY_BYTES: u64 = 320 << 10;

pub(crate) struct ScopedStubReply {
    reply: NativeStubReply,
    // Drop response allocations before returning their retained grant.
    _memory: MemoryLease,
}

impl Deref for ScopedStubReply {
    type Target = NativeStubReply;

    fn deref(&self) -> &Self::Target {
        &self.reply
    }
}

fn refusal(error: StubRefusal) -> IndexError {
    match error {
        StubRefusal::UnsupportedLanguage(language) => {
            IndexError::InvalidInput(format!("Native Stub language unsupported: {language}"))
        }
        StubRefusal::UnsupportedCapability(language) => {
            IndexError::InvalidInput(format!("Native Stub capability unsupported: {language}"))
        }
        StubRefusal::StaleBinding => IndexError::Conflict,
        StubRefusal::InvalidCoordinate => {
            IndexError::InvalidInput("Native Stub coordinate is invalid".into())
        }
        StubRefusal::InvalidManifest => {
            IndexError::InvalidInput("Native Stub manifest is invalid".into())
        }
        StubRefusal::Resource(error) => error,
    }
}

fn checked_bytes(count: usize, element: usize, payload: usize) -> Result<u64, IndexError> {
    let bytes = count
        .checked_mul(element)
        .and_then(|bytes| bytes.checked_add(payload))
        .and_then(|bytes| bytes.checked_add(128))
        .ok_or(IndexError::ResourceLimited)?;
    u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)
}

fn admitted_memory(
    budget: &mut WorkBudget,
    bytes: u64,
    lifetime: MemoryLifetime,
) -> Result<MemoryLease, IndexError> {
    budget.charge(WorkUsage {
        bytes,
        ..WorkUsage::default()
    })?;
    budget.reserve_memory(bytes, lifetime)?.ok_or_else(|| {
        IndexError::BackendViolation("Native Stub reply needs retained memory custody".into())
    })
}

fn stub_context(binding: &NativeStubBinding) -> StubContext {
    StubContext {
        workspace: binding.document.session.workspace,
        publisher: binding.document.session.publisher,
        session_epoch: binding.document.session.epoch,
        owner: binding.document.owner,
        view: binding.selected,
        source: binding.source,
        document_revision: binding.revision,
        language: binding.language.clone(),
    }
}

fn items_from_wire(
    wire: Vec<NativeStubItem>,
    budget: &mut WorkBudget,
) -> Result<(Vec<StubItem>, MemoryLease), IndexError> {
    if wire.is_empty() {
        return Err(IndexError::InvalidInput(
            "Native Stub manifest is empty".into(),
        ));
    }
    if wire.len() > MAX_STUB_ITEMS {
        return Err(IndexError::ResourceLimited);
    }
    let bytes = checked_bytes(wire.len(), std::mem::size_of::<StubItem>(), 0)?;
    let memory = admitted_memory(budget, bytes, MemoryLifetime::Temporary)?;
    let mut items = Vec::new();
    items
        .try_reserve_exact(wire.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    for item in wire {
        budget.checkpoint()?;
        items.push(StubItem {
            stable_id: item.stable_id,
            ast_digest: item.ast_digest,
            span: ByteRange {
                start: item.start,
                end: item.end,
            },
            label: item.label,
        });
    }
    Ok((items, memory))
}

fn items_to_wire(
    items: &[StubItem],
    binding: &NativeStubBinding,
    budget: &mut WorkBudget,
) -> Result<(Vec<NativeStubItem>, MemoryLease), IndexError> {
    let labels = items.iter().try_fold(0usize, |total, item| {
        total
            .checked_add(item.label.len())
            .ok_or(IndexError::ResourceLimited)
    })?;
    let payload = labels
        .checked_add(binding.language.len())
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<ScopedStubReply>()))
        .ok_or(IndexError::ResourceLimited)?;
    let bytes = checked_bytes(items.len(), std::mem::size_of::<NativeStubItem>(), payload)?;
    if bytes > MAX_STUB_REPLY_BYTES {
        return Err(IndexError::ResourceLimited);
    }
    let memory = admitted_memory(budget, bytes, MemoryLifetime::Retained)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(items.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    for item in items {
        budget.checkpoint()?;
        let mut label = String::new();
        label
            .try_reserve_exact(item.label.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        label.push_str(&item.label);
        output.push(NativeStubItem {
            stable_id: item.stable_id,
            ast_digest: item.ast_digest,
            start: item.span.start,
            end: item.span.end,
            label,
        });
    }
    Ok((output, memory))
}

fn edits_from_wire<'a>(
    wire: &'a [NativeStubEdit],
    budget: &mut WorkBudget,
) -> Result<(Vec<ProposedEdit<'a>>, MemoryLease), IndexError> {
    if wire.is_empty() {
        return Err(IndexError::InvalidInput(
            "Native Stub action is empty".into(),
        ));
    }
    if wire.len() > MAX_STUB_EDITS {
        return Err(IndexError::ResourceLimited);
    }
    let bytes = checked_bytes(wire.len(), std::mem::size_of::<ProposedEdit<'_>>(), 0)?;
    let memory = admitted_memory(budget, bytes, MemoryLifetime::Temporary)?;
    let mut edits = Vec::new();
    edits
        .try_reserve_exact(wire.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    for edit in wire {
        budget.checkpoint()?;
        edits.push(ProposedEdit {
            target_id: edit.target_id,
            expected_ast: edit.expected_ast,
            range: TextRange {
                start: TextPosition {
                    line: edit.range.start.line,
                    column: edit.range.start.column,
                },
                end: TextPosition {
                    line: edit.range.end.line,
                    column: edit.range.end.column,
                },
            },
            replacement: &edit.replacement,
        });
    }
    Ok((edits, memory))
}

fn edits_to_wire(
    edits: &[quanta_index_lexical::native::stub::ByteEdit],
    binding: &NativeStubBinding,
    budget: &mut WorkBudget,
) -> Result<(Vec<NativeStubByteEdit>, MemoryLease), IndexError> {
    let replacements = edits.iter().try_fold(0usize, |total, edit| {
        total
            .checked_add(edit.replacement.len())
            .ok_or(IndexError::ResourceLimited)
    })?;
    let payload = replacements
        .checked_add(binding.language.len())
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<ScopedStubReply>()))
        .ok_or(IndexError::ResourceLimited)?;
    let bytes = checked_bytes(
        edits.len(),
        std::mem::size_of::<NativeStubByteEdit>(),
        payload,
    )?;
    if bytes > MAX_STUB_REPLY_BYTES {
        return Err(IndexError::ResourceLimited);
    }
    let memory = admitted_memory(budget, bytes, MemoryLifetime::Retained)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(edits.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    for edit in edits {
        budget.checkpoint()?;
        let mut replacement = Vec::new();
        replacement
            .try_reserve_exact(edit.replacement.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        replacement.extend_from_slice(&edit.replacement);
        output.push(NativeStubByteEdit {
            target_id: edit.target_id,
            start: edit.range.start,
            end: edit.range.end,
            replacement,
        });
    }
    Ok((output, memory))
}

fn selected_source(
    dispatcher: &NativeDispatcher,
    binding: &NativeStubBinding,
    budget: &mut WorkBudget,
) -> Result<
    (
        quanta_index_kernel::ReadTicket,
        quanta_index_core::index::StoredObject,
    ),
    IndexError,
> {
    if binding.selected.workspace != binding.document.session.workspace
        || binding.revision == 0
        || binding.language.is_empty()
        || binding.language.len() > 64
    {
        return Err(IndexError::InvalidInput(
            "Native Stub binding identity is invalid".into(),
        ));
    }
    let ticket = dispatcher
        .kernel
        .pin_view(ViewSelection::Exact(binding.selected), budget)?;
    ticket.validate()?;
    if ticket.view().identity != binding.selected {
        return Err(IndexError::Conflict);
    }
    let source = dispatcher.objects.lookup_object(
        ticket.view().source.owners,
        &owner_map_key(binding.document.owner),
        budget,
    )?;
    if source != Some(binding.source) {
        return Err(IndexError::Conflict);
    }
    let body = verified_source_body(dispatcher.objects.as_ref(), binding.source, budget)?;
    ticket.validate()?;
    Ok((ticket, body))
}

fn verified_source_body(
    objects: &dyn ImmutableObjectPort,
    source: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<StoredObject, IndexError> {
    let record_object = objects.get(source, budget)?;
    if record_object.payload.len() > MAX_STUB_RECORD_BYTES {
        return Err(IndexError::CorruptObject(
            "Native Stub selected source record exceeds its bound".into(),
        ));
    }
    let record =
        SourceRecordOwner::decode_json(&record_object.payload, budget, MemoryLifetime::Temporary)?;
    if record_object.registration.reference != source
        || record_object.registration.children.as_slice() != [record.bytes]
    {
        return Err(IndexError::CorruptObject(
            "Native Stub selected source record registration differs".into(),
        ));
    }
    if record.byte_len
        > u64::try_from(MAX_STUB_SOURCE_BYTES).map_err(|_error| IndexError::ResourceLimited)?
    {
        return Err(IndexError::ResourceLimited);
    }
    let body = objects.get(record.bytes, budget)?;
    if body.registration.reference != record.bytes
        || !body.registration.children.is_empty()
        || u64::try_from(body.payload.len()).map_err(|_error| IndexError::ResourceLimited)?
            != record.byte_len
    {
        return Err(IndexError::CorruptObject(
            "Native Stub selected source body differs".into(),
        ));
    }
    if body.payload.len() > MAX_STUB_SOURCE_BYTES || str::from_utf8(&body.payload).is_err() {
        return Err(IndexError::InvalidInput(
            "Native Stub needs bounded UTF-8 selected source".into(),
        ));
    }
    Ok(body)
}

fn saved_frontier(
    dispatcher: &NativeDispatcher,
    context: &DispatchContextV1,
    binding: &NativeStubBinding,
    budget: &WorkBudget,
) -> Result<(), IndexError> {
    let overlay = dispatcher.overlay.get().ok_or_else(|| {
        IndexError::InvalidInput("Native Stub requires a configured document session".into())
    })?;
    let uid = context
        .principal
        .ok_or_else(|| IndexError::InvalidInput("Native Stub requires peer credentials".into()))?
        .uid;
    let document = OverlayDocumentKey::from_wire(binding.document, uid);
    let (selected, source) =
        overlay.certified_saved_frontier(document, binding.revision, budget)?;
    validate_saved_source_join(selected, source, binding.selected, binding.source)
}

fn validate_saved_source_join(
    saved: quanta_index_core::index::ViewRef,
    source: Option<ObjectRef>,
    selected: quanta_index_core::index::ViewRef,
    expected: ObjectRef,
) -> Result<(), IndexError> {
    // The immutable save receipt stays at its Source CAS view. AST projections
    // may subsequently advance the same workspace/epoch without changing this
    // exact owner/source. selected_source and current_owner_source separately
    // authenticate that owner in both selected and latest Catalog views.
    if source != Some(expected)
        || saved.workspace != selected.workspace
        || saved.epoch != selected.epoch
        || saved.sequence > selected.sequence
        || (saved.sequence == selected.sequence && saved != selected)
    {
        return Err(IndexError::Conflict);
    }
    Ok(())
}

fn current_owner_source(
    dispatcher: &NativeDispatcher,
    binding: &NativeStubBinding,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let ticket = dispatcher.kernel.pin_view(
        ViewSelection::Latest {
            workspace: binding.document.session.workspace,
            epoch: binding.selected.epoch,
        },
        budget,
    )?;
    ticket.validate()?;
    let source = dispatcher.objects.lookup_object(
        ticket.view().source.owners,
        &owner_map_key(binding.document.owner),
        budget,
    )?;
    if source != Some(binding.source) {
        return Err(IndexError::Conflict);
    }
    ticket.validate()?;
    ticket.release()
}

pub(super) fn dispatch(
    dispatcher: &NativeDispatcher,
    context: &DispatchContextV1,
    request: NativeStubRequest,
    request_budget: &RequestBudgetV1,
) -> Result<ScopedStubReply, IndexError> {
    dispatch_bound(dispatcher, context, request, request_budget)
        .map_err(|cause| refusal(StubRefusal::from_selected_view_error(cause)))
}

fn dispatch_bound(
    dispatcher: &NativeDispatcher,
    context: &DispatchContextV1,
    request: NativeStubRequest,
    request_budget: &RequestBudgetV1,
) -> Result<ScopedStubReply, IndexError> {
    let reservation = ReservationGuard::try_acquire(
        Arc::clone(&dispatcher.interactive_scheduler),
        dispatcher.interactive_need,
    )
    .ok_or_else(|| dispatcher.interactive_scheduler.denial())?;
    let mut budget = WorkBudget::with_request(dispatcher.limits, request_budget.clone());
    budget.bind_memory_scope(reservation.memory_scope());
    budget.checkpoint()?;
    let ast: &dyn StubAstCertificatePort = dispatcher
        .stub_ast
        .get()
        .ok_or_else(|| {
            IndexError::InvalidInput("Native Stub AST authority is not configured".into())
        })?
        .as_ref();
    let (binding, items, selected_id, edits, unit) = match request {
        NativeStubRequest::Certify { binding, items } => (binding, items, None, None, None),
        NativeStubRequest::Preflight {
            binding,
            items,
            selected_id,
            edits,
            unit,
        } => (binding, items, Some(selected_id), Some(edits), Some(unit)),
    };
    saved_frontier(dispatcher, context, &binding, &budget)?;
    current_owner_source(dispatcher, &binding, &mut budget)?;
    let (ticket, body) = selected_source(dispatcher, &binding, &mut budget)?;
    let source = str::from_utf8(&body.payload).map_err(|_error| {
        IndexError::InvalidInput("Native Stub selected source is not UTF-8".into())
    })?;
    let (items, _input_memory) = items_from_wire(items, &mut budget)?;
    let context_bytes = checked_bytes(
        1,
        std::mem::size_of::<StubContext>(),
        binding.language.len(),
    )?;
    let _context_memory = admitted_memory(&mut budget, context_bytes, MemoryLifetime::Temporary)?;
    let manifest = certify_manifest(stub_context(&binding), source, &items, ast, &mut budget)
        .map_err(refusal)?;
    let (reply, memory) =
        if let (Some(selected_id), Some(edits), Some(unit)) = (selected_id, edits, unit) {
            let (edits, _edits_memory) = edits_from_wire(&edits, &mut budget)?;
            let unit = match unit {
                NativeStubCoordinateUnit::Utf8Bytes => CoordinateUnit::Utf8Bytes,
                NativeStubCoordinateUnit::Utf16Units => CoordinateUnit::Utf16Units,
            };
            let prepared = preflight_action(
                &manifest,
                source,
                manifest.context(),
                selected_id,
                &edits,
                unit,
                &mut budget,
            )
            .map_err(refusal)?;
            let (edits, memory) = edits_to_wire(prepared.edits(), &binding, &mut budget)?;
            (
                NativeStubReply::Prepared {
                    binding: binding.clone(),
                    selected_id: prepared.selected_id(),
                    edits,
                },
                memory,
            )
        } else {
            let (items, memory) = items_to_wire(manifest.items(), &binding, &mut budget)?;
            (
                NativeStubReply::Certified {
                    binding: binding.clone(),
                    items,
                    action_supported: manifest.action_supported(),
                },
                memory,
            )
        };
    // Both mutable document custody and immutable source custody remain valid
    // through the final preflight. The external consumer must atomically apply
    // the returned edits against this same explicit fence.
    saved_frontier(dispatcher, context, &binding, &budget)?;
    current_owner_source(dispatcher, &binding, &mut budget)?;
    ticket.validate()?;
    budget.checkpoint()?;
    ticket.release()?;
    Ok(ScopedStubReply {
        reply,
        _memory: memory,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        os::unix::fs::PermissionsExt,
        time::{Duration, Instant},
    };

    use quanta_index_contract::index::{SourceRecord, SourceTextAdmission};
    use quanta_index_core::index::WorkLimits;
    use quanta_index_storage::FileObjectStore;

    use super::*;

    #[test]
    fn saved_source_allows_later_projection_view_but_rejects_identity_changes() {
        use quanta_index_core::index::{
            CollectionId, Digest, SlotId, TenantId, ViewRef, WorkspaceKey, WorktreeId,
        };
        let saved = ViewRef {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 5,
            sequence: 6,
            digest: Digest([7; 32]),
        };
        let source = ObjectRef(Digest([8; 32]));
        let later = ViewRef {
            sequence: 7,
            digest: Digest([9; 32]),
            ..saved
        };
        assert!(validate_saved_source_join(saved, Some(source), saved, source).is_ok());
        assert!(validate_saved_source_join(saved, Some(source), later, source).is_ok());
        for selected in [
            ViewRef { epoch: 6, ..later },
            ViewRef {
                sequence: 5,
                ..later
            },
            ViewRef {
                sequence: saved.sequence,
                ..later
            },
            ViewRef {
                workspace: WorkspaceKey {
                    slot: SlotId([10; 16]),
                    ..saved.workspace
                },
                ..later
            },
        ] {
            assert!(matches!(
                validate_saved_source_join(saved, Some(source), selected, source),
                Err(IndexError::Conflict)
            ));
        }
        for wrong in [None, Some(ObjectRef(Digest([11; 32])))] {
            assert!(matches!(
                validate_saved_source_join(saved, wrong, later, source),
                Err(IndexError::Conflict)
            ));
        }
    }

    #[test]
    fn selected_source_requires_direct_record_to_leaf_edge()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
        let objects = FileObjectStore::open(root.path())?;
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?;
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 256,
                writes: 256,
                bytes: 8 << 20,
                mutations: 256,
            },
            deadline,
        );
        let source = b"let name = \"\xF0\x9F\xA6\x80\";\n";
        let body = objects.put(source, &[], &mut budget)?;
        let payload = serde_json::to_vec(&SourceRecord {
            path: "src/main.rs".into(),
            bytes: body.reference,
            byte_len: u64::try_from(source.len())?,
            text: SourceTextAdmission::Excluded,
        })?;
        let certified = objects.put(&payload, &[body.reference], &mut budget)?;
        let read = verified_source_body(&objects, certified.reference, &mut budget)?;
        if read.payload.as_slice() != source {
            return Err(std::io::Error::other("selected source bytes differ").into());
        }
        let forged = objects.put(&payload, &[], &mut budget)?;
        if !matches!(
            verified_source_body(&objects, forged.reference, &mut budget),
            Err(IndexError::CorruptObject(_))
        ) {
            return Err(std::io::Error::other("source record without byte edge accepted").into());
        }
        Ok(())
    }
}
