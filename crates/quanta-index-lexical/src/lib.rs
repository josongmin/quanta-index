//! Native lexical queries and read-only access to persisted Legacy DATA.

#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

pub mod legacy;
pub mod native;

/// Global distinct path and content trigram posting memberships admitted by
/// the persisted F15 file-authority format.
pub const FILE_AUTHORITY_POSTING_MEMBERSHIP_LIMIT: u32 = 20_000_000;

/// Aggregate immutable coverage-page bytes admitted by the DATA reader.
pub const FILE_COVERAGE_ENCODED_BYTES_LIMIT: u64 = 64 * 1024 * 1024;

/// Conservative effective-coverage decode admission.
pub const FILE_COVERAGE_DECODE_HEAP_BYTES_LIMIT: u64 = 256 * 1024 * 1024;

pub mod filters;
pub mod phrase;
pub mod plan;
pub mod planner;
mod predicate_registry;
mod preview;
pub mod regex;
#[path = "sealed_generation/path_io.rs"]
mod sealed_path_io;
pub mod symbol;
pub mod trigram_plan;

mod boot_metadata;
mod causal_profile;
mod channel_payloads;
mod inventory;
mod lexical_root_names;
mod metadata_normalize;

/// The shared normalization contract for native text and query lowering.
pub(crate) use quanta_index_lq_text_normalizer as normalize;

pub use crate::inventory::{LexicalGenerationScanner, inventory_sealed_generations};

/// Maximum emitted lexical preview length in bytes.
const SNIPPET_WINDOW_BYTES: usize =
    quanta_index_contract::native_protocol::MAX_NATIVE_TEXT_PREVIEW_BYTES;

/// Leading context when centering a preview window on a hit.
const SNIPPET_LEAD_BYTES: usize = 120;
