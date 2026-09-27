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

type Rows = OrdMap<SourceFileKey, Arc<SourceFileCoverage>>;

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
        self.rows.get_max().map(|(key, row)| (key, row.as_ref()))
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
            .map(|(key, row)| (key, row.as_ref()))
    }

    /// Publish both indexes from the same shared immutable row.
    pub fn insert(
        &mut self,
        key: SourceFileKey,
        value: SourceFileCoverage,
    ) -> Option<SourceFileCoverage> {
        let partition = Self::partition_for(&key);
        let row = Arc::new(value);
        let mut page = self.partitions.get(&partition).cloned().unwrap_or_default();
        let _previous = page.insert(key.clone(), Arc::clone(&row));
        let _previous = self.partitions.insert(partition, page);
        self.rows
            .insert(key, row)
            .map(|previous| (*previous).clone())
    }

    pub fn remove(&mut self, key: &SourceFileKey) -> Option<SourceFileCoverage> {
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
        Some((*previous).clone())
    }
}

pub struct FileCoverageIter<'a>(
    imbl::ordmap::Iter<
        'a,
        SourceFileKey,
        Arc<SourceFileCoverage>,
        imbl::shared_ptr::DefaultSharedPtr,
    >,
);

impl<'a> Iterator for FileCoverageIter<'a> {
    type Item = (&'a SourceFileKey, &'a SourceFileCoverage);

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(key, row)| (key, row.as_ref()))
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
            let _previous = result.insert(key, row);
        }
        result
    }
}

impl<const N: usize> From<[(SourceFileKey, SourceFileCoverage); N]> for FileCoverageSnapshot {
    fn from(rows: [(SourceFileKey, SourceFileCoverage); N]) -> Self {
        rows.into_iter().collect()
    }
}
