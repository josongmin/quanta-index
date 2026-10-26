//! Authenticated, bounded headless overlay route over the native control IPC.

use std::{ops::Deref, sync::Arc};

use quanta_index_contract::{
    index::{OperationStatus, SourceOperationMode, UpdateKind, ViewSelection},
    native_protocol::{
        NativeOverlayReply, NativeOverlayRequest, NativeOverlaySaveIntent,
        NativeOverlaySaveReceipt, NativeOverlaySnapshot,
    },
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{
        Digest, IndexError, MemoryLease, MemoryLifetime, ObjectRef, SourceRecordOwner, WorkBudget,
        WorkUsage, decode_owner_mutation_value,
        effort::ReservationGuard,
        owner_map_key,
        ports::{ImmutableObjectPort, OperationJournalPort, PersistentMapPort},
    },
};
use quanta_index_ipc::DispatchContextV1;
use quanta_index_kernel::ReadTicket;
use sha2::{Digest as _, Sha256};

use crate::{
    native::NativeDispatcher,
    native_overlay::{
        BoundSave, OverlayDocumentKey, OverlaySessionKey, SelectedSourceFrontier,
        SourceSaveFrontierPort, SourceSaveIntentPort, path_digest,
    },
};

pub(crate) struct ScopedOverlayReply {
    reply: NativeOverlayReply,
    // The wire reply is destroyed before this retained allocation is refunded.
    _memory: Option<MemoryLease>,
}

impl Deref for ScopedOverlayReply {
    type Target = NativeOverlayReply;

    fn deref(&self) -> &Self::Target {
        &self.reply
    }
}

struct SaveAuthority<'a>(&'a NativeDispatcher);

