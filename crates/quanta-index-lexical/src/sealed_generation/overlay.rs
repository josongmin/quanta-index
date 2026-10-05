//! The repo-metadata overlay sidecars: one file per family beside the index.
//!
//! Seven families, each a whole-snapshot CBOR file a query decodes at open:
//! the `FullBundle` repo-metadata payload and the six source-repo keyed
//! authorities the aux ingest routes publish. They are written into the
//! generation before its seal, by atomic durable rename, and the seal lists
//! every family present with its length and digest; after the seal no
//! family may change (QI-BB-030). A family the seal did not list is one the
//! generation does not carry, and a query asking for it is refused typed —
//! never one that was lost.

use std::fs::File;
use std::path::Path;

use quanta_index_core::CoreError;

use crate::overlay_codec::OverlayFamily;

/// Write one family's snapshot durably: temporary file, fsync, rename,
/// directory fsync. Creates the generation directory when this is the first
/// thing published into it.
pub(crate) fn persist_overlay(
    generation_dir: &Path,
    family: OverlayFamily,
    bytes: &[u8],
) -> Result<(), CoreError> {
    std::fs::create_dir_all(generation_dir).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: create generation directory {} for {}: {err}",
            generation_dir.display(),
            family.file_name()
        ))
    })?;
    crate::index_store::write_atomic_durable(
        &family.path(generation_dir),
        bytes,
        family.file_name(),
    )
}

/// Remove one family's snapshot durably; an absent file is already removed.
pub(crate) fn remove_overlay(
    generation_dir: &Path,
    family: OverlayFamily,
) -> Result<(), CoreError> {
    let path = family.path(generation_dir);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(CoreError::Storage(format!(
                "lexical: remove {} snapshot {}: {err}",
                family.file_name(),
                path.display()
            )));
        }
    }
    File::open(generation_dir)
        .and_then(|directory| {
            crate::causal_profile::timed_sync("overlay_directory", || directory.sync_all())
        })
        .map_err(|err| {
            CoreError::Storage(format!(
                "lexical: fsync {} after removing {}: {err}",
                generation_dir.display(),
                family.file_name()
            ))
        })
}

#[cfg(test)]
mod tests {
    use crate::overlay_codec::OverlayFamily;

    #[test]
    fn every_family_round_trips_through_its_file_name() {
        for family in OverlayFamily::ALL {
            assert_eq!(
                OverlayFamily::from_file_name(family.file_name()),
                Some(family)
            );
        }
        assert_eq!(OverlayFamily::from_file_name("meta.json"), None);
    }
}
