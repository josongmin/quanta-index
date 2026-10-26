//! Original-budget catalog side of a paired managed object installation.
use super::{
    SqliteCatalog, at_most_one, begin, bootstrap, cached_execute, canonical_object, check_attempt,
    confirmed_ingress_object, encode, fields_digest, ingress_intent, object_install, object_key,
    operation_key, reclaim, register_confirmed_preflighted_children,
    require_intent_with_attempt_key, require_open_ingress, require_registered_edges,
    require_reserved_ingress_intent, require_reserved_intent, sql, try_insert_ingress_intent,
    try_insert_object_intent,
};
use quanta_index_core::index::{
    IndexError, ObjectInstallScope, ObjectRef, RegisteredObject, WorkBudget, WorkUsage,
};
use quanta_index_core::{
    DiagnosticHealthCodeV1, DiagnosticLaneV1, DiagnosticStageV1, DiagnosticTerminalV1,
    DiagnosticTraceErrorV1,
};
use rusqlite::{Connection, OptionalExtension as _, Transaction, params};

pub(super) const MAX_OBJECTS: usize = 256;
const PROTECT_ATTEMPT_OBJECT_SQL: &str = "INSERT OR IGNORE INTO native_attempt_object_v1(operation,fence,object,row_digest) VALUES (?1,?2,?3,?4)";

// Count the same canonical JSON that `encode` writes, without retaining a
// throwaway payload merely to estimate the existing work fee.
#[derive(Default)]
struct EncodedLength {
    bytes: u64,
    overflowed: bool,
}