impl SaveAuthority<'_> {
    fn owner_at(
        &self,
        ticket: &ReadTicket,
        owner: quanta_index_core::index::OwnerKey,
        budget: &mut WorkBudget,
    ) -> Result<Option<ObjectRef>, IndexError> {
        ticket.validate()?;
        self.0
            .objects
            .lookup_object(ticket.view().source.owners, &owner_map_key(owner), budget)
    }

    fn source_content_and_path_digests(
        &self,
        record_ref: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<(Digest, Digest), IndexError> {
        let record_object = self.0.objects.get(record_ref, budget)?;
        let record = SourceRecordOwner::decode_json(
            &record_object.payload,
            budget,
            MemoryLifetime::Temporary,
        )?;
        if record_object.registration.reference != record_ref
            || record_object.registration.children.as_slice() != [record.bytes]
            || !crate::native::valid_native_source_path(&record.path)
        {
            return Err(IndexError::CorruptObject(
                "save source record registration differs".into(),
            ));
        }
        let bytes = self.0.objects.get(record.bytes, budget)?;
        if bytes.registration.reference != record.bytes || !bytes.registration.children.is_empty() {
            return Err(IndexError::CorruptObject(
                "save source bytes are not the selected immutable leaf".into(),
            ));
        }
        if u64::try_from(bytes.payload.len()).map_err(|_error| IndexError::ResourceLimited)?
            != record.byte_len
        {
            return Err(IndexError::CorruptObject(
                "selected source byte length mismatches record".into(),
            ));
        }
        budget.charge(WorkUsage {
            bytes: record
                .byte_len
                .checked_add(
                    u64::try_from(record.path.len())
                        .map_err(|_error| IndexError::ResourceLimited)?,
                )
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let source_path_digest = path_digest(&record.path);
        let mut hasher = Sha256::new();
        hasher.update(b"quanta-index/native-overlay/source-bytes/v1\0");
        hasher.update(&bytes.payload);
        Ok((Digest(hasher.finalize().into()), source_path_digest))
    }
}

impl SourceSaveIntentPort for SaveAuthority<'_> {
    fn identify_save_request(
        &self,
        intent: &NativeOverlaySaveIntent,
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        self.0
            .kernel
            .identify_update_request(&intent.request, budget)
    }

    fn authenticate_save_intent(
        &self,
        intent: &NativeOverlaySaveIntent,
        content_digest: Option<Digest>,
        expected_path_digest: Option<Digest>,
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        let UpdateKind::SourceAdvance(transition) = &intent.request.kind else {
            return Err(IndexError::Conflict);
        };
        let pending_request = match self.0.kernel.status(
            intent.operation(),
            quanta_index_core::index::ports::OperationInspectAccess::Request(budget),
        )? {
            OperationStatus::Published(_) | OperationStatus::Refused(_) => {
                return Err(IndexError::Conflict);
            }
            OperationStatus::Pending(_) => {
                let original = self.0.catalog.operation(intent.operation())?;
                if original.key != intent.operation()
                    || original.source_mode != SourceOperationMode::Advance
                {
                    return Err(IndexError::Conflict);
                }
                Some(original.request)
            }
            OperationStatus::Unknown => None,
        };
        let ticket = self
            .0
            .kernel
            .pin_view(ViewSelection::Exact(intent.base_view()), budget)?;
        if transition.expected_source != ticket.view().source {
            return Err(IndexError::Conflict);
        }
        if self.owner_at(&ticket, intent.document.owner, budget)? != intent.expected_source {
            return Err(IndexError::Conflict);
        }
        let mutation_value = self
            .0
            .objects
            .lookup(
                transition.owners.root,
                &owner_map_key(intent.document.owner),
                budget,
            )?
            .ok_or(IndexError::Conflict)?;
        budget.charge(WorkUsage {
            mutations: 1,
            ..WorkUsage::default()
        })?;
        let mutation = decode_owner_mutation_value(
            intent.document.owner,
            mutation_value,
            self.0.objects.as_ref(),
            budget,
        )?;
        if mutation.owner != intent.document.owner || mutation.expected != intent.expected_source {
            return Err(IndexError::Conflict);
        }
        let next_digests = mutation
            .next
            .map(|next| self.source_content_and_path_digests(next, budget))
            .transpose()?;
        if next_digests.map(|(content, _path)| content) != content_digest
            || expected_path_digest
                .is_some_and(|expected| next_digests.map(|(_content, path)| path) != Some(expected))
        {
            return Err(IndexError::Conflict);
        }
        let request = self.identify_save_request(intent, budget)?;
        if pending_request.is_some_and(|original| original != request) {
            return Err(IndexError::Conflict);
        }
        ticket.validate()?;
        budget.checkpoint()?;
        Ok(request)
    }
}

impl SourceSaveFrontierPort for SaveAuthority<'_> {
    fn authenticate_selected_frontier(
        &self,
        bound: &BoundSave,
        receipt: &NativeOverlaySaveReceipt,
        budget: &mut WorkBudget,
    ) -> Result<SelectedSourceFrontier, IndexError> {
        if receipt.operation != bound.operation
            || receipt.revision != bound.revision
            || receipt.document != bound.document
        {
            return Err(IndexError::Conflict);
        }
        match self.0.kernel.status(
            receipt.operation,
            quanta_index_core::index::ports::OperationInspectAccess::Request(budget),
        )? {
            OperationStatus::Published(view) if view == receipt.selected => {}
            OperationStatus::Published(_)
            | OperationStatus::Refused(_)
            | OperationStatus::Pending(_)
            | OperationStatus::Unknown => return Err(IndexError::Conflict),
        }
        let original = self.0.catalog.operation(receipt.operation)?;
        if original.key != receipt.operation
            || original.source_mode != SourceOperationMode::Advance
            || original.request != bound.request
        {
            return Err(IndexError::Conflict);
        }
        let ticket = self
            .0
            .kernel
            .pin_view(ViewSelection::Exact(receipt.selected), budget)?;
        let source = self.owner_at(&ticket, receipt.document.owner, budget)?;
        if source != receipt.source {
            return Err(IndexError::Conflict);
        }
        let overlay = self
            .0
            .overlay
            .get()
            .ok_or(IndexError::SnapshotUnavailable)?;
        self.0.kernel.validate_source_searchable(
            &self.0.text_queries,
            &ticket,
            overlay.text_instance,
            receipt.document.owner,
            source,
            budget,
        )?;
        let source_digests = source
            .map(|record| self.source_content_and_path_digests(record, budget))
            .transpose()?;
        ticket.validate()?;
        budget.checkpoint()?;
        Ok(SelectedSourceFrontier {
            ticket,
            owner: receipt.document.owner,
            revision: receipt.revision,
            source,
            content_digest: source_digests.map(|(content, _path)| content),
            path_digest: source_digests.map(|(_content, path)| path),
        })
    }
}

