//! Plan one complete Native source-bundle mutation before Tantivy is changed.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is required across crate modules"
)]

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_contract::SearchScopeSurface;
use quanta_index_contract::SourceFileKey;
use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_core::CoreError;

use crate::channel_payloads::{decode_replace_scope_payload, decode_tombstone_scope_payload};
use crate::lexical_allocation::DocVersionReservation;
use crate::lexical_store::ObjectStore;
use crate::text_authority::{AddedTextDoc, NativeRoot, NativeSourceEdit};
use crate::{TextAuthorityPlan, TextAuthorityWrite, TextDocAllocator};

impl TextDocAllocator {
    pub(crate) fn from_reservation(reservation: &DocVersionReservation) -> Result<Self, CoreError> {
        reservation.validate()?;
        Ok(Self {
            next_doc_id: reservation.first,
            remaining: reservation.count,
            added: Vec::new(),
        })
    }

    /// Consume one ID from the event-bound range; never infer a watermark.
    pub(crate) fn allocate(
        &mut self,
        candidate_id: &str,
        text: &str,
        source_file: &SourceFileKey,
    ) -> Result<u64, CoreError> {
        if self.remaining == 0 {
            return Err(CoreError::InvalidContract(
                "lexical: Native text ID reservation exhausted".into(),
            ));
        }
        let id = self.next_doc_id;
        self.remaining = self.remaining.saturating_sub(1);
        if self.remaining != 0 {
            self.next_doc_id = self.next_doc_id.checked_add(1).ok_or_else(|| {
                CoreError::InvalidContract("lexical: Native text ID reservation overflows".into())
            })?;
        }
        self.added.try_reserve(1).map_err(|error| {
            CoreError::Storage(format!("lexical: reserve Native added text doc: {error}"))
        })?;
        self.added.push(AddedTextDoc {
            doc_id: id,
            source_file: source_file.clone(),
            candidate_id: candidate_id.to_string(),
            text: text.to_string(),
        });
        Ok(id)
    }

    pub(crate) fn validate_fully_consumed(&self) -> Result<(), CoreError> {
        if self.remaining != 0 {
            return Err(CoreError::InvalidContract(format!(
                "lexical: Native text ID reservation has {} unused IDs",
                self.remaining
            )));
        }
        Ok(())
    }
}

pub(crate) fn plan_text_authority_delta(
    store: &ObjectStore,
    prior: Option<&NativeRoot>,
    ops: &[LexicalChannelOp],
    reservation: &DocVersionReservation,
) -> Result<TextAuthorityPlan, CoreError> {
    reservation.validate()?;
    if prior.is_some_and(|root| root.allocation_scope_sha256 != reservation.scope_sha256) {
        return Err(CoreError::InvalidContract(
            "lexical: Native prior root allocation scope differs".into(),
        ));
    }
    let allocator = TextDocAllocator::from_reservation(reservation)?;
    let mut source_edits = BTreeMap::<SourceFileKey, NativeSourceEdit>::new();
    let mut added_count = 0_u64;
    let mut clear_all = false;
    let mut touches_text = false;
    for op in ops {
        match op {
            LexicalChannelOp::ReplaceLexicalScope(payload) => {
                let (_mode, _base, scope) = decode_replace_scope_payload(&payload.payload)?;
                touches_text = true;
                added_count = added_count
                    .checked_add(u64::try_from(scope.chunks.len()).map_err(|error| {
                        CoreError::InvalidContract(format!(
                            "lexical: Native scope chunk count width: {error}"
                        ))
                    })?)
                    .ok_or_else(|| {
                        CoreError::InvalidContract("lexical: Native chunk count overflow".into())
                    })?;
                let source = scope.coverage.source.file.clone();
                if source_edits
                    .insert(source, NativeSourceEdit::Upsert(scope.coverage))
                    .is_some()
                {
                    return Err(CoreError::InvalidContract(
                        "lexical: duplicate Native source edit".into(),
                    ));
                }
            }
            LexicalChannelOp::TombstoneLexicalScope(payload) => {
                let (_mode, _base, scope) = decode_tombstone_scope_payload(&payload.payload)?;
                touches_text = true;
                let source = scope.file.clone();
                if source_edits
                    .insert(source.clone(), NativeSourceEdit::Delete(source))
                    .is_some()
                {
                    return Err(CoreError::InvalidContract(
                        "lexical: duplicate Native source edit".into(),
                    ));
                }
            }
            LexicalChannelOp::ClearLexicalSurface(payload)
                if payload.surface == SearchScopeSurface::Chunk =>
            {
                touches_text = true;
                clear_all = true;
            }
            LexicalChannelOp::UpsertChunk(_) => {
                return Err(CoreError::InvalidContract(
                    "lexical: unscoped chunk mutation cannot establish Native source identity"
                        .into(),
                ));
            }
            LexicalChannelOp::ClearLexicalSurface(_)
            | LexicalChannelOp::FullBundle(_)
            | LexicalChannelOp::UpsertSymbol(_)
            | LexicalChannelOp::Seal(_)
            | LexicalChannelOp::UpsertCommit(_)
            | LexicalChannelOp::UpsertRef(_)
            | LexicalChannelOp::UpsertParseTree(_)
            | LexicalChannelOp::ReplaceStructuralScope(_) => {}
        }
    }
    if clear_all && !source_edits.is_empty() {
        return Err(CoreError::InvalidContract(
            "lexical: Native clear and source edits must be separate events".into(),
        ));
    }
    if reservation.count != added_count {
        return Err(CoreError::InvalidContract(format!(
            "lexical: Native reservation count {} differs from {} added chunks",
            reservation.count, added_count
        )));
    }
    let write = if touches_text {
        let mut retired = BTreeSet::new();
        if !clear_all && let Some(prior) = prior {
            for key in source_edits.keys() {
                for id in crate::text_authority::source_ids(store, prior, key)? {
                    if !retired.insert(id) {
                        return Err(CoreError::InvalidContract(
                            "lexical: Native source memberships overlap".into(),
                        ));
                    }
                }
            }
        }
        TextAuthorityWrite::Delta {
            retired: retired.into_iter().collect(),
            source_edits: source_edits.into_values().collect(),
            clear_all,
        }
    } else {
        TextAuthorityWrite::None
    };
    // Added documents are filled by `allocate` while the ops are applied.
    Ok(TextAuthorityPlan {
        write,
        allocator,
        prior: prior.cloned(),
    })
}