impl std::io::Write for EncodedLength {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let length = u64::try_from(bytes.len()).map_err(|_error| {
            self.overflowed = true;
            std::io::Error::other("native catalog row length overflow")
        })?;
        self.bytes = self.bytes.checked_add(length).ok_or_else(|| {
            self.overflowed = true;
            std::io::Error::other("native catalog row length overflow")
        })?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn encoded_object_len(object: &RegisteredObject) -> Result<u64, IndexError> {
    let mut length = EncodedLength::default();
    if let Err(error) = serde_json::to_writer(&mut length, object) {
        return if length.overflowed {
            Err(IndexError::ResourceLimited)
        } else {
            Err(IndexError::Storage(format!(
                "encode native catalog row: {error}"
            )))
        };
    }
    Ok(length.bytes)
}

fn charge_object(
    budget: &mut WorkBudget,
    object: &RegisteredObject,
    confirming: bool,
    unique_registration: bool,
    scope: &ObjectInstallScope,
    omitted_reserved_adoptions: u64,
) -> Result<(), IndexError> {
    let payload_bytes = encoded_object_len(object)?;
    let edges =
        u64::try_from(object.children.len()).map_err(|_error| IndexError::ResourceLimited)?;
    // A unique managed confirmation consumes its exact current-transaction
    // registration proof. It removes one full metadata/edge recheck and the
    // provisional-existence probe, on both initial confirmation and replay.
    let (base_reads, edge_reads): (u64, u64) =
        if unique_registration && matches!(scope, ObjectInstallScope::Ingress(_)) {
            // The scoped receipt still checks its own checksum. Its registration
            // is bound by the same proof, removing another full row/edge recheck.
            // Retain the edge allowance conservatively for the remaining preflight.
            (16, 1)
        } else if unique_registration {
            (20, 1)
        } else {
            (32, 4)
        };
    // Its one metadata shape probe was fused with the guarded body read.
    // Unique confirmation consumes one authenticated fixed-field lifecycle row
    // through its combined confirm/queue transition. Generic confirmation and
    // child allowances retain their conservative envelopes.
    // Reservation saves both enqueue lifecycle SELECTs and the fresh
    // provisional/intent absence lookups. An existing slot pays its full validation and the extra
    // failed insertion probe before those reads (two units per collision).
    // Attempt counter reads are charged by their transaction-bound sequence;
    // Live rows restore the old allowance, so only omitted reads receive credit.
    let base_reads = if !confirming && !matches!(scope, ObjectInstallScope::Bootstrap(_)) {
        base_reads
            .checked_sub(if matches!(scope, ObjectInstallScope::Attempt(_)) {
                5
            } else {
                4
            })
            .ok_or(IndexError::ResourceLimited)?
    } else {
        base_reads
    };
    budget.charge(WorkUsage {
        reads: edges
            .checked_mul(edge_reads)
            .and_then(|v| v.checked_add(base_reads))
            .and_then(|v| {
                omitted_reserved_adoptions
                    .checked_mul(4)
                    .and_then(|c| v.checked_sub(c))
            })
            .ok_or(IndexError::ResourceLimited)?,
        writes: edges
            .checked_mul(2)
            .and_then(|v| v.checked_add(if confirming { 8 } else { 6 }))
            .ok_or(IndexError::ResourceLimited)?,
        bytes: payload_bytes
            .checked_mul(4)
            .and_then(|v| edges.checked_mul(256).and_then(|e| v.checked_add(e)))
            .ok_or(IndexError::ResourceLimited)?,
        mutations: edges
            .checked_mul(2)
            .and_then(|v| v.checked_add(if confirming { 8 } else { 6 }))
            .ok_or(IndexError::ResourceLimited)?,
    })
}

fn require_child_charged(
    connection: &Connection,
    reference: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<RegisteredObject, IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let mut metadata_bytes = 0;
    let registration = object_install::read_metadata(
        connection,
        reference,
        object_install::MetadataLedger::Registered,
        || IndexError::CorruptObject("registered child metadata shape mismatch".into()),
        |bytes| {
            // The shape and body now share one row. Bill its bytes before
            // decoding, without a second SELECT or an intermediate Vec copy.
            budget.charge(WorkUsage {
                bytes,
                ..WorkUsage::default()
            })?;
            metadata_bytes = bytes;
            Ok(())
        },
    )?
    .ok_or_else(|| IndexError::CorruptObject("referenced child is not registered".into()))?;
    if registration.reference != reference
        || registration.children.len() > object_install::MAX_CHILDREN
    {
        return Err(IndexError::CorruptObject(
            "registered child metadata identity or bound mismatch".into(),
        ));
    }
    let edges =
        u64::try_from(registration.children.len()).map_err(|_error| IndexError::ResourceLimited)?;
    // The caller performs one current-transaction edge validation. Attempt
    // intent insertion is mutation-only after that preflight; it must not bill
    // the second validation removed from reserve_object_intent.
    budget.charge(WorkUsage {
        reads: edges.checked_add(5).ok_or(IndexError::ResourceLimited)?,
        bytes: metadata_bytes
            .checked_add(edges.checked_mul(128).ok_or(IndexError::ResourceLimited)?)
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    require_registered_edges(connection, reference, &registration)?;
    Ok(registration)
}

/// A fixed data window, not registration authority. The billing plan and
/// completed transaction preflight use separate instances and original grants.
struct ChildWindow {
    references: [Option<ObjectRef>; 64],
    _memory: Option<quanta_index_core::index::MemoryLease>,
}
impl ChildWindow {
    fn new(budget: &WorkBudget) -> Result<Self, IndexError> {
        let memory = budget.reserve_memory(
            u64::try_from(std::mem::size_of::<[Option<ObjectRef>; 64]>())
                .map_err(|_error| IndexError::ResourceLimited)?,
            quanta_index_core::index::MemoryLifetime::Temporary,
        )?;
        Ok(Self {
            references: [None; 64],
            _memory: memory,
        })
    }
    fn slot(reference: ObjectRef) -> Result<usize, IndexError> {
        let byte = reference.0.0.first().ok_or(IndexError::ResourceLimited)?;
        Ok(usize::from(*byte & 63))
    }
    fn contains(&self, reference: ObjectRef) -> Result<bool, IndexError> {
        Ok(self
            .references
            .get(Self::slot(reference)?)
            .ok_or(IndexError::ResourceLimited)?
            == &Some(reference))
    }
    fn remember(&mut self, reference: ObjectRef) -> Result<(), IndexError> {
        *self
            .references
            .get_mut(Self::slot(reference)?)
            .ok_or(IndexError::ResourceLimited)? = Some(reference);
        Ok(())
    }
}

/// Completed metadata, complete edge-ledger and GC adoption checks borrowed
/// from one actual locked transaction.
///
/// A collision merely repeats the checks. Nothing can serialize or carry this authority into another transaction.
struct AdoptedChildrenInTransaction<'tx, 'conn> {
    transaction: &'tx Transaction<'conn>,
    completed: Option<ChildWindow>,
}
impl<'tx, 'conn> AdoptedChildrenInTransaction<'tx, 'conn> {
    fn new(transaction: &'tx Transaction<'conn>) -> Result<Self, IndexError> {
        if transaction.is_autocommit() {
            return Err(IndexError::BackendViolation(
                "child preflight requires a transaction".into(),
            ));
        }
        Ok(Self {
            transaction,
            completed: None,
        })
    }
    fn require(&mut self, child: ObjectRef, budget: &mut WorkBudget) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if let Some(completed) = self.completed.as_ref()
            && completed.contains(child)?
        {
            return Ok(());
        }
        if self.completed.is_none() {
            self.completed = Some(ChildWindow::new(budget)?);
        }
        let _registered = require_child_charged(self.transaction, child, budget)?;
        reclaim::adopt(self.transaction, child)?;
        // Failed, provisional, frozen or corrupt children never enter the window.
        self.completed
            .as_mut()
            .ok_or(IndexError::ResourceLimited)?
            .remember(child)?;
        budget.checkpoint()
    }
}

pub(super) struct PreflightedRegistrationChildren<'tx, 'conn> {
    transaction: &'tx Transaction<'conn>,
    objects: &'tx [RegisteredObject],
    unique: bool,
}

impl<'tx, 'conn> PreflightedRegistrationChildren<'tx, 'conn> {
    pub(super) fn is_unique(&self) -> bool {
        self.unique
    }

    pub(super) fn into_parts(self) -> (&'tx Transaction<'conn>, &'tx [RegisteredObject]) {
        (self.transaction, self.objects)
    }
}

fn preflight_registration_children<'tx, 'conn>(
    transaction: &'tx Transaction<'conn>,
    objects: &'tx [RegisteredObject],
    budget: &mut WorkBudget,
) -> Result<PreflightedRegistrationChildren<'tx, 'conn>, IndexError> {
    if transaction.is_autocommit() {
        return Err(IndexError::BackendViolation(
            "object confirmation requires a transaction".into(),
        ));
    }
    let mut adopted = AdoptedChildrenInTransaction::new(transaction)?;
    let mut earlier = std::collections::BTreeSet::new();
    let mut unique = true;
    for object in objects {
        for child in &object.children {
            // Earlier input registrations are authenticated by the complete
            // same-transaction confirmation preflight. There is no SQL metadata
            // read or JSON decode here to bill a second time. Each input is
            // confirmed and enqueued later in this transaction; preserve an
            // authenticated queue row instead of deleting and reinserting it.
            if earlier.contains(child) {
                reclaim::adopt_confirming_input(transaction, *child)?;
            } else {
                adopted.require(*child, budget)?;
            }
        }
        unique &= earlier.insert(object.reference);
    }
    Ok(PreflightedRegistrationChildren {
        transaction,
        objects,
        unique,
    })
}

/// Attach an already confirmed exact registration to an open ingress.
/// No provisional intent, physical write, or new registration is permitted.
pub(super) fn adopt_registered_inputs(
    catalog: &SqliteCatalog,
    stage: quanta_index_core::index::IngressRef,
    objects: &[RegisteredObject],
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if objects.len() > 256 {
        return Err(IndexError::ResourceLimited);
    }
    budget.checkpoint()?;
    let mut guard = catalog.lock_native_budget(budget)?;
    let tx = begin(&mut guard)?;
    let operation = operation_key(stage.operation)?;
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let ingress_size: Option<(i64, i64)> = tx
        .query_row(
            "SELECT length(payload),length(row_digest) FROM native_ingress_v1 WHERE operation=?1",
            params![&operation],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    if let Some((payload_size, digest_size)) = ingress_size {
        if !(1..=4096).contains(&payload_size) || digest_size != 32 {
            return Err(IndexError::Storage("ingress shape invalid".into()));
        }
        budget.charge(WorkUsage {
            reads: 1,
            bytes: u64::try_from(payload_size)
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_add(32)
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
    }
    require_open_ingress(&tx, stage)?;
    let mut seen = std::collections::BTreeSet::new();
    for object in objects {
        budget.checkpoint()?;
        if object.bytes == 0
            || object.bytes > object_install::MAX_OBJECT_BYTES
            || object.children.len() > object_install::MAX_CHILDREN
        {
            return Err(IndexError::ResourceLimited);
        }
        if object.children.contains(&object.reference) || !seen.insert(object.reference) {
            return Err(IndexError::InvalidInput(
                "duplicate or self-referencing ingress adoption".into(),
            ));
        }
        let registered = require_child_charged(&tx, object.reference, budget)?;
        if registered != *object {
            return Err(IndexError::CorruptObject(
                "ingress adoption differs from confirmed registration".into(),
            ));
        }
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let provisional: i64 = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM native_object_install_v1 WHERE object=?1)",
                params![object_key(object.reference)],
                |row| row.get(0),
            )
            .map_err(|error| sql(&error))?;
        if provisional != 0 {
            return Err(IndexError::BackendViolation(
                "ingress adoption requires confirmed registration".into(),
            ));
        }
        // Charge the worst local Retiring cancellation (counter + row + dequeue)
        // and exact ingress-row insertion before mutating either table.
        budget.charge(WorkUsage {
            reads: 16,
            writes: 4,
            bytes: 2048,
            mutations: 4,
        })?;
        reclaim::adopt(&tx, object.reference)?;
        let key = object_key(object.reference);
        let expected = fields_digest(b"qidx.native.ingressobject.v1", &(&operation, &key))?;
        at_most_one(
            tx.execute(
                "INSERT OR IGNORE INTO native_ingress_object_v1(operation,object,row_digest) VALUES (?1,?2,?3)",
                params![&operation, &key, &expected],
            )
            .map_err(|error| sql(&error))?,
            "adopt confirmed ingress object",
        )?;
        let stored: Vec<u8> = tx
            .query_row(
                "SELECT row_digest FROM native_ingress_object_v1 WHERE operation=?1 AND object=?2",
                params![&operation, &key],
                |row| row.get(0),
            )
            .map_err(|error| sql(&error))?;
        if stored != expected {
            return Err(IndexError::Storage(
                "adopted ingress object digest mismatch".into(),
            ));
        }
    }
    budget.checkpoint()?;
    tx.commit().map_err(|error| sql(&error))?;
    drop(guard);
    Ok(())
}

/// Bound to the actual locked transaction and exact canonical input. No
/// reservation result or validation cache escapes to another transaction.
struct ReservedInTransaction<'tx, 'conn, 'input> {
    transaction: &'tx Transaction<'conn>,
    object: &'input RegisteredObject,
    payload: Vec<u8>,
}

fn reserve_charged<'tx, 'conn, 'input>(
    transaction: &'tx Transaction<'conn>,
    object: &'input RegisteredObject,
    budget: &mut WorkBudget,
) -> Result<ReservedInTransaction<'tx, 'conn, 'input>, IndexError> {
    let payload = if let Some(payload) = object_install::try_reserve_new(transaction, object)? {
        payload
    } else {
        budget.charge(WorkUsage {
            reads: 2,
            ..WorkUsage::default()
        })?;
        object_install::require_reserved(transaction, object)?;
        encode(object)?
    };
    Ok(ReservedInTransaction {
        transaction,
        object,
        payload,
    })
}