pub(super) fn dispatch(
    dispatcher: &NativeDispatcher,
    context: &DispatchContextV1,
    request: NativeOverlayRequest,
    request_budget: &RequestBudgetV1,
) -> Result<ScopedOverlayReply, IndexError> {
    let reservation = ReservationGuard::try_acquire(
        Arc::clone(&dispatcher.interactive_scheduler),
        dispatcher.interactive_need,
    )
    .ok_or_else(|| dispatcher.interactive_scheduler.denial())?;
    let mut budget = WorkBudget::with_request(dispatcher.limits, request_budget.clone());
    budget.bind_memory_scope(reservation.memory_scope());
    budget.checkpoint()?;
    let overlay = dispatcher.overlay.get().ok_or_else(|| {
        IndexError::InvalidInput("Native headless overlay is not configured".into())
    })?;
    let uid = context
        .principal
        .ok_or_else(|| IndexError::InvalidInput("Native overlay requires peer credentials".into()))?
        .uid;
    let mut reply_memory = None;
    let reply = match request {
        NativeOverlayRequest::Begin { session } => {
            overlay.begin_session(
                OverlaySessionKey::from_dispatch(session, context)?,
                &mut budget,
            )?;
            NativeOverlayReply::Begun(session)
        }
        NativeOverlayRequest::Update {
            document,
            expected_revision,
            revision,
            text,
            path,
        } => {
            overlay.update(
                OverlayDocumentKey::from_wire(document, uid),
                expected_revision,
                revision,
                text.as_deref(),
                path.as_deref(),
                &mut budget,
            )?;
            NativeOverlayReply::Updated { document, revision }
        }
        NativeOverlayRequest::Snapshot { document } => {
            let snapshot = overlay
                .snapshot_document(OverlayDocumentKey::from_wire(document, uid), &mut budget)?;
            let overlay_reply = if let Some(snapshot) = snapshot {
                let text_bytes = u64::try_from(snapshot.text.as_ref().map_or(0, |text| text.len()))
                    .map_err(|_error| IndexError::ResourceLimited)?;
                let path_bytes = u64::try_from(snapshot.path.as_ref().map_or(0, |path| path.len()))
                    .map_err(|_error| IndexError::ResourceLimited)?;
                let bytes = text_bytes
                    .checked_add(path_bytes)
                    .ok_or(IndexError::ResourceLimited)?;
                budget.charge(WorkUsage {
                    bytes,
                    ..WorkUsage::default()
                })?;
                reply_memory = if bytes == 0 {
                    None
                } else {
                    budget.reserve_memory(bytes, MemoryLifetime::Retained)?
                };
                if bytes != 0 && reply_memory.is_none() {
                    return Err(IndexError::BackendViolation(
                        "Native overlay reply needs retained memory custody".into(),
                    ));
                }
                let text = snapshot.text.as_deref().map(str::to_owned);
                let path = snapshot.path.as_deref().map(str::to_owned);
                Some(NativeOverlaySnapshot {
                    revision: snapshot.revision,
                    text,
                    path,
                })
            } else {
                None
            };
            NativeOverlayReply::Snapshot {
                document,
                overlay: overlay_reply,
            }
        }
        NativeOverlayRequest::Masks { session } => {
            let snapshot = overlay.snapshot_masks(
                OverlaySessionKey::from_dispatch(session, context)?,
                &mut budget,
            )?;
            reply_memory = Some(snapshot.memory);
            NativeOverlayReply::Masks {
                session,
                masks: snapshot.entries,
            }
        }
        NativeOverlayRequest::Close { session } => {
            overlay.close_session(
                OverlaySessionKey::from_dispatch(session, context)?,
                &mut budget,
            )?;
            NativeOverlayReply::Closed(session)
        }
    };
    budget.checkpoint()?;
    Ok(ScopedOverlayReply {
        reply,
        _memory: reply_memory,
    })
}

/// Bind the original source CAS outside the small overlay request enum. The
/// original request is the sole operation/base-view authority.
pub(super) fn bind_save(
    dispatcher: &NativeDispatcher,
    context: &DispatchContextV1,
    intent: &NativeOverlaySaveIntent,
    request_budget: &RequestBudgetV1,
) -> Result<ScopedOverlayReply, IndexError> {
    let reservation = ReservationGuard::try_acquire(
        Arc::clone(&dispatcher.interactive_scheduler),
        dispatcher.interactive_need,
    )
    .ok_or_else(|| dispatcher.interactive_scheduler.denial())?;
    let mut budget = WorkBudget::with_request(dispatcher.limits, request_budget.clone());
    budget.bind_memory_scope(reservation.memory_scope());
    budget.checkpoint()?;
    let overlay = dispatcher.overlay.get().ok_or_else(|| {
        IndexError::InvalidInput("Native headless overlay is not configured".into())
    })?;
    overlay.bind_save_intent(
        OverlaySessionKey::from_dispatch(intent.document.session, context)?,
        intent,
        &SaveAuthority(dispatcher),
        &mut budget,
    )?;
    budget.checkpoint()?;
    Ok(ScopedOverlayReply {
        reply: NativeOverlayReply::SaveBound {
            document: intent.document,
            revision: intent.revision,
            operation: intent.operation(),
        },
        _memory: None,
    })
}

/// Complete a source save against the published, searchable selected view.
pub(super) fn complete_save(
    dispatcher: &NativeDispatcher,
    context: &DispatchContextV1,
    receipt: &NativeOverlaySaveReceipt,
    request_budget: &RequestBudgetV1,
) -> Result<ScopedOverlayReply, IndexError> {
    let reservation = ReservationGuard::try_acquire(
        Arc::clone(&dispatcher.interactive_scheduler),
        dispatcher.interactive_need,
    )
    .ok_or_else(|| dispatcher.interactive_scheduler.denial())?;
    let mut budget = WorkBudget::with_request(dispatcher.limits, request_budget.clone());
    budget.bind_memory_scope(reservation.memory_scope());
    budget.checkpoint()?;
    let overlay = dispatcher.overlay.get().ok_or_else(|| {
        IndexError::InvalidInput("Native headless overlay is not configured".into())
    })?;
    let selected = overlay.save_handoff(
        OverlaySessionKey::from_dispatch(receipt.document.session, context)?,
        receipt,
        &SaveAuthority(dispatcher),
        &mut budget,
    )?;
    budget.checkpoint()?;
    Ok(ScopedOverlayReply {
        reply: NativeOverlayReply::Saved {
            document: receipt.document,
            revision: receipt.revision,
            selected,
        },
        _memory: None,
    })
}
