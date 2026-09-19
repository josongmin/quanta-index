//! The sealed lexical generation: what it commits to and how it is proved.
//!
//! See [`manifest`] for the commitment and its format, [`overlay`] for the
//! repo-metadata sidecars it lists, [`seal`] for the measurement that
//! writes it, [`verify`] for the walk both doors share, and [`scrub`] for
//! the deep re-measurement between seals.

mod index_files;
mod manifest;
mod overlay;
mod scrub;
mod seal;
mod verify;

pub use seal::LexicalSealCommitmentStats;

pub(crate) use manifest::{
    GENERATION_MANIFEST_FORMAT_UNSUPPORTED_CODE, LEXICAL_SEALED_MANIFEST_FILE_NAME, manifest_path,
    read_manifest,
};
pub(crate) use overlay::{persist_overlay, remove_overlay};
pub(crate) use scrub::{
    LEXICAL_QUARANTINE_RECEIPT_FILE_NAME, LEXICAL_SCRUB_RECEIPT_FILE_NAME, last_completed_scrub,
    quarantine_content_corrupt, quarantined_by_scrub, refuse_if_quarantined, scrub_step,
};
pub(crate) use seal::seal_generation;
pub(crate) use verify::{DiscardingVisitor, SealedGenerationVisitor, walk_sealed_generation};