impl ReservedInTransaction<'_, '_, '_> {
    fn bind_attempt(
        self,
        attempt: quanta_index_core::index::AttemptRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if !try_insert_object_intent(self.transaction, attempt, self.object, &self.payload)? {
            budget.charge(WorkUsage {
                reads: 2,
                ..WorkUsage::default()
            })?;
            require_reserved_intent(self.transaction, attempt, self.object, &self.payload)?;
        }
        Ok(())
    }

    fn bind_ingress(
        self,
        stage: quanta_index_core::index::IngressRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if !try_insert_ingress_intent(
            self.transaction,
            stage.operation,
            self.object,
            &self.payload,
        )? {
            budget.charge(WorkUsage {
                reads: 2,
                ..WorkUsage::default()
            })?;
            require_reserved_ingress_intent(
                self.transaction,
                stage.operation,
                self.object,
                &self.payload,
            )?;
        }
        Ok(())
    }
}

fn reserve_ingress(
    connection: &Transaction<'_>,
    stage: quanta_index_core::index::IngressRef,
    objects: &[RegisteredObject],
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    require_open_ingress(connection, stage)?;
    let mut earlier = std::collections::BTreeSet::new();
    for object in objects {
        budget.checkpoint()?;
        let canonical = object_install::checked_object(object)?;
        if !earlier.insert(canonical.reference) {
            return Err(IndexError::InvalidInput(
                "duplicate ingress reservation".into(),
            ));
        }
        for child in &canonical.children {
            if !earlier.contains(child) {
                let _registered = require_child_charged(connection, *child, budget)?;
            }
            reclaim::adopt(connection, *child)?;
        }
        reserve_charged(connection, &canonical, budget)?.bind_ingress(stage, budget)?;
        let _queued = reclaim::reserve_and_enqueue(connection, canonical.reference)?;
    }
    Ok(())
}

