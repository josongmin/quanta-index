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
    MAX_DOC_ID, MAX_MANIFEST_BYTES, TEXT_AUTHORITY_DIR_NAME, TEXT_AUTHORITY_MANIFEST_FILE_NAME,
    TextAuthorityManifest, leading_format_version, read_manifest, shard_index_of,
    text_authority_dir,
};
#[cfg(test)]
pub(crate) use reader::load_shard;
pub(crate) use reader::{
    ProvedTextShard, ShardedTextAuthority, TextDocIdentity, document_identities, load_shard_file_at,
};
pub(crate) use shard::{ShardBody, sha256_of_bytes};
pub(crate) use writer::{
    AddedTextDoc, TextAuthorityWriteReceipt, TextAuthorityWriteResult, finalize_for_seal, rebuild,
    update,
};
