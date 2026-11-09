//! Operation-independent child custody for reserved, unconfirmed objects.
//!
//! Physical reclamation remains disabled; an orphan reservation over-retains
//! its children until a later install/reclaim authority can reconcile it.

use quanta_index_core::index::{IndexError, RegisteredObject};
use rusqlite::{Connection, Transaction, params};

use super::registration_codec::{
    MAX_ENCODED_BYTES, decode as decode_registration, encode as encode_registration,
};
use super::{
    RegisteredBatchInTransaction, cached_execute, cached_query_row, canonical_object, check_digest,
    digest, exactly_one, fields_digest, object_key, require_object, require_registered_edges, sql,
};

pub(super) use super::registration_codec::{MAX_CHILDREN, MAX_OBJECT_BYTES};
const MAX_METADATA_BYTES: usize = 1_048_576;
pub(super) const MAX_METADATA_BYTES_SQL: i64 = 1_048_576;
const _: () = assert!(MAX_ENCODED_BYTES <= MAX_METADATA_BYTES);

#[derive(Clone, Copy)]
pub(super) enum EdgeLedger {
    Registered,
    Provisional,
}

enum EdgeProblem {
    Shape,
    Digest,
    Mismatch,
    Count,
}

impl EdgeLedger {
    fn query(self) -> &'static str {
        // CASE guards the fixed-width values before SQLite materializes an
        // oversized corrupt BLOB. The qualified ORDER BY uses the stored key.
        match self {
            Self::Registered => {
                "SELECT CASE WHEN length(child)=32 THEN child END,
                        CASE WHEN length(row_digest)=32 THEN row_digest END
                 FROM native_object_edge_v1 WHERE parent=?1
                 ORDER BY native_object_edge_v1.child LIMIT 4097"
            }
            Self::Provisional => {
                "SELECT CASE WHEN length(child)=32 THEN child END,
                        CASE WHEN length(row_digest)=32 THEN row_digest END
                 FROM native_object_install_edge_v1 WHERE parent=?1
                 ORDER BY native_object_install_edge_v1.child LIMIT 4097"
            }
        }
    }

    fn domain(self) -> &'static [u8] {
        match self {
            Self::Registered => b"qidx.native.edge.v1",
            Self::Provisional => b"qidx.native.installedge.v1",
        }
    }

    fn error(self, problem: EdgeProblem) -> IndexError {
        let message = match (self, problem) {
            (Self::Registered, EdgeProblem::Shape) => "registered object edge shape mismatch",
            (Self::Registered, EdgeProblem::Digest) => "object edge digest mismatch",
            (Self::Registered, EdgeProblem::Mismatch | EdgeProblem::Count) => {
                "object edge ledger differs from registration"
            }
            (Self::Provisional, EdgeProblem::Shape) => "provisional child metadata shape mismatch",
            (Self::Provisional, EdgeProblem::Digest) => "provisional child ledger digest mismatch",
            (Self::Provisional, EdgeProblem::Mismatch) => {
                "provisional child ledger differs from registration"
            }
            (Self::Provisional, EdgeProblem::Count) => "provisional child ledger count mismatch",
        };
        match self {
            Self::Registered => IndexError::CorruptObject(message.into()),
            Self::Provisional => IndexError::Storage(message.into()),
        }
    }
}

