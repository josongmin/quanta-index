//! Immutable coverage snapshots with shared rows and a derived partition index.
//!
//! Cloning a read view shares its trees. Mutating a candidate copies only tree
//! paths; retained readers keep their original rows. The index is private and
//! cannot become a second, independently writable coverage authority.

use std::sync::Arc;

use imbl::OrdMap;
use quanta_index_contract_base::SourceFileKey;
use sha2::{Digest as _, Sha256};

use crate::SourceFileCoverage;

// Both derived trees and their branch separators share the same key allocation.
// Tree path copies must not clone source repository/path strings.
type Rows = OrdMap<Arc<SourceFileKey>, Arc<SourceFileCoverage>>;

// Partition lengths use a canonical u64 encoding on supported pointer widths.
const _: () = assert!(usize::BITS <= u64::BITS);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FileCoverageSnapshot {
    rows: Rows,
    partitions: OrdMap<u8, Rows>,
}

impl FileCoverageSnapshot {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Conservative inline/tree allocation bound for this snapshot representation.
    ///
    /// Each nonempty imbl 7 B+ tree has at most one leaf and one branch per row.
    /// Charge both full 16-slot node layouts, including chunk metadata and shared
    /// allocation headers. Rows and keys each have one shared allocation, not one
    /// full row per tree slot. The partition directory has at most 256 entries.
    /// String buffers and decoder temporaries require a separate byte charge.
    #[must_use]
    pub fn structural_heap_bytes_bound(row_count: u64) -> Option<u64> {
        fn tree_bytes<K, V>(entries: u64) -> Option<u64> {
            let leaf = std::mem::size_of::<[(K, V); 16]>()
                .checked_add(std::mem::size_of::<[usize; 4]>())?;
            let branch = std::mem::size_of::<[K; 16]>()
                .checked_add(std::mem::size_of::<[usize; 17]>())?
                .checked_add(std::mem::size_of::<[usize; 8]>())?;
            entries.checked_mul(u64::try_from(leaf.checked_add(branch)?).ok()?)
        }
        let indexes =
            tree_bytes::<Arc<SourceFileKey>, Arc<SourceFileCoverage>>(row_count)?.checked_mul(2)?;
        let shared = std::mem::size_of::<SourceFileKey>()
            .checked_add(std::mem::size_of::<SourceFileCoverage>())?
            .checked_add(std::mem::size_of::<[usize; 4]>())?;
        let shared = row_count.checked_mul(u64::try_from(shared).ok()?)?;
        let directory = tree_bytes::<u8, Rows>(row_count.min(256))?;
        indexes
            .checked_add(shared)?
            .checked_add(directory)?
            .checked_add(u64::try_from(std::mem::size_of::<Self>()).ok()?)
    }

    #[must_use]
    pub fn get(&self, key: &SourceFileKey) -> Option<&SourceFileCoverage> {
        self.rows.get(key).map(Arc::as_ref)
    }

    #[must_use]
    pub fn contains_key(&self, key: &SourceFileKey) -> bool {
        self.rows.contains_key(key)
    }

    #[must_use]
    pub fn last_key_value(&self) -> Option<(&SourceFileKey, &SourceFileCoverage)> {
        self.rows
            .get_max()
            .map(|(key, row)| (key.as_ref(), row.as_ref()))
    }

    #[must_use]
    pub fn iter(&self) -> FileCoverageIter<'_> {
        FileCoverageIter(self.rows.iter())
    }

    pub fn values(&self) -> impl Iterator<Item = &SourceFileCoverage> {
        self.rows.values().map(Arc::as_ref)
    }

    /// Stable, length-framed routing; different source repositories never alias.
    #[must_use]
    pub fn partition_for(key: &SourceFileKey) -> u8 {
        let mut hash = Sha256::new();
        for text in [key.source_repo_id.as_str(), key.repo_relative_path.as_str()] {
            // Encoding is independent of pointer width and host endianness.
            let mut length = [0_u8; 8];
            for (byte, value) in length.iter_mut().zip(text.len().to_le_bytes()) {
                *byte = value;
            }
            hash.update(length);
            hash.update(text.as_bytes());
        }
        let [partition, ..] = <[u8; 32]>::from(hash.finalize());
        partition
    }

    pub fn partition(
        &self,
        partition: u8,
    ) -> impl Iterator<Item = (&SourceFileKey, &SourceFileCoverage)> {
        self.partitions
            .get(&partition)
            .into_iter()
            .flat_map(|rows| rows.iter())
            .map(|(key, row)| (key.as_ref(), row.as_ref()))
    }

    /// Publish both indexes from the same shared immutable row.
    pub fn insert(
        &mut self,
        key: SourceFileKey,
        value: SourceFileCoverage,
    ) -> Option<SourceFileCoverage> {
        self.insert_shared(key, value)
            .map(|previous| (*previous).clone())
    }

    /// Replace a row when the caller does not need the previous value.
    ///
    /// A delta shares the base snapshot's rows. Returning an owned previous
    /// value would clone that row even when the caller immediately drops it.
    pub fn insert_without_previous(&mut self, key: SourceFileKey, value: SourceFileCoverage) {
        drop(self.insert_shared(key, value));
    }

    fn insert_shared(
        &mut self,
        key: SourceFileKey,
        value: SourceFileCoverage,
    ) -> Option<Arc<SourceFileCoverage>> {
        let partition = Self::partition_for(&key);
        let key = Arc::new(key);
        let row = Arc::new(value);
        let mut page = self.partitions.get(&partition).cloned().unwrap_or_default();
        let _previous = page.insert(key.clone(), Arc::clone(&row));
        let _previous = self.partitions.insert(partition, page);
        self.rows.insert(key, row)
    }

    pub fn remove(&mut self, key: &SourceFileKey) -> Option<SourceFileCoverage> {
        self.remove_shared(key).map(|previous| (*previous).clone())
    }

    /// Remove a row without cloning a shared previous value for a discarded
    /// return value.
    pub fn remove_without_previous(&mut self, key: &SourceFileKey) {
        drop(self.remove_shared(key));
    }

    fn remove_shared(&mut self, key: &SourceFileKey) -> Option<Arc<SourceFileCoverage>> {
        let previous = self.rows.remove(key)?;
        let partition = Self::partition_for(key);
        if let Some(mut page) = self.partitions.get(&partition).cloned() {
            let _previous = page.remove(key);
            if page.is_empty() {
                let _previous = self.partitions.remove(&partition);
            } else {
                let _previous = self.partitions.insert(partition, page);
            }
        }
        Some(previous)
    }
}

