//! Planning a batch's text-authority update: the documents it adds and retires.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::channel_payloads::{decode_replace_scope_payload, decode_tombstone_scope_payload};
use crate::text_authority::{AddedTextDoc, shard_index_of};
use crate::text_docs::{collect_text_authority_docs, text_candidates_at_file};
use crate::{
    SchemaFields, TextAuthorityPlan, TextAuthorityWrite, TextDocAllocator, TextOpSummary,
    text_authority,
};
use quanta_index_contract::SearchScopeSurface;
use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_core::CoreError;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use tantivy::Index;

impl TextDocAllocator {
    pub(crate) fn from_watermark(watermark: u64) -> Result<Self, CoreError> {
        Ok(Self {
            next_doc_id: watermark.checked_add(1).ok_or_else(|| {
                CoreError::InvalidContract("lexical: text authority doc id overflow".to_string())
            })?,
            added: Vec::new(),
        })
    }

    /// The id for one text document the batch writes.
    pub(crate) fn allocate(&mut self, candidate_id: &str, text: &str) -> Result<u64, CoreError> {
        let doc_id = self.next_doc_id;
        if doc_id > text_authority::MAX_DOC_ID {
            return Err(CoreError::InvalidContract(format!(
                "lexical: text authority doc id {doc_id} exceeds the encodable range"
            )));
        }
        self.next_doc_id = doc_id.checked_add(1).ok_or_else(|| {
            CoreError::InvalidContract("lexical: text authority doc id overflow".to_string())
        })?;
        self.added.push(AddedTextDoc {
            doc_id,
            candidate_id: candidate_id.to_string(),
            text: text.to_string(),
        });
        Ok(doc_id)
    }

    /// The watermark after every allocation so far.
    pub(crate) fn max_doc_id(&self) -> u64 {
        self.next_doc_id.saturating_sub(1)
    }
}

pub(crate) fn summarize_text_ops(ops: &[LexicalChannelOp]) -> Result<TextOpSummary, CoreError> {
    let mut summary = TextOpSummary {
        touches_text: false,
        forces_rebuild: false,
        retired_files: Vec::new(),
        added_count: 0,
    };
    for op in ops {
        match op {
            LexicalChannelOp::ReplaceLexicalScope(payload) => {
                summary.touches_text = true;
                let (_mode, _base, scope) = decode_replace_scope_payload(&payload.payload)?;
                summary
                    .retired_files
                    .push(scope.coverage.source.file.clone());
                let chunks = u64::try_from(scope.chunks.len()).map_err(|err| {
                    CoreError::InvalidContract(format!("lexical: scope chunk count: {err}"))
                })?;
                summary.added_count = summary.added_count.saturating_add(chunks);
            }
            LexicalChannelOp::TombstoneLexicalScope(payload) => {
                summary.touches_text = true;
                let (_mode, _base, scope) = decode_tombstone_scope_payload(&payload.payload)?;
                summary.retired_files.push(scope.file.clone());
            }
            LexicalChannelOp::ClearLexicalSurface(payload) => {
                // Only the chunk surface holds text documents; clearing
                // symbols leaves the text authority as it is.
                if payload.surface == SearchScopeSurface::Chunk {
                    summary.touches_text = true;
                    summary.forces_rebuild = true;
                }
            }
            LexicalChannelOp::UpsertChunk(_) => {
                summary.touches_text = true;
                summary.forces_rebuild = true;
                summary.added_count = summary.added_count.saturating_add(1);
            }
            LexicalChannelOp::FullBundle(_)
            | LexicalChannelOp::UpsertSymbol(_)
            | LexicalChannelOp::Seal(_)
            | LexicalChannelOp::UpsertCommit(_)
            | LexicalChannelOp::UpsertRef(_)
            | LexicalChannelOp::UpsertParseTree(_)
            | LexicalChannelOp::ReplaceStructuralScope(_) => {}
        }
    }
    Ok(summary)
}

/// The highest text-authority doc id the index stores, or 0 when it holds
/// no text document: the watermark when the authority cannot supply one.
///
/// A full scan, taken only on the rebuild paths that scan anyway.
pub(crate) fn stored_doc_id_watermark(
    index: &Index,
    fields: &SchemaFields,
) -> Result<u64, CoreError> {
    Ok(collect_text_authority_docs(index, fields)?
        .iter()
        .map(|doc| doc.doc_id)
        .max()
        .unwrap_or(0))
}

/// Classify a batch before it is applied and fix the doc ids it will hand
/// out, capturing the documents each scope mutation retires while the
/// pre-mutation index can still name them.
///
/// A generation without a prior authority, or whose index is ahead of it,
/// takes its watermark from the index itself (the highest stored doc id,
/// from a full scan), so a publish that crashed after its commit never
/// hands out an id twice.
pub(crate) fn plan_text_authority_delta(
    index: &Index,
    fields: &SchemaFields,
    ops: &[LexicalChannelOp],
    generation_dir: &Path,
) -> Result<TextAuthorityPlan, CoreError> {
    let summary = summarize_text_ops(ops)?;
    let prior = text_authority::read_manifest(generation_dir)?;
    if !summary.touches_text {
        let watermark = prior.as_ref().map_or(0, |manifest| manifest.max_doc_id);
        return Ok(TextAuthorityPlan {
            write: TextAuthorityWrite::None,
            allocator: TextDocAllocator::from_watermark(watermark)?,
            prior,
        });
    }
    let Some(manifest) = prior.as_ref() else {
        let watermark = stored_doc_id_watermark(index, fields)?;
        return Ok(TextAuthorityPlan {
            write: TextAuthorityWrite::Rebuild,
            allocator: TextDocAllocator::from_watermark(watermark)?,
            prior,
        });
    };
    let watermark = manifest.max_doc_id;
    let allocator = TextDocAllocator::from_watermark(watermark)?;
    if summary.forces_rebuild {
        return Ok(TextAuthorityPlan {
            write: TextAuthorityWrite::Rebuild,
            allocator,
            prior,
        });
    }
    let mut retired: BTreeMap<u64, String> = BTreeMap::new();
    for file in &summary.retired_files {
        for candidate in text_candidates_at_file(index, fields, file)? {
            if candidate.doc_id > watermark {
                // The index holds a document the prior authority never
                // listed: a publish crashed between its commit and its
                // sidecar write. Only a full derivation can catch up, and
                // new ids must continue past what the index already
                // stores, not past the stale manifest.
                let watermark = stored_doc_id_watermark(index, fields)?.max(watermark);
                return Ok(TextAuthorityPlan {
                    write: TextAuthorityWrite::Rebuild,
                    allocator: TextDocAllocator::from_watermark(watermark)?,
                    prior,
                });
            }
            let _prior = retired.insert(candidate.doc_id, candidate.candidate_id);
        }
    }
    let mut touched_shards: BTreeSet<u64> = retired
        .keys()
        .map(|doc_id| shard_index_of(*doc_id))
        .collect();
    let first_new = allocator.next_doc_id;
    let last_new = watermark.checked_add(summary.added_count).ok_or_else(|| {
        CoreError::InvalidContract("lexical: text authority doc id overflow".to_string())
    })?;
    if summary.added_count > 0 {
        for shard in shard_index_of(first_new)..=shard_index_of(last_new) {
            let _inserted = touched_shards.insert(shard);
        }
    }
    Ok(TextAuthorityPlan {
        write: TextAuthorityWrite::Incremental {
            retired,
            touched_shards,
        },
        allocator,
        prior,
    })
}
