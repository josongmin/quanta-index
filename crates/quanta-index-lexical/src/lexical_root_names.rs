//! Shared lexical authority root names and control-file bound.

pub(crate) const LEXICAL_SEALED_MANIFEST_FORMAT_VERSION: u32 = 23;
pub(crate) const LEXICAL_UNPACKED_BODY_MANIFEST_FORMAT_VERSION: u32 = 22;
// Body packing changes generation reader capabilities, not adoption replay.
pub(crate) const LEXICAL_PUBLICATION_ATTEMPT_FORMAT_VERSION: u16 = 22;
pub(crate) const NATIVE_ROOT_NAME: &str = "text-authority/manifest.cbor";
pub(crate) const FILE_ROOT_NAME: &str = "file-authority/root.cbor";
pub(crate) const MAX_ROOT_BYTES: usize = 16 * 1024;