/// Authenticate the complete bounded edge ledger in one guarded scan.
pub(super) fn require_edge_ledger(
    connection: &Connection,
    reference: quanta_index_core::index::ObjectRef,
    children: &[quanta_index_core::index::ObjectRef],
    ledger: EdgeLedger,
) -> Result<(), IndexError> {
    if children.len() > MAX_CHILDREN {
        return Err(ledger.error(EdgeProblem::Shape));
    }
    // One contiguous key array replaces a separate heap allocation per key.
    let mut expected = children.iter().map(|child| child.0.0).collect::<Vec<_>>();
    expected.sort_unstable();
    expected.dedup();
    let parent = object_key(reference);
    let mut statement = connection
        .prepare_cached(ledger.query())
        .map_err(|error| sql(&error))?;
    let rows = statement
        .query_map(params![&parent], |row| {
            Ok((
                row.get::<_, Option<Vec<u8>>>(0)?,
                row.get::<_, Option<Vec<u8>>>(1)?,
            ))
        })
        .map_err(|error| sql(&error))?;
    let mut count = 0_usize;
    for row in rows {
        let (child, checksum) = row.map_err(|error| sql(&error))?;
        let (Some(child), Some(checksum)) = (child, checksum) else {
            return Err(ledger.error(EdgeProblem::Shape));
        };
        if count == MAX_CHILDREN {
            return Err(ledger.error(EdgeProblem::Shape));
        }
        if checksum != fields_digest(ledger.domain(), &(&parent, &child))? {
            return Err(ledger.error(EdgeProblem::Digest));
        }
        if expected
            .get(count)
            .is_none_or(|wanted| wanted.as_slice() != child.as_slice())
        {
            return Err(ledger.error(EdgeProblem::Mismatch));
        }
        count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    }
    if count != expected.len() {
        return Err(ledger.error(EdgeProblem::Count));
    }
    Ok(())
}

pub(super) fn checked_object(object: &RegisteredObject) -> Result<RegisteredObject, IndexError> {
    // The maximum canonical encoding fits the metadata bound. Callers that
    // need only the checked registration must not allocate and discard it.
    canonical_object(object)
}

/// Metadata and checksum share one guarded `SQLite` row.
///
/// The caller admits owned decoding before it occurs; no payload/checksum copy or shape probe is
/// needed. An absent row and a present malformed row remain distinct.
#[derive(Clone, Copy)]
pub(super) enum MetadataLedger {
    Registered,
    Provisional,
}

pub(super) const REGISTERED_METADATA_SQL: &str =
    "SELECT CASE WHEN typeof(payload)='blob' AND length(payload) BETWEEN 1 AND ?2
              AND typeof(row_digest)='blob' AND length(row_digest)=32 THEN payload END,
            CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32
              THEN row_digest END
     FROM native_object_v1 WHERE object=?1";
const PROVISIONAL_METADATA_SQL: &str =
    "SELECT CASE WHEN typeof(payload)='blob' AND length(payload) BETWEEN 1 AND ?2
              AND typeof(row_digest)='blob' AND length(row_digest)=32 THEN payload END,
            CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32
              THEN row_digest END
     FROM native_object_install_v1 WHERE object=?1";

pub(super) fn read_metadata(
    connection: &Connection,
    reference: quanta_index_core::index::ObjectRef,
    ledger: MetadataLedger,
    shape_error: impl FnOnce() -> IndexError,
    admit: impl FnOnce(u64) -> Result<(), IndexError>,
) -> Result<Option<RegisteredObject>, IndexError> {
    let (query, domain): (_, &[u8]) = match ledger {
        MetadataLedger::Registered => (REGISTERED_METADATA_SQL, b"qidx.native.object.v1"),
        MetadataLedger::Provisional => (PROVISIONAL_METADATA_SQL, b"qidx.native.install.v1"),
    };
    let mut statement = connection
        .prepare_cached(query)
        .map_err(|error| sql(&error))?;
    let mut rows = statement
        .query(params![object_key(reference), MAX_METADATA_BYTES_SQL])
        .map_err(|error| sql(&error))?;
    let Some(row) = rows.next().map_err(|error| sql(&error))? else {
        return Ok(None);
    };
    let payload = row.get_ref(0).map_err(|error| sql(&error))?;
    let checksum = row.get_ref(1).map_err(|error| sql(&error))?;
    let (rusqlite::types::ValueRef::Blob(payload), rusqlite::types::ValueRef::Blob(checksum)) =
        (payload, checksum)
    else {
        return Err(shape_error());
    };
    // Also check at the Rust boundary: these are borrowed SQLite values, so
    // even a malformed row cannot cause an unchecked Rust allocation.
    if payload.is_empty() || payload.len() > MAX_METADATA_BYTES || checksum.len() != 32 {
        return Err(shape_error());
    }
    let bytes = u64::try_from(payload.len())
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_add(32)
        .ok_or(IndexError::ResourceLimited)?;
    admit(bytes)?;
    check_digest(domain, payload, checksum)?;
    decode_registration(payload).map(Some)
}

