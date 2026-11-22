//! Durable, body-keyed authority for Native Source quarantine.

use quanta_index_contract::index::Digest;
use quanta_index_core::index::WorkUsage;
use rusqlite::{Connection, OptionalExtension as _, params, types::ValueRef};

use super::*;

const DOMAIN: &[u8] = b"qidx.native.source-body-incident.v1\0";
const ROW_WORK_BYTES: u64 = 256;

#[derive(PartialEq, Eq)]
pub(super) enum State {
    Pending,
    Complete,
}

fn malformed() -> IndexError {
    IndexError::Storage("native Source body incident row mismatch".into())
}

fn read_state(
    body: ObjectRef,
    pending: i64,
    digest: Option<[u8; 32]>,
) -> Result<State, IndexError> {
    let expected = fields_digest(DOMAIN, &(body.0.0, pending))?;
    if !matches!(pending, 0 | 1)
        || digest.is_none_or(|digest| digest.as_slice() != expected.as_slice())
    {
        return Err(malformed());
    }
    Ok(if pending == 1 {
        State::Pending
    } else {
        State::Complete
    })
}

fn fixed32(value: ValueRef<'_>) -> Option<[u8; 32]> {
    match value {
        ValueRef::Blob(bytes) => bytes.try_into().ok(),
        _ => None,
    }
}

pub(super) fn state(
    connection: &Connection,
    body: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<Option<State>, IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        bytes: ROW_WORK_BYTES,
        ..WorkUsage::default()
    })?;
    let row = cached_query_row(
        connection,
        "SELECT pending, row_digest FROM native_source_body_incident_v1 WHERE object=?1",
        params![body.0.0.as_slice()],
        |row| Ok((row.get::<_, i64>(0)?, fixed32(row.get_ref(1)?))),
    )
    .optional()
    .map_err(|error| sql(&error))?;
    row.map(|(pending, digest)| read_state(body, pending, digest))
        .transpose()
}

pub(super) fn seed(
    connection: &Connection,
    body: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if state(connection, body, budget)?.is_some() {
        return Ok(());
    }
    charge_write(budget)?;
    let changed = cached_execute(
        connection,
        "INSERT INTO native_source_body_incident_v1(object, pending, row_digest) VALUES (?1, 1, ?2)",
        params![body.0.0.as_slice(), fields_digest(DOMAIN, &(body.0.0, 1_i64))?],
    )
    .map_err(|error| sql(&error))?;
    exactly_one(changed, "record pending native Source body incident")
}

pub(super) fn complete(
    connection: &Connection,
    body: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    charge_write(budget)?;
    let changed = cached_execute(
        connection,
        "UPDATE native_source_body_incident_v1 SET pending=0, row_digest=?2
         WHERE object=?1 AND pending=1 AND row_digest=?3",
        params![
            body.0.0.as_slice(),
            fields_digest(DOMAIN, &(body.0.0, 0_i64))?,
            fields_digest(DOMAIN, &(body.0.0, 1_i64))?,
        ],
    )
    .map_err(|error| sql(&error))?;
    exactly_one(changed, "complete native Source body incident")
}

fn charge_write(budget: &mut WorkBudget) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        writes: 1,
        mutations: 1,
        bytes: ROW_WORK_BYTES,
        ..WorkUsage::default()
    })
}

pub(super) fn visit(
    connection: &Connection,
    budget: &mut WorkBudget,
    mut visit: impl FnMut(ObjectRef, State, &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    // Authenticate the state before a caller decides which incidents need
    // materialization. A damaged pending flag must not hide its own row.
    let mut statement = connection
        .prepare_cached("SELECT object, pending, row_digest FROM native_source_body_incident_v1 ORDER BY object")
        .map_err(|error| sql(&error))?;
    let mut rows = statement.query([]).map_err(|error| sql(&error))?;
    while let Some(row) = rows.next().map_err(|error| sql(&error))? {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: ROW_WORK_BYTES,
            ..WorkUsage::default()
        })?;
        let body = fixed32(row.get_ref(0).map_err(|error| sql(&error))?).ok_or_else(malformed)?;
        let body = ObjectRef(Digest(body));
        let pending = row.get(1).map_err(|error| sql(&error))?;
        let digest = fixed32(row.get_ref(2).map_err(|error| sql(&error))?);
        let state = read_state(body, pending, digest)?;
        visit(body, state, budget)?;
    }
    Ok(())
}
