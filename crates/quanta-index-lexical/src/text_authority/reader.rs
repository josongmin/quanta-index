//! The sharded text authority as a query reads it.
//!
//! [`ShardedTextAuthority`] loads the manifest and every listed shard —
//! plain reads, each shard proved against its committed length and digest
//! before it is decoded — and answers the byte surfaces (raw substring,
//! regex prefilter and verify) and the token surfaces (phrase, adjacency)
//! by handing the `lq-trigram` and `lq-positions` algorithms a union view
//! over the shards. The algorithms are the single-index ones, unchanged;
//! the union is exact because shards partition the doc-id space in
//! ascending ranges, and doc ids are global so nothing is remapped.

use std::path::Path;

use quanta_index_core::CoreError;
use quanta_index_lq_positions::ShardedPositionsIndex;
use quanta_index_lq_trigram::{DocId as TrigramDocId, DocResolver, ShardedTrigramIndex};

use crate::text_authority::manifest::{ShardEntry, TEXT_AUTHORITY_DIR_NAME, shard_index_of};
use crate::text_authority::shard::{ShardBody, TextAuthorityDoc, sha256_of_bytes};

/// One loaded shard, in manifest order.
struct LoadedShard {
    index: u64,
    body: ShardBody,
}

/// A generation's text authority, every shard resident.
pub(crate) struct ShardedTextAuthority {
    shards: Vec<LoadedShard>,
}

impl ShardedTextAuthority {
    /// Load the text authority under `generation_dir`, or `None` when the
    /// generation published none.
    ///
    /// Every listed shard is read, proved against the manifest's length and
    /// digest, decoded and checked against the manifest's row count and
    /// doc-id extremes; any disagreement is a typed refusal, never a
    /// partial authority.
    pub(crate) fn load(generation_dir: &Path) -> Result<Option<Self>, CoreError> {
        let Some(manifest) = super::manifest::read_manifest(generation_dir)? else {
            return Ok(None);
        };
        let mut shards = Vec::with_capacity(manifest.shards.len());
        for entry in &manifest.shards {
            shards.push(LoadedShard {
                index: entry.index,
                body: load_shard(generation_dir, entry)?,
            });
        }
        Ok(Some(Self { shards }))
    }

    /// The document with `doc_id`, if the authority holds it.
    pub(crate) fn doc(&self, doc_id: u64) -> Option<&TextAuthorityDoc> {
        let index = shard_index_of(doc_id);
        self.shards
            .binary_search_by_key(&index, |shard| shard.index)
            .map_or(None, |position| self.shards.get(position))
            .and_then(|shard| shard.body.docs_by_id.get(&doc_id))
    }

    /// Every doc id, ascending: the universe a verify-only regex walks when
    /// the prefilter is unusable.
    pub(crate) fn doc_ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.shards
            .iter()
            .flat_map(|shard| shard.body.docs_by_id.keys().copied())
    }

    /// The trigram union over every shard: the NFC copy, or the folded copy
    /// the `case:no` byte surfaces search.
    pub(crate) fn trigram_index(&self, folded: bool) -> ShardedTrigramIndex<'_> {
        ShardedTrigramIndex::new(
            self.shards
                .iter()
                .map(|shard| {
                    if folded {
                        &shard.body.trigram_folded
                    } else {
                        &shard.body.trigram
                    }
                })
                .collect(),
        )
    }

    /// The positions chain over every shard: the case-sensitive token
    /// stream, or the folded one.
    pub(crate) fn positions_index(&self, case_sensitive: bool) -> ShardedPositionsIndex<'_> {
        ShardedPositionsIndex::new(
            self.shards
                .iter()
                .map(|shard| {
                    if case_sensitive {
                        &shard.body.positions
                    } else {
                        &shard.body.positions_folded
                    }
                })
                .collect(),
        )
    }

    /// Doc id → document bytes for the verify passes.
    pub(crate) const fn resolver(&self, folded: bool) -> TextAuthorityResolver<'_> {
        TextAuthorityResolver {
            authority: self,
            folded,
        }
    }
}

/// Resolves a doc id to the bytes the byte surfaces verify against.
pub(crate) struct TextAuthorityResolver<'a> {
    authority: &'a ShardedTextAuthority,
    folded: bool,
}

impl DocResolver for TextAuthorityResolver<'_> {
    fn resolve(&self, doc_id: TrigramDocId) -> Option<&[u8]> {
        let doc = self.authority.doc(doc_id.0)?;
        if self.folded {
            Some(doc.folded_indexed_text.as_bytes())
        } else {
            Some(doc.indexed_text.as_bytes())
        }
    }
}

/// Read, prove and decode one listed shard.
///
/// Shared by the query reader (every shard) and the incremental writer
/// (the touched shards only), so both doors prove the same commitment.
pub(crate) fn load_shard(
    generation_dir: &Path,
    entry: &ShardEntry,
) -> Result<ShardBody, CoreError> {
    let path = entry.path(generation_dir);
    let name = format!("{TEXT_AUTHORITY_DIR_NAME}/{}", entry.file_name());
    let bytes = std::fs::read(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            crate::sidecar_corrupt(generation_dir, &name, "missing")
        } else {
            CoreError::Storage(format!(
                "lexical: read text authority shard {}: {error}",
                path.display()
            ))
        }
    })?;
    let length = u64::try_from(bytes.len()).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: text authority shard {} length: {err}",
            path.display()
        ))
    })?;
    if length != entry.bytes {
        return Err(crate::sidecar_corrupt(
            generation_dir,
            &name,
            &format!("{length} bytes on disk, {} committed", entry.bytes),
        ));
    }
    if sha256_of_bytes(&bytes) != entry.sha256 {
        return Err(crate::sidecar_corrupt(
            generation_dir,
            &name,
            "content digest differs from the committed digest",
        ));
    }
    let body = ShardBody::decode(&bytes, entry.index, generation_dir, &name)?;
    let rows = body.rows()?;
    if rows != entry.rows {
        return Err(crate::sidecar_corrupt(
            generation_dir,
            &name,
            &format!("{rows} rows decoded, {} committed", entry.rows),
        ));
    }
    if body.doc_id_extremes() != Some((entry.min_doc_id, entry.max_doc_id)) {
        return Err(crate::sidecar_corrupt(
            generation_dir,
            &name,
            &format!(
                "doc ids {:?} decoded, {}..={} committed",
                body.doc_id_extremes(),
                entry.min_doc_id,
                entry.max_doc_id
            ),
        ));
    }
    Ok(body)
}
