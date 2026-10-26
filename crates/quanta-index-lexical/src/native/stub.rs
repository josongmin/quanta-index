//! Certified headless stubs and fenced, all-or-nothing action preflight.
//!
//! The AST authority and selected source reader are injected by the Native
//! product. No row offset or rendered list position is treated as identity.

use quanta_index_contract::native_protocol::NativeInteractionPublisherId;
use quanta_index_core::index::{
    Digest, IndexError, MemoryLease, MemoryLifetime, ObjectRef, OwnerKey, ViewRef, WorkBudget,
    WorkUsage, WorkspaceKey,
};
use sha2::{Digest as _, Sha256};

const MAX_SOURCE_BYTES: usize = 1 << 20;
const MAX_ITEMS: usize = 128;
const MAX_LABEL_BYTES: usize = 256;
const MAX_EDITS: usize = 64;
const MAX_EDIT_BYTES: usize = 64 << 10;
const MAX_ACTION_BYTES: usize = 256 << 10;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StubRefusal {
    UnsupportedLanguage(String),
    UnsupportedCapability(String),
    StaleBinding,
    InvalidCoordinate,
    InvalidManifest,
    Resource(IndexError),
}

impl StubRefusal {
    /// Exact-view resolution is part of a saved-source binding. A missing or
    /// fenced view is stale; budgets, cancellation and corrupt/storage causes
    /// keep their original classification and payload.
    #[must_use]
    pub fn from_selected_view_error(error: IndexError) -> Self {
        match error {
            IndexError::Conflict | IndexError::SnapshotUnavailable | IndexError::FenceLost => {
                Self::StaleBinding
            }
            other @ (IndexError::InvalidInput(_)
            | IndexError::FocusSubjectNotFound
            | IndexError::DigestConflict
            | IndexError::WorkFenceLost
            | IndexError::ResourceLimited
            | IndexError::WorkLimitExceeded
            | IndexError::Cancelled
            | IndexError::CorruptObject(_)
            | IndexError::BackendViolation(_)
            | IndexError::Storage(_)
            | IndexError::RebuildRequired
            | IndexError::MaintenanceRequired) => Self::Resource(other),
        }
    }
}

