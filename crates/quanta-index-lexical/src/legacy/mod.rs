//! Existing persisted lexical DATA codecs, independent of serving engines.
//! A decoded row is not Source, physical, installation, or publication authority.
pub(crate) mod blob_codec;
pub(crate) mod map_codec;
pub(crate) mod object_binary;
pub(crate) mod object_codec;
pub(crate) mod source_head;

pub(crate) mod errors;
pub(crate) mod manifest;
pub(crate) mod names;
pub(crate) mod overlay_names;

pub(crate) mod file_policy;
pub(crate) mod file_root;
pub(crate) mod native_root;
pub(crate) mod run_registry;

pub(crate) mod coverage_root;

mod existing;
mod existing_source;
pub(crate) mod identity;
pub(crate) mod object_closure;
pub use existing::verify_existing_lexical_generation_read_only;
pub use existing_source::{SourceVisitFailureV1, visit_existing_complete_source_files_v1};
