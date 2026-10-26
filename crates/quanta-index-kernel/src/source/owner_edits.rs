//! Attempt-local reuse of complete validated owner edits. The existing canonical
//! mutation digest is rechecked before the map consumer may install any roots.

use quanta_index_core::index::{
    Digest, IndexError, MemoryLease, MemoryLifetime, ObjectRef, OwnerMutation, OwnerMutationCursor,
    OwnerMutationDigest, OwnerMutationSet, WorkBudget, WorkUsage, owner_from_map_key,
    owner_map_key,
};
use rusqlite::{params, types::ValueRef};

use super::{PathScratch, scratch_error};

// All fields are fixed-width identities or optional identities, with no strings.
const MAX_MUTATION_JSON: usize = 1024;
const PAGE_ROWS: u64 = 128;
const OWNER_EDIT_SQL: &str = "SELECT CASE WHEN typeof(owner)='blob' AND length(owner)=38
      AND (expected IS NULL OR (typeof(expected)='blob' AND length(expected)=32))
      AND (next IS NULL OR (typeof(next)='blob' AND length(next)=32)) THEN owner END,
     CASE WHEN typeof(expected)='blob' AND length(expected)=32 THEN expected END,
     CASE WHEN typeof(next)='blob' AND length(next)=32 THEN next END
     FROM owner_edits WHERE owner>=?1 ORDER BY owner LIMIT ?2";

fn decode_row(
    key: ValueRef<'_>,
    expected: ValueRef<'_>,
    next: ValueRef<'_>,
) -> Result<OwnerMutation, IndexError> {
    let ValueRef::Blob(key) = key else {
        return Err(IndexError::CorruptObject("owner edit row shape".into()));
    };
    Ok(OwnerMutation {
        owner: owner_from_map_key(key)?,
        expected: super::optional_path_blob(expected)?.map(|bytes| ObjectRef(Digest(bytes))),
        next: super::optional_path_blob(next)?.map(|bytes| ObjectRef(Digest(bytes))),
    })
}

impl PathScratch {
    pub(super) fn begin_owner_spool(&self, budget: &WorkBudget) -> Result<(), IndexError> {
        budget.checkpoint()?;
        self.connection
            .scoped()?
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| scratch_error(&error))
    }

    pub(super) fn append_owner_edit(
        &self,
        mutation: &OwnerMutation,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.charge(WorkUsage {
            writes: 1,
            mutations: 1,
            bytes: 160,
            ..WorkUsage::default()
        })?;
        let db = self.connection.scoped()?;
        quanta_index_scratch_sqlite::reserve_for_write(&db, self.allocation.as_ref(), 160, budget)?;
        let expected = mutation.expected.map(|reference| reference.0.0);
        let next = mutation.next.map(|reference| reference.0.0);
        let changed = db
            .prepare_cached("INSERT INTO owner_edits(owner,expected,next) VALUES(?1,?2,?3)")
            .map_err(|error| scratch_error(&error))?
            .execute(params![
                owner_map_key(mutation.owner).as_slice(),
                expected.as_ref().map(<[u8; 32]>::as_slice),
                next.as_ref().map(<[u8; 32]>::as_slice),
            ])
            .map_err(|error| scratch_error(&error))?;
        if changed != 1 {
            return Err(IndexError::Storage("owner edit insertion count".into()));
        }
        Ok(())
    }

    pub(super) fn validated_owner_edits<'a>(
        &'a self,
        completed: &OwnerMutationCursor<'_>,
        expected: OwnerMutationSet,
        budget: &mut WorkBudget,
    ) -> Result<OwnerEditCursor<'a>, IndexError> {
        if completed.sealed_set() != Some(expected) {
            return Err(IndexError::BackendViolation(
                "owner edits require the complete original manifest".into(),
            ));
        }
        budget.checkpoint()?;
        self.connection
            .scoped()?
            .execute_batch("COMMIT")
            .map_err(|error| scratch_error(&error))?;
        let capacity = usize::try_from(expected.count.min(PAGE_ROWS))
            .map_err(|_error| IndexError::ResourceLimited)?;
        let bytes = capacity
            .checked_mul(std::mem::size_of::<Option<OwnerMutation>>())
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        let memory = if bytes == 0 {
            None
        } else {
            budget.reserve_memory(bytes, MemoryLifetime::Retained)?
        };
        let mut page = Vec::new();
        page.try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(OwnerEditCursor {
            scratch: self,
            page,
            capacity,
            offset: 0,
            _page_memory: memory,
            expected,
            last: None,
            digest: OwnerMutationDigest::default(),
            finished: false,
        })
    }
}

