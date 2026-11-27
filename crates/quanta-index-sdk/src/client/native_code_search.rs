//! The default `.text()` code search on a Native connection.
//!
//! Native addresses a repository revision through its sealed protected Source
//! head, not a Legacy manifest generation. A first page resolves that head to
//! its exact view and reads one `select:path` file page on a read session for
//! that view. The response generation names the head: its
//! `manifest_generation` is the view's sequence, so a pinned first page
//! refuses once the head has moved. The continuation token carries the Native
//! path cursor plus the selected revision. The daemon binds the wire cursor
//! to the view, text member and query; the SDK binds the revision.
//!
//! Each row carries the position of its representative match: a content
//! match gives its lines and its excerpt, a path match the path with the
//! matched bytes highlighted.
//!
//! The client owns one read session per view while continuations on it are
//! outstanding, for this route and for text matches; see
//! [`NativeViewSessions`] for the ownership model.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use quanta_index_contract::index::{IndexInstanceId, InstanceSpec, ViewRef, ViewSelection};
use quanta_index_contract::native_protocol::{
    NATIVE_TEXT_KIND, NativeCodeSearchMatch, NativeIndexErrorCode, NativeReadSessionStatus,
    NativeTextHit, NativeTextPathCursorV1, NativeTextPathPageV1,
};
use quanta_index_contract::{
    CandidateCountV1, ContinuationTokenV2, GenerationPin, GenerationSelector, HighlightSpan,
    LaneTraceV1, LexicalCandidate, LexicalLineRange, LexicalMatchExcerpt, ManifestGeneration,
    NativeSourceIdentity, QueryResultWindowV2, RepoId, RepoRelativePath, RevisionId,
    SearchExplanation, SearchPlaneErrorCodeV2, TextQueryRequest, TextQueryResponse,
    TextQuerySyntax, TextRankUnit, code_search_file_candidate_id,
};
use quanta_index_ipc::{ClientIoPolicy, RequestBudgetV1};
use serde::ser::SerializeStruct as _;
use sha2::{Digest as _, Sha256};

use crate::native_uds::{NativeUdsClient, NativeUdsError, native_code_search_request};
use crate::text_query_route_shared::{TextQueryBuilderState, accepted_top_k};
use crate::{ClientLexicalQueryObservationV1, NativeQueryError, SdkError};

/// Continuation tokens minted by this route start with this tag.
const CURSOR_TAG: &str = "native-code-search-path-v2:";
/// A cursor holds at most one Source path plus fixed-width identities.
const MAX_CURSOR_TOKEN_BYTES: usize = 32 * 1024;
const LANE: &str = "native_text_code_search";
/// The renewer never sleeps longer than this between lease checks.
const MAX_RENEW_TICK: Duration = Duration::from_secs(1);
const MIN_RENEW_TICK: Duration = Duration::from_millis(10);
const MAX_CLEANUP_ITEMS: usize = 64;
// Catalog read-ticket leases are capped at 300 seconds. A late accepted Open
// can still live until daemon expiry after this local retry window.
const MAX_UNCERTAIN_OPEN_AGE: Duration = Duration::from_secs(600);

/// SDK-only generation binding around the unchanged Native page cursor wire.
#[derive(Clone, Debug, Eq, PartialEq)]
struct CodeSearchCursor {
    revision_id: RevisionId,
    cursor_id: [u8; 32],
    native: NativeTextPathCursorV1,
}

impl serde::Serialize for CodeSearchCursor {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields = serializer.serialize_struct("CodeSearchCursor", 3)?;
        fields.serialize_field("revision_id", &self.revision_id)?;
        fields.serialize_field("cursor_id", &self.cursor_id)?;
        fields.serialize_field("native", &self.native)?;
        fields.end()
    }
}

impl<'de> serde::Deserialize<'de> for CodeSearchCursor {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &["revision_id", "cursor_id", "native"];

        enum CursorField {
            RevisionId,
            CursorId,
            Native,
        }

        impl<'de> serde::Deserialize<'de> for CursorField {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct FieldVisitor;
                impl<'de> serde::de::Visitor<'de> for FieldVisitor {
                    type Value = CursorField;

                    fn expecting(
                        &self,
                        formatter: &mut std::fmt::Formatter<'_>,
                    ) -> std::fmt::Result {
                        formatter.write_str("field identifier")
                    }

                    // Only the text identifiers the encoder writes; a byte
                    // string key would alias the canonical token.
                    fn visit_str<E: serde::de::Error>(self, field: &str) -> Result<CursorField, E> {
                        match field {
                            "revision_id" => Ok(CursorField::RevisionId),
                            "cursor_id" => Ok(CursorField::CursorId),
                            "native" => Ok(CursorField::Native),
                            _ => Err(E::unknown_field(field, FIELDS)),
                        }
                    }
                }

                deserializer.deserialize_identifier(FieldVisitor)
            }
        }

        struct CursorVisitor;
        impl<'de> serde::de::Visitor<'de> for CursorVisitor {
            type Value = CodeSearchCursor;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct CodeSearchCursor")
            }

            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut revision_id = None;
                let mut cursor_id = None;
                let mut native = None;
                while let Some(field) = map.next_key::<CursorField>()? {
                    match field {
                        CursorField::RevisionId if revision_id.is_none() => {
                            revision_id = Some(map.next_value()?);
                        }
                        CursorField::CursorId if cursor_id.is_none() => {
                            cursor_id = Some(map.next_value()?);
                        }
                        CursorField::Native if native.is_none() => {
                            native = Some(map.next_value()?);
                        }
                        CursorField::RevisionId => {
                            return Err(serde::de::Error::duplicate_field("revision_id"));
                        }
                        CursorField::CursorId => {
                            return Err(serde::de::Error::duplicate_field("cursor_id"));
                        }
                        CursorField::Native => {
                            return Err(serde::de::Error::duplicate_field("native"));
                        }
                    }
                }
                Ok(CodeSearchCursor {
                    revision_id: revision_id
                        .ok_or_else(|| serde::de::Error::missing_field("revision_id"))?,
                    cursor_id: cursor_id
                        .ok_or_else(|| serde::de::Error::missing_field("cursor_id"))?,
                    native: native.ok_or_else(|| serde::de::Error::missing_field("native"))?,
                })
            }

            fn visit_seq<S: serde::de::SeqAccess<'de>>(
                self,
                mut seq: S,
            ) -> Result<Self::Value, S::Error> {
                let revision_id = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;
                let cursor_id = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                let native = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(2, &self))?;
                if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(serde::de::Error::invalid_length(4, &self));
                }
                Ok(CodeSearchCursor {
                    revision_id,
                    cursor_id,
                    native,
                })
            }
        }

        deserializer.deserialize_struct("CodeSearchCursor", FIELDS, CursorVisitor)
    }
}

struct CodeSearchPageRequest {
    view: ViewRef,
    generation: GenerationPin,
    configured_instance: Option<IndexInstanceId>,
    query: TextQueryRequest,
    after: Option<CodeSearchCursor>,
}

/// Native transport state the default `.text()` route needs.
pub(crate) struct NativeTextRoute<'a> {
    pub(crate) client: &'a NativeUdsClient,
    pub(crate) selection: ViewSelection,
    pub(crate) io_policy: ClientIoPolicy,
    pub(crate) text_instance: Option<IndexInstanceId>,
    pub(crate) sessions: &'a NativeViewSessions,
}

impl NativeTextRoute<'_> {
    pub(crate) fn budget(&self) -> RequestBudgetV1 {
        bounded_budget(self.io_policy)
    }
}

fn bounded_budget(io_policy: ClientIoPolicy) -> RequestBudgetV1 {
    let budget = RequestBudgetV1::for_duration(io_policy.request_timeout());
    if let Some(deadline) = io_policy.absolute_deadline() {
        return budget.capped_at(deadline);
    }
    budget
}

/// One retained read session and the text member captured by its profile.
struct HeldSession {
    status: NativeReadSessionStatus,
    spec: InstanceSpec,
    lease_ms: u64,
    /// SDK cursor identities still live on this client and view.
    active_cursors: BTreeSet<[u8; 32]>,
    pages_in_flight: usize,
    release_only: bool,
}

impl HeldSession {
    fn is_idle(&self) -> bool {
        !self.release_only && self.active_cursors.is_empty() && self.pages_in_flight == 0
    }
}

fn record_page_cursor(
    active: &mut BTreeSet<[u8; 32]>,
    consumed: Option<[u8; 32]>,
    issued: Option<[u8; 32]>,
) {
    if let Some(consumed) = consumed {
        let _was_live = active.remove(&consumed);
    }
    if let Some(issued) = issued {
        let _new_cursor = active.insert(issued);
    }
}

fn apply_renewal(
    held: &mut BTreeMap<ViewRef, HeldSession>,
    view: ViewRef,
    prior: NativeReadSessionStatus,
    renewed: Result<NativeReadSessionStatus, NativeUdsError>,
) {
    let Some(entry) = held.get_mut(&view) else {
        return;
    };
    if entry.release_only
        || entry.status.session != prior.session
        || entry.status.revision != prior.revision
    {
        return;
    }
    match renewed {
        Ok(next) => entry.status = next,
        Err(NativeUdsError::Refused(error))
            if error.code == NativeIndexErrorCode::SnapshotUnavailable =>
        {
            let _lost = held.remove(&view);
        }
        // The daemon may have committed the renewal before losing its ACK.
        // Keep the last acknowledged revision so the next tick can replay it.
        Err(_) => {}
    }
}

/// An Open with an uncertain ACK can only be reconciled with its original key.
/// A confirmed session awaiting Release must never become a query lease.
enum CleanupKind {
    Opening,
    Open,
    Release(NativeReadSessionStatus),
}

