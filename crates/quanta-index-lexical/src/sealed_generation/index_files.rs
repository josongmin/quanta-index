//! The index files a committed generation references.
//!
//! The Tantivy commit (`meta.json`) names its segments, and each segment
//! is a fixed set of component files beside it. That set — never "whatever
//! is in the directory" — is what the seal commits to and what a door
//! checks, because it is exactly what a query maps: a file the commit does
//! not reference is never opened, so a leftover of an aborted merge or a
//! not-yet-collected predecessor cannot change what a query answers and is
//! not part of the contract.

use std::path::Path;

use quanta_index_core::CoreError;
use tantivy::{Index, SegmentComponent};

/// Every segment component file the index's committed segments reference,
/// as `/`-free names inside `generation_dir`, ascending.
///
/// Every component of every segment must exist, except the temporary store
/// (never part of a commit) and the delete bitset, which exists exactly when
/// the segment has deletes; a missing component is a corrupt generation,
/// refused typed.
pub(crate) fn referenced_index_files(
    index: &Index,
    generation_dir: &Path,
) -> Result<Vec<String>, CoreError> {
    let metas = index.searchable_segment_metas().map_err(|error| {
        CoreError::Storage(format!(
            "lexical: list committed segments of {}: {error}",
            generation_dir.display()
        ))
    })?;
    let mut names = Vec::new();
    for meta in metas {
        let has_deletes = meta.delete_opstamp().is_some();
        for component in SegmentComponent::iterator() {
            match component {
                SegmentComponent::TempStore => continue,
                SegmentComponent::Delete if !has_deletes => continue,
                SegmentComponent::Delete
                | SegmentComponent::Postings
                | SegmentComponent::Positions
                | SegmentComponent::FastFields
                | SegmentComponent::FieldNorms
                | SegmentComponent::Terms
                | SegmentComponent::Store => {}
            }
            let relative = meta.relative_path(*component);
            let name = relative
                .to_str()
                .ok_or_else(|| {
                    CoreError::Storage(format!(
                        "lexical: segment file name is not UTF-8 under {}",
                        generation_dir.display()
                    ))
                })?
                .to_string();
            if !generation_dir.join(&name).is_file() {
                return Err(crate::index_store::sidecar_corrupt(
                    generation_dir,
                    &name,
                    "missing although the sealed commit references it",
                ));
            }
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}