fn confirm_ingress(
    connection: &Transaction<'_>,
    stage: quanta_index_core::index::IngressRef,
    objects: &[RegisteredObject],
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    require_open_ingress(connection, stage)?;
    let mut seen = std::collections::BTreeSet::new();
    for object in objects {
        budget.checkpoint()?;
        if !seen.insert(object.reference) {
            return Err(IndexError::InvalidInput(
                "duplicate ingress confirmation".into(),
            ));
        }
        if ingress_intent(connection, stage.operation, object)?
            != Some(encode(&object_install::checked_object(object)?)?)
        {
            return Err(IndexError::BackendViolation(
                "ingress confirmation lacks matching allocation intent".into(),
            ));
        }
    }
    let proof = preflight_registration_children(connection, objects, budget)?;
    let registered = register_confirmed_preflighted_children(proof)?;
    let operation = operation_key(stage.operation)?;
    for object in objects {
        let key = object_key(object.reference);
        let changed = cached_execute(connection,
            "INSERT OR IGNORE INTO native_ingress_object_v1(operation,object,row_digest) VALUES (?1,?2,?3)",
            params![&operation,&key,fields_digest(b"qidx.native.ingressobject.v1", &(&operation,&key))?],
        ).map_err(|error| sql(&error))?;
        at_most_one(changed, "confirm managed ingress object")?;
        let confirmed = match registered.as_ref() {
            Some(proof) => proof.confirmed_ingress(stage.operation, object)?,
            None => confirmed_ingress_object(connection, stage.operation, object.reference)?,
        };
        if !confirmed {
            return Err(IndexError::Storage(
                "confirmed ingress object missing".into(),
            ));
        }
        let _queued = reclaim::confirm_and_enqueue(connection, object.reference)?;
    }
    Ok(())
}

