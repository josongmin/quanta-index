//! Shared lexical authority root names and control-file bound.

pub(crate) const LEXICAL_SEALED_MANIFEST_FORMAT_VERSION: u32 = 21;
pub(crate) const NATIVE_ROOT_NAME: &str = "text-authority/manifest.cbor";
pub(crate) const FILE_ROOT_NAME: &str = "file-authority/root.cbor";
pub(crate) const MAX_ROOT_BYTES: usize = 16 * 1024;