pub(super) fn registered_matches(
    connection: &Connection,
    object: &RegisteredObject,
) -> Result<bool, IndexError> {
    let Some(prior) = read_metadata(
        connection,
        object.reference,
        MetadataLedger::Registered,
        || IndexError::Storage("registered object metadata shape mismatch".into()),
        |_bytes| Ok(()),
    )?
    else {
        return Ok(false);
    };
    if prior != *object || prior.reference != object.reference {
        return Err(IndexError::CorruptObject(
            "registered object metadata changed for one digest".into(),
        ));
    }
    require_registered_edges(connection, object.reference, &prior)?;
    Ok(true)
}

pub(super) fn confirmed_matches(
    connection: &Connection,
    object: &RegisteredObject,
) -> Result<bool, IndexError> {
    if !registered_matches(connection, object)? {
        return Ok(false);
    }
    if provisional_matches(connection, object)? {
        return Err(IndexError::Storage(
            "registered object retains provisional child custody".into(),
        ));
    }
    Ok(true)
}

fn provisional_matches(
    connection: &Connection,
    object: &RegisteredObject,
) -> Result<bool, IndexError> {
    let Some(prior) = read_metadata(
        connection,
        object.reference,
        MetadataLedger::Provisional,
        || IndexError::Storage("provisional object metadata shape mismatch".into()),
        |_bytes| Ok(()),
    )?
    else {
        return Ok(false);
    };
    if prior != *object || prior.reference != object.reference {
        return Err(IndexError::CorruptObject(
            "provisional object metadata changed for one digest".into(),
        ));
    }
    require_edge_ledger(
        connection,
        object.reference,
        &object.children,
        EdgeLedger::Provisional,
    )?;
    Ok(true)
}

pub(super) fn reserve(
    connection: &Transaction<'_>,
    object: &RegisteredObject,
) -> Result<(), IndexError> {
    let (write, _) = try_reserve_new(connection, object)?;
    if matches!(write, ReservationWrite::Existing) {
        require_reserved(connection, object)?;
    }
    Ok(())
}

pub(super) const RESERVE_NEW_SQL: &str =
    "INSERT OR ROLLBACK INTO native_object_install_v1(object,payload,row_digest)
     SELECT ?1,?2,?3 WHERE NOT EXISTS(SELECT 1 FROM native_object_v1 WHERE object=?1)
     ON CONFLICT(object) DO NOTHING";

pub(super) enum ReservationWrite {
    Inserted,
    Existing,
}

/// The conditional insert preserves the permanent-row probe, but replaces a
/// separate provisional-absence probe with the primary-key insertion itself.
///
/// Returns the input encoding for intent binding even when a row already exists.
/// The encoding carries no custody proof. Callers must authenticate existing rows.
/// Callers return on any error and drop the whole transaction. For constraints
/// governed by this conflict policy, ROLLBACK makes that refusal immediate;
/// the named primary-key DO NOTHING remains the idempotent replay path.
pub(super) fn try_reserve_new(
    connection: &Transaction<'_>,
    object: &RegisteredObject,
) -> Result<(ReservationWrite, Vec<u8>), IndexError> {
    let payload = encode_registration(object)?;
    let key = object_key(object.reference);
    let checksum = digest(b"qidx.native.install.v1", &payload);
    let inserted = cached_execute(
        connection,
        RESERVE_NEW_SQL,
        params![&key, &payload, checksum.as_slice()],
    )
    .map_err(|error| sql(&error))?;
    super::at_most_one(inserted, "reserve provisional object")?;
    if inserted == 0 {
        return Ok((ReservationWrite::Existing, payload));
    }
    let mut children = object.children.clone();
    children.sort_unstable();
    children.dedup();
    for child in children {
        let child = object_key(child);
        let inserted = cached_execute(
            connection,
            "INSERT INTO native_object_install_edge_v1(parent,child,row_digest) VALUES (?1,?2,?3)",
            params![
                &key,
                &child,
                fields_digest(b"qidx.native.installedge.v1", &(&key, &child))?
            ],
        )
        .map_err(|error| sql(&error))?;
        exactly_one(inserted, "reserve provisional child")?;
    }
    Ok((ReservationWrite::Inserted, payload))
}

