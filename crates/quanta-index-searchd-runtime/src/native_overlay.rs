//! Bounded, process-local editor text over a pinned Native source view.
//!
//! This store has no publication path. A save only retires a revision after
//! the existing source-CAS authority certifies its selected frontier.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use quanta_index_contract::native_protocol::{
    NativeInteractionPublisherId, NativeOverlayDocument, NativeOverlayMask,
    NativeOverlaySaveIntent, NativeOverlaySaveReceipt, NativeOverlaySession,
};
use quanta_index_core::RequestBudgetV1;
use quanta_index_core::index::{
    Digest, IndexError, IndexInstanceId, MemoryLease, MemoryLifetime, ObjectRef, OperationKey,
    OwnerKey, ViewRef, WorkBudget, WorkUsage,
};
use quanta_index_ipc::DispatchContextV1;
use quanta_index_kernel::ReadTicket;
use sha2::{Digest as _, Sha256};

const HARD_MAX_SESSIONS: usize = 16;
pub(super) const HARD_MAX_DOCUMENTS: usize = 256;
const HARD_MAX_DOCUMENT_BYTES: usize = 1 << 20;
const HARD_MAX_TOTAL_BYTES: usize = 16 << 20;
// Logical admission for a map insertion covers a whole bounded B-tree node
// and a split, rather than charging only the payload of one key/value pair.
// This is not a claim about allocator RSS.
const BTREE_NODE_SLOTS: usize = 16;
const BTREE_NODE_EDGE_SLOTS: usize = 17;
const BTREE_SPLIT_NODES: usize = 2;
const BTREE_NODE_HEADER_WORDS: usize = 8;

fn map_slot_metadata_bytes<K, V>() -> Result<u64, IndexError> {
    let node_bytes = std::mem::size_of::<(K, V)>()
        .checked_mul(BTREE_NODE_SLOTS)
        .and_then(|bytes| {
            BTREE_NODE_EDGE_SLOTS
                .checked_add(BTREE_NODE_HEADER_WORDS)
                .and_then(|words| std::mem::size_of::<usize>().checked_mul(words))
                .and_then(|edges| bytes.checked_add(edges))
        })
        .and_then(|bytes| bytes.checked_mul(BTREE_SPLIT_NODES))
        .ok_or(IndexError::ResourceLimited)?;
    u64::try_from(node_bytes).map_err(|_error| IndexError::ResourceLimited)
}

fn payload_allocation_bytes(text: &str) -> Result<u64, IndexError> {
    let bytes = text
        .len()
        .checked_add(std::mem::size_of::<Arc<str>>())
        .and_then(|bytes| {
            std::mem::size_of::<usize>()
                .checked_mul(2)
                .and_then(|overhead| bytes.checked_add(overhead))
        })
        .ok_or(IndexError::ResourceLimited)?;
    u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)
}

fn retained_memory(budget: &WorkBudget, bytes: u64) -> Result<MemoryLease, IndexError> {
    budget
        .reserve_memory(bytes, MemoryLifetime::Retained)?
        .ok_or_else(|| {
            IndexError::BackendViolation("Native overlay needs a retained memory scope".into())
        })
}