fn reserve_attempt(
    connection: &Transaction<'_>,
    attempt: quanta_index_core::index::AttemptRef,
    objects: &[RegisteredObject],
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let (_operation, _adopted, _earlier) = observe_sqlite(budget, 4, |budget| {
        let operation = check_attempt(connection, attempt)?;
        let mut adopted = AdoptedChildrenInTransaction::new(connection)?;
        // Complete child-first authentication before writing any input intent.
        let mut earlier = std::collections::BTreeSet::new();
        for object in objects {
            budget.checkpoint()?;
            let canonical = object_install::checked_object(object)?;
            if !earlier.insert(canonical.reference) {
                return Err(IndexError::InvalidInput(
                    "duplicate managed attempt reservation".into(),
                ));
            }
            for child in &canonical.children {
                if !earlier.contains(child) {
                    adopted.require(*child, budget)?;
                }
            }
        }
        // Preserve the original cache/data lifetime through reservation writes.
        Ok((operation, adopted, earlier))
    })?;
    observe_sqlite(budget, 5, |budget| {
        // The locked transaction and exact input remain unchanged across spans.
        super::fence_counter::ReservationSequence::run(
            connection,
            objects.len(),
            budget,
            |fences, budget| {
                for object in objects {
                    let canonical = canonical_object(object)?;
                    reserve_charged(connection, &canonical, budget)?
                        .bind_attempt(attempt, budget)?;
                    let _queued = reclaim::reserve_and_enqueue_sequenced(
                        canonical.reference,
                        fences,
                        budget,
                    )?;
                }
                Ok(())
            },
        )
    })
}

