use super::{optional_path_blob, owner_bytes, owner_from_bytes, scratch_error};
use quanta_index_contract::RepoId;
use quanta_index_core::index::{
    IndexError, OwnerKey, WorkBudget, validate_source_path_repository_key,
};
use rusqlite::{Connection, params, types::ValueRef};

const REPOSITORY_KEY_PREFIX: &[u8; 5] = b"repo\0";
const REPOSITORY_KEY_CAPACITY: usize = REPOSITORY_KEY_PREFIX.len() + RepoId::MAX_UTF8_BYTES_V1;

pub(super) struct RepositoryClaims;

pub(super) struct RepositoryClaim {
    key: [u8; REPOSITORY_KEY_CAPACITY],
    key_len: usize,
    pub(super) leaving: Option<OwnerKey>,
    pub(super) incoming: Option<OwnerKey>,
}

impl RepositoryClaim {
    pub(super) fn key(&self) -> Result<&[u8], IndexError> {
        self.key
            .get(..self.key_len)
            .ok_or_else(|| IndexError::CorruptObject("source repository scratch key length".into()))
    }
}

impl RepositoryClaims {
    pub(super) fn create(db: &Connection) -> Result<(), IndexError> {
        db.execute_batch(
            "CREATE TABLE source_repository_claims(\
             path TEXT NOT NULL, repo_key BLOB NOT NULL, leaving BLOB, incoming BLOB,\
             PRIMARY KEY(path,repo_key)) WITHOUT ROWID",
        )
        .map_err(|error| scratch_error(&error))
    }

    pub(super) fn mark_reserved(
        db: &Connection,
        path: &str,
        key: &[u8],
        owner: OwnerKey,
        leaving: bool,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        validate_key(key, budget)?;
        let query = if leaving {
            "INSERT INTO source_repository_claims(path,repo_key,leaving) VALUES(?1,?2,?3) \
             ON CONFLICT(path,repo_key) DO UPDATE SET leaving=excluded.leaving \
             WHERE source_repository_claims.leaving IS NULL"
        } else {
            "INSERT INTO source_repository_claims(path,repo_key,incoming) VALUES(?1,?2,?3) \
             ON CONFLICT(path,repo_key) DO UPDATE SET incoming=excluded.incoming \
             WHERE source_repository_claims.incoming IS NULL"
        };
        let changed = db
            .prepare_cached(query)
            .map_err(|error| scratch_error(&error))?
            .execute(params![path, key, owner_bytes(owner).as_slice()])
            .map_err(|error| scratch_error(&error))?;
        if changed != 1 {
            return Err(if leaving {
                IndexError::CorruptObject("duplicate prior path owner".into())
            } else {
                IndexError::InvalidInput("duplicate target path".into())
            });
        }
        Ok(())
    }

    pub(super) fn next(
        db: &Connection,
        path: &str,
        after: Option<&[u8]>,
        budget: &mut WorkBudget,
    ) -> Result<Option<RepositoryClaim>, IndexError> {
        budget.checkpoint()?;
        let query = if after.is_some() {
            "SELECT repo_key,leaving,incoming FROM source_repository_claims \
             WHERE path=?1 AND repo_key>?2 ORDER BY repo_key LIMIT 1"
        } else {
            "SELECT repo_key,leaving,incoming FROM source_repository_claims \
             WHERE path=?1 ORDER BY repo_key LIMIT 1"
        };
        let mut statement = db
            .prepare_cached(query)
            .map_err(|error| scratch_error(&error))?;
        let mut rows = if let Some(after) = after {
            statement
                .query(params![path, after])
                .map_err(|error| scratch_error(&error))?
        } else {
            statement
                .query(params![path])
                .map_err(|error| scratch_error(&error))?
        };
        let Some(row) = rows.next().map_err(|error| scratch_error(&error))? else {
            return Ok(None);
        };
        let ValueRef::Blob(bytes) = row.get_ref(0).map_err(|error| scratch_error(&error))? else {
            return Err(IndexError::CorruptObject(
                "source repository scratch key type".into(),
            ));
        };
        validate_key(bytes, budget)?;
        let mut key = [0; REPOSITORY_KEY_CAPACITY];
        key.get_mut(..bytes.len())
            .ok_or_else(|| {
                IndexError::CorruptObject("source repository scratch key length".into())
            })?
            .copy_from_slice(bytes);
        let leaving = optional_path_blob(row.get_ref(1).map_err(|error| scratch_error(&error))?)?
            .map(|bytes| owner_from_bytes(&bytes))
            .transpose()?;
        let incoming = optional_path_blob(row.get_ref(2).map_err(|error| scratch_error(&error))?)?
            .map(|bytes| owner_from_bytes(&bytes))
            .transpose()?;
        Ok(Some(RepositoryClaim {
            key,
            key_len: bytes.len(),
            leaving,
            incoming,
        }))
    }
}

fn validate_key(key: &[u8], budget: &mut WorkBudget) -> Result<(), IndexError> {
    if key.len() > REPOSITORY_KEY_CAPACITY {
        return Err(IndexError::CorruptObject(
            "source repository scratch key length".into(),
        ));
    }
    let name = key
        .strip_prefix(REPOSITORY_KEY_PREFIX)
        .ok_or_else(|| IndexError::CorruptObject("source repository scratch key prefix".into()))?;
    let name = std::str::from_utf8(name).map_err(|_error| {
        IndexError::CorruptObject("source repository scratch key UTF-8".into())
    })?;
    if name.is_empty() {
        return Err(IndexError::CorruptObject(
            "source repository scratch repository ID".into(),
        ));
    }
    validate_source_path_repository_key(key, budget).map_err(|error| match error {
        IndexError::CorruptObject(_) => {
            IndexError::CorruptObject("source repository scratch repository ID".into())
        }
        other => other,
    })
}