impl From<IndexError> for StubRefusal {
    fn from(value: IndexError) -> Self {
        Self::Resource(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ByteRange {
    pub start: u32,
    pub end: u32,
}

pub use quanta_index_contract::native_protocol::{
    NativeStubCoordinateUnit as CoordinateUnit, NativeStubPosition as TextPosition,
    NativeStubRange as TextRange,
};

/// Caller binds `publisher` to the authenticated foreground session and
/// `view/source/revision` to one selected, ticket-pinned source owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StubContext {
    pub workspace: WorkspaceKey,
    pub publisher: NativeInteractionPublisherId,
    pub session_epoch: u64,
    pub owner: OwnerKey,
    pub view: ViewRef,
    pub source: ObjectRef,
    pub document_revision: u64,
    pub language: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StubItem {
    pub stable_id: Digest,
    pub ast_digest: Digest,
    pub span: ByteRange,
    pub label: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StubCapability {
    pub stub: bool,
    pub action: bool,
}

/// Trusted parser/AST certificate owner. Unsupported languages and missing
/// action capability must be explicit, never inferred from a nonempty label.
pub trait StubAstCertificatePort: Send + Sync {
    /// Open one bounded certificate under the selected source fence. The
    /// returned session retains its backing for manifest and action checks.
    fn begin(
        &self,
        context: &StubContext,
        exact_source: &str,
        budget: &mut WorkBudget,
    ) -> Result<StubAstAttestation, StubRefusal>;
}

pub trait StubAstAttestationSession: Send + Sync {
    fn capability(&self) -> StubCapability;
    fn attest_item(&self, item: &StubItem, budget: &mut WorkBudget) -> Result<(), StubRefusal>;
}

/// The lease is outside the box so its accounting survives the box allocation
/// and the session's source/projection backing until their final drop.
pub struct StubAstAttestation {
    session: Box<dyn StubAstAttestationSession>,
    _memory: MemoryLease,
}

impl StubAstAttestation {
    #[must_use]
    pub fn new(session: Box<dyn StubAstAttestationSession>, memory: MemoryLease) -> Self {
        Self {
            session,
            _memory: memory,
        }
    }

    fn capability(&self) -> StubCapability {
        self.session.capability()
    }

    fn attest_item(&self, item: &StubItem, budget: &mut WorkBudget) -> Result<(), StubRefusal> {
        self.session.attest_item(item, budget)
    }
}

/// Created only through `certify_manifest`. The source reader must supply the
/// exact borrowed bytes of `context.source` from `context.view`.
pub struct CertifiedStubManifest {
    context: StubContext,
    source_digest: Digest,
    items: Vec<StubItem>,
    action_supported: bool,
    ast_session: StubAstAttestation,
    _memory: MemoryLease,
}

fn digest_source(source: &str, budget: &mut WorkBudget) -> Result<Digest, StubRefusal> {
    budget.checkpoint()?;
    budget.charge(WorkUsage {
        reads: 1,
        bytes: u64::try_from(source.len()).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let mut hash = Sha256::new();
    hash.update(b"quanta-index/native-stub/source-bytes/v1\0");
    hash.update(
        u64::try_from(source.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .to_be_bytes(),
    );
    hash.update(source.as_bytes());
    budget.checkpoint()?;
    Ok(Digest(hash.finalize().into()))
}

impl CertifiedStubManifest {
    #[must_use]
    pub fn context(&self) -> &StubContext {
        &self.context
    }
    #[must_use]
    pub fn items(&self) -> &[StubItem] {
        &self.items
    }
    #[must_use]
    pub fn action_supported(&self) -> bool {
        self.action_supported
    }
    #[must_use]
    pub fn selected(&self, id: Digest) -> Option<&StubItem> {
        match self.items.binary_search_by_key(&id, |item| item.stable_id) {
            Ok(at) => self.items.get(at),
            Err(_insertion) => None,
        }
    }
}

fn checked_span(source: &str, span: ByteRange) -> Result<(), StubRefusal> {
    let start = usize::try_from(span.start).map_err(|_error| StubRefusal::InvalidCoordinate)?;
    let end = usize::try_from(span.end).map_err(|_error| StubRefusal::InvalidCoordinate)?;
    if start > end
        || end > source.len()
        || !source.is_char_boundary(start)
        || !source.is_char_boundary(end)
    {
        return Err(StubRefusal::InvalidCoordinate);
    }
    Ok(())
}

/// Validate the complete producer manifest against one exact borrowed source.
/// Sorting uses stable logical IDs; a changed display order does not change
/// identity or action selection.
pub fn certify_manifest(
    context: StubContext,
    exact_source: &str,
    items: &[StubItem],
    ast: &dyn StubAstCertificatePort,
    budget: &mut WorkBudget,
) -> Result<CertifiedStubManifest, StubRefusal> {
    budget.checkpoint()?;
    if context.view.workspace != context.workspace
        || context.view.epoch == 0
        || context.source == ObjectRef(Digest([0; 32]))
        || context.publisher.0 == [0; 32]
        || context.session_epoch == 0
        || context.document_revision == 0
        || context.language.is_empty()
        || context.language.len() > 64
        || exact_source.len() > MAX_SOURCE_BYTES
        || items.is_empty()
        || items.len() > MAX_ITEMS
    {
        return Err(StubRefusal::InvalidManifest);
    }
    let source_digest = digest_source(exact_source, budget)?;
    let ast_session = ast.begin(&context, exact_source, budget)?;
    let capability = ast_session.capability();
    if !capability.stub {
        return Err(StubRefusal::UnsupportedCapability(context.language.clone()));
    }
    for (at, item) in items.iter().enumerate() {
        budget.checkpoint()?;
        if item.stable_id.0 == [0; 32]
            || item.ast_digest.0 == [0; 32]
            || item.label.is_empty()
            || item.label.len() > MAX_LABEL_BYTES
            || items
                .iter()
                .take(at)
                .any(|prior| prior.stable_id == item.stable_id)
        {
            return Err(StubRefusal::InvalidManifest);
        }
        checked_span(exact_source, item.span)?;
        if item.span.start == item.span.end {
            return Err(StubRefusal::InvalidManifest);
        }
        budget.charge(WorkUsage {
            reads: 1,
            bytes: u64::try_from(item.label.len()).map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        ast_session.attest_item(item, budget)?;
    }
    let labels = items
        .iter()
        .try_fold(0usize, |total, item| total.checked_add(item.label.len()))
        .ok_or(IndexError::ResourceLimited)?;
    let bytes = items
        .len()
        .checked_mul(std::mem::size_of::<StubItem>())
        .and_then(|value| value.checked_add(labels))
        .and_then(|value| value.checked_add(context.language.len()))
        .and_then(|value| value.checked_add(128))
        .ok_or(IndexError::ResourceLimited)?;
    let bytes = u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes,
        ..WorkUsage::default()
    })?;
    let memory = budget
        .reserve_memory(bytes, MemoryLifetime::Retained)?
        .ok_or_else(|| {
            StubRefusal::Resource(IndexError::BackendViolation(
                "Certified stub manifest requires retained memory scope".into(),
            ))
        })?;
    let mut owned = items.to_vec();
    owned.sort_unstable_by_key(|item| item.stable_id);
    Ok(CertifiedStubManifest {
        context,
        source_digest,
        items: owned,
        action_supported: capability.action,
        ast_session,
        _memory: memory,
    })
}

fn charge_coordinates(source: &str, budget: &mut WorkBudget) -> Result<(), StubRefusal> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(StubRefusal::Resource(IndexError::ResourceLimited));
    }
    budget.charge(WorkUsage {
        reads: 1,
        bytes: u64::try_from(source.len()).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    Ok(())
}

/// Resolve one visible position using the shared producer/consumer contract.
pub fn position_to_byte(
    source: &str,
    position: TextPosition,
    unit: CoordinateUnit,
    budget: &mut WorkBudget,
) -> Result<usize, StubRefusal> {
    charge_coordinates(source, budget)?;
    position
        .to_byte(source, unit)
        .ok_or(StubRefusal::InvalidCoordinate)
}

pub fn range_to_bytes(
    source: &str,
    range: TextRange,
    unit: CoordinateUnit,
    budget: &mut WorkBudget,
) -> Result<ByteRange, StubRefusal> {
    let start = position_to_byte(source, range.start, unit, budget)?;
    let end = position_to_byte(source, range.end, unit, budget)?;
    if start > end {
        return Err(StubRefusal::InvalidCoordinate);
    }
    Ok(ByteRange {
        start: u32::try_from(start).map_err(|_error| StubRefusal::InvalidCoordinate)?,
        end: u32::try_from(end).map_err(|_error| StubRefusal::InvalidCoordinate)?,
    })
}

pub fn byte_to_position(
    source: &str,
    byte: usize,
    unit: CoordinateUnit,
    budget: &mut WorkBudget,
) -> Result<TextPosition, StubRefusal> {
    charge_coordinates(source, budget)?;
    // Match the original two-pass charge for locating and measuring the line.
    charge_coordinates(source, budget)?;
    TextPosition::from_byte(source, byte, unit).ok_or(StubRefusal::InvalidCoordinate)
}

#[derive(Clone, Debug)]
pub struct ProposedEdit<'a> {
    pub target_id: Digest,
    pub expected_ast: Digest,
    pub range: TextRange,
    pub replacement: &'a str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ByteEdit {
    pub target_id: Digest,
    pub range: ByteRange,
    pub replacement: Vec<u8>,
}

/// The external consumer must recheck this entire fence immediately before
/// applying all edits as one operation. A display position is never a fence.
pub struct PreparedAction {
    context: StubContext,
    selected_id: Digest,
    edits: Vec<ByteEdit>,
    _memory: MemoryLease,
}

impl PreparedAction {
    #[must_use]
    pub fn context(&self) -> &StubContext {
        &self.context
    }
    #[must_use]
    pub fn selected_id(&self) -> Digest {
        self.selected_id
    }
    #[must_use]
    pub fn edits(&self) -> &[ByteEdit] {
        &self.edits
    }
}

pub fn preflight_action(
    manifest: &CertifiedStubManifest,
    selected_source: &str,
    selected_context: &StubContext,
    selected_id: Digest,
    edits: &[ProposedEdit<'_>],
    unit: CoordinateUnit,
    budget: &mut WorkBudget,
) -> Result<PreparedAction, StubRefusal> {
    budget.checkpoint()?;
    if &manifest.context != selected_context || selected_source.len() > MAX_SOURCE_BYTES {
        return Err(StubRefusal::StaleBinding);
    }
    if digest_source(selected_source, budget)? != manifest.source_digest {
        return Err(StubRefusal::StaleBinding);
    }
    if !manifest.action_supported {
        return Err(StubRefusal::UnsupportedCapability(
            manifest.context.language.clone(),
        ));
    }
    if manifest.selected(selected_id).is_none() || edits.is_empty() || edits.len() > MAX_EDITS {
        return Err(StubRefusal::InvalidManifest);
    }
    if !manifest.ast_session.capability().action {
        return Err(StubRefusal::UnsupportedCapability(
            manifest.context.language.clone(),
        ));
    }
    for item in &manifest.items {
        budget.checkpoint()?;
        checked_span(selected_source, item.span)?;
        manifest.ast_session.attest_item(item, budget)?;
    }
    let checked_bytes = edits
        .len()
        .checked_mul(std::mem::size_of::<(Digest, ByteRange, &str)>())
        .and_then(|value| value.checked_add(128))
        .ok_or(IndexError::ResourceLimited)?;
    let _checked_memory = budget
        .reserve_memory(
            u64::try_from(checked_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
        .ok_or_else(|| {
            StubRefusal::Resource(IndexError::BackendViolation(
                "Native action requires temporary memory scope".into(),
            ))
        })?;
    let mut checked = Vec::new();
    checked
        .try_reserve_exact(edits.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut copied = 0usize;
    let mut selected_target_seen = false;
    for edit in edits {
        budget.checkpoint()?;
        let item = manifest
            .selected(edit.target_id)
            .ok_or(StubRefusal::InvalidManifest)?;
        if edit.expected_ast != item.ast_digest {
            return Err(StubRefusal::StaleBinding);
        }
        let range = range_to_bytes(selected_source, edit.range, unit, budget)?;
        if range.start < item.span.start || range.end > item.span.end {
            return Err(StubRefusal::InvalidCoordinate);
        }
        if edit.replacement.len() > MAX_EDIT_BYTES {
            return Err(StubRefusal::Resource(IndexError::ResourceLimited));
        }
        copied = copied
            .checked_add(edit.replacement.len())
            .ok_or(IndexError::ResourceLimited)?;
        if copied > MAX_ACTION_BYTES {
            return Err(StubRefusal::Resource(IndexError::ResourceLimited));
        }
        if edit.target_id == selected_id {
            selected_target_seen = true;
        }
        checked.push((edit.target_id, range, edit.replacement));
    }
    if !selected_target_seen {
        return Err(StubRefusal::InvalidManifest);
    }
    checked.sort_unstable_by_key(|(_, range, _)| (range.start, range.end));
    let mut prior_end = None;
    let mut prior_start = None;
    for (_, range, _) in &checked {
        if prior_end.is_some_and(|end| range.start < end) || prior_start == Some(range.start) {
            return Err(StubRefusal::InvalidCoordinate);
        }
        prior_start = Some(range.start);
        prior_end = Some(range.end);
    }
    let metadata = checked
        .len()
        .checked_mul(std::mem::size_of::<ByteEdit>())
        .ok_or(IndexError::ResourceLimited)?;
    let total = copied
        .checked_add(metadata)
        .and_then(|value| value.checked_add(128))
        .ok_or(IndexError::ResourceLimited)?;
    let total = u64::try_from(total).map_err(|_error| IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes: total,
        mutations: 1,
        ..WorkUsage::default()
    })?;
    let memory = budget
        .reserve_memory(total, MemoryLifetime::Retained)?
        .ok_or_else(|| {
            StubRefusal::Resource(IndexError::BackendViolation(
                "Native action requires retained memory scope".into(),
            ))
        })?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(checked.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    for (target_id, range, replacement) in checked {
        budget.checkpoint()?;
        result.push(ByteEdit {
            target_id,
            range,
            replacement: replacement.as_bytes().to_vec(),
        });
    }
    Ok(PreparedAction {
        context: manifest.context.clone(),
        selected_id,
        edits: result,
        _memory: memory,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::{
        RequestBudgetV1,
        index::{
            CollectionId, MemoryLeaseOwner, MemoryScope, MemoryScopePort, ProducerId, SlotId,
            StableOwnerId, TenantId, WorkLimits, WorktreeId,
        },
    };
    use std::{
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::Duration,
    };

    macro_rules! check {
        ($condition:expr) => {
            if !$condition {
                return Err(IndexError::BackendViolation(format!(
                    "stub test condition failed at {}:{}: {}",
                    file!(),
                    line!(),
                    stringify!($condition)
                ))
                .into());
            }
        };
    }

    const SOURCE: &str = "let x = 1;\nlet y = 2;";

    #[derive(Default)]
    struct Live(AtomicU64);
    impl MemoryLeaseOwner for Live {
        fn release(&self, bytes: u64) {
            let _prior = self.0.fetch_sub(bytes, Ordering::SeqCst);
        }
    }
    struct Scope(Arc<Live>);
    impl MemoryScopePort for Scope {
        fn reserve(
            &self,
            bytes: u64,
            _lifetime: MemoryLifetime,
        ) -> Result<MemoryLease, IndexError> {
            let _prior = self.0.0.fetch_add(bytes, Ordering::SeqCst);
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }
    fn budget(live: Arc<Live>) -> WorkBudget {
        let mut budget = WorkBudget::with_request(
            WorkLimits {
                reads: 1_000,
                writes: 10,
                bytes: 1_000_000,
                mutations: 10,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(live)))));
        budget
    }
    fn context(language: &str) -> StubContext {
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        };
        StubContext {
            workspace,
            publisher: NativeInteractionPublisherId([5; 32]),
            session_epoch: 1,
            owner: OwnerKey {
                producer: ProducerId([6; 16]),
                stable_id: StableOwnerId([7; 16]),
            },
            view: ViewRef {
                workspace,
                epoch: 1,
                sequence: 1,
                digest: Digest([8; 32]),
            },
            source: ObjectRef(Digest([9; 32])),
            document_revision: 1,
            language: language.to_owned(),
        }
    }
    fn items() -> Vec<StubItem> {
        vec![
            StubItem {
                stable_id: Digest([2; 32]),
                ast_digest: Digest([12; 32]),
                span: ByteRange { start: 11, end: 21 },
                label: "y".into(),
            },
            StubItem {
                stable_id: Digest([1; 32]),
                ast_digest: Digest([11; 32]),
                span: ByteRange { start: 0, end: 10 },
                label: "x".into(),
            },
        ]
    }
    fn test_session(
        session: impl StubAstAttestationSession + 'static,
        budget: &mut WorkBudget,
    ) -> Result<StubAstAttestation, StubRefusal> {
        let bytes = u64::try_from(std::mem::size_of_val(&session).max(128))
            .map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        let memory = budget
            .reserve_memory(bytes, MemoryLifetime::Retained)?
            .ok_or_else(|| {
                StubRefusal::Resource(IndexError::BackendViolation(
                    "test session needs memory scope".into(),
                ))
            })?;
        Ok(StubAstAttestation::new(Box::new(session), memory))
    }
    struct Ast;
    struct AstSession;
    impl StubAstCertificatePort for Ast {
        fn begin(
            &self,
            context: &StubContext,
            exact_source: &str,
            budget: &mut WorkBudget,
        ) -> Result<StubAstAttestation, StubRefusal> {
            if context.language != "rust" {
                return Err(StubRefusal::UnsupportedLanguage(context.language.clone()));
            }
            if exact_source != SOURCE {
                return Err(StubRefusal::StaleBinding);
            }
            test_session(AstSession, budget)
        }
    }
    impl StubAstAttestationSession for AstSession {
        fn capability(&self) -> StubCapability {
            StubCapability {
                stub: true,
                action: true,
            }
        }
        fn attest_item(
            &self,
            item: &StubItem,
            _budget: &mut WorkBudget,
        ) -> Result<(), StubRefusal> {
            let expected_ast = if item.stable_id == Digest([1; 32]) {
                Digest([11; 32])
            } else {
                Digest([12; 32])
            };
            if item.ast_digest == expected_ast {
                Ok(())
            } else {
                Err(StubRefusal::StaleBinding)
            }
        }
    }

    #[test]
    fn utf8_utf16_coordinates_refuse_scalar_and_crlf_splits() -> Result<(), StubRefusal> {
        let live = Arc::new(Live::default());
        let mut work = budget(live);
        let source = "a🦀b\r\nz";
        check!(
            position_to_byte(
                source,
                TextPosition { line: 0, column: 1 },
                CoordinateUnit::Utf16Units,
                &mut work
            )? == 1
        );
        check!(matches!(
            position_to_byte(
                source,
                TextPosition { line: 0, column: 2 },
                CoordinateUnit::Utf16Units,
                &mut work
            ),
            Err(StubRefusal::InvalidCoordinate)
        ));
        check!(
            position_to_byte(
                source,
                TextPosition { line: 0, column: 3 },
                CoordinateUnit::Utf16Units,
                &mut work
            )? == 5
        );
        check!(matches!(
            position_to_byte(
                source,
                TextPosition { line: 0, column: 2 },
                CoordinateUnit::Utf8Bytes,
                &mut work
            ),
            Err(StubRefusal::InvalidCoordinate)
        ));
        check!(byte_to_position(source, 5, CoordinateUnit::Utf16Units, &mut work)?.column == 3);
        check!(matches!(
            byte_to_position(source, 7, CoordinateUnit::Utf8Bytes, &mut work),
            Err(StubRefusal::InvalidCoordinate)
        ));
        Ok(())
    }

    #[test]
    fn action_uses_stable_id_and_returns_no_partial_edits() -> Result<(), StubRefusal> {
        let live = Arc::new(Live::default());
        let mut work = budget(live.clone());
        let context = context("rust");
        let raw = items();
        let manifest = certify_manifest(context.clone(), SOURCE, &raw, &Ast, &mut work)?;
        check!(
            manifest
                .items()
                .first()
                .is_some_and(|item| item.stable_id == Digest([1; 32]))
        );
        let edit = ProposedEdit {
            target_id: Digest([1; 32]),
            expected_ast: Digest([11; 32]),
            range: TextRange {
                start: TextPosition { line: 0, column: 4 },
                end: TextPosition { line: 0, column: 5 },
            },
            replacement: "z",
        };
        let prepared = preflight_action(
            &manifest,
            SOURCE,
            &context,
            Digest([1; 32]),
            std::slice::from_ref(&edit),
            CoordinateUnit::Utf8Bytes,
            &mut work,
        )?;
        check!(prepared.selected_id() == Digest([1; 32]));
        check!(
            prepared
                .edits()
                .first()
                .is_some_and(|item| item.replacement.as_slice() == b"z")
        );
        drop(prepared);
        let stale = ProposedEdit {
            expected_ast: Digest([99; 32]),
            ..edit.clone()
        };
        check!(matches!(
            preflight_action(
                &manifest,
                SOURCE,
                &context,
                Digest([1; 32]),
                &[edit, stale],
                CoordinateUnit::Utf8Bytes,
                &mut work
            ),
            Err(StubRefusal::StaleBinding)
        ));
        check!(matches!(
            preflight_action(
                &manifest,
                "let z = 1;\nlet y = 2;",
                &context,
                Digest([1; 32]),
                &[],
                CoordinateUnit::Utf8Bytes,
                &mut work
            ),
            Err(StubRefusal::InvalidManifest | StubRefusal::StaleBinding)
        ));
        drop(manifest);
        check!(live.0.load(Ordering::SeqCst) == 0);
        Ok(())
    }

    struct PermissiveAst;
    impl StubAstCertificatePort for PermissiveAst {
        fn begin(
            &self,
            _context: &StubContext,
            _exact_source: &str,
            budget: &mut WorkBudget,
        ) -> Result<StubAstAttestation, StubRefusal> {
            test_session(PermissiveAst, budget)
        }
    }
    impl StubAstAttestationSession for PermissiveAst {
        fn capability(&self) -> StubCapability {
            StubCapability {
                stub: true,
                action: true,
            }
        }

        fn attest_item(
            &self,
            _item: &StubItem,
            _budget: &mut WorkBudget,
        ) -> Result<(), StubRefusal> {
            Ok(())
        }
    }

    #[test]
    fn preflight_reuses_one_attestation_session_and_refunds_it() -> Result<(), StubRefusal> {
        struct CountingAst(Arc<AtomicU64>);
        impl StubAstCertificatePort for CountingAst {
            fn begin(
                &self,
                _context: &StubContext,
                _exact_source: &str,
                budget: &mut WorkBudget,
            ) -> Result<StubAstAttestation, StubRefusal> {
                let _previous = self.0.fetch_add(1, Ordering::SeqCst);
                test_session(PermissiveAst, budget)
            }
        }
        let live = Arc::new(Live::default());
        let begun = Arc::new(AtomicU64::new(0));
        let ast = CountingAst(Arc::clone(&begun));
        let mut work = budget(Arc::clone(&live));
        let context = context("rust");
        let manifest = certify_manifest(context.clone(), SOURCE, &items(), &ast, &mut work)?;
        let edit = ProposedEdit {
            target_id: Digest([1; 32]),
            expected_ast: Digest([11; 32]),
            range: TextRange {
                start: TextPosition { line: 0, column: 4 },
                end: TextPosition { line: 0, column: 5 },
            },
            replacement: "z",
        };
        let prepared = preflight_action(
            &manifest,
            SOURCE,
            &context,
            Digest([1; 32]),
            &[edit],
            CoordinateUnit::Utf8Bytes,
            &mut work,
        )?;
        check!(begun.load(Ordering::SeqCst) == 1);
        drop(prepared);
        drop(manifest);
        check!(live.0.load(Ordering::SeqCst) == 0);
        Ok(())
    }

    #[test]
    fn same_context_cannot_preflight_changed_source_bytes() -> Result<(), StubRefusal> {
        let live = Arc::new(Live::default());
        let mut work = budget(live.clone());
        let context = context("rust");
        let manifest =
            certify_manifest(context.clone(), SOURCE, &items(), &PermissiveAst, &mut work)?;
        let changed = "let x = 2;\nlet y = 2;";
        let edit = ProposedEdit {
            target_id: Digest([1; 32]),
            expected_ast: Digest([11; 32]),
            range: TextRange {
                start: TextPosition { line: 0, column: 4 },
                end: TextPosition { line: 0, column: 5 },
            },
            replacement: "z",
        };
        check!(matches!(
            preflight_action(
                &manifest,
                changed,
                &context,
                Digest([1; 32]),
                &[edit],
                CoordinateUnit::Utf8Bytes,
                &mut work,
            ),
            Err(StubRefusal::StaleBinding)
        ));
        drop(manifest);
        check!(live.0.load(Ordering::SeqCst) == 0);
        Ok(())
    }

    #[test]
    fn zero_source_or_view_epoch_cannot_certify() {
        let mut work = budget(Arc::new(Live::default()));
        let mut invalid = context("rust");
        invalid.source = ObjectRef(Digest([0; 32]));
        assert!(matches!(
            certify_manifest(invalid, SOURCE, &items(), &PermissiveAst, &mut work),
            Err(StubRefusal::InvalidManifest)
        ));
        let mut invalid = context("rust");
        invalid.view.epoch = 0;
        assert!(matches!(
            certify_manifest(invalid, SOURCE, &items(), &PermissiveAst, &mut work),
            Err(StubRefusal::InvalidManifest)
        ));
    }

    #[test]
    fn unsupported_language_is_typed() {
        let mut work = budget(Arc::new(Live::default()));
        let raw = items();
        assert!(matches!(
            certify_manifest(context("unknown"), SOURCE, &raw, &Ast, &mut work),
            Err(StubRefusal::UnsupportedLanguage(_))
        ));
    }

    #[test]
    fn selected_view_refusal_preserves_resource_cancel_and_corruption_causes() {
        for error in [
            IndexError::Conflict,
            IndexError::SnapshotUnavailable,
            IndexError::FenceLost,
        ] {
            assert_eq!(
                StubRefusal::from_selected_view_error(error),
                StubRefusal::StaleBinding
            );
        }
        for error in [
            IndexError::ResourceLimited,
            IndexError::WorkLimitExceeded,
            IndexError::Cancelled,
            IndexError::CorruptObject("fixed corruption".into()),
        ] {
            assert_eq!(
                StubRefusal::from_selected_view_error(error.clone()),
                StubRefusal::Resource(error)
            );
        }
    }
}