fn confirm_attempt(
    connection: &Transaction<'_>,
    attempt: quanta_index_core::index::AttemptRef,
    objects: &[RegisteredObject],
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let (attempt_key, proof) = observe_sqlite(budget, 6, |budget| {
        let _operation = check_attempt(connection, attempt)?;
        let mut attempt_key = None;
        for object in objects {
            budget.checkpoint()?;
            require_intent_with_attempt_key(connection, attempt, object, &mut attempt_key)?;
        }
        let proof = preflight_registration_children(connection, objects, budget)?;
        let attempt_key = attempt_key.ok_or_else(|| {
            IndexError::BackendViolation("managed confirmation attempt key missing".into())
        })?;
        Ok((attempt_key, proof))
    })?;
    let _registered = observe_sqlite(budget, 7, |_budget| {
        register_confirmed_preflighted_children(proof)
    })?;
    observe_sqlite(budget, 8, |budget| {
        let (operation, fence) = attempt_key;
        // Complete every protection write in a packet before its reclaim
        // transitions. The static statement is cached across all rows; the
        // surrounding transaction retains the original all-or-nothing outcome.
        for packet in objects.chunks(64) {
            budget.checkpoint()?;
            // Retain the original packet-wide key/digest preparation order:
            // all preparation checkpoints and digests precede the first SQL write.
            let row_bytes = std::mem::size_of::<(Vec<u8>, Vec<u8>)>()
                .checked_add(64)
                .ok_or(IndexError::ResourceLimited)?;
            let bytes = packet
                .len()
                .checked_mul(row_bytes)
                .ok_or(IndexError::ResourceLimited)?;
            let _memory = budget.reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                quanta_index_core::index::MemoryLifetime::Temporary,
            )?;
            let mut rows = Vec::new();
            rows.try_reserve_exact(packet.len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            for object in packet {
                budget.checkpoint()?;
                let key = object_key(object.reference);
                let checksum =
                    fields_digest(b"qidx.native.attemptobject.v1", &(&operation, fence, &key))?;
                rows.push((key, checksum));
            }
            for (key, checksum) in &rows {
                let inserted = cached_execute(
                    connection,
                    PROTECT_ATTEMPT_OBJECT_SQL,
                    params![&operation, fence, key, checksum],
                )
                .map_err(|error| sql(&error))?;
                at_most_one(inserted, "protect attempt object")?;
            }
            for object in packet {
                budget.checkpoint()?;
                let _queued = reclaim::confirm_and_enqueue(connection, object.reference)?;
            }
        }
        Ok(())
    })
}

/// Observe existing transaction work, never grant authority or change its result.
///
/// `SqliteWork` lanes:3 COMMIT,4 reservation preflight,5 reservation mutation,
/// 6 confirmation preflight,7 registration/transfer,8 protection/lifecycle.
/// Lane 9 observes the existing budgeted catalog lock; lane 10 observes `BEGIN`.
/// These are disjoint regions within one managed call, not additive with its
/// enclosing reserve/confirm span. Off does not start a diagnostic clock.
fn observe_sqlite<T>(
    budget: &mut WorkBudget,
    lane: u16,
    work: impl FnOnce(&mut WorkBudget) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    let span = budget.request().diagnostic_context().and_then(|context| {
        context
            .enter(DiagnosticStageV1::SqliteWork, DiagnosticLaneV1(lane))
            .map_or_else(
                |error| {
                    context.health(match error {
                        DiagnosticTraceErrorV1::SpanIdentityExhausted => {
                            DiagnosticHealthCodeV1::CounterOverflow
                        }
                        DiagnosticTraceErrorV1::ClockRegressed => {
                            DiagnosticHealthCodeV1::ClockRegressed
                        }
                        DiagnosticTraceErrorV1::ForeignCaptureSpan => {
                            DiagnosticHealthCodeV1::ForeignCaptureSpan
                        }
                        DiagnosticTraceErrorV1::MissingParentSpan => {
                            DiagnosticHealthCodeV1::MissingParentSpan
                        }
                    });
                    None
                },
                Some,
            )
    });
    let result = work(budget);
    if let Some(span) = span {
        let _finished = span.finish(DiagnosticTerminalV1::from_result(&result));
    }
    result
}

