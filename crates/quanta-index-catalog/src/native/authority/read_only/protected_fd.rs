use std::{ffi::CString, path::Path, sync::Arc};

use quanta_index_core::index::{IndexError, MemoryLease, WorkBudget};
use quanta_index_sqlite_unix_syscall::{
    ProtectedFileIdentity, ProtectedReadError, ProtectedReadExpectation, ProtectedReadLimits,
    with_protected_read,
};
use rusqlite::Connection;

use super::{CATALOG_FILE_NAME, CatalogReadOnlyIoAdmission, FileIdentity, engine, sql};

pub(super) enum Outcome<T, E, F> {
    Declined(F, Option<MemoryLease>),
    Complete(Result<T, E>),
}

enum Failure<E> {
    Reader(ProtectedReadError),
    Inspect(E),
}

impl<E> From<ProtectedReadError> for Failure<E> {
    fn from(error: ProtectedReadError) -> Self {
        Self::Reader(error)
    }
}

pub(super) fn inspect_if_selected<T, E, F>(
    io: &dyn CatalogReadOnlyIoAdmission,
    path: &Path,
    main: FileIdentity,
    wal: Option<FileIdentity>,
    budget: &mut WorkBudget,
    map_close: fn(IndexError) -> E,
    pending: (F, Option<MemoryLease>),
) -> Result<Outcome<T, E, F>, E>
where
    E: From<IndexError>,
    F: FnOnce(&mut Connection, &Path, &mut WorkBudget, Option<MemoryLease>) -> Result<T, E>,
{
    let directory = path.parent().ok_or_else(|| {
        IndexError::BackendViolation("protected Catalog file has no directory".into())
    })?;
    let name = CString::new(CATALOG_FILE_NAME).map_err(|_error| {
        IndexError::BackendViolation("protected Catalog filename contains NUL".into())
    })?;
    let limits = ProtectedReadLimits {
        main_bytes: main.bytes,
        wal_bytes: wal.map_or(0, |wal| wal.bytes),
        expected: Some(ProtectedReadExpectation {
            main: main.into(),
            wal: wal.map(Into::into),
        }),
    };
    let request = budget.request().clone();
    let observed = request.clone();
    let mut pending = Some(pending);
    let mut completed = None;
    let selected = io.with_protected_catalog_directory_v1(
        directory,
        &request,
        wal.is_some(),
        &mut |directory| {
            let Some((inspect, allocation)) = pending.take() else {
                completed = Some(Err(protocol_error().into()));
                return;
            };
            let observed = observed.clone();
            let check = Arc::new(move || observed.interruption().is_none());
            completed = Some(
                match with_protected_read(directory, &name, limits, check, |connection| {
                    inspect(connection, path, budget, allocation).map_err(Failure::Inspect)
                }) {
                    Ok(value) => Ok(value),
                    Err(Failure::Inspect(error)) => Err(error),
                    Err(Failure::Reader(error)) => {
                        Err(map_reader_error(io, error, budget, map_close))
                    }
                },
            );
        },
    )?;
    match (selected, pending, completed) {
        (true, None, Some(value)) => Ok(Outcome::Complete(value)),
        (false, Some((inspect, allocation)), None) => Ok(Outcome::Declined(inspect, allocation)),
        _ => Err(protocol_error().into()),
    }
}

fn protocol_error() -> IndexError {
    IndexError::BackendViolation(
        "protected Catalog directory callback violated its contract".into(),
    )
}

fn map_reader_error<E: From<IndexError>>(
    io: &dyn CatalogReadOnlyIoAdmission,
    error: ProtectedReadError,
    budget: &WorkBudget,
    map_close: fn(IndexError) -> E,
) -> E {
    match error {
        ProtectedReadError::Cancelled => budget
            .checkpoint()
            .err()
            .unwrap_or(IndexError::Cancelled)
            .into(),
        ProtectedReadError::Contended => IndexError::Conflict.into(),
        ProtectedReadError::Sqlite(error) => engine(&error).into(),
        ProtectedReadError::SqliteClose(error) => {
            io.poison_protected_catalog_close_v1();
            map_close(sql(&error))
        }
        ProtectedReadError::Close(error) => {
            io.poison_protected_catalog_close_v1();
            map_close(IndexError::Storage(format!(
                "close protected Catalog input: {error}"
            )))
        }
        ProtectedReadError::Unretired => {
            io.poison_protected_catalog_close_v1();
            map_close(IndexError::Storage(
                "protected Catalog input did not retire".into(),
            ))
        }
        ProtectedReadError::Changed | ProtectedReadError::ByteLimit => {
            IndexError::Storage("Catalog inputs changed during verification".into()).into()
        }
        ProtectedReadError::UnsafeInput => IndexError::Storage(
            "Catalog verification input must be a private, owned, single-link regular file".into(),
        )
        .into(),
        ProtectedReadError::TruncatedMain => {
            IndexError::Storage("Catalog header is truncated".into()).into()
        }
        ProtectedReadError::RollbackJournal => {
            IndexError::Storage("Catalog has a rollback journal".into()).into()
        }
        ProtectedReadError::Io(error) => {
            IndexError::Storage(format!("inspect Catalog input: {error}")).into()
        }
        ProtectedReadError::VfsUnavailable => {
            IndexError::BackendViolation("protected Catalog FD reader unavailable".into()).into()
        }
        ProtectedReadError::Reentrant => {
            IndexError::BackendViolation("protected Catalog FD reader is already active".into())
                .into()
        }
    }
}

impl From<FileIdentity> for ProtectedFileIdentity {
    fn from(identity: FileIdentity) -> Self {
        Self {
            dev: identity.device,
            ino: identity.inode,
            bytes: identity.bytes,
            modified: (identity.modified_seconds, identity.modified_nanos),
            changed: (identity.changed_seconds, identity.changed_nanos),
        }
    }
}
