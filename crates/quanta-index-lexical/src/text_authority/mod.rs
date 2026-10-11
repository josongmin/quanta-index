//! Paged Native text authority with durable logical IDs and selected reads.

mod paged_bootstrap;
mod paged_manifest;
mod paged_posting_page;
mod paged_reader;
mod paged_segment;
mod paged_transition;
mod paged_verify;
mod paged_wire;
mod paged_writer;
mod shard;
mod types;

#[cfg(test)]
mod paged_bootstrap_tests;

pub(crate) use paged_manifest::NativeRoot;
pub(crate) use paged_reader::NativeAuthority;
pub(crate) use paged_transition::verify_transition_for_seal;
pub(crate) use paged_verify::{
    NativeScrubCursor, verify_all_for_scrub, verify_changed_segments_for_seal,
};
pub(crate) use paged_wire::NativeSourceEdit;
#[cfg(test)]
pub(crate) use paged_writer::publish_delta;
pub(crate) use paged_writer::{
    PreparedNativeCompaction, prepare_compaction, publish_delta_prepared, source_ids,
};
pub(crate) use types::{AddedTextDoc, TEXT_AUTHORITY_DIR_NAME, TextAuthorityWriteReceipt};
