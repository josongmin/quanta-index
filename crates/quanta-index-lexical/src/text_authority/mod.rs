//! The sharded text authority: manifest, shards, reader, writer.
//!
//! See [`manifest`] for the layout and its versioning, [`shard`] for the
//! shard file, [`reader`] for the query-time union and [`writer`] for the
//! O(delta) publish.

mod manifest;
mod reader;
mod shard;
mod writer;

pub(crate) use manifest::{
    MAX_DOC_ID, TEXT_AUTHORITY_DIR_NAME, TEXT_AUTHORITY_FORMAT_UNSUPPORTED_CODE,
    TEXT_AUTHORITY_MANIFEST_FILE_NAME, TextAuthorityManifest, leading_format_version,
    read_manifest, shard_index_of, text_authority_dir,
};
pub(crate) use reader::{ShardedTextAuthority, load_shard};
pub(crate) use shard::{ShardBody, sha256_of_bytes};
pub(crate) use writer::{
    AddedTextDoc, TextAuthorityWriteReceipt, finalize_for_seal, rebuild, update,
};