fn check_request(request: &RequestBudgetV1) -> Result<(), IndexError> {
    if request.is_cancelled() {
        Err(IndexError::Cancelled)
    } else if Instant::now() >= request.deadline() {
        Err(IndexError::ResourceLimited)
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct OverlayLimits {
    pub sessions: usize,
    pub documents: usize,
    pub document_bytes: usize,
    pub total_bytes: usize,
}

impl OverlayLimits {
    pub(super) fn validate(self) -> Result<Self, IndexError> {
        if self.sessions == 0
            || self.sessions > HARD_MAX_SESSIONS
            || self.documents == 0
            || self.documents > HARD_MAX_DOCUMENTS
            || self.document_bytes == 0
            || self.document_bytes > HARD_MAX_DOCUMENT_BYTES
            || self.total_bytes == 0
            || self.total_bytes > HARD_MAX_TOTAL_BYTES
            || self.total_bytes < self.document_bytes
        {
            return Err(IndexError::InvalidInput(
                "Native overlay limits exceed hard caps".into(),
            ));
        }
        Ok(self)
    }
}

/// `epoch` is strictly increasing under one authenticated publisher. It is
/// not derived from a client wall clock or a new unverified identity scheme.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct OverlaySessionKey {
    pub wire: NativeOverlaySession,
    uid: u32,
}

impl OverlaySessionKey {
    pub(super) fn from_dispatch(
        wire: NativeOverlaySession,
        context: &DispatchContextV1,
    ) -> Result<Self, IndexError> {
        let principal = context.principal.ok_or_else(|| {
            IndexError::InvalidInput("Native overlay request has no authenticated peer".into())
        })?;
        Ok(Self {
            wire,
            uid: principal.uid,
        })
    }
}

impl std::ops::Deref for OverlaySessionKey {
    type Target = NativeOverlaySession;

    fn deref(&self) -> &Self::Target {
        &self.wire
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct OverlayDocumentKey {
    pub session: OverlaySessionKey,
    pub owner: OwnerKey,
}

impl OverlayDocumentKey {
    pub(super) fn from_wire(wire: NativeOverlayDocument, uid: u32) -> Self {
        Self {
            session: OverlaySessionKey {
                wire: wire.session,
                uid,
            },
            owner: wire.owner,
        }
    }
}

/// The existing foreground recorder or its authenticated transport adapter
/// must implement this. Checking a claimed publisher ID in request payloads
/// alone does not establish live peer custody.
pub(super) trait OverlayPublisherAuthPort: Send + Sync {
    fn authenticate(
        &self,
        uid: u32,
        publisher: NativeInteractionPublisherId,
    ) -> Result<(), IndexError>;
}

pub(super) trait SourceSaveIntentPort: Send + Sync {
    /// Identify the immutable original request for an already-bound retry.
    /// This does not grant a new save intent after the operation is terminal.
    fn identify_save_request(
        &self,
        intent: &NativeOverlaySaveIntent,
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError>;

    fn authenticate_save_intent(
        &self,
        intent: &NativeOverlaySaveIntent,
        content_digest: Option<Digest>,
        path_digest: Option<Digest>,
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError>;
}

/// The port verifies source-CAS success, not merely an ingress or transport
/// ACK. Its returned ticket retains the exact selected view through handoff.
pub(super) trait SourceSaveFrontierPort: Send + Sync {
    fn authenticate_selected_frontier(
        &self,
        bound: &BoundSave,
        receipt: &NativeOverlaySaveReceipt,
        budget: &mut WorkBudget,
    ) -> Result<SelectedSourceFrontier, IndexError>;
}

pub(super) struct SelectedSourceFrontier {
    pub ticket: ReadTicket,
    pub owner: OwnerKey,
    pub revision: u64,
    pub source: Option<ObjectRef>,
    /// SHA-256 over the exact selected source bytes with the overlay domain.
    pub content_digest: Option<Digest>,
    pub path_digest: Option<Digest>,
}

/// Presence masks the durable owner. `text=None` is an unsaved deletion;
/// absence of a snapshot means no overlay for that owner/session.
#[derive(Clone)]
pub(super) struct OverlaySnapshot {
    pub revision: u64,
    pub text: Option<Arc<str>>,
    pub path: Option<Arc<str>>,
    _memory: Option<MemoryLease>,
    _path_memory: Option<MemoryLease>,
}

pub(super) struct OverlayMaskSnapshot {
    pub entries: Vec<NativeOverlayMask>,
    pub memory: MemoryLease,
}

pub(super) struct OverlayCorpusDocument {
    pub owner: OwnerKey,
    pub revision: u64,
    pub retired: bool,
    pub text: Option<Arc<str>>,
    pub path: Option<Arc<str>>,
    changed_at: Instant,
    _text_memory: Option<MemoryLease>,
    _path_memory: Option<MemoryLease>,
}

pub(super) struct OverlayCorpusSnapshot {
    pub session: OverlaySessionKey,
    pub documents: Vec<OverlayCorpusDocument>,
    _metadata_memory: MemoryLease,
    pub identity: Digest,
}

struct DocumentEntry {
    revision: u64,
    text: Option<Arc<str>>,
    path: Option<Arc<str>>,
    bytes: usize,
    save_binding: Option<BoundSave>,
    retired: bool,
    saved_receipt: Option<NativeOverlaySaveReceipt>,
    changed_at: Instant,
    // Slot metadata survives Saved to fence stale revisions until Close.
    _metadata_memory: MemoryLease,
    // Destroy text before refunding its separately admitted Arc allocation.
    payload_memory: Option<MemoryLease>,
    path_memory: Option<MemoryLease>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BoundSave {
    pub document: NativeOverlayDocument,
    pub revision: u64,
    pub operation: OperationKey,
    pub request: ObjectRef,
    expected_source: Option<ObjectRef>,
    content_digest: Option<Digest>,
    pub(super) path_digest: Option<Digest>,
}

impl BoundSave {
    fn matches_intent(
        self,
        intent: &NativeOverlaySaveIntent,
        request: ObjectRef,
        digest: Option<Digest>,
        path_digest: Option<Digest>,
    ) -> bool {
        self.document == intent.document
            && self.revision == intent.revision
            && self.operation == intent.operation()
            && self.request == request
            && self.expected_source == intent.expected_source
            && self.content_digest == digest
            && self.path_digest == path_digest
    }
}

fn content_digest(text: &str) -> Digest {
    let mut hasher = Sha256::new();
    hasher.update(b"quanta-index/native-overlay/source-bytes/v1\0");
    hasher.update(text.as_bytes());
    Digest(hasher.finalize().into())
}

#[must_use]
pub(super) fn path_digest(path: &str) -> Digest {
    let mut hasher = Sha256::new();
    hasher.update(b"quanta-index/native-overlay/source-path/v1\0");
    hasher.update(path.as_bytes());
    Digest(hasher.finalize().into())
}

fn corpus_identity(
    session: NativeOverlaySession,
    documents: &[OverlayCorpusDocument],
) -> Result<Digest, IndexError> {
    let mut hasher = Sha256::new();
    hasher.update(b"quanta-index/native-overlay/corpus/v1\0");
    hasher.update(session.workspace.tenant.0);
    hasher.update(session.workspace.collection.0);
    hasher.update(session.workspace.worktree.0);
    hasher.update(session.workspace.slot.0);
    hasher.update(session.publisher.0);
    hasher.update(session.epoch.to_be_bytes());
    hasher.update(
        u64::try_from(documents.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .to_be_bytes(),
    );
    for row in documents {
        hasher.update(row.owner.producer.0);
        hasher.update(row.owner.stable_id.0);
        hasher.update(row.revision.to_be_bytes());
        hasher.update([u8::from(row.retired)]);
        for value in [row.text.as_deref(), row.path.as_deref()] {
            if let Some(value) = value {
                hasher.update([1]);
                hasher.update(
                    u64::try_from(value.len())
                        .map_err(|_error| IndexError::ResourceLimited)?
                        .to_be_bytes(),
                );
                hasher.update(value.as_bytes());
            } else {
                hasher.update([0]);
            }
        }
    }
    Ok(Digest(hasher.finalize().into()))
}

fn same_arc(left: Option<&Arc<str>>, right: Option<&Arc<str>>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => Arc::ptr_eq(left, right),
        (None, None) => true,
        (Some(_), None) | (None, Some(_)) => false,
    }
}

struct Session {
    key: OverlaySessionKey,
    documents: BTreeMap<OwnerKey, DocumentEntry>,
    _metadata_memory: MemoryLease,
}

#[derive(Default)]
struct State {
    publisher: Option<(u32, NativeInteractionPublisherId)>,
    max_epoch: u64,
    sessions: BTreeMap<u64, Session>,
    documents: usize,
    bytes: usize,
}

pub(super) struct NativeOverlay {
    pub(super) text_instance: IndexInstanceId,
    limits: OverlayLimits,
    publisher: Arc<dyn OverlayPublisherAuthPort>,
    state: Mutex<State>,
}

impl NativeOverlay {
    pub(super) fn new(
        limits: OverlayLimits,
        publisher: Arc<dyn OverlayPublisherAuthPort>,
        text_instance: IndexInstanceId,
    ) -> Result<Self, IndexError> {
        if text_instance.0 == [0; 16] {
            return Err(IndexError::InvalidInput(
                "Native overlay text instance is zero".into(),
            ));
        }
        Ok(Self {
            text_instance,
            limits: limits.validate()?,
            publisher,
            state: Mutex::new(State::default()),
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, State>, IndexError> {
        self.state
            .lock()
            .map_err(|_error| IndexError::Storage("Native overlay lock poisoned".into()))
    }

    fn check_publisher(
        &self,
        key: OverlaySessionKey,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        self.check_publisher_request(key, budget.request())?;
        budget.checkpoint()
    }

    fn check_publisher_request(
        &self,
        key: OverlaySessionKey,
        request: &RequestBudgetV1,
    ) -> Result<(), IndexError> {
        check_request(request)?;
        if key.epoch == 0 || key.publisher.0 == [0; 32] {
            return Err(IndexError::InvalidInput(
                "Native overlay session identity is zero".into(),
            ));
        }
        if let Err(refusal) = self.publisher.authenticate(key.uid, key.publisher) {
            let mut state = self.lock()?;
            if state.publisher == Some((key.uid, key.publisher)) {
                state.sessions.clear();
                state.documents = 0;
                state.bytes = 0;
                state.publisher = None;
            }
            drop(state);
            return Err(refusal);
        }
        check_request(request)
    }

    fn session(state: &State, key: OverlaySessionKey) -> Result<&Session, IndexError> {
        let session = state.sessions.get(&key.epoch).ok_or(IndexError::Conflict)?;
        if session.key != key {
            return Err(IndexError::Conflict);
        }
        Ok(session)
    }

    pub(super) fn begin_session(
        &self,
        key: OverlaySessionKey,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.check_publisher(key, budget)?;
        let metadata_bytes = map_slot_metadata_bytes::<u64, Session>()?;
        budget.charge(WorkUsage {
            bytes: metadata_bytes,
            mutations: 1,
            ..WorkUsage::default()
        })?;
        let metadata_memory = retained_memory(budget, metadata_bytes)?;
        budget.checkpoint()?;
        let mut state = self.lock()?;
        if state.publisher != Some((key.uid, key.publisher)) {
            // The external publisher validator has certified a new live peer;
            // all previous sessions and their retained text lose authority.
            state.sessions.clear();
            state.documents = 0;
            state.bytes = 0;
            state.max_epoch = 0;
            state.publisher = Some((key.uid, key.publisher));
        }
        if key.epoch <= state.max_epoch {
            return Err(IndexError::Conflict);
        }
        if state.sessions.len() >= self.limits.sessions {
            return Err(IndexError::ResourceLimited);
        }
        state.max_epoch = key.epoch;
        let prior = state.sessions.insert(
            key.epoch,
            Session {
                key,
                documents: BTreeMap::new(),
                _metadata_memory: metadata_memory,
            },
        );
        if prior.is_some() {
            return Err(IndexError::BackendViolation(
                "Native overlay session epoch reused".into(),
            ));
        }
        drop(state);
        Ok(())
    }

    /// Prepare retained backing under the original request's memory scope
    /// before changing any visible slot. `None` represents an unsaved delete.
    pub(super) fn update(
        &self,
        document: OverlayDocumentKey,
        expected_revision: Option<u64>,
        revision: u64,
        text: Option<&str>,
        path: Option<&str>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.check_publisher(document.session, budget)?;
        if revision == 0 || expected_revision.is_some_and(|previous| revision <= previous) {
            return Err(IndexError::InvalidInput(
                "Native overlay revision must advance".into(),
            ));
        }
        if path.is_some_and(|value| !crate::native::valid_native_source_path(value))
            || (text.is_none() && path.is_some())
        {
            return Err(IndexError::InvalidInput(
                "Native overlay path is invalid for this revision".into(),
            ));
        }
        let text_bytes = text.map_or(0, str::len);
        if text_bytes > self.limits.document_bytes {
            return Err(IndexError::ResourceLimited);
        }
        let bytes = text_bytes
            .checked_add(path.map_or(0, str::len))
            .ok_or(IndexError::ResourceLimited)?;
        let new_slot = {
            let state = self.lock()?;
            let previous = Self::session(&state, document.session)?
                .documents
                .get(&document.owner);
            if previous.map(|entry| entry.revision) != expected_revision {
                return Err(IndexError::Conflict);
            }
            let new_slot = previous.is_none();
            drop(state);
            new_slot
        };
        let metadata_bytes = if new_slot {
            map_slot_metadata_bytes::<OwnerKey, DocumentEntry>()?
        } else {
            0
        };
        let payload_bytes = text.map(payload_allocation_bytes).transpose()?.unwrap_or(0);
        let path_payload_bytes = path.map(payload_allocation_bytes).transpose()?.unwrap_or(0);
        budget.charge(WorkUsage {
            bytes: metadata_bytes
                .checked_add(payload_bytes)
                .and_then(|bytes| bytes.checked_add(path_payload_bytes))
                .ok_or(IndexError::ResourceLimited)?,
            mutations: 1,
            ..WorkUsage::default()
        })?;
        let metadata_memory = if new_slot {
            Some(retained_memory(budget, metadata_bytes)?)
        } else {
            None
        };
        let payload_memory = if payload_bytes != 0 {
            Some(retained_memory(budget, payload_bytes)?)
        } else {
            None
        };
        let path_memory = if path_payload_bytes != 0 {
            Some(retained_memory(budget, path_payload_bytes)?)
        } else {
            None
        };
        let owned = text.map(Arc::<str>::from);
        let owned_path = path.map(Arc::<str>::from);
        budget.checkpoint()?;
        let mut state = self.lock()?;
        let session = Self::session(&state, document.session)?;
        let previous = session.documents.get(&document.owner);
        if previous.map(|entry| entry.revision) != expected_revision {
            return Err(IndexError::Conflict);
        }
        let old_bytes = previous.map_or(0, |entry| entry.bytes);
        let additional_document = usize::from(previous.is_none());
        let next_documents = state
            .documents
            .checked_add(additional_document)
            .ok_or(IndexError::ResourceLimited)?;
        let next_bytes = state
            .bytes
            .checked_sub(old_bytes)
            .and_then(|current| current.checked_add(bytes))
            .ok_or(IndexError::ResourceLimited)?;
        if next_documents > self.limits.documents || next_bytes > self.limits.total_bytes {
            return Err(IndexError::ResourceLimited);
        }
        budget.checkpoint()?;
        let session = state
            .sessions
            .get_mut(&document.session.epoch)
            .ok_or(IndexError::Conflict)?;
        let changed_at = Instant::now();
        let previous_payload = if let Some(entry) = session.documents.get_mut(&document.owner) {
            let old_text = std::mem::replace(&mut entry.text, owned);
            let old_memory = std::mem::replace(&mut entry.payload_memory, payload_memory);
            let old_path = std::mem::replace(&mut entry.path, owned_path);
            let old_path_memory = std::mem::replace(&mut entry.path_memory, path_memory);
            entry.revision = revision;
            entry.bytes = bytes;
            entry.save_binding = None;
            entry.retired = false;
            entry.saved_receipt = None;
            entry.changed_at = changed_at;
            Some((old_text, old_path, old_memory, old_path_memory))
        } else {
            let metadata_memory = metadata_memory.ok_or_else(|| {
                IndexError::BackendViolation("Native overlay slot lacks metadata grant".into())
            })?;
            let previous = session.documents.insert(
                document.owner,
                DocumentEntry {
                    revision,
                    text: owned,
                    path: owned_path,
                    bytes,
                    save_binding: None,
                    retired: false,
                    saved_receipt: None,
                    changed_at,
                    _metadata_memory: metadata_memory,
                    payload_memory,
                    path_memory,
                },
            );
            if previous.is_some() {
                return Err(IndexError::BackendViolation(
                    "Native overlay slot appeared during insert".into(),
                ));
            }
            None
        };
        state.documents = next_documents;
        state.bytes = next_bytes;
        drop(state);
        drop(previous_payload);
        Ok(())
    }

    /// A query holds the returned Arc and retained lease until its result
    /// assembly ends. It must mask a selected durable owner before merging.
    pub(super) fn snapshot_document(
        &self,
        document: OverlayDocumentKey,
        budget: &mut WorkBudget,
    ) -> Result<Option<OverlaySnapshot>, IndexError> {
        self.check_publisher(document.session, budget)?;
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let state = self.lock()?;
        let entry = Self::session(&state, document.session)?
            .documents
            .get(&document.owner)
            .filter(|entry| !entry.retired);
        let snapshot = entry.map(|entry| OverlaySnapshot {
            revision: entry.revision,
            text: entry.text.clone(),
            path: entry.path.clone(),
            _memory: entry.payload_memory.clone(),
            _path_memory: entry.path_memory.clone(),
        });
        drop(state);
        Ok(snapshot)
    }

    pub(super) fn snapshot_masks(
        &self,
        key: OverlaySessionKey,
        budget: &mut WorkBudget,
    ) -> Result<OverlayMaskSnapshot, IndexError> {
        self.check_publisher(key, budget)?;
        let count = {
            let state = self.lock()?;
            Self::session(&state, key)?
                .documents
                .values()
                .filter(|entry| !entry.retired)
                .count()
        };
        let bytes = u64::try_from(count)
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(96)
            .and_then(|bytes| bytes.checked_add(128))
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            reads: 1,
            bytes,
            ..WorkUsage::default()
        })?;
        let memory = budget
            .reserve_memory(bytes, MemoryLifetime::Retained)?
            .ok_or_else(|| {
                IndexError::BackendViolation(
                    "Native overlay mask snapshot needs a retained memory scope".into(),
                )
            })?;
        let state = self.lock()?;
        let session = Self::session(&state, key)?;
        if session
            .documents
            .values()
            .filter(|entry| !entry.retired)
            .count()
            != count
        {
            return Err(IndexError::Conflict);
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(count)
            .map_err(|_error| IndexError::ResourceLimited)?;
        for (owner, entry) in &session.documents {
            if !entry.retired {
                entries.push(NativeOverlayMask {
                    owner: *owner,
                    revision: entry.revision,
                    deleted: entry.text.is_none(),
                });
            }
        }
        drop(state);
        Ok(OverlayMaskSnapshot { entries, memory })
    }

    /// Capture every document slot, including retired revision watermarks,
    /// under one state lock. Active `text=None` rows are deletion masks.
    pub(super) fn snapshot_corpus(
        &self,
        key: OverlaySessionKey,
        budget: &mut WorkBudget,
    ) -> Result<OverlayCorpusSnapshot, IndexError> {
        self.check_publisher(key, budget)?;
        let capacity_bytes = self
            .limits
            .documents
            .checked_mul(std::mem::size_of::<OverlayCorpusDocument>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Vec<OverlayCorpusDocument>>()))
            .ok_or(IndexError::ResourceLimited)?;
        let memory_bytes =
            u64::try_from(capacity_bytes).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            reads: 1,
            bytes: memory_bytes,
            ..WorkUsage::default()
        })?;
        let metadata_memory = retained_memory(budget, memory_bytes)?;
        let mut documents = Vec::new();
        documents
            .try_reserve_exact(self.limits.documents)
            .map_err(|_error| IndexError::ResourceLimited)?;
        budget.checkpoint()?;
        let state = self.lock()?;
        let session = Self::session(&state, key)?;
        for (owner, entry) in &session.documents {
            if documents.len() >= self.limits.documents {
                return Err(IndexError::BackendViolation(
                    "Native overlay corpus exceeds admitted document slots".into(),
                ));
            }
            documents.push(OverlayCorpusDocument {
                owner: *owner,
                revision: entry.revision,
                retired: entry.retired,
                text: entry.text.clone(),
                path: entry.path.clone(),
                changed_at: entry.changed_at,
                _text_memory: entry.payload_memory.clone(),
                _path_memory: entry.path_memory.clone(),
            });
        }
        drop(state);
        let hash_bytes = documents.iter().try_fold(0_u64, |total, row| {
            let text_bytes = u64::try_from(row.text.as_ref().map_or(0, |text| text.len()))
                .map_err(|_error| IndexError::ResourceLimited)?;
            let path_bytes = u64::try_from(row.path.as_ref().map_or(0, |path| path.len()))
                .map_err(|_error| IndexError::ResourceLimited)?;
            total
                .checked_add(text_bytes)
                .and_then(|bytes| bytes.checked_add(path_bytes))
                .and_then(|bytes| bytes.checked_add(128))
                .ok_or(IndexError::ResourceLimited)
        })?;
        budget.charge(WorkUsage {
            bytes: hash_bytes,
            ..WorkUsage::default()
        })?;
        budget.checkpoint()?;
        let identity = corpus_identity(key.wire, &documents)?;
        Ok(OverlayCorpusSnapshot {
            session: key,
            documents,
            _metadata_memory: metadata_memory,
            identity,
        })
    }

    /// A captured corpus is usable only while the same authenticated session
    /// still owns exactly these owner/revision/deletion/path slots.
    pub(super) fn validate_corpus(
        &self,
        snapshot: &OverlayCorpusSnapshot,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        self.validate_corpus_request(snapshot, budget.request())?;
        budget.checkpoint()
    }

    pub(super) fn validate_corpus_request(
        &self,
        snapshot: &OverlayCorpusSnapshot,
        request: &RequestBudgetV1,
    ) -> Result<(), IndexError> {
        self.check_publisher_request(snapshot.session, request)?;
        let state = self.lock()?;
        let session = Self::session(&state, snapshot.session)?;
        if session.documents.len() != snapshot.documents.len() {
            return Err(IndexError::Conflict);
        }
        for ((owner, entry), captured) in session.documents.iter().zip(&snapshot.documents) {
            check_request(request)?;
            if *owner != captured.owner
                || entry.revision != captured.revision
                || entry.retired != captured.retired
                || !same_arc(entry.text.as_ref(), captured.text.as_ref())
                || !same_arc(entry.path.as_ref(), captured.path.as_ref())
                || entry.changed_at != captured.changed_at
            {
                return Err(IndexError::Conflict);
            }
        }
        drop(state);
        check_request(request)
    }

    /// Return the age of the youngest row in this exact live corpus. A corpus
    /// without an active document has no document-local idle observation.
    pub(super) fn corpus_unchanged_for_request(
        &self,
        snapshot: &OverlayCorpusSnapshot,
        request: &RequestBudgetV1,
    ) -> Result<Option<Duration>, IndexError> {
        self.check_publisher_request(snapshot.session, request)?;
        let state = self.lock()?;
        let session = Self::session(&state, snapshot.session)?;
        if session.documents.len() != snapshot.documents.len() {
            return Err(IndexError::Conflict);
        }
        let now = Instant::now();
        let mut minimum = None::<Duration>;
        let mut active = false;
        for ((owner, entry), captured) in session.documents.iter().zip(&snapshot.documents) {
            check_request(request)?;
            if *owner != captured.owner
                || entry.revision != captured.revision
                || entry.retired != captured.retired
                || !same_arc(entry.text.as_ref(), captured.text.as_ref())
                || !same_arc(entry.path.as_ref(), captured.path.as_ref())
                || entry.changed_at != captured.changed_at
            {
                return Err(IndexError::Conflict);
            }
            active |= !entry.retired;
            let age = now.saturating_duration_since(entry.changed_at);
            minimum = Some(minimum.map_or(age, |prior| prior.min(age)));
        }
        drop(state);
        check_request(request)?;
        Ok(if active { minimum } else { None })
    }

    pub(super) fn bind_save_intent(
        &self,
        key: OverlaySessionKey,
        intent: &NativeOverlaySaveIntent,
        authority: &dyn SourceSaveIntentPort,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.check_publisher(key, budget)?;
        if intent.document.session != key.wire
            || intent.operation().workspace != key.workspace
            || intent.operation().producer != intent.document.owner.producer
            || intent.base_view().workspace != key.workspace
        {
            return Err(IndexError::Conflict);
        }
        let (expected_digest, expected_path_digest, already_bound) = {
            let state = self.lock()?;
            let entry = Self::session(&state, key)?
                .documents
                .get(&intent.document.owner)
                .ok_or(IndexError::Conflict)?;
            if entry.revision != intent.revision {
                return Err(IndexError::Conflict);
            }
            if entry.retired != entry.saved_receipt.is_some() {
                return Err(IndexError::BackendViolation(
                    "Native overlay saved state lacks its receipt".into(),
                ));
            }
            let (digest, path) = if entry.retired {
                let bound = entry.save_binding.ok_or_else(|| {
                    IndexError::BackendViolation(
                        "Saved overlay slot lacks its original binding".into(),
                    )
                })?;
                (bound.content_digest, bound.path_digest)
            } else {
                (
                    entry.text.as_deref().map(content_digest),
                    entry.path.as_deref().map(path_digest),
                )
            };
            let already_bound = entry.save_binding.is_some();
            drop(state);
            (digest, path, already_bound)
        };
        let request = if already_bound {
            authority.identify_save_request(intent, budget)?
        } else {
            authority.authenticate_save_intent(
                intent,
                expected_digest,
                expected_path_digest,
                budget,
            )?
        };
        budget.checkpoint()?;
        let mut state = self.lock()?;
        let entry = state
            .sessions
            .get_mut(&key.epoch)
            .filter(|session| session.key == key)
            .and_then(|session| session.documents.get_mut(&intent.document.owner))
            .ok_or(IndexError::Conflict)?;
        if entry.revision != intent.revision {
            return Err(IndexError::Conflict);
        }
        if entry.retired != entry.saved_receipt.is_some() {
            return Err(IndexError::BackendViolation(
                "Native overlay saved state lacks its receipt".into(),
            ));
        }
        let current_digest = if entry.retired {
            entry
                .save_binding
                .map(|bound| (bound.content_digest, bound.path_digest))
        } else {
            Some((
                entry.text.as_deref().map(content_digest),
                entry.path.as_deref().map(path_digest),
            ))
        };
        if current_digest != Some((expected_digest, expected_path_digest)) {
            return Err(IndexError::Conflict);
        }
        if let Some(bound) = entry.save_binding {
            let same = bound.matches_intent(intent, request, expected_digest, expected_path_digest);
            drop(state);
            if !same {
                return Err(IndexError::Conflict);
            }
            return Ok(());
        }
        if entry.retired {
            return Err(IndexError::Conflict);
        }
        entry.save_binding = Some(BoundSave {
            document: intent.document,
            revision: intent.revision,
            operation: intent.operation(),
            request,
            expected_source: intent.expected_source,
            content_digest: expected_digest,
            path_digest: expected_path_digest,
        });
        drop(state);
        Ok(())
    }

    /// An ACK alone never retires an overlay. The source-CAS adapter must
    /// authenticate the operation and return an exact selected frontier.
    pub(super) fn save_handoff(
        &self,
        key: OverlaySessionKey,
        receipt: &NativeOverlaySaveReceipt,
        frontier: &dyn SourceSaveFrontierPort,
        budget: &mut WorkBudget,
    ) -> Result<ViewRef, IndexError> {
        self.check_publisher(key, budget)?;
        if receipt.document.session != key.wire
            || receipt.operation.workspace != key.workspace
            || receipt.operation.producer != receipt.document.owner.producer
            || receipt.selected.workspace != key.workspace
        {
            return Err(IndexError::Conflict);
        }
        let bound = {
            let state = self.lock()?;
            let entry = Self::session(&state, key)?
                .documents
                .get(&receipt.document.owner)
                .ok_or(IndexError::Conflict)?;
            if entry.revision != receipt.revision {
                return Err(IndexError::Conflict);
            }
            let bound = entry.save_binding.ok_or(IndexError::Conflict)?;
            if bound.document != receipt.document
                || bound.operation != receipt.operation
                || bound.revision != receipt.revision
            {
                return Err(IndexError::Conflict);
            }
            if entry.retired {
                if entry.saved_receipt != Some(*receipt) {
                    return Err(IndexError::Conflict);
                }
                drop(state);
                budget.checkpoint()?;
                return Ok(receipt.selected);
            }
            drop(state);
            bound
        };
        let witness = frontier.authenticate_selected_frontier(&bound, receipt, budget)?;
        witness.ticket.validate()?;
        if witness.ticket.view().identity != receipt.selected
            || witness.owner != receipt.document.owner
            || witness.revision != receipt.revision
            || witness.source != receipt.source
            || witness.content_digest != bound.content_digest
            || bound
                .path_digest
                .is_some_and(|expected| witness.path_digest != Some(expected))
        {
            return Err(IndexError::BackendViolation(
                "Native overlay selected source frontier mismatched receipt".into(),
            ));
        }
        self.retire_verified(key, receipt, &bound, budget)?;
        Ok(witness.ticket.view().identity)
    }

    // Only the authenticated selected-frontier path calls this in production.
    // Retain the slot's revision watermark after releasing its payload.
    fn retire_verified(
        &self,
        key: OverlaySessionKey,
        receipt: &NativeOverlaySaveReceipt,
        bound: &BoundSave,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        let mut state = self.lock()?;
        let current_bytes = state.bytes;
        let session = state
            .sessions
            .get_mut(&key.epoch)
            .ok_or(IndexError::Conflict)?;
        if session.key != key {
            return Err(IndexError::Conflict);
        }
        let entry = session
            .documents
            .get(&receipt.document.owner)
            .ok_or(IndexError::Conflict)?;
        if entry.revision != receipt.revision || entry.save_binding != Some(*bound) {
            return Err(IndexError::Conflict);
        }
        if entry.retired {
            let same = entry.saved_receipt == Some(*receipt);
            drop(state);
            if !same {
                return Err(IndexError::Conflict);
            }
            return Ok(());
        }
        let next_bytes = current_bytes.checked_sub(entry.bytes).ok_or_else(|| {
            IndexError::BackendViolation("Native overlay byte count underflow".into())
        })?;
        let entry = session
            .documents
            .get_mut(&receipt.document.owner)
            .ok_or(IndexError::Conflict)?;
        let old_text = entry.text.take();
        let old_path = entry.path.take();
        let old_memory = entry.payload_memory.take();
        let old_path_memory = entry.path_memory.take();
        entry.bytes = 0;
        entry.retired = true;
        entry.saved_receipt = Some(*receipt);
        entry.changed_at = Instant::now();
        state.bytes = next_bytes;
        drop(state);
        drop(old_text);
        drop(old_path);
        drop(old_memory);
        drop(old_path_memory);
        Ok(())
    }

    pub(super) fn close_session(
        &self,
        key: OverlaySessionKey,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.check_publisher(key, budget)?;
        budget.charge(WorkUsage {
            mutations: 1,
            ..WorkUsage::default()
        })?;
        let mut state = self.lock()?;
        let session = Self::session(&state, key)?;
        let documents = session.documents.len();
        let bytes = session
            .documents
            .values()
            .map(|entry| entry.bytes)
            .sum::<usize>();
        let next_documents = state.documents.checked_sub(documents).ok_or_else(|| {
            IndexError::BackendViolation("Native overlay document count underflow".into())
        })?;
        let next_bytes = state.bytes.checked_sub(bytes).ok_or_else(|| {
            IndexError::BackendViolation("Native overlay byte count underflow".into())
        })?;
        let _removed = state
            .sessions
            .remove(&key.epoch)
            .ok_or(IndexError::Conflict)?;
        state.documents = next_documents;
        state.bytes = next_bytes;
        drop(state);
        Ok(())
    }

    /// Return the authenticated saved frontier of one live session revision.
    /// Unsaved text has no immutable source authority for action certification.
    pub(super) fn certified_saved_frontier(
        &self,
        document: OverlayDocumentKey,
        revision: u64,
        budget: &WorkBudget,
    ) -> Result<(ViewRef, Option<ObjectRef>), IndexError> {
        self.check_publisher(document.session, budget)?;
        let state = self.lock()?;
        let entry = Self::session(&state, document.session)?
            .documents
            .get(&document.owner)
            .ok_or(IndexError::Conflict)?;
        if !entry.retired || entry.revision != revision {
            return Err(IndexError::Conflict);
        }
        let receipt = entry.saved_receipt.ok_or_else(|| {
            IndexError::BackendViolation("Saved overlay slot lacks its certified frontier".into())
        })?;
        drop(state);
        Ok((receipt.selected, receipt.source))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::{
        RequestBudgetV1,
        index::{
            CollectionId, Digest, MapRoot, MemoryLeaseOwner, MemoryScope, MemoryScopePort,
            OperationKey, OwnerMutationSet, ProducerId, SlotId, SourceCut, SourceTransition,
            StableOwnerId, StreamId, TenantId, UpdateKind, UpdateRequest, WorkLimits, WorkspaceKey,
            WorktreeId,
        },
    };
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };

    macro_rules! check {
        ($condition:expr) => {
            if !$condition {
                return Err(IndexError::BackendViolation(format!(
                    "overlay test condition failed at {}:{}: {}",
                    file!(),
                    line!(),
                    stringify!($condition)
                )));
            }
        };
    }

    #[derive(Default)]
    struct LiveBytes(AtomicU64);
    impl MemoryLeaseOwner for LiveBytes {
        fn release(&self, bytes: u64) {
            let _previous = self.0.fetch_sub(bytes, Ordering::SeqCst);
        }
    }
    struct Scope(Arc<LiveBytes>);
    impl MemoryScopePort for Scope {
        fn reserve(
            &self,
            bytes: u64,
            _lifetime: MemoryLifetime,
        ) -> Result<MemoryLease, IndexError> {
            let _previous = self.0.0.fetch_add(bytes, Ordering::SeqCst);
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    struct Auth;
    impl OverlayPublisherAuthPort for Auth {
        fn authenticate(
            &self,
            uid: u32,
            publisher: NativeInteractionPublisherId,
        ) -> Result<(), IndexError> {
            if uid == 7 && publisher == NativeInteractionPublisherId([8; 32]) {
                Ok(())
            } else {
                Err(IndexError::Conflict)
            }
        }
    }

    fn workspace() -> WorkspaceKey {
        WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        }
    }
    fn key(epoch: u64) -> OverlaySessionKey {
        OverlaySessionKey {
            wire: NativeOverlaySession {
                workspace: workspace(),
                publisher: NativeInteractionPublisherId([8; 32]),
                epoch,
            },
            uid: 7,
        }
    }
    fn document(epoch: u64) -> OverlayDocumentKey {
        OverlayDocumentKey {
            session: key(epoch),
            owner: OwnerKey {
                producer: ProducerId([5; 16]),
                stable_id: StableOwnerId([6; 16]),
            },
        }
    }
    fn budget(scope: Arc<dyn MemoryScopePort>) -> WorkBudget {
        let mut work = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        work.bind_memory_scope(Some(MemoryScope::new(scope)));
        work
    }
    fn overlay() -> NativeOverlay {
        NativeOverlay::new(
            OverlayLimits {
                sessions: 2,
                documents: 2,
                document_bytes: 32,
                total_bytes: 64,
            },
            Arc::new(Auth),
            IndexInstanceId([31; 16]),
        )
        .expect("valid limits")
    }

    struct RefusingFrontier;
    impl SourceSaveFrontierPort for RefusingFrontier {
        fn authenticate_selected_frontier(
            &self,
            _bound: &BoundSave,
            _receipt: &NativeOverlaySaveReceipt,
            _budget: &mut WorkBudget,
        ) -> Result<SelectedSourceFrontier, IndexError> {
            Err(IndexError::Conflict)
        }
    }

    struct AcceptingIntent;
    impl SourceSaveIntentPort for AcceptingIntent {
        fn identify_save_request(
            &self,
            original: &NativeOverlaySaveIntent,
            _budget: &mut WorkBudget,
        ) -> Result<ObjectRef, IndexError> {
            if original.request != intent(&receipt(document(1), 1)).request {
                return Err(IndexError::Conflict);
            }
            Ok(ObjectRef(Digest([12; 32])))
        }

        fn authenticate_save_intent(
            &self,
            _intent: &NativeOverlaySaveIntent,
            _content_digest: Option<Digest>,
            _path_digest: Option<Digest>,
            _budget: &mut WorkBudget,
        ) -> Result<ObjectRef, IndexError> {
            Ok(ObjectRef(Digest([12; 32])))
        }
    }

    fn receipt(document: OverlayDocumentKey, revision: u64) -> NativeOverlaySaveReceipt {
        NativeOverlaySaveReceipt {
            operation: OperationKey {
                workspace: workspace(),
                epoch: 1,
                producer: document.owner.producer,
                stream: StreamId([9; 16]),
                stream_epoch: 1,
                sequence: 1,
            },
            document: NativeOverlayDocument {
                session: document.session.wire,
                owner: document.owner,
            },
            revision,
            source: Some(ObjectRef(Digest([10; 32]))),
            selected: ViewRef {
                workspace: workspace(),
                epoch: 1,
                sequence: 1,
                digest: Digest([11; 32]),
            },
        }
    }

    fn intent(receipt: &NativeOverlaySaveReceipt) -> NativeOverlaySaveIntent {
        NativeOverlaySaveIntent {
            document: receipt.document,
            revision: receipt.revision,
            expected_source: None,
            request: UpdateRequest {
                operation: receipt.operation,
                expected_view: receipt.selected,
                kind: UpdateKind::SourceAdvance(SourceTransition {
                    expected_source: SourceCut {
                        owners: MapRoot(None),
                        paths: MapRoot(None),
                        frontiers: MapRoot(None),
                    },
                    owners: OwnerMutationSet {
                        root: MapRoot(None),
                        count: 0,
                        digest: Digest([0; 32]),
                    },
                    events: Vec::new(),
                }),
            },
        }
    }

    #[test]
    fn revisions_slots_and_save_ack_refusal_keep_text() -> Result<(), IndexError> {
        let live = Arc::new(LiveBytes::default());
        let mut work = budget(Arc::new(Scope(live.clone())));
        let store = overlay();
        let doc = document(1);
        store.begin_session(doc.session, &mut work)?;
        store.update(doc, None, 1, Some("typed"), None, &mut work)?;
        check!(matches!(
            store.update(doc, Some(0), 2, Some("stale"), None, &mut work),
            Err(IndexError::Conflict)
        ));
        let snapshot = store
            .snapshot_document(doc, &mut work)?
            .ok_or(IndexError::Conflict)?;
        check!(snapshot.text.as_deref() == Some("typed"));
        store.bind_save_intent(
            doc.session,
            &intent(&receipt(doc, 1)),
            &AcceptingIntent,
            &mut work,
        )?;
        check!(matches!(
            store.save_handoff(doc.session, &receipt(doc, 1), &RefusingFrontier, &mut work),
            Err(IndexError::Conflict)
        ));
        let masks = store.snapshot_masks(doc.session, &mut work)?;
        check!(
            masks.entries
                == vec![NativeOverlayMask {
                    owner: doc.owner,
                    revision: 1,
                    deleted: false,
                }]
        );
        drop(masks);
        store.update(doc, Some(1), 2, None, None, &mut work)?;
        let masks = store.snapshot_masks(doc.session, &mut work)?;
        check!(
            masks.entries
                == vec![NativeOverlayMask {
                    owner: doc.owner,
                    revision: 2,
                    deleted: true,
                }]
        );
        drop(masks);
        check!(
            store
                .snapshot_document(doc, &mut work)?
                .ok_or(IndexError::Conflict)?
                .text
                .is_none()
        );
        check!(matches!(
            store.save_handoff(doc.session, &receipt(doc, 1), &RefusingFrontier, &mut work),
            Err(IndexError::Conflict)
        ));
        store.close_session(doc.session, &mut work)?;
        check!(matches!(
            store.snapshot_document(doc, &mut work),
            Err(IndexError::Conflict)
        ));
        drop(snapshot);
        check!(live.0.load(Ordering::SeqCst) == 0);
        Ok(())
    }

    #[test]
    fn retired_slot_fences_replay_and_refunds_payload_until_close() -> Result<(), IndexError> {
        let live = Arc::new(LiveBytes::default());
        let mut work = budget(Arc::new(Scope(live.clone())));
        let store = overlay();
        let doc = document(1);
        store.begin_session(doc.session, &mut work)?;
        let session_memory = live.0.load(Ordering::SeqCst);
        store.update(doc, None, 1, Some("saved text"), None, &mut work)?;
        let active_memory = live.0.load(Ordering::SeqCst);
        let saved = receipt(doc, 1);
        store.bind_save_intent(doc.session, &intent(&saved), &AcceptingIntent, &mut work)?;
        let bound = {
            let state = store.lock()?;
            NativeOverlay::session(&state, doc.session)?
                .documents
                .get(&doc.owner)
                .and_then(|entry| entry.save_binding)
                .ok_or(IndexError::Conflict)?
        };
        store.retire_verified(doc.session, &saved, &bound, &work)?;
        let retired_memory = live.0.load(Ordering::SeqCst);
        check!(retired_memory > session_memory && retired_memory < active_memory);
        check!(store.snapshot_document(doc, &mut work)?.is_none());
        check!(
            store
                .snapshot_masks(doc.session, &mut work)?
                .entries
                .is_empty()
        );
        check!(matches!(
            store.update(doc, None, 1, Some("replayed"), None, &mut work),
            Err(IndexError::Conflict)
        ));
        let mut second = doc;
        second.owner.stable_id = StableOwnerId([7; 16]);
        store.update(second, None, 1, Some("second"), None, &mut work)?;
        let mut third = doc;
        third.owner.stable_id = StableOwnerId([8; 16]);
        let before_full_refusal = live.0.load(Ordering::SeqCst);
        check!(matches!(
            store.update(third, None, 1, Some("third"), None, &mut work),
            Err(IndexError::ResourceLimited)
        ));
        check!(live.0.load(Ordering::SeqCst) == before_full_refusal);
        store.update(doc, Some(1), 2, Some("new revision"), None, &mut work)?;
        check!(
            store
                .snapshot_document(doc, &mut work)?
                .ok_or(IndexError::Conflict)?
                .text
                .as_deref()
                == Some("new revision")
        );
        store.close_session(doc.session, &mut work)?;
        check!(live.0.load(Ordering::SeqCst) == 0);
        Ok(())
    }

    #[test]
    fn corpus_age_is_live_revision_bound_and_empty_or_retired_is_unknown() -> Result<(), IndexError>
    {
        let live = Arc::new(LiveBytes::default());
        let mut work = budget(Arc::new(Scope(live.clone())));
        let store = overlay();
        let doc = document(1);
        store.begin_session(doc.session, &mut work)?;
        let empty = store.snapshot_corpus(doc.session, &mut work)?;
        check!(
            store
                .corpus_unchanged_for_request(&empty, work.request())?
                .is_none()
        );
        store.update(doc, None, 1, Some("first"), None, &mut work)?;
        check!(matches!(
            store.corpus_unchanged_for_request(&empty, work.request()),
            Err(IndexError::Conflict)
        ));
        let first = store.snapshot_corpus(doc.session, &mut work)?;
        check!(
            store
                .corpus_unchanged_for_request(&first, work.request())?
                .is_some()
        );
        store.validate_corpus_request(&first, work.request())?;
        let saved = receipt(doc, 1);
        store.bind_save_intent(doc.session, &intent(&saved), &AcceptingIntent, &mut work)?;
        let bound = {
            let state = store.lock()?;
            NativeOverlay::session(&state, doc.session)?
                .documents
                .get(&doc.owner)
                .and_then(|entry| entry.save_binding)
                .ok_or(IndexError::Conflict)?
        };
        store.retire_verified(doc.session, &saved, &bound, &work)?;
        check!(matches!(
            store.corpus_unchanged_for_request(&first, work.request()),
            Err(IndexError::Conflict)
        ));
        let retired = store.snapshot_corpus(doc.session, &mut work)?;
        check!(
            store
                .corpus_unchanged_for_request(&retired, work.request())?
                .is_none()
        );
        store.update(doc, Some(1), 2, Some("second"), None, &mut work)?;
        check!(matches!(
            store.corpus_unchanged_for_request(&retired, work.request()),
            Err(IndexError::Conflict)
        ));
        let second = store.snapshot_corpus(doc.session, &mut work)?;
        check!(
            store
                .corpus_unchanged_for_request(&second, work.request())?
                .is_some()
        );
        let mut foreign = second;
        foreign.session.uid = 99;
        check!(matches!(
            store.corpus_unchanged_for_request(&foreign, work.request()),
            Err(IndexError::Conflict)
        ));
        store.close_session(doc.session, &mut work)?;
        drop(foreign);
        drop(retired);
        drop(first);
        drop(empty);
        check!(live.0.load(Ordering::SeqCst) == 0);
        Ok(())
    }

    #[test]
    fn exact_save_retries_return_original_result_and_foreign_retries_conflict()
    -> Result<(), IndexError> {
        let live = Arc::new(LiveBytes::default());
        let mut work = budget(Arc::new(Scope(live.clone())));
        let store = overlay();
        let doc = document(1);
        let saved = receipt(doc, 1);
        let original = intent(&saved);
        store.begin_session(doc.session, &mut work)?;
        store.update(doc, None, 1, Some("saved text"), None, &mut work)?;
        store.bind_save_intent(doc.session, &original, &AcceptingIntent, &mut work)?;
        store.bind_save_intent(doc.session, &original, &AcceptingIntent, &mut work)?;

        let mut changed_source = original.clone();
        changed_source.expected_source = saved.source;
        check!(matches!(
            store.bind_save_intent(doc.session, &changed_source, &AcceptingIntent, &mut work),
            Err(IndexError::Conflict)
        ));
        let mut changed_request = original.clone();
        changed_request.request.expected_view.sequence = changed_request
            .request
            .expected_view
            .sequence
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        check!(matches!(
            store.bind_save_intent(doc.session, &changed_request, &AcceptingIntent, &mut work),
            Err(IndexError::Conflict)
        ));

        let bound = {
            let state = store.lock()?;
            NativeOverlay::session(&state, doc.session)?
                .documents
                .get(&doc.owner)
                .and_then(|entry| entry.save_binding)
                .ok_or(IndexError::Conflict)?
        };
        let before_save = store.snapshot_corpus(doc.session, &mut work)?;
        store.retire_verified(doc.session, &saved, &bound, &work)?;
        check!(matches!(
            store.validate_corpus(&before_save, &work),
            Err(IndexError::Conflict)
        ));
        drop(before_save);
        let saved_corpus = store.snapshot_corpus(doc.session, &mut work)?;
        check!(matches!(
            saved_corpus.documents.as_slice(),
            [row] if row.retired && row.revision == 1 && row.text.is_none() && row.path.is_none()
        ));
        store.bind_save_intent(doc.session, &original, &AcceptingIntent, &mut work)?;
        check!(
            store.save_handoff(doc.session, &saved, &RefusingFrontier, &mut work)?
                == saved.selected
        );
        store.retire_verified(doc.session, &saved, &bound, &work)?;

        for changed in [
            NativeOverlaySaveReceipt {
                operation: OperationKey {
                    sequence: saved
                        .operation
                        .sequence
                        .checked_add(1)
                        .ok_or(IndexError::ResourceLimited)?,
                    ..saved.operation
                },
                ..saved
            },
            NativeOverlaySaveReceipt {
                source: None,
                ..saved
            },
            NativeOverlaySaveReceipt {
                selected: ViewRef {
                    sequence: saved
                        .selected
                        .sequence
                        .checked_add(1)
                        .ok_or(IndexError::ResourceLimited)?,
                    ..saved.selected
                },
                ..saved
            },
            NativeOverlaySaveReceipt {
                revision: 2,
                ..saved
            },
        ] {
            check!(matches!(
                store.save_handoff(doc.session, &changed, &RefusingFrontier, &mut work),
                Err(IndexError::Conflict)
            ));
            check!(matches!(
                store.retire_verified(doc.session, &changed, &bound, &work),
                Err(IndexError::Conflict)
            ));
        }

        store.update(doc, Some(1), 2, Some("next revision"), None, &mut work)?;
        check!(matches!(
            store.validate_corpus(&saved_corpus, &work),
            Err(IndexError::Conflict)
        ));
        drop(saved_corpus);
        check!(matches!(
            store.bind_save_intent(doc.session, &original, &AcceptingIntent, &mut work),
            Err(IndexError::Conflict)
        ));
        check!(matches!(
            store.save_handoff(doc.session, &saved, &RefusingFrontier, &mut work),
            Err(IndexError::Conflict)
        ));
        store.close_session(doc.session, &mut work)?;
        check!(live.0.load(Ordering::SeqCst) == 0);
        Ok(())
    }

    #[test]
    fn corpus_capture_retains_one_atomic_owner_set_and_fences_changes() -> Result<(), IndexError> {
        let live = Arc::new(LiveBytes::default());
        let mut work = budget(Arc::new(Scope(live.clone())));
        let store = overlay();
        let doc = document(1);
        let mut deleted = doc;
        deleted.owner.stable_id = StableOwnerId([7; 16]);
        store.begin_session(doc.session, &mut work)?;
        store.update(doc, None, 1, Some("typed"), Some("src/typed.rs"), &mut work)?;
        store.update(deleted, None, 1, None, None, &mut work)?;
        let captured = store.snapshot_corpus(doc.session, &mut work)?;
        let same = store.snapshot_corpus(doc.session, &mut work)?;
        check!(captured.identity == same.identity);
        check!(captured.session == doc.session);
        let [first, second] = captured.documents.as_slice() else {
            return Err(IndexError::BackendViolation(
                "fixed corpus must contain two owner slots".into(),
            ));
        };
        check!(
            first.owner == doc.owner
                && first.revision == 1
                && !first.retired
                && first.text.as_deref() == Some("typed")
                && first.path.as_deref() == Some("src/typed.rs")
        );
        check!(
            second.owner == deleted.owner
                && !second.retired
                && second.text.is_none()
                && second.path.is_none()
        );
        store.validate_corpus(&captured, &work)?;
        drop(same);
        for bad_path in ["", "/absolute.rs", "a/../b.rs", "a//b.rs", "a\0b.rs"] {
            check!(matches!(
                store.update(doc, Some(1), 2, Some("next"), Some(bad_path), &mut work),
                Err(IndexError::InvalidInput(_))
            ));
        }
        check!(matches!(
            store.update(deleted, Some(1), 2, None, Some("deleted.rs"), &mut work),
            Err(IndexError::InvalidInput(_))
        ));
        store.update(
            doc,
            Some(1),
            2,
            Some("next"),
            Some("src/next.rs"),
            &mut work,
        )?;
        check!(matches!(
            store.validate_corpus(&captured, &work),
            Err(IndexError::Conflict)
        ));
        check!(first.text.as_deref() == Some("typed"));
        check!(first.path.as_deref() == Some("src/typed.rs"));
        let changed = store.snapshot_corpus(doc.session, &mut work)?;
        check!(changed.identity != captured.identity);
        store.validate_corpus(&changed, &work)?;
        drop(changed);
        store.close_session(doc.session, &mut work)?;
        check!(live.0.load(Ordering::SeqCst) > 0);
        drop(captured);
        check!(live.0.load(Ordering::SeqCst) == 0);
        Ok(())
    }
}
