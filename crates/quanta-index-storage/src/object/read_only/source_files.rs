//! Repository-aware Source readback using the same held immutable reader as
//! offline BOOT. No writer, scratch store or alternate root is constructed.

use super::*;
use quanta_index_core::index::{
    OwnerKey, SourceCut, SourcePathKey, SourcePathOwners, SourcePathRepositoryKey, SourceRecord,
    SourceRecordOwner, owner_map_key,
};

impl FileObjectReader {
    /// Lend exactly one repository/path's authenticated original bytes. Source
    /// revision and complete-census authority remain the caller's responsibility.
    pub fn with_repository_source_file_v1<T>(
        &self,
        cut: &SourceCut,
        repository: &quanta_index_core::index::RepoId,
        path: &str,
        budget: &mut WorkBudget,
        visit: impl FnOnce(OwnerKey, &SourceRecord, &[u8], &mut WorkBudget) -> Result<T, IndexError>,
    ) -> Result<T, IndexError> {
        let key = SourcePathKey::copy(path, budget)?;
        let entry = self
            .lookup_map_value_v1(cut.paths, key.as_slice(), budget)?
            .ok_or(IndexError::SnapshotUnavailable)?;
        let SourcePathOwners::Repositories(repositories) = SourcePathOwners::decode(entry, budget)?
        else {
            return Err(IndexError::DigestConflict);
        };
        let key = SourcePathRepositoryKey::new(Some(repository))?;
        let owner = self
            .lookup_map_value_v1(
                quanta_index_core::index::MapRoot(Some(repositories)),
                key.as_slice()?,
                budget,
            )?
            .ok_or(IndexError::SnapshotUnavailable)?;
        let SourcePathOwners::LegacyOwner(owner) = SourcePathOwners::decode(owner, budget)? else {
            return Err(IndexError::DigestConflict);
        };
        let reference = self
            .lookup_map_object(cut.owners, &owner_map_key(owner), budget)?
            .ok_or(IndexError::SnapshotUnavailable)?;
        let stored = self.get(reference, budget)?;
        let record =
            SourceRecordOwner::decode_json(&stored.payload, budget, MemoryLifetime::Retained)?;
        if record.path != path
            || record.source_repo_id.as_ref() != Some(repository)
            || stored.registration.children.as_slice() != [record.bytes]
        {
            return Err(IndexError::DigestConflict);
        }
        let body = self.get(record.bytes, budget)?;
        if !body.registration.children.is_empty()
            || u64::try_from(body.payload.len()).map_err(|_| IndexError::ResourceLimited)?
                != record.byte_len
        {
            return Err(IndexError::DigestConflict);
        }
        visit(owner, &record, &body.payload, budget)
    }

    /// Count every original owner from the actual selected Source map. Combine
    /// with unique per-file equality loans to reject an extra or omitted file.
    pub fn source_file_count_v1(
        &self,
        cut: &SourceCut,
        budget: &mut WorkBudget,
    ) -> Result<u64, IndexError> {
        let mut count = 0_u64;
        let mut after: Option<quanta_index_core::index::MapPageEntry> = None;
        loop {
            let page = self.scan_map(
                cut.owners,
                after.as_ref().map(|entry| entry.key()).transpose()?,
                128,
                budget,
            )?;
            for (key, value) in &page.entries {
                let _owner = quanta_index_core::index::owner_from_map_key(key)?;
                if !matches!(value, quanta_index_core::index::MapValue::Object(_)) {
                    return Err(IndexError::DigestConflict);
                }
                count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
            if !page.has_more {
                return Ok(count);
            }
            let next = page.cursor().last().ok_or(IndexError::ResourceLimited)?;
            if after
                .as_ref()
                .map(|entry| entry.key())
                .transpose()?
                .is_some_and(|previous| next.key().is_ok_and(|next| next <= previous))
            {
                return Err(IndexError::DigestConflict);
            }
            after = Some(next);
        }
    }
}