struct CleanupSession {
    view: ViewRef,
    key: [u8; 32],
    kind: CleanupKind,
    in_progress: bool,
    created_at: Instant,
}

fn cleanup_session_gone(error: &SdkError) -> bool {
    matches!(
        error,
        SdkError::Native(inner)
            if matches!(
                inner.as_ref(),
                NativeQueryError::Transport(NativeUdsError::Refused(refusal))
                    if refusal.code == NativeIndexErrorCode::SnapshotUnavailable
            )
    )
}

fn open_ack_may_be_lost(error: &NativeUdsError) -> bool {
    match error {
        // A local cancellation after send is surfaced as Refused(Cancelled).
        NativeUdsError::Refused(refusal) => refusal.code == NativeIndexErrorCode::Cancelled,
        NativeUdsError::QueryRejected { .. } | NativeUdsError::PeerBindingRefused(_) => false,
        NativeUdsError::Protocol(_) | NativeUdsError::Transport(_) | NativeUdsError::Unknown(_) => {
            true
        }
    }
}

struct SessionTable {
    client: NativeUdsClient,
    io_policy: ClientIoPolicy,
    held: Mutex<BTreeMap<ViewRef, HeldSession>>,
    cleanup: Mutex<Vec<CleanupSession>>,
}

impl SessionTable {
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<ViewRef, HeldSession>> {
        // A poisoned table still holds valid lease records; release must run.
        match self.held.lock() {
            Ok(held) => held,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn cleanup_lock(&self) -> std::sync::MutexGuard<'_, Vec<CleanupSession>> {
        match self.cleanup.lock() {
            Ok(cleanup) => cleanup,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn reserve_open(&self, view: ViewRef, key: [u8; 32]) -> Result<(), SdkError> {
        let mut cleanup = self.cleanup_lock();
        if cleanup
            .iter()
            .any(|entry| entry.view == view && !matches!(&entry.kind, CleanupKind::Opening))
        {
            return Err(SdkError::Protocol(
                "Native read session cleanup is pending for this view".into(),
            ));
        }
        if cleanup.len().saturating_add(self.lock().len()) >= MAX_CLEANUP_ITEMS {
            return Err(SdkError::Protocol(
                "Native read session cleanup capacity is exhausted".into(),
            ));
        }
        cleanup.push(CleanupSession {
            view,
            key,
            kind: CleanupKind::Opening,
            in_progress: false,
            created_at: Instant::now(),
        });
        Ok(())
    }

    fn change_cleanup(&self, key: [u8; 32], kind: Option<CleanupKind>) {
        let mut cleanup = self.cleanup_lock();
        if let Some(index) = cleanup.iter().position(|entry| entry.key == key) {
            if let Some(kind) = kind {
                cleanup[index].kind = kind;
            } else {
                let _removed = cleanup.remove(index);
            }
        }
    }

    /// Mark one entry under a short lock, perform I/O outside it, then update
    /// the entry. One shared budget bounds the whole pass across all entries.
    fn settle_cleanup(
        &self,
        only_view: Option<ViewRef>,
        budget: &RequestBudgetV1,
    ) -> Result<(), SdkError> {
        let mut attempted = BTreeSet::new();
        let mut first_error = None;
        for _ in 0..MAX_CLEANUP_ITEMS {
            let selected = {
                let mut cleanup = self.cleanup_lock();
                cleanup.retain(|entry| {
                    entry.in_progress
                        || !matches!(&entry.kind, CleanupKind::Open)
                        || entry.created_at.elapsed() < MAX_UNCERTAIN_OPEN_AGE
                });
                let entry = cleanup.iter_mut().find(|entry| {
                    only_view.is_none_or(|view| view == entry.view)
                        && !entry.in_progress
                        && !matches!(&entry.kind, CleanupKind::Opening)
                        && !attempted.contains(&entry.key)
                });
                entry.map(|entry| {
                    entry.in_progress = true;
                    (
                        entry.view,
                        entry.key,
                        match &entry.kind {
                            CleanupKind::Open => None,
                            CleanupKind::Release(status) => Some(*status),
                            CleanupKind::Opening => unreachable!(),
                        },
                    )
                })
            };
            let Some((view, key, known)) = selected else {
                break;
            };
            let _attempted = attempted.insert(key);
            let status = match known {
                Some(status) => Ok(status),
                None => self
                    .client
                    .open_read_session(ViewSelection::Exact(view), key, budget)
                    .map_err(native_transport),
            };
            let outcome = status.and_then(|status| {
                self.change_cleanup(key, Some(CleanupKind::Release(status)));
                self.release(&status, budget)
            });
            let mut cleanup = self.cleanup_lock();
            if let Some(index) = cleanup.iter().position(|entry| entry.key == key) {
                match outcome {
                    Ok(()) => {
                        let _removed = cleanup.remove(index);
                    }
                    Err(error) if cleanup_session_gone(&error) => {
                        let _removed = cleanup.remove(index);
                    }
                    Err(error) => {
                        cleanup[index].in_progress = false;
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                }
            }
        }
        if self.cleanup_lock().iter().any(|entry| {
            only_view.is_none_or(|view| view == entry.view)
                && !matches!(&entry.kind, CleanupKind::Opening)
        }) {
            return Err(first_error.unwrap_or_else(|| {
                SdkError::Protocol("Native read session cleanup remains pending".into())
            }));
        }
        Ok(())
    }

    /// Renew leases that have used half their term. Replay the last
    /// acknowledged revision on uncertain outcomes.
    fn renew_due(&self) {
        let now = now_ms();
        let due = self
            .lock()
            .iter()
            .filter(|(_, held)| {
                !held.release_only
                    && now
                        .and_then(|now| now.checked_add(held.lease_ms.div_euclid(2)))
                        .is_none_or(|horizon| horizon >= held.status.expires_at_ms)
            })
            .map(|(view, held)| (*view, held.status))
            .collect::<Vec<_>>();
        for (view, status) in due {
            if self
                .lock()
                .get(&view)
                .is_none_or(|entry| entry.release_only || entry.status.session != status.session)
            {
                continue;
            }
            let renewed = self
                .client
                .renew_read_session(&status, &bounded_budget(self.io_policy));
            apply_renewal(&mut self.lock(), view, status, renewed);
        }
        let budget = bounded_budget(self.io_policy);
        let _expires_instead = self.release_only(&budget);
        let _expires_instead = self.settle_cleanup(None, &budget);
    }

    fn release(
        &self,
        status: &NativeReadSessionStatus,
        budget: &RequestBudgetV1,
    ) -> Result<(), SdkError> {
        // Other sessions may still pin the view; this client owns only its lease.
        let _other_pins = self
            .client
            .release_read_session(status, budget)
            .map_err(native_transport)?;
        Ok(())
    }

    fn release_only(&self, budget: &RequestBudgetV1) -> Result<(), SdkError> {
        let releases = self
            .lock()
            .iter()
            .filter(|(_, entry)| entry.release_only)
            .map(|(view, entry)| (*view, entry.status))
            .collect::<Vec<_>>();
        let mut first_error = None;
        for (view, status) in releases {
            match self.release(&status, budget) {
                Ok(()) => {
                    let mut held = self.lock();
                    if held.get(&view).is_some_and(|entry| {
                        entry.release_only && entry.status.session == status.session
                    }) {
                        let _released = held.remove(&view);
                    }
                }
                Err(error) if cleanup_session_gone(&error) => {
                    let mut held = self.lock();
                    if held.get(&view).is_some_and(|entry| {
                        entry.release_only && entry.status.session == status.session
                    }) {
                        let _expired = held.remove(&view);
                    }
                }
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Mark before I/O so an uncertain Release cannot be reused as a query lease.
    fn release_now(&self) -> Result<(), SdkError> {
        {
            let mut held = self.lock();
            for entry in held.values_mut() {
                entry.release_only = true;
            }
        }
        let budget = bounded_budget(self.io_policy);
        let mut first_error = self.release_only(&budget).err();
        if let Err(error) = self.settle_cleanup(None, &budget)
            && first_error.is_none()
        {
            first_error = Some(error);
        }
        if self
            .cleanup_lock()
            .iter()
            .any(|entry| matches!(&entry.kind, CleanupKind::Opening))
            && first_error.is_none()
        {
            first_error = Some(SdkError::Protocol(
                "Native read session Open remains in flight".into(),
            ));
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Release every held lease after the owning client disappears.
    fn release_all(&self) {
        let _expires_instead = self.release_now();
    }

    fn next_tick(&self) -> Duration {
        let shortest = self
            .lock()
            .values()
            .map(|held| held.lease_ms.div_euclid(4))
            .min()
            .map_or(MAX_RENEW_TICK, Duration::from_millis);
        shortest.clamp(MIN_RENEW_TICK, MAX_RENEW_TICK)
    }
}

struct Renewer {
    _stop: mpsc::Sender<()>,
    _thread: JoinHandle<()>,
}

/// The read sessions this client holds for outstanding code-search cursors.
///
/// Ownership: the client opens at most one lease per view and reuses it for
/// every first page and continuation on that view, so later publications and
/// retention cannot reclaim the view between pages. A background renewer
/// thread renews each lease through the read-session renew protocol once half
/// its term has passed. When the client first reads a new view (the protected
/// head moved), it releases its leases on other views that have no
/// outstanding cursor; a view with a live cursor stays held until the client
/// is dropped. A held session avoids opening a new one for every page. The
/// daemon keeps released sessions for idempotent replay in a bounded window.
/// Dropping the client performs no I/O: it only signals the renewer, which releases
/// every remaining lease, each bounded by the client's I/O timeout, and exits
/// on its own. If the process exits first, the daemon's lease expiry reclaims
/// the sessions.
pub(crate) struct NativeViewSessions {
    table: Arc<SessionTable>,
    renewer: Mutex<Option<Renewer>>,
}

impl NativeViewSessions {
    pub(crate) fn new(client: NativeUdsClient, io_policy: ClientIoPolicy) -> Self {
        Self {
            table: Arc::new(SessionTable {
                client,
                io_policy,
                held: Mutex::new(BTreeMap::new()),
                cleanup: Mutex::new(Vec::new()),
            }),
            renewer: Mutex::new(None),
        }
    }

    /// Explicit cleanup for a short-lived caller. The renewer remains
    /// detached so an I/O timeout never makes client destruction block.
    pub(crate) fn release_now(&self) -> Result<(), SdkError> {
        self.table.release_now()
    }

    fn ensure_renewer(&self) -> Result<(), SdkError> {
        let mut renewer = match self.renewer.lock() {
            Ok(renewer) => renewer,
            Err(poisoned) => poisoned.into_inner(),
        };
        if renewer.is_some() {
            return Ok(());
        }
        let (stop, stopped) = mpsc::channel::<()>();
        let table = Arc::clone(&self.table);
        let thread = std::thread::Builder::new()
            .name("qidx-native-code-search-renew".into())
            .spawn(move || {
                while stopped.recv_timeout(table.next_tick())
                    == Err(mpsc::RecvTimeoutError::Timeout)
                {
                    table.renew_due();
                }
                // The owning client is gone: settle its leases off its thread.
                table.release_all();
            })
            .map_err(|error| SdkError::Protocol(format!("Native session renewer: {error}")))?;
        *renewer = Some(Renewer {
            _stop: stop,
            _thread: thread,
        });
        drop(renewer);
        Ok(())
    }

    /// Read one file page on `view`, reusing or retaining the client's session.
    fn page(
        &self,
        client: &NativeUdsClient,
        page_request: CodeSearchPageRequest,
        budget: &RequestBudgetV1,
        observation: Option<&mut ClientLexicalQueryObservationV1>,
    ) -> Result<TextQueryResponse, SdkError> {
        let CodeSearchPageRequest {
            view,
            generation,
            configured_instance,
            query,
            after,
        } = page_request;
        let consumed = after.as_ref().map(|cursor| cursor.cursor_id);
        let limit = query.top_k;
        self.on_view(
            client,
            view,
            configured_instance,
            consumed,
            budget,
            observation,
            |client, status, spec, observation| {
                client
                    .search_text_path_page_with_observation(
                        status,
                        spec.instance,
                        query,
                        after.map(|cursor| cursor.native),
                        budget,
                        observation,
                    )
                    .map_err(native_transport)
                    .and_then(|page| response(&generation, limit, &page))
            },
        )
    }

    /// Run one page read on `view` under this client's lease for that view,
    /// opening and retaining one first if needed.
    ///
    /// `consumed` names the SDK cursor the read continues, if any; the read
    /// returns its page and the SDK cursor it issues, if any. A lease with a
    /// live cursor or a page in flight is never idle.
    pub(crate) fn on_view<T>(
        &self,
        client: &NativeUdsClient,
        view: ViewRef,
        configured_instance: Option<IndexInstanceId>,
        consumed: Option<[u8; 32]>,
        budget: &RequestBudgetV1,
        mut observation: Option<&mut ClientLexicalQueryObservationV1>,
        read: impl FnOnce(
            &NativeUdsClient,
            &NativeReadSessionStatus,
            &InstanceSpec,
            Option<&mut ClientLexicalQueryObservationV1>,
        ) -> Result<(T, Option<[u8; 32]>), SdkError>,
    ) -> Result<T, SdkError> {
        let held = {
            let mut table = self.table.lock();
            match table.get_mut(&view) {
                Some(held) if held.release_only => {
                    return Err(SdkError::Protocol(
                        "Native read session Release is pending for this view".into(),
                    ));
                }
                Some(held) => {
                    held.pages_in_flight += 1;
                    Some((held.status, held.spec.clone()))
                }
                None => None,
            }
        };
        let opened = held.is_none();
        if opened {
            // A failed thread spawn must not leave a newly opened lease with
            // no renewer to keep its view available to the next page.
            self.ensure_renewer()?;
        }
        let (status, spec, opened_key) = match held {
            Some((status, spec)) => (status, spec, None),
            None => self
                .open(
                    client,
                    view,
                    configured_instance,
                    budget,
                    observation.as_deref_mut(),
                )
                .map(|(status, spec, key)| (status, spec, Some(key)))?,
        };
        let mut page = read(client, &status, &spec, observation);
        let mut released = Vec::new();
        let mut retained_open = false;
        {
            let mut held = self.table.lock();
            if opened {
                if let Some(existing) = held.get_mut(&view) {
                    // Another call opened this view first; keep its lease.
                    if existing.release_only {
                        page = Err(SdkError::Protocol(
                            "Native read session Release is pending for this view".into(),
                        ));
                    } else if let Ok((_, issued)) = &page {
                        record_page_cursor(&mut existing.active_cursors, consumed, *issued);
                    }
                    released.push((status, opened_key));
                } else {
                    // A newly read view retires this client's idle leases on
                    // other views; views with live cursors stay held.
                    let idle = held
                        .iter()
                        .filter(|(_, entry)| entry.is_idle())
                        .map(|(idle, _)| *idle)
                        .collect::<Vec<_>>();
                    for idle in idle {
                        if let Some(entry) = held.get_mut(&idle) {
                            entry.release_only = true;
                            released.push((entry.status, None));
                        }
                    }
                    let lease_ms =
                        now_ms().map_or(0, |now| status.expires_at_ms.saturating_sub(now));
                    let mut active_cursors = BTreeSet::new();
                    if let Ok((_, Some(issued))) = &page {
                        let _new_cursor = active_cursors.insert(*issued);
                    }
                    let _previous = held.insert(
                        view,
                        HeldSession {
                            status,
                            spec,
                            lease_ms,
                            active_cursors,
                            pages_in_flight: 0,
                            release_only: false,
                        },
                    );
                    retained_open = true;
                }
            } else if let Some(entry) = held.get_mut(&view)
                && entry.status.session == status.session
            {
                entry.pages_in_flight -= 1;
                if let Ok((_, issued)) = &page {
                    record_page_cursor(&mut entry.active_cursors, consumed, *issued);
                }
            }
            drop(held);
        }
        if retained_open && let Some(key) = opened_key {
            self.table.change_cleanup(key, None);
        }
        let cleanup_budget = bounded_budget(self.table.io_policy);
        for (status, key) in released {
            // Preserve an uncertain Release without hiding the page or its
            // original error after the cursor state has been updated.
            if let Some(key) = key {
                self.table
                    .change_cleanup(key, Some(CleanupKind::Release(status)));
            }
            let released = match self.table.release(&status, &cleanup_budget) {
                Ok(()) => true,
                Err(error) => cleanup_session_gone(&error),
            };
            if released {
                if let Some(key) = key {
                    self.table.change_cleanup(key, None);
                } else {
                    let mut held = self.table.lock();
                    if held.get(&status.view).is_some_and(|entry| {
                        entry.release_only && entry.status.session == status.session
                    }) {
                        let _released = held.remove(&status.view);
                    }
                }
            }
        }
        page.map(|(page, _)| page)
    }

    fn open(
        &self,
        client: &NativeUdsClient,
        view: ViewRef,
        configured: Option<IndexInstanceId>,
        budget: &RequestBudgetV1,
        mut observation: Option<&mut ClientLexicalQueryObservationV1>,
    ) -> Result<(NativeReadSessionStatus, InstanceSpec, [u8; 32]), SdkError> {
        // An earlier uncertain Open for this view must be settled before a
        // fresh key could create a second pin for the same caller.
        self.table
            .settle_cleanup(Some(view), &bounded_budget(self.table.io_policy))?;
        if self
            .table
            .lock()
            .get(&view)
            .is_some_and(|entry| entry.release_only)
        {
            return Err(SdkError::Protocol(
                "Native read session Release is pending for this view".into(),
            ));
        }
        let key = open_key(view)?;
        self.table.reserve_open(view, key)?;
        let status = match client.open_read_session_with_observation(
            ViewSelection::Exact(view),
            key,
            budget,
            observation.as_deref_mut(),
        ) {
            Ok(status) => status,
            Err(error) => {
                if open_ack_may_be_lost(&error) {
                    self.table.change_cleanup(key, Some(CleanupKind::Open));
                } else {
                    self.table.change_cleanup(key, None);
                }
                return Err(native_transport(error));
            }
        };
        let spec = client
            .read_session_profile_with_observation(&status, budget, observation)
            .map_err(native_transport)
            .and_then(|profile| select_text_instance(&profile.profile.instances, configured));
        match spec {
            Ok(spec) => Ok((status, spec, key)),
            Err(error) => {
                // The profile refusal is the caller's error; the lease is ours.
                self.table
                    .change_cleanup(key, Some(CleanupKind::Release(status)));
                let released = match self
                    .table
                    .release(&status, &bounded_budget(self.table.io_policy))
                {
                    Ok(()) => true,
                    Err(release_error) => cleanup_session_gone(&release_error),
                };
                if released {
                    self.table.change_cleanup(key, None);
                }
                Err(error)
            }
        }
    }
}

impl Drop for NativeViewSessions {
    fn drop(&mut self) {
        // Signal only. Closing the channel wakes the detached renewer, which
        // releases the remaining leases; no socket I/O runs on this thread.
        let renewer = match self.renewer.get_mut() {
            Ok(renewer) => renewer.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        // Dropping the sender closes the channel; dropping the handle detaches.
        drop(renewer);
    }
}

/// The text member serving `.text()`: the configured instance, otherwise the
/// unique captured instance whose kind is the product Native Text adapter.
fn select_text_instance(
    instances: &[InstanceSpec],
    configured: Option<IndexInstanceId>,
) -> Result<InstanceSpec, SdkError> {
    let (mut matches, missing) = configured.map_or_else(
        || {
            (
                instances
                    .iter()
                    .filter(|spec| spec.kind == NATIVE_TEXT_KIND)
                    .collect::<Vec<_>>(),
                SdkError::Usage(
                    "the Native profile has no product Text member; set ConnectOptions::with_native_text_instance"
                        .into(),
                ),
            )
        },
        |id| {
            (
                instances
                    .iter()
                    .filter(|spec| spec.instance == id)
                    .collect::<Vec<_>>(),
                native(NativeQueryError::InstanceMismatch),
            )
        },
    );
    let spec = matches.pop().ok_or(missing)?;
    if !matches.is_empty() {
        return Err(SdkError::Usage(
            "the Native profile has several Text members; set ConnectOptions::with_native_text_instance"
                .into(),
        ));
    }
    Ok(spec.clone())
}

/// Execute the default `.text()` code search on a Native connection.
pub(crate) fn execute(
    route: &NativeTextRoute<'_>,
    state: TextQueryBuilderState,
    mut observation: Option<&mut ClientLexicalQueryObservationV1>,
) -> Result<TextQueryResponse, SdkError> {
    if state.syntax != TextQuerySyntax::CodeSearch {
        return Err(SdkError::UnsupportedNativeQuery {
            capability: crate::LegacyQueryCapabilityV1::Text,
        });
    }
    let query_text = state
        .query_text
        .ok_or_else(|| SdkError::Usage("lexical query text is required".into()))?;
    let selection = state
        .selection
        .ok_or_else(|| SdkError::Usage("lexical generation selection is required".into()))?;
    let top_k = accepted_top_k(
        "lexical",
        state
            .top_k
            .ok_or_else(|| SdkError::Usage("lexical top_k is required".into()))?,
    )?;
    let (repo_id, revision_id, pinned) = match selection {
        GenerationSelector::Active {
            repo_id,
            revision_id,
        } => (repo_id, revision_id, None),
        GenerationSelector::Pinned(pin) => {
            (pin.repo_id, pin.revision_id, Some(pin.manifest_generation))
        }
        GenerationSelector::ResolvedActive { .. } => {
            return Err(SdkError::Usage(
                "Native has no Legacy activation token; select active() or pinned()".into(),
            ));
        }
    };
    let after = state
        .after
        .as_ref()
        .map(|token| checked_cursor(token, &repo_id, &revision_id, pinned))
        .transpose()?;
    let budget = route.budget();
    let view = if let Some(cursor) = &after {
        // A continuation reads the view its first page read, even after later
        // publications move the protected head. The caller-selected Native
        // scope remains the SDK authority for this path.
        cursor.native.view
    } else {
        let head = route
            .client
            .resolve_protected_source_head_with_observation(
                repo_id.clone(),
                revision_id.clone(),
                &budget,
                observation.as_deref_mut(),
            )
            .map_err(native_transport)?;
        let view = head.view.identity;
        if pinned.is_some_and(|pinned| pinned.get() != view.sequence) {
            return Err(SdkError::Remote {
                code: SearchPlaneErrorCodeV2::GenerationMismatch,
                message: "the pinned generation no longer names the protected Source head".into(),
                repair: None,
            });
        }
        view
    };
    if !selection_contains(route.selection, view) {
        return Err(SdkError::Usage(
            "the Native view lies outside this connection's Native selection".into(),
        ));
    }
    let generation =
        GenerationPin::new(repo_id, revision_id, ManifestGeneration::new(view.sequence));
    let mut request = native_code_search_request(query_text, top_k);
    request.constraints = state.constraints;
    route.sessions.page(
        route.client,
        CodeSearchPageRequest {
            view,
            generation,
            configured_instance: route.text_instance,
            query: request,
            after,
        },
        &budget,
        observation,
    )
}

fn response(
    generation: &GenerationPin,
    top_k: u32,
    page: &NativeTextPathPageV1,
) -> Result<(TextQueryResponse, Option<[u8; 32]>), SdkError> {
    let ranking = &page.ranked.ranking;
    let returned = u32::try_from(ranking.hits.len())
        .map_err(|error| SdkError::Protocol(format!("Native file page width: {error}")))?;
    if returned > top_k || ranking.hits.len() != page.ranked.repositories.len() {
        return Err(SdkError::Protocol(
            "Native file page exceeds its cap or lost a repository".into(),
        ));
    }
    // A code-search page pairs every row with its representative match.
    if !ranking.hits.is_empty() && page.code_search.len() != ranking.hits.len() {
        return Err(SdkError::Protocol(
            "Native code-search page carries no match per file".into(),
        ));
    }
    let mut results = Vec::new();
    results
        .try_reserve_exact(ranking.hits.len())
        .map_err(|error| SdkError::Protocol(format!("Native file page allocation: {error}")))?;
    for ((hit, repository), matched) in ranking
        .hits
        .iter()
        .zip(&page.ranked.repositories)
        .zip(&page.code_search)
    {
        if *repository != generation.repo_id {
            return Err(SdkError::Protocol(
                "the protected Source head returned a file of another repository".into(),
            ));
        }
        results.push(file_candidate(generation, hit, matched.as_ref())?);
    }
    let count = CandidateCountV1::Exact(page.remaining);
    let window = QueryResultWindowV2::pageable(
        returned,
        count,
        page.after.is_some(),
        vec![LaneTraceV1::new(LANE, true, returned != 0).with_candidates(count)],
    )
    .map_err(|error| SdkError::Protocol(format!("Native file page window: {error}")))?;
    let (next_cursor, issued) = if let Some(cursor) = &page.after {
        let id = open_key(cursor.view)?;
        (
            Some(encode_cursor(cursor, &generation.revision_id, id)?),
            Some(id),
        )
    } else {
        (None, None)
    };
    let response = TextQueryResponse {
        generation: generation.clone(),
        selected_active_head: None,
        rank_unit: TextRankUnit::File,
        results,
        window,
        explanation: SearchExplanation::empty(),
        file_owner_rows: None,
        next_cursor,
    };
    Ok((response, issued))
}

/// A distinct-file row carrying its Native Source record identity and the
/// position of its representative match.
///
/// Native reports no Legacy preview metadata or Source byte SHA-256, so
/// those fields stay empty. The match maps as Legacy rows did: a content
/// match gives its lines and its excerpt with one highlight, and a path match
/// gives no lines and shows the path with the matched bytes highlighted.
fn file_candidate(
    generation: &GenerationPin,
    hit: &NativeTextHit,
    matched: Option<&NativeCodeSearchMatch>,
) -> Result<LexicalCandidate, SdkError> {
    let (lines, excerpt) = match_position(hit, matched)?;
    let path = hit.path.as_str();
    let native_score = hit.score.get();
    #[expect(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        reason = "the public row score is an f32, checked for an exact round trip"
    )]
    let score = native_score as f32;
    if f64::from(score).to_bits() != native_score.to_bits() {
        return Err(SdkError::Protocol(
            "Native CodeSearch score cannot be represented exactly in the public result".into(),
        ));
    }
    Ok(LexicalCandidate {
        source_repo_id: generation.repo_id.clone(),
        source: None,
        preview: None,
        candidate_id: code_search_file_candidate_id(generation.repo_id.as_str(), path)
            .map_err(|error| SdkError::Protocol(error.to_string()))?,
        repo_id: generation.repo_id.clone(),
        revision_id: generation.revision_id.clone(),
        manifest_generation: generation.manifest_generation,
        repo_relative_path: RepoRelativePath::new(path),
        lines,
        score,
        excerpt,
        native_source: Some(NativeSourceIdentity {
            owner: hit.owner,
            source: hit.source,
        }),
    })
}

/// The row's lines and excerpt from its validated representative match.
fn match_position(
    hit: &NativeTextHit,
    matched: Option<&NativeCodeSearchMatch>,
) -> Result<(Option<LexicalLineRange>, Option<LexicalMatchExcerpt>), SdkError> {
    let offset = |value: u64| {
        u32::try_from(value)
            .map_err(|error| SdkError::Protocol(format!("Native match offset width: {error}")))
    };
    let highlighted = |snippet: String, start: u64, end: u64| -> Result<_, SdkError> {
        let start = offset(start)?;
        let len = offset(end)?
            .checked_sub(start)
            .ok_or_else(|| SdkError::Protocol("Native match range is reversed".into()))?;
        Ok(LexicalMatchExcerpt {
            snippet,
            snippet_hit_offset: Some(start),
            highlights: vec![HighlightSpan { start, len }],
        })
    };
    Ok(match matched {
        None => (None, None),
        Some(NativeCodeSearchMatch::Path { start, end }) => {
            (None, Some(highlighted(hit.path.clone(), *start, *end)?))
        }
        Some(NativeCodeSearchMatch::Content {
            start,
            end,
            start_line,
            end_line,
            excerpt,
        }) => {
            let lines = Some(LexicalLineRange {
                start_line: *start_line,
                end_line: *end_line,
            });
            let excerpt = excerpt
                .as_ref()
                .map(|excerpt| {
                    let context = excerpt.context.start;
                    let relative = |value: u64| {
                        value.checked_sub(context).ok_or_else(|| {
                            SdkError::Protocol("Native match precedes its excerpt".into())
                        })
                    };
                    highlighted(excerpt.text.clone(), relative(*start)?, relative(*end)?)
                })
                .transpose()?;
            (lines, excerpt)
        }
    })
}

pub(crate) fn selection_contains(selection: ViewSelection, view: ViewRef) -> bool {
    match selection {
        ViewSelection::Exact(expected) => expected == view,
        ViewSelection::Latest { workspace, epoch } => {
            view.workspace == workspace && view.epoch == epoch
        }
    }
}

/// A fresh idempotency key for one read-session open. Process identity,
/// wall time and a process-local counter keep concurrent callers apart.
pub(crate) fn open_key(view: ViewRef) -> Result<[u8; 32], SdkError> {
    static OPENS: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| SdkError::Protocol(format!("Native open key clock: {error}")))?;
    let mut hash = Sha256::new();
    hash.update(b"quanta-index/sdk/native-code-search/open-key/v1\0");
    hash.update(std::process::id().to_le_bytes());
    hash.update(now.as_nanos().to_le_bytes());
    hash.update(OPENS.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    hash.update(view.sequence.to_le_bytes());
    hash.update(view.digest.0);
    Ok(hash.finalize().into())
}

/// Wall-clock milliseconds, matching the daemon's lease expiry clock. None
/// when the clock cannot be read, which makes every lease due for renewal.
fn now_ms() -> Option<u64> {
    let Ok(elapsed) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return None;
    };
    let Ok(millis) = u64::try_from(elapsed.as_millis()) else {
        return None;
    };
    Some(millis)
}

fn encode_cursor(
    cursor: &NativeTextPathCursorV1,
    revision_id: &RevisionId,
    cursor_id: [u8; 32],
) -> Result<ContinuationTokenV2, SdkError> {
    encode_token(
        CURSOR_TAG,
        &CodeSearchCursor {
            revision_id: revision_id.clone(),
            cursor_id,
            native: cursor.clone(),
        },
    )
}

/// Only the token [`encode_cursor`] mints for a cursor decodes to it. The
/// encoder has always written text field identifiers, so byte-string keys,
/// like any other re-encoding, are an alias and are refused.
fn decode_cursor(token: &ContinuationTokenV2) -> Result<CodeSearchCursor, SdkError> {
    decode_token(CURSOR_TAG, "the Native code-search route", token)
}

/// `tag` + lowercase hex of the cursor's canonical CBOR.
pub(crate) fn encode_token<T: serde::Serialize>(
    tag: &str,
    cursor: &T,
) -> Result<ContinuationTokenV2, SdkError> {
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(cursor, &mut bytes)
        .map_err(|error| SdkError::Serialization(format!("Native cursor: {error}")))?;
    let mut token = String::from(tag);
    for byte in bytes {
        write!(token, "{byte:02x}")
            .map_err(|error| SdkError::Serialization(format!("Native cursor: {error}")))?;
    }
    ContinuationTokenV2::new(token)
        .map_err(|error| SdkError::Serialization(format!("Native cursor: {error}")))
}

/// Decode a token minted by [`encode_token`] with the same `tag`.
///
/// Anything else is a typed `CursorInvalid` refusal naming `route`. Only the
/// canonical token of the decoded cursor is accepted, so no other text, such
/// as trailing bytes, signed or uppercase hex, aliases a minted token.
pub(crate) fn decode_token<T: serde::de::DeserializeOwned + serde::Serialize>(
    tag: &str,
    route: &str,
    token: &ContinuationTokenV2,
) -> Result<T, SdkError> {
    let cursor: T = decode_token_body(tag, route, token)?;
    if encode_token(tag, &cursor)?.as_str() != token.as_str() {
        return Err(invalid_cursor_token(
            "not the canonical token of its cursor",
        ));
    }
    Ok(cursor)
}

fn invalid_cursor_token(message: &str) -> SdkError {
    SdkError::Remote {
        code: SearchPlaneErrorCodeV2::CursorInvalid,
        message: format!("Native cursor: {message}"),
        repair: None,
    }
}

fn decode_token_body<T: serde::de::DeserializeOwned>(
    tag: &str,
    route: &str,
    token: &ContinuationTokenV2,
) -> Result<T, SdkError> {
    let hex = token
        .as_str()
        .strip_prefix(tag)
        .ok_or_else(|| invalid_cursor_token(&format!("not minted by {route}")))?;
    let width = hex
        .len()
        .checked_div(2)
        .filter(|width| {
            hex.len() <= MAX_CURSOR_TOKEN_BYTES && hex.len().is_multiple_of(2) && *width != 0
        })
        .ok_or_else(|| invalid_cursor_token("malformed length"))?;
    if !hex
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid_cursor_token("not lowercase hexadecimal"));
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(width)
        .map_err(|_error| invalid_cursor_token("allocation refused"))?;
    for pair in hex.as_bytes().chunks_exact(2) {
        let text =
            std::str::from_utf8(pair).map_err(|_error| invalid_cursor_token("not hexadecimal"))?;
        bytes.push(
            u8::from_str_radix(text, 16)
                .map_err(|_error| invalid_cursor_token("not hexadecimal"))?,
        );
    }
    let mut input = bytes.as_slice();
    let cursor: T = ciborium::de::from_reader(&mut input)
        .map_err(|error| invalid_cursor_token(&error.to_string()))?;
    if !input.is_empty() {
        return Err(invalid_cursor_token("trailing CBOR"));
    }
    Ok(cursor)
}

fn checked_cursor(
    token: &ContinuationTokenV2,
    repository: &RepoId,
    revision_id: &RevisionId,
    pinned: Option<ManifestGeneration>,
) -> Result<CodeSearchCursor, SdkError> {
    let decoded = decode_cursor(token)?;
    if decoded.native.repository != *repository
        || decoded.revision_id != *revision_id
        || pinned.is_some_and(|pinned| pinned.get() != decoded.native.view.sequence)
    {
        return Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::QueryCursorGenerationMismatch,
            message: "the continuation belongs to another repository, revision or Native view"
                .into(),
            repair: None,
        });
    }
    Ok(decoded)
}

/// Resolve CLI continuation scope from the serving root after validating the
/// cursor's public repo/revision/generation binding. The decoded view is only
/// a lookup hint; the daemon proves the historical protected Source route.
pub fn resolve_native_code_search_continuation_view_v1<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
    repository: &RepoId,
    revision_id: &RevisionId,
    generation: ManifestGeneration,
    token: &ContinuationTokenV2,
    budget: &RequestBudgetV1,
) -> Result<quanta_index_contract::native_protocol::NativeRootScopedViewV1, SdkError> {
    let cursor = checked_cursor(token, repository, revision_id, Some(generation))?;
    client
        .resolve_protected_source_view_v1(
            repository.clone(),
            revision_id.clone(),
            cursor.native.view,
            budget,
        )
        .map_err(native_transport)
}

pub(crate) fn native(error: NativeQueryError) -> SdkError {
    // The SDK refuses the same grammar the Legacy daemon refused, with the
    // same typed code; every other Native refusal keeps its Native type.
    if let NativeQueryError::Transport(NativeUdsError::QueryRejected { code, message }) = error {
        return SdkError::Remote {
            code,
            message,
            repair: None,
        };
    }
    SdkError::Native(Box::new(error))
}

pub(crate) fn native_transport(error: NativeUdsError) -> SdkError {
    native(NativeQueryError::Transport(error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::FiniteScore;
    use quanta_index_contract::index::{
        CollectionId, Digest, IndexInstanceId, ObjectRef, SlotId, TenantId, WorkspaceKey,
        WorktreeId,
    };

    fn view(sequence: u64) -> ViewRef {
        ViewRef {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
            sequence,
            digest: Digest([5; 32]),
        }
    }

    #[test]
    fn file_candidate_rejects_unrepresentable_rank_scores() -> Result<(), Box<dyn std::error::Error>>
    {
        use quanta_index_contract::index::{OwnerKey, ProducerId, StableOwnerId};

        let generation = GenerationPin::new(
            RepoId::new("repo-e2e")?,
            RevisionId::new("revision-a")?,
            ManifestGeneration::new(7),
        );
        let mut hit = NativeTextHit {
            owner: OwnerKey {
                producer: ProducerId([1; 16]),
                stable_id: StableOwnerId([2; 16]),
            },
            source: ObjectRef(Digest([3; 32])),
            path: "src/alpha.rs".into(),
            score: FiniteScore::new(f64::from(u16::MAX))?,
        };
        let supported = file_candidate(&generation, &hit, None)?;
        assert_eq!(supported.score.to_bits(), f32::from(u16::MAX).to_bits());

        // Overflow and a distinct Native rank that would round to public 1.0.
        for value in [f64::MAX, 1.0 + f64::EPSILON] {
            hit.score = FiniteScore::new(value)?;
            assert!(matches!(
                file_candidate(&generation, &hit, None),
                Err(SdkError::Protocol(_))
            ));
        }
        Ok(())
    }

    #[test]
    fn cursor_tokens_round_trip_and_refuse_foreign_or_corrupt_text() {
        let cursor = NativeTextPathCursorV1 {
            view: view(7),
            instance: IndexInstanceId([6; 16]),
            query: Digest([7; 32]),
            artifact: ObjectRef(Digest([8; 32])),
            score: FiniteScore::new(1.5).expect("finite score"),
            repository: RepoId::new("repo-e2e").expect("valid repository"),
            path: "src/alpha.rs".into(),
        };
        let revision = RevisionId::new("revision-a").expect("valid revision");
        let other_revision = RevisionId::new("revision-b").expect("valid revision");
        let token = encode_cursor(&cursor, &revision, [11; 32]).expect("cursor encodes");
        assert!(token.as_str().starts_with(CURSOR_TAG));
        assert_eq!(
            decode_cursor(&token).expect("cursor decodes"),
            CodeSearchCursor {
                revision_id: revision.clone(),
                cursor_id: [11; 32],
                native: cursor.clone(),
            }
        );
        assert_eq!(
            checked_cursor(
                &token,
                &cursor.repository,
                &revision,
                Some(ManifestGeneration::new(7))
            )
            .expect("matching revision and view continue"),
            CodeSearchCursor {
                revision_id: revision.clone(),
                cursor_id: [11; 32],
                native: cursor.clone(),
            }
        );
        for (repository, revision_id, pinned) in [
            (cursor.repository.clone(), other_revision, None),
            (
                RepoId::new("other-repo").expect("valid repository"),
                revision.clone(),
                None,
            ),
            (
                cursor.repository.clone(),
                revision.clone(),
                Some(ManifestGeneration::new(8)),
            ),
        ] {
            assert!(matches!(
                checked_cursor(&token, &repository, &revision_id, pinned),
                Err(SdkError::Remote {
                    code: SearchPlaneErrorCodeV2::QueryCursorGenerationMismatch,
                    ..
                })
            ));
        }
        let mut odd = token.as_str().to_owned();
        let _last = odd.pop();
        let unversioned = token
            .as_str()
            .replacen(CURSOR_TAG, "native-code-search-path:", 1);
        for refused in [
            ContinuationTokenV2::new("legacy-cursor").expect("token"),
            ContinuationTokenV2::new(unversioned).expect("token"),
            ContinuationTokenV2::new(odd).expect("token"),
            ContinuationTokenV2::new(format!("{CURSOR_TAG}zz")).expect("token"),
            ContinuationTokenV2::new(format!("{CURSOR_TAG}00")).expect("token"),
            ContinuationTokenV2::new(format!("{}f6", token.as_str())).expect("token"),
        ] {
            assert!(matches!(
                decode_cursor(&refused),
                Err(SdkError::Remote {
                    code: SearchPlaneErrorCodeV2::CursorInvalid,
                    ..
                })
            ));
        }
    }

    /// Tampered `.text()` tokens are refused or another bound cursor.
    ///
    /// Every one-character substitution from a fixed alphabet, every
    /// truncation, fixed extensions and the uppercase token. A truncation,
    /// an extension or the uppercase token is never the canonical token of
    /// any cursor, so each one is refused with a typed cursor code. A
    /// substitution is refused the same way, or is the canonical token of
    /// another cursor of the same repository revision and pinned view, which
    /// the daemon then binds to its own view, instance and query. No tampered
    /// text continues the original page under another revision, and none
    /// aliases the original.
    #[test]
    fn tampered_continuation_tokens_are_refused_or_another_bound_cursor() {
        let repo = RepoId::new("repo-e2e").expect("valid repository");
        let main = RevisionId::new("main").expect("valid revision");
        let cursor = NativeTextPathCursorV1 {
            view: view(7),
            instance: IndexInstanceId([6; 16]),
            query: Digest([7; 32]),
            artifact: ObjectRef(Digest([8; 32])),
            score: FiniteScore::new(1.5).expect("finite score"),
            repository: repo.clone(),
            path: "src/alpha.rs".into(),
        };
        let original = CodeSearchCursor {
            revision_id: main.clone(),
            cursor_id: [11; 32],
            native: cursor.clone(),
        };
        let token = encode_cursor(&cursor, &main, [11; 32]).expect("cursor encodes");
        let text = token.as_str().to_owned();
        // `(variant, whether it may be another canonical cursor)`.
        let mut variants = Vec::new();
        for (index, original) in text.char_indices() {
            for replacement in ['0', '7', 'f', 'F', 'g', '+', ':'] {
                if replacement != original {
                    let mut mutated = text.clone();
                    mutated.replace_range(index..index.saturating_add(1), &replacement.to_string());
                    variants.push((mutated, true));
                }
            }
        }
        for length in 0..text.len() {
            variants.push((text.get(..length).expect("ASCII token").to_owned(), false));
        }
        for suffix in ["0", "00", "f6", "ff00"] {
            variants.push((format!("{text}{suffix}"), false));
        }
        variants.push((text.to_uppercase(), false));
        let pinned = Some(ManifestGeneration::new(7));
        for (variant, may_bind) in &variants {
            // A text the token type refuses never reaches the route.
            let Ok(tampered) = ContinuationTokenV2::new(variant.clone()) else {
                continue;
            };
            let outcome = checked_cursor(&tampered, &repo, &main, pinned);
            if let Ok(decoded) = &outcome {
                assert!(*may_bind, "{variant} decodes to {decoded:?}");
                assert_ne!(*decoded, original, "{variant} aliases the original token");
                assert_eq!(decoded.native.repository, repo, "{variant}");
                assert_eq!(decoded.revision_id, main, "{variant}");
                assert_eq!(decoded.native.view.sequence, 7, "{variant}");
            } else {
                assert!(
                    matches!(
                        &outcome,
                        Err(SdkError::Remote {
                            code: SearchPlaneErrorCodeV2::CursorInvalid
                                | SearchPlaneErrorCodeV2::QueryCursorGenerationMismatch,
                            ..
                        })
                    ),
                    "{variant}: untyped refusal {outcome:?}"
                );
            }
        }
    }

    #[test]
    fn cursor_v2_wire_keeps_field_order_and_rejects_unknown_duplicate_or_missing_fields() {
        use ciborium::value::Value;
        use std::fmt::Write as _;

        const V2_TAG: &str = "native-code-search-path-v2:";

        let native = NativeTextPathCursorV1 {
            view: view(7),
            instance: IndexInstanceId([6; 16]),
            query: Digest([7; 32]),
            artifact: ObjectRef(Digest([8; 32])),
            score: FiniteScore::new(1.5).expect("finite score"),
            repository: RepoId::new("repo-e2e").expect("valid repository"),
            path: "src/alpha.rs".into(),
        };
        let revision_id = RevisionId::new("revision-a").expect("valid revision");
        let cursor_id = [11; 32];
        let fields = vec![
            (
                Value::Text("revision_id".into()),
                Value::serialized(&revision_id).expect("revision value"),
            ),
            (
                Value::Text("cursor_id".into()),
                Value::serialized(&cursor_id).expect("cursor id value"),
            ),
            (
                Value::Text("native".into()),
                Value::serialized(&native).expect("native cursor value"),
            ),
        ];
        let token = encode_cursor(&native, &revision_id, cursor_id).expect("cursor encodes");
        let hex = token.as_str().strip_prefix(V2_TAG).expect("v2 tag");
        let actual = hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("hex pair"), 16)
                    .expect("hex byte")
            })
            .collect::<Vec<_>>();
        let mut expected = Vec::new();
        ciborium::ser::into_writer(&Value::Map(fields.clone()), &mut expected)
            .expect("old v2 map encoding");
        assert_eq!(actual, expected);

        let token_for = |fields: Vec<(Value, Value)>| {
            let mut wire = Vec::new();
            ciborium::ser::into_writer(&Value::Map(fields), &mut wire).expect("CBOR map");
            let mut text = String::from(V2_TAG);
            for byte in wire {
                write!(text, "{byte:02x}").expect("hex cursor byte");
            }
            ContinuationTokenV2::new(text).expect("cursor token")
        };
        let byte_fields = fields
            .iter()
            .map(|(name, value)| {
                let name = name.as_text().expect("v2 field name");
                (Value::Bytes(name.as_bytes().to_vec()), value.clone())
            })
            .collect::<Vec<_>>();
        let mut mixed_fields = fields.clone();
        *mixed_fields.first_mut().expect("first v2 field") =
            byte_fields.first().expect("first v2 byte field").clone();
        let decoded = CodeSearchCursor {
            revision_id,
            cursor_id,
            native,
        };
        assert_eq!(
            decode_cursor(&token_for(fields.clone())).expect("canonical v2 map"),
            decoded
        );
        // The same fields under byte-string keys name the same cursor with a
        // different token: an alias, refused like any non-canonical text.
        for alias_fields in [byte_fields.clone(), mixed_fields] {
            assert!(matches!(
                decode_cursor(&token_for(alias_fields)),
                Err(SdkError::Remote {
                    code: SearchPlaneErrorCodeV2::CursorInvalid,
                    ..
                })
            ));
        }
        for name in [
            Value::Text("foreign".into()),
            Value::Bytes(b"foreign".to_vec()),
        ] {
            let mut unknown = fields.clone();
            unknown.push((name, Value::Null));
            assert!(matches!(
                decode_cursor(&token_for(unknown)),
                Err(SdkError::Remote {
                    code: SearchPlaneErrorCodeV2::CursorInvalid,
                    ..
                })
            ));
        }
        for (field, byte_field) in fields.iter().zip(byte_fields) {
            let mut duplicate = fields.clone();
            duplicate.push(byte_field);
            assert!(matches!(
                decode_cursor(&token_for(duplicate)),
                Err(SdkError::Remote {
                    code: SearchPlaneErrorCodeV2::CursorInvalid,
                    ..
                })
            ));
            let missing = fields
                .iter()
                .filter(|entry| &entry.0 != &field.0)
                .cloned()
                .collect();
            assert!(matches!(
                decode_cursor(&token_for(missing)),
                Err(SdkError::Remote {
                    code: SearchPlaneErrorCodeV2::CursorInvalid,
                    ..
                })
            ));
        }
    }

    #[test]
    fn terminal_cursor_replay_preserves_other_live_page_sequence() {
        let first = [1; 32];
        let second = [2; 32];
        let next = [3; 32];
        let mut active = BTreeSet::new();
        record_page_cursor(&mut active, None, Some(first));
        record_page_cursor(&mut active, None, Some(second));
        record_page_cursor(&mut active, Some(first), None);
        record_page_cursor(&mut active, Some(first), None);
        assert_eq!(active, BTreeSet::from([second]));
        record_page_cursor(&mut active, Some(second), Some(next));
        assert_eq!(active, BTreeSet::from([next]));
    }

    #[test]
    fn a_session_with_a_page_in_flight_is_not_idle() {
        use quanta_index_contract::native_protocol::{NativeReadSessionId, NativeReadSessionState};

        let mut active = HeldSession {
            status: NativeReadSessionStatus {
                session: NativeReadSessionId([1; 32]),
                view: view(3),
                expires_at_ms: 100,
                revision: 1,
                state: NativeReadSessionState::Active,
            },
            spec: spec(2, NATIVE_TEXT_KIND),
            lease_ms: 100,
            active_cursors: BTreeSet::new(),
            pages_in_flight: 1,
            release_only: false,
        };
        assert!(!active.is_idle());
        active.pages_in_flight = 0;
        assert!(active.is_idle());
        record_page_cursor(&mut active.active_cursors, None, Some([1; 32]));
        assert!(!active.is_idle());
    }

    #[test]
    fn stale_renewal_cannot_replace_or_remove_new_session() {
        use quanta_index_contract::native_protocol::{
            NativeIndexWireError, NativeReadSessionState,
        };

        let unavailable = || {
            Err(NativeUdsError::Refused(NativeIndexWireError {
                code: NativeIndexErrorCode::SnapshotUnavailable,
                message: "session absent".into(),
            }))
        };
        let view = view(3);
        let old = NativeReadSessionStatus {
            session: quanta_index_contract::native_protocol::NativeReadSessionId([1; 32]),
            view,
            expires_at_ms: 100,
            revision: 1,
            state: NativeReadSessionState::Active,
        };
        let mut current = old;
        current.session = quanta_index_contract::native_protocol::NativeReadSessionId([2; 32]);
        let mut held = BTreeMap::from([(
            view,
            HeldSession {
                status: current,
                spec: spec(2, NATIVE_TEXT_KIND),
                lease_ms: 100,
                active_cursors: BTreeSet::new(),
                pages_in_flight: 0,
                release_only: false,
            },
        )]);
        let mut stale_renewed = old;
        stale_renewed.revision = 2;
        apply_renewal(&mut held, view, old, Ok(stale_renewed));
        assert_eq!(held.get(&view).map(|entry| entry.status), Some(current));
        apply_renewal(&mut held, view, old, unavailable());
        assert_eq!(held.get(&view).map(|entry| entry.status), Some(current));
        let mut advanced = current;
        advanced.revision = 2;
        if let Some(entry) = held.get_mut(&view) {
            entry.status = advanced;
        }
        apply_renewal(&mut held, view, current, Ok(stale_renewed));
        apply_renewal(&mut held, view, current, unavailable());
        assert_eq!(held.get(&view).map(|entry| entry.status), Some(advanced));
        current = advanced;
        apply_renewal(&mut held, view, current, unavailable());
        assert!(!held.contains_key(&view));
    }

    #[test]
    fn uncertain_renewal_preserves_the_acknowledged_revision_and_live_cursor() {
        use quanta_index_contract::native_protocol::{
            NativeIndexWireError, NativeReadSessionId, NativeReadSessionState,
        };

        let view = view(3);
        let acknowledged = NativeReadSessionStatus {
            session: NativeReadSessionId([1; 32]),
            view,
            expires_at_ms: 100,
            revision: 1,
            state: NativeReadSessionState::Active,
        };
        let live = BTreeSet::from([[3; 32]]);
        let mut held = BTreeMap::from([(
            view,
            HeldSession {
                status: acknowledged,
                spec: spec(2, NATIVE_TEXT_KIND),
                lease_ms: 100,
                active_cursors: live.clone(),
                pages_in_flight: 0,
                release_only: false,
            },
        )]);
        for error in [
            NativeUdsError::Transport("renewal ACK lost".into()),
            NativeUdsError::Refused(NativeIndexWireError {
                code: NativeIndexErrorCode::Cancelled,
                message: "renewal cancelled".into(),
            }),
        ] {
            apply_renewal(&mut held, view, acknowledged, Err(error));
            let session = held
                .get(&view)
                .expect("uncertain renewal retains the session");
            assert_eq!(session.status, acknowledged);
            assert_eq!(session.active_cursors, live);
        }
        let next = NativeReadSessionStatus {
            expires_at_ms: 200,
            revision: 2,
            ..acknowledged
        };
        apply_renewal(&mut held, view, acknowledged, Ok(next));
        let session = held.get(&view).expect("renewed session stays held");
        assert_eq!(session.status, next);
        assert_eq!(session.active_cursors, live);
    }

    #[test]
    fn selection_scope_and_open_keys_are_exact() {
        let held = view(3);
        assert!(selection_contains(ViewSelection::Exact(held), held));
        assert!(!selection_contains(ViewSelection::Exact(view(4)), held));
        assert!(selection_contains(
            ViewSelection::Latest {
                workspace: held.workspace,
                epoch: 1
            },
            held
        ));
        assert!(!selection_contains(
            ViewSelection::Latest {
                workspace: held.workspace,
                epoch: 2
            },
            held
        ));
        let first = open_key(held).expect("open key");
        let second = open_key(held).expect("open key");
        assert_ne!(first, [0; 32]);
        assert_ne!(first, second, "every open uses a fresh idempotency key");
    }

    fn spec(instance: u8, kind: quanta_index_contract::index::IndexKind) -> InstanceSpec {
        use quanta_index_contract::index::{
            GroupActivation, InputDomain, PublicationGroupId, SchemaId, StorageFormatId,
        };
        InstanceSpec {
            instance: IndexInstanceId([instance; 16]),
            kind,
            group: PublicationGroupId([3; 16]),
            activation: GroupActivation::CoreRequired,
            input_domain: InputDomain::SourceLocal,
            schema: SchemaId([4; 16]),
            policy: Digest([5; 32]),
            format: StorageFormatId([6; 16]),
        }
    }

    #[test]
    fn default_text_member_is_the_unique_product_text_kind() {
        let semantic = spec(1, quanta_index_contract::index::IndexKind([9; 16]));
        let text = spec(2, NATIVE_TEXT_KIND);
        assert_eq!(
            select_text_instance(&[semantic.clone(), text.clone()], None).expect("unique"),
            text
        );
        let configured = select_text_instance(
            &[semantic.clone(), text.clone()],
            Some(IndexInstanceId([1; 16])),
        )
        .expect("an explicit member overrides discovery");
        assert_eq!(configured, semantic);
        for instances in [
            vec![semantic],
            vec![text.clone(), spec(3, NATIVE_TEXT_KIND)],
        ] {
            assert!(matches!(
                select_text_instance(&instances, None),
                Err(SdkError::Usage(message)) if message.contains("with_native_text_instance")
            ));
        }
        assert!(matches!(
            select_text_instance(&[text], Some(IndexInstanceId([7; 16]))),
            Err(SdkError::Native(error))
                if matches!(*error, NativeQueryError::InstanceMismatch)
        ));
    }

    #[cfg(unix)]
    fn cleanup_peer(
        listener: std::os::unix::net::UnixListener,
        replies: Vec<Option<quanta_index_contract::native_protocol::NativeIndexResponse>>,
    ) -> std::thread::JoinHandle<Vec<quanta_index_contract::native_protocol::NativeIndexRequest>>
    {
        use quanta_index_contract::native_protocol::{
            NativeIndexRequestEnvelope, NativeIndexResponseEnvelope,
        };
        use std::io::Write as _;
        use std::time::Instant;

        std::thread::spawn(move || {
            listener
                .set_nonblocking(true)
                .expect("nonblocking listener");
            let mut requests = Vec::new();
            for reply in replies {
                let deadline = Instant::now() + Duration::from_secs(3);
                let (mut stream, _) = loop {
                    match listener.accept() {
                        Ok(accepted) => break accepted,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("scripted accept: {error}"),
                    }
                };
                stream.set_nonblocking(false).expect("blocking stream");
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .expect("scripted read timeout");
                let request: NativeIndexRequestEnvelope =
                    quanta_index_ipc::decode_request(&mut stream).expect("scripted request");
                requests.push(request.payload);
                if let Some(payload) = reply {
                    let response = NativeIndexResponseEnvelope {
                        request_id: request.request_id,
                        payload,
                    };
                    let bytes =
                        quanta_index_ipc::encode_response(&response).expect("scripted response");
                    stream.write_all(&bytes).expect("scripted write");
                }
            }
            requests
        })
    }

    #[cfg(unix)]
    fn cleanup_fixture() -> (NativeReadSessionStatus, ClientIoPolicy) {
        use quanta_index_contract::native_protocol::{NativeReadSessionId, NativeReadSessionState};
        let status = NativeReadSessionStatus {
            session: NativeReadSessionId([21; 32]),
            view: view(7),
            expires_at_ms: u64::MAX,
            revision: 1,
            state: NativeReadSessionState::Active,
        };
        let policy = ClientIoPolicy::try_new(Duration::from_millis(300)).expect("I/O policy");
        (status, policy)
    }

    #[test]
    fn only_uncertain_open_outcomes_need_key_replay() {
        use quanta_index_contract::native_protocol::NativeIndexWireError;

        assert!(open_ack_may_be_lost(&NativeUdsError::Transport(
            "lost ACK".into()
        )));
        assert!(open_ack_may_be_lost(&NativeUdsError::Refused(
            NativeIndexWireError {
                code: NativeIndexErrorCode::Cancelled,
                message: "cancelled after send".into(),
            }
        )));
        assert!(!open_ack_may_be_lost(&NativeUdsError::Refused(
            NativeIndexWireError {
                code: NativeIndexErrorCode::ResourceLimited,
                message: "refused".into(),
            }
        )));
        let gone = native_transport(NativeUdsError::Refused(NativeIndexWireError {
            code: NativeIndexErrorCode::SnapshotUnavailable,
            message: "session expired".into(),
        }));
        assert!(cleanup_session_gone(&gone));
    }

    #[test]
    fn cleanup_capacity_is_reserved_before_open_and_healthy_same_view_opens_can_overlap() {
        let policy = ClientIoPolicy::try_new(Duration::from_millis(300)).expect("I/O policy");
        let sessions = NativeViewSessions::new(
            NativeUdsClient::new(std::path::PathBuf::from("/unused-native.sock")),
            policy,
        );
        let first = view(7);
        sessions
            .table
            .reserve_open(first, [1; 32])
            .expect("first reservation");
        sessions
            .table
            .reserve_open(first, [2; 32])
            .expect("concurrent reservation");
        sessions
            .table
            .settle_cleanup(
                Some(first),
                &RequestBudgetV1::for_duration(Duration::from_millis(1)),
            )
            .expect("Opening is a healthy in-flight request");
        for index in 2..MAX_CLEANUP_ITEMS {
            let key = [u8::try_from(index + 1).expect("bounded index"); 32];
            sessions
                .table
                .reserve_open(view(u64::try_from(index + 7).expect("bounded index")), key)
                .expect("reserved capacity");
        }
        assert!(sessions.table.reserve_open(view(1_000), [99; 32]).is_err());
        sessions.table.change_cleanup([1; 32], None);
        sessions
            .table
            .reserve_open(view(1_000), [99; 32])
            .expect("released capacity admits Open");
    }

    #[test]
    fn expired_unknown_open_is_not_replayed_as_a_fresh_pin() {
        let policy = ClientIoPolicy::try_new(Duration::from_millis(300)).expect("I/O policy");
        let sessions = NativeViewSessions::new(
            NativeUdsClient::new(std::path::PathBuf::from("/unused-native.sock")),
            policy,
        );
        sessions
            .table
            .reserve_open(view(7), [7; 32])
            .expect("reserve Open");
        sessions
            .table
            .change_cleanup([7; 32], Some(CleanupKind::Open));
        sessions.table.cleanup_lock()[0].created_at = Instant::now()
            .checked_sub(MAX_UNCERTAIN_OPEN_AGE + Duration::from_secs(1))
            .expect("past instant");
        sessions
            .table
            .settle_cleanup(
                None,
                &RequestBudgetV1::for_duration(Duration::from_millis(1)),
            )
            .expect("expired key needs no replay");
        assert!(sessions.table.cleanup_lock().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn lost_open_ack_replays_original_key_then_releases_without_querying() {
        use quanta_index_contract::native_protocol::{
            NativeIndexRequest, NativeIndexResponse, NativeReadSessionReply,
            NativeReadSessionRequest, NativeReadSessionState,
        };

        let (status, policy) = cleanup_fixture();
        let directory = tempfile::tempdir().expect("temporary directory");
        let socket = directory.path().join("lost-open-ack.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind");
        let peer = cleanup_peer(
            listener,
            vec![
                None,
                Some(NativeIndexResponse::ReadSession(
                    NativeReadSessionReply::Status(status),
                )),
                Some(NativeIndexResponse::ReadSession(
                    NativeReadSessionReply::Released {
                        status: NativeReadSessionStatus {
                            state: NativeReadSessionState::Released,
                            ..status
                        },
                        has_remaining_target_pins: false,
                    },
                )),
            ],
        );
        let client = NativeUdsClient::new(socket.clone());
        let sessions = NativeViewSessions::new(NativeUdsClient::new(socket), policy);
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(2));
        assert!(
            sessions
                .on_view::<()>(
                    &client,
                    status.view,
                    None,
                    None,
                    &budget,
                    None,
                    |_, _, _, _| { panic!("an unacknowledged Open cannot query") }
                )
                .is_err()
        );
        sessions
            .release_now()
            .expect("replay and release ambiguous Open");
        let requests = peer.join().expect("scripted peer");
        let (first, replay, release) = (&requests[0], &requests[1], &requests[2]);
        assert_eq!(first, replay, "cleanup must reuse the original Open key");
        assert!(matches!(
            first,
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::Open {
                selection: ViewSelection::Exact(view),
                open_key,
            }) if *view == status.view && *open_key != [0; 32]
        ));
        assert_eq!(
            release,
            &NativeIndexRequest::ReadSession(NativeReadSessionRequest::Release(status.session))
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_profile_release_ack_retries_release_without_reopening_or_renewing() {
        use quanta_index_contract::native_protocol::{
            NativeIndexErrorCode, NativeIndexRequest, NativeIndexResponse, NativeIndexWireError,
            NativeReadSessionReply, NativeReadSessionRequest, NativeReadSessionState,
        };

        let (status, policy) = cleanup_fixture();
        let directory = tempfile::tempdir().expect("temporary directory");
        let socket = directory.path().join("lost-release-ack.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind");
        let peer = cleanup_peer(
            listener,
            vec![
                Some(NativeIndexResponse::ReadSession(
                    NativeReadSessionReply::Status(status),
                )),
                Some(NativeIndexResponse::Error(NativeIndexWireError {
                    code: NativeIndexErrorCode::ResourceLimited,
                    message: "profile refused".into(),
                })),
                None,
                None,
                Some(NativeIndexResponse::ReadSession(
                    NativeReadSessionReply::Released {
                        status: NativeReadSessionStatus {
                            state: NativeReadSessionState::Released,
                            ..status
                        },
                        has_remaining_target_pins: false,
                    },
                )),
            ],
        );
        let client = NativeUdsClient::new(socket.clone());
        let sessions = NativeViewSessions::new(NativeUdsClient::new(socket), policy);
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(2));
        assert!(matches!(
            sessions.on_view::<()>(
                &client,
                status.view,
                None,
                None,
                &budget,
                None,
                |_, _, _, _| { panic!("a failed profile cannot query") }
            ),
            Err(SdkError::Native(_))
        ));
        assert!(
            sessions
                .on_view::<()>(
                    &client,
                    status.view,
                    None,
                    None,
                    &budget,
                    None,
                    |_, _, _, _| { panic!("pending Release cannot query") }
                )
                .is_err()
        );
        sessions.table.renew_due();
        sessions.release_now().expect("retry failed Release");
        let requests = peer.join().expect("scripted peer");
        assert_eq!(requests.len(), 5);
        assert!(matches!(
            &requests[0],
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::Open { selection: ViewSelection::Exact(view), .. })
                if *view == status.view
        ));
        assert_eq!(
            requests[1],
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadProfile {
                session: status.session,
            })
        );
        for request in &requests[2..] {
            assert_eq!(
                *request,
                NativeIndexRequest::ReadSession(NativeReadSessionRequest::Release(status.session))
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn failed_release_now_does_not_renew_or_reuse_a_known_session() {
        use quanta_index_contract::native_protocol::{
            NativeIndexRequest, NativeIndexResponse, NativeReadSessionReply,
            NativeReadSessionRequest, NativeReadSessionState,
        };

        let (status, policy) = cleanup_fixture();
        let directory = tempfile::tempdir().expect("temporary directory");
        let socket = directory.path().join("known-release-ack.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind");
        let peer = cleanup_peer(
            listener,
            vec![
                None,
                Some(NativeIndexResponse::ReadSession(
                    NativeReadSessionReply::Released {
                        status: NativeReadSessionStatus {
                            state: NativeReadSessionState::Released,
                            ..status
                        },
                        has_remaining_target_pins: false,
                    },
                )),
            ],
        );
        let client = NativeUdsClient::new(socket.clone());
        let sessions = NativeViewSessions::new(NativeUdsClient::new(socket), policy);
        let _previous = sessions.table.lock().insert(
            status.view,
            HeldSession {
                status,
                spec: spec(2, NATIVE_TEXT_KIND),
                lease_ms: 60_000,
                active_cursors: BTreeSet::new(),
                pages_in_flight: 0,
                release_only: false,
            },
        );
        assert!(sessions.release_now().is_err());
        assert!(
            sessions
                .on_view::<()>(
                    &client,
                    status.view,
                    None,
                    None,
                    &RequestBudgetV1::for_duration(Duration::from_secs(1)),
                    None,
                    |_, _, _, _| panic!("release-only session cannot query"),
                )
                .is_err()
        );
        sessions.release_now().expect("retry Release");
        let requests = peer.join().expect("scripted peer");
        assert_eq!(requests.len(), 2);
        for request in requests {
            assert_eq!(
                request,
                NativeIndexRequest::ReadSession(NativeReadSessionRequest::Release(status.session))
            );
        }
    }

    #[test]
    fn dropping_the_client_never_waits_on_a_stalled_socket() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let socket = directory.path().join("stalled.sock");
        // The listener accepts connections into its backlog but never answers.
        let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind");
        let io_policy =
            ClientIoPolicy::try_new(Duration::from_secs(5)).expect("five-second I/O policy");
        let sessions = NativeViewSessions::new(NativeUdsClient::new(socket), io_policy);
        let held = view(1);
        let _previous = sessions.table.lock().insert(
            held,
            HeldSession {
                status: NativeReadSessionStatus {
                    session: quanta_index_contract::native_protocol::NativeReadSessionId([1; 32]),
                    view: held,
                    expires_at_ms: u64::MAX,
                    revision: 1,
                    state: quanta_index_contract::native_protocol::NativeReadSessionState::Active,
                },
                spec: spec(2, NATIVE_TEXT_KIND),
                lease_ms: 60_000,
                active_cursors: BTreeSet::from([[1; 32]]),
                pages_in_flight: 0,
                release_only: false,
            },
        );
        sessions.ensure_renewer().expect("renewer starts");
        let started = std::time::Instant::now();
        drop(sessions);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "client drop waited on the stalled socket: {:?}",
            started.elapsed()
        );
        drop(listener);
    }
}