/// Authenticate every existing permanent/provisional payload and edge on retry.
pub(super) fn require_reserved(
    connection: &Connection,
    object: &RegisteredObject,
) -> Result<(), IndexError> {
    let canonical = checked_object(object)?;
    if registered_matches(connection, &canonical)? {
        if provisional_matches(connection, &canonical)? {
            return Err(IndexError::Storage(
                "registered object retains provisional child custody".into(),
            ));
        }
        return Ok(());
    }
    if provisional_matches(connection, &canonical)? {
        return Ok(());
    }
    Err(IndexError::Storage(
        "reservation insert did not create or find an object".into(),
    ))
}

pub(super) fn require_confirmable(
    connection: &Connection,
    object: &RegisteredObject,
) -> Result<(), IndexError> {
    confirmation_state(connection, object).map(|_state| ())
}

pub(super) enum ConfirmationState {
    Provisional,
    Registered,
}

pub(super) fn confirmation_state(
    connection: &Connection,
    object: &RegisteredObject,
) -> Result<ConfirmationState, IndexError> {
    let canonical = checked_object(object)?;
    if provisional_matches(connection, &canonical)? {
        // A permanent shadow cannot survive a valid atomic transfer. Check it
        // after the complete provisional proof, before any batch registration.
        if registered_matches(connection, &canonical)? {
            return Err(IndexError::Storage(
                "registered object retains provisional child custody".into(),
            ));
        }
        Ok(ConfirmationState::Provisional)
    } else if registered_matches(connection, &canonical)? {
        Ok(ConfirmationState::Registered)
    } else {
        Err(IndexError::BackendViolation(
            "object confirmation lacks provisional registration".into(),
        ))
    }
}

/// Transfer custody after the caller's complete batch preflight.
///
/// Only the shared confirmation orchestrator calls this, inside the same transaction that
/// authenticated every provisional payload and edge before registration.
/// Registration changes permanent tables, leaving provisional rows untouched.
pub(super) fn transfer_preflighted(
    connection: &Connection,
    object: &RegisteredObject,
) -> Result<(), IndexError> {
    let canonical = checked_object(object)?;
    let key = object_key(canonical.reference);
    let pending: bool = cached_query_row(
        connection,
        "SELECT EXISTS(SELECT 1 FROM native_object_install_v1 WHERE object=?1)",
        params![&key],
        |row| row.get(0),
    )
    .map_err(|error| sql(&error))?;
    // A replay or an earlier duplicate in this batch already transferred it.
    if !pending {
        return Ok(());
    }
    // Registered edges were inserted earlier in this same transaction.
    require_object(connection, canonical.reference)?;
    transfer_children(connection, &canonical)
}

/// Consume only the exact batch whose unique registrations were authenticated
/// or inserted by the immediately preceding producer in this transaction.
pub(super) fn transfer_registered_batch<'tx, 'conn>(
    registered: RegisteredBatchInTransaction<'tx, 'conn>,
) -> Result<RegisteredBatchInTransaction<'tx, 'conn>, IndexError> {
    let (transaction, objects, provisional) = registered.parts();
    for (pending, object) in provisional.iter().zip(objects) {
        if *pending {
            // Only permanent tables changed since the complete preflight.
            // Exact delete counts still reject missing or altered custody.
            transfer_children(transaction, object)?;
        }
    }
    Ok(registered)
}

fn transfer_children(
    connection: &Connection,
    canonical: &RegisteredObject,
) -> Result<(), IndexError> {
    let key = object_key(canonical.reference);
    let mut children = canonical.children.clone();
    children.sort_unstable();
    children.dedup();
    let removed = cached_execute(
        connection,
        "DELETE FROM native_object_install_edge_v1 WHERE parent=?1",
        params![&key],
    )
    .map_err(|error| sql(&error))?;
    if removed != children.len() {
        return Err(IndexError::Storage(
            "provisional child transfer count mismatch".into(),
        ));
    }
    let removed = cached_execute(
        connection,
        "DELETE FROM native_object_install_v1 WHERE object=?1",
        params![&key],
    )
    .map_err(|error| sql(&error))?;
    exactly_one(removed, "transfer provisional object")
}