pub struct FileCoverageIter<'a>(
    imbl::ordmap::Iter<
        'a,
        Arc<SourceFileKey>,
        Arc<SourceFileCoverage>,
        imbl::shared_ptr::DefaultSharedPtr,
    >,
);

impl<'a> Iterator for FileCoverageIter<'a> {
    type Item = (&'a SourceFileKey, &'a SourceFileCoverage);

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(key, row)| (key.as_ref(), row.as_ref()))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<'a> IntoIterator for &'a FileCoverageSnapshot {
    type Item = (&'a SourceFileKey, &'a SourceFileCoverage);
    type IntoIter = FileCoverageIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl FromIterator<(SourceFileKey, SourceFileCoverage)> for FileCoverageSnapshot {
    fn from_iter<T: IntoIterator<Item = (SourceFileKey, SourceFileCoverage)>>(iter: T) -> Self {
        let mut result = Self::new();
        for (key, row) in iter {
            result.insert_without_previous(key, row);
        }
        result
    }
}

impl<const N: usize> From<[(SourceFileKey, SourceFileCoverage); N]> for FileCoverageSnapshot {
    fn from(rows: [(SourceFileKey, SourceFileCoverage); N]) -> Self {
        rows.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::FileCoverageSnapshot;
    use crate::lex::LanguageCode;
    use crate::{
        RepoId, RepoRelativePath, RevisionId, SourceFileCoverage, SourceFileRevision,
        SymbolCoverage, SymbolNameSourcePolicyV1,
    };
    use std::sync::Arc;

    #[test]
    fn coverage_indexes_share_keys_and_rows_while_old_views_remain_immutable()
    -> Result<(), Box<dyn std::error::Error>> {
        let row = SourceFileCoverage {
            source: SourceFileRevision {
                file: crate::SourceFileKey {
                    source_repo_id: RepoId::new("source")?,
                    repo_relative_path: RepoRelativePath::new("src/shared.rs"),
                },
                revision_id: RevisionId::new("revision")?,
                source_sha256: [2; 32],
            },
            language: LanguageCode::new("rust")?,
            producer_policy_sha256: [1; 32],
            symbol_name_source_policy: SymbolNameSourcePolicyV1::Unspecified,
            unit_set_sha256: [3; 32],
            text_admitted: true,
            symbols: SymbolCoverage::NotRequested,
        };
        let original = FileCoverageSnapshot::from([(row.source.file.clone(), row.clone())]);
        let (global_key, global_row) = original.rows.iter().next().ok_or("missing global row")?;
        let partition = original
            .partitions
            .get(&FileCoverageSnapshot::partition_for(&row.source.file))
            .ok_or("missing partition")?;
        let (partition_key, partition_row) =
            partition.iter().next().ok_or("missing partition row")?;
        if !Arc::ptr_eq(global_key, partition_key) || !Arc::ptr_eq(global_row, partition_row) {
            return Err("derived indexes did not share key and row allocations".into());
        }
        let mut candidate = original.clone();
        let mut updated = row.clone();
        updated.symbols = SymbolCoverage::ParseFailed;
        candidate.insert_without_previous(updated.source.file.clone(), updated.clone());
        if original.get(&row.source.file) != Some(&row)
            || candidate.get(&row.source.file) != Some(&updated)
            || candidate
                .partition(FileCoverageSnapshot::partition_for(&row.source.file))
                .next()
                != Some((&updated.source.file, &updated))
        {
            return Err("candidate mutation changed an old view or split its indexes".into());
        }
        candidate.remove_without_previous(&row.source.file);
        if !candidate.is_empty() || original.get(&row.source.file) != Some(&row) {
            return Err("candidate deletion changed the old view".into());
        }
        Ok(())
    }

    #[test]
    fn coverage_structural_bound_supports_fixed_xl_rows_and_refuses_overflow()
    -> Result<(), Box<dyn std::error::Error>> {
        let bound = FileCoverageSnapshot::structural_heap_bytes_bound(32_768)
            .ok_or("XL structural charge overflowed")?;
        if bound > 64 * 1024 * 1024
            || FileCoverageSnapshot::structural_heap_bytes_bound(u64::MAX).is_some()
        {
            return Err(
                format!("shared XL structural bound or overflow refusal drifted: {bound}").into(),
            );
        }
        Ok(())
    }
}
