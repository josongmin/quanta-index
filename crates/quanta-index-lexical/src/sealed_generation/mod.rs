//! The sealed lexical generation: what it commits to and how it is proved.
//!
//! See [`manifest`] for the commitment and its format, [`overlay`] for the
//! repo-metadata sidecars it lists, [`seal`] for the measurement that
//! writes it, [`verify`] for the walk both doors share, and [`scrub`] for
//! the deep re-measurement between seals.

pub(crate) mod coverage;
mod index_directory;
mod index_files;
mod manifest;
mod overlay;
mod path_io;
mod scrub;
mod seal;
mod verify;

pub use seal::LexicalSealCommitmentStats;

pub(crate) use index_directory::SealedIndexDirectory;
pub(crate) use manifest::{LEXICAL_SEALED_MANIFEST_FILE_NAME, manifest_path, read_manifest};
pub(crate) use overlay::{persist_overlay, remove_overlay};
pub(crate) use path_io::{
    entry_names_at, is_unsafe_artifact_path, open_generation_dir_nofollow, open_regular_below,
    open_regular_nofollow, optional_entry_at, optional_entry_metadata, read_opened_bounded,
};
pub(crate) use scrub::{
    LEXICAL_QUARANTINE_RECEIPT_FILE_NAME, LEXICAL_SCRUB_RECEIPT_FILE_NAME, last_completed_scrub,
    quarantine_seal_failure_at, quarantined_by_scrub, quarantined_by_scrub_at,
    refuse_if_quarantined_at, scrub_step, seal_failure_quarantine_reason,
};
pub(crate) use seal::seal_generation;
pub(crate) use verify::{
    DiscardingVisitor, SealedGenerationVisitor, walk_sealed_generation, walk_sealed_generation_at,
    walk_sealed_generation_reusing_coverage,
};