pub(super) struct OwnerEditCursor<'a> {
    scratch: &'a PathScratch,
    expected: OwnerMutationSet,
    last: Option<[u8; 38]>,
    digest: OwnerMutationDigest,
    finished: bool,
    page: Vec<Option<OwnerMutation>>,
    capacity: usize,
    offset: usize,
    // Owned rows die before their original retained backing reservation.
    _page_memory: Option<MemoryLease>,
}

impl OwnerEditCursor<'_> {
    fn read_page(&mut self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        let limit = self.capacity.max(1);
        budget.charge(WorkUsage {
            reads: 1,
            bytes: u64::try_from(limit)
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_mul(102)
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        // A strict successor permits one indexed range query for every page,
        // including malformed empty or extended keys, without an OR/full scan.
        let mut successor = [0_u8; 39];
        let lower = if let Some(last) = &self.last {
            successor
                .get_mut(..38)
                .ok_or(IndexError::ResourceLimited)?
                .copy_from_slice(last);
            successor.as_slice()
        } else {
            &[]
        };
        let db = self.scratch.connection.scoped()?;
        let mut statement = db
            .prepare_cached(OWNER_EDIT_SQL)
            .map_err(|error| scratch_error(&error))?;
        let mut rows = statement
            .query(params![
                lower,
                i64::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?
            ])
            .map_err(|error| scratch_error(&error))?;
        while let Some(row) = rows.next().map_err(|error| scratch_error(&error))? {
            budget.checkpoint()?;
            let mutation = decode_row(
                row.get_ref(0).map_err(|error| scratch_error(&error))?,
                row.get_ref(1).map_err(|error| scratch_error(&error))?,
                row.get_ref(2).map_err(|error| scratch_error(&error))?,
            )?;
            if self.page.len() == self.capacity {
                return Err(IndexError::CorruptObject("owner edit excess row".into()));
            }
            self.page.push(Some(mutation));
        }
        Ok(())
    }

    pub(super) fn next_mutation(
        &mut self,
        budget: &mut WorkBudget,
    ) -> Result<Option<OwnerMutation>, IndexError> {
        budget.checkpoint()?;
        if self.finished {
            return Ok(None);
        }
        if self.offset == self.page.len() {
            self.page.clear();
            self.offset = 0;
            if let Err(error) = self.read_page(budget) {
                self.page.clear();
                return Err(error);
            }
        }
        let Some(mutation) = self.page.get(self.offset).and_then(Option::as_ref) else {
            if self.digest.count() != self.expected.count
                || self.digest.finish() != self.expected.digest
            {
                return Err(IndexError::CorruptObject(
                    "owner edit manifest mismatch".into(),
                ));
            }
            // Non-BLOB keys sort before the indexed BLOB range. Count every
            // physical row as well, so an extra malformed key cannot hide there.
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            let rows: i64 = self
                .scratch
                .connection
                .scoped()?
                .prepare_cached("SELECT COUNT(*) FROM owner_edits")
                .map_err(|error| scratch_error(&error))?
                .query_row([], |row| row.get(0))
                .map_err(|error| scratch_error(&error))?;
            let rows = u64::try_from(rows).map_err(|_error| {
                IndexError::CorruptObject("owner edit negative row count".into())
            })?;
            if rows != self.expected.count {
                return Err(IndexError::CorruptObject(
                    "owner edit row count mismatch".into(),
                ));
            }
            self.finished = true;
            return Ok(None);
        };
        if self.digest.count() >= self.expected.count {
            return Err(IndexError::CorruptObject("owner edit excess row".into()));
        }
        budget.charge(WorkUsage {
            mutations: 1,
            bytes: u64::try_from(MAX_MUTATION_JSON)
                .map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let encoded = crate::staging_payload::encode(mutation, MAX_MUTATION_JSON, budget)?;
        let key = owner_map_key(mutation.owner);
        self.digest.push_encoded(&key, &encoded.bytes)?;
        self.last = Some(key);
        let mutation = self
            .page
            .get_mut(self.offset)
            .and_then(Option::take)
            .ok_or_else(|| IndexError::BackendViolation("owner edit row disappeared".into()))?;
        self.offset = self
            .offset
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(Some(mutation))
    }
}

#[cfg(test)]
mod tests;