pub(super) fn install(
    catalog: &SqliteCatalog,
    scope: ObjectInstallScope,
    objects: &[RegisteredObject],
    budget: &mut WorkBudget,
    confirming: bool,
) -> Result<(), IndexError> {
    if objects.is_empty() || objects.len() > MAX_OBJECTS {
        return Err(IndexError::ResourceLimited);
    }
    budget.checkpoint()?;
    let mut guard = observe_sqlite(budget, 9, |budget| catalog.lock_native_budget(budget))?;
    let tx = observe_sqlite(budget, 10, |_budget| begin(&mut guard))?;
    budget.charge(WorkUsage {
        reads: 2,
        bytes: 256,
        ..WorkUsage::default()
    })?;
    reclaim::require_bound(&tx)?;
    // This bounded, allocation-free check selects the same unique-input proof
    // path as the child preflight. Duplicate and bootstrap paths retain their
    // original confirmation checks and conservative admission.
    let unique_registration = confirming
        && !matches!(scope, ObjectInstallScope::Bootstrap(_))
        && objects.iter().enumerate().all(|(ordinal, object)| {
            objects
                .iter()
                .take(ordinal)
                .all(|prior| prior.reference != object.reference)
        });
    let attempt_reservation = !confirming && matches!(scope, ObjectInstallScope::Attempt(_));
    let mut earlier_reserved = std::collections::BTreeSet::new();
    // Predict only actually omitted reservation adoptions. This data window is
    // destroyed before preflight; it is never a completed authentication proof.
    let mut planned =
        if attempt_reservation && objects.iter().any(|object| !object.children.is_empty()) {
            Some(ChildWindow::new(budget)?)
        } else {
            None
        };
    for object in objects {
        // Only Attempt reservation queues every input after the full preflight.
        // Ingress queues incrementally and retains its original child adoption.
        let omitted_reserved_adoptions = if let Some(planned) = planned.as_mut() {
            let mut count = 0_u64;
            for child in &object.children {
                budget.checkpoint()?;
                if earlier_reserved.contains(child) || planned.contains(*child)? {
                    count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                } else {
                    planned.remember(*child)?;
                }
            }
            let _new = earlier_reserved.insert(object.reference);
            count
        } else {
            0
        };
        charge_object(
            budget,
            object,
            confirming,
            unique_registration,
            &scope,
            omitted_reserved_adoptions,
        )?;
    }
    drop(planned);
    match (scope, confirming) {
        (ObjectInstallScope::Ingress(stage), false) => {
            reserve_ingress(&tx, stage, objects, budget)?;
        }
        (ObjectInstallScope::Ingress(stage), true) => confirm_ingress(&tx, stage, objects, budget)?,
        (ObjectInstallScope::Attempt(attempt), false) => {
            reserve_attempt(&tx, attempt, objects, budget)?;
        }
        (ObjectInstallScope::Attempt(attempt), true) => {
            confirm_attempt(&tx, attempt, objects, budget)?;
        }
        (ObjectInstallScope::Bootstrap(stage), false) => {
            let [object] = objects else {
                return Err(IndexError::InvalidInput(
                    "bootstrap object batch must contain one profile".into(),
                ));
            };
            bootstrap::reserve_bootstrap_tx(&tx, stage, object)?;
            reclaim::reserve_object(&tx, object.reference)?;
            let _queued = reclaim::enqueue(&tx, object.reference)?;
        }
        (ObjectInstallScope::Bootstrap(stage), true) => {
            let [object] = objects else {
                return Err(IndexError::InvalidInput(
                    "bootstrap object batch must contain one profile".into(),
                ));
            };
            bootstrap::confirm_bootstrap_tx(&tx, stage, object)?;
            reclaim::confirm_object(&tx, object.reference)?;
            let _queued = reclaim::enqueue(&tx, object.reference)?;
        }
    }
    budget.checkpoint()?;
    // Retain the existing COMMIT-only scope and lane identity.
    observe_sqlite(budget, 3, |_budget| {
        tx.commit().map_err(|error| sql(&error))
    })?;
    drop(guard);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::RequestBudgetV1;
    use quanta_index_core::index::{
        BootstrapRef, CollectionId, Digest, SlotId, TenantId, WorkLimits, WorkspaceKey, WorktreeId,
    };
    use std::time::Duration;

    fn budget(bytes: u64) -> WorkBudget {
        WorkBudget::with_request(
            WorkLimits {
                reads: u64::MAX,
                writes: u64::MAX,
                bytes,
                mutations: u64::MAX,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        )
    }

    fn object(children: usize, bytes: u64) -> Result<RegisteredObject, IndexError> {
        let children = (0..children)
            .map(|index| {
                let mut digest = [0_u8; 32];
                digest
                    .get_mut(..8)
                    .ok_or(IndexError::ResourceLimited)?
                    .copy_from_slice(
                        &u64::try_from(index)
                            .map_err(|_error| IndexError::ResourceLimited)?
                            .to_be_bytes(),
                    );
                Ok(ObjectRef(Digest(digest)))
            })
            .collect::<Result<Vec<_>, IndexError>>()?;
        Ok(RegisteredObject::unscoped(
            ObjectRef(Digest([255; 32])),
            bytes,
            children,
        ))
    }

    fn scope(reference: ObjectRef) -> ObjectInstallScope {
        ObjectInstallScope::Bootstrap(BootstrapRef {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
            fence: 1,
            profile: reference,
        })
    }

    #[test]
    fn charged_json_census_matches_canonical_bytes_and_original_fee() -> Result<(), IndexError> {
        for children in [0_usize, 1, 64, 4096] {
            for bytes in [0_u64, u64::MAX] {
                let object = object(children, bytes)?;
                let canonical = encode(&object)?;
                let length =
                    u64::try_from(canonical.len()).map_err(|_error| IndexError::ResourceLimited)?;
                if encoded_object_len(&object)? != length {
                    return Err(IndexError::BackendViolation(
                        "managed object JSON census differs from canonical bytes".into(),
                    ));
                }
                let edges =
                    u64::try_from(children).map_err(|_error| IndexError::ResourceLimited)?;
                for confirming in [false, true] {
                    let mut funded = budget(u64::MAX);
                    charge_object(
                        &mut funded,
                        &object,
                        confirming,
                        false,
                        &scope(object.reference),
                        0,
                    )?;
                    let expected = WorkUsage {
                        reads: edges
                            .checked_mul(4)
                            .and_then(|value| value.checked_add(32))
                            .ok_or(IndexError::ResourceLimited)?,
                        writes: edges
                            .checked_mul(2)
                            .and_then(|value| value.checked_add(if confirming { 8 } else { 6 }))
                            .ok_or(IndexError::ResourceLimited)?,
                        bytes: length
                            .checked_mul(4)
                            .and_then(|value| {
                                edges
                                    .checked_mul(256)
                                    .and_then(|edges| value.checked_add(edges))
                            })
                            .ok_or(IndexError::ResourceLimited)?,
                        mutations: edges
                            .checked_mul(2)
                            .and_then(|value| value.checked_add(if confirming { 8 } else { 6 }))
                            .ok_or(IndexError::ResourceLimited)?,
                    };
                    if funded.usage() != expected {
                        return Err(IndexError::BackendViolation(
                            "managed object fee differs from original formula".into(),
                        ));
                    }
                    let mut denied = budget(
                        expected
                            .bytes
                            .checked_sub(1)
                            .ok_or(IndexError::ResourceLimited)?,
                    );
                    if charge_object(
                        &mut denied,
                        &object,
                        confirming,
                        false,
                        &scope(object.reference),
                        0,
                    ) != Err(IndexError::ResourceLimited)
                        || denied.usage() != WorkUsage::default()
                    {
                        return Err(IndexError::BackendViolation(
                            "managed object denied fee changed usage".into(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn counting_writer_rejects_overflow_without_wrapping() -> Result<(), IndexError> {
        let mut length = EncodedLength {
            bytes: u64::MAX,
            overflowed: false,
        };
        if std::io::Write::write(&mut length, b"x").is_ok()
            || !length.overflowed
            || length.bytes != u64::MAX
        {
            return Err(IndexError::BackendViolation(
                "managed object JSON census wrapped".into(),
            ));
        }
        Ok(())
    }
}
