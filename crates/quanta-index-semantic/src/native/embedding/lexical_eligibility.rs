//! Request-local custody of the selected lexical eligibility set.

use quanta_index_contract::native_protocol::MAX_NATIVE_TEXT_TOP_K;
use quanta_index_core::index::{IndexError, MemoryLease, NativeTextHitSource, OwnerKey, ViewRef};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SemanticLexicalEligibleSource {
    pub owner: OwnerKey,
    pub source: NativeTextHitSource,
}

/// A request-local copy of the validated lexical hit universe, not a query plan.
/// Data is destroyed before its original retained allocation grant.
#[derive(Debug)]
pub struct SemanticLexicalEligibility {
    view: ViewRef,
    entries: Vec<SemanticLexicalEligibleSource>,
    entries_memory: Option<MemoryLease>,
}

impl SemanticLexicalEligibility {
    pub fn new(
        view: ViewRef,
        entries: Vec<SemanticLexicalEligibleSource>,
        entries_memory: Option<MemoryLease>,
    ) -> Result<Self, IndexError> {
        // Form the owner before any error: function parameters otherwise drop
        // in reverse order and could refund the grant before the Vec backing.
        let mut value = Self {
            view,
            entries,
            entries_memory,
        };
        let maximum =
            usize::try_from(MAX_NATIVE_TEXT_TOP_K).map_err(|_error| IndexError::ResourceLimited)?;
        if value.entries.capacity() > maximum {
            return Err(IndexError::ResourceLimited);
        }
        let bytes = value
            .entries
            .capacity()
            .checked_mul(std::mem::size_of::<SemanticLexicalEligibleSource>())
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?;
        // None is the existing explicit standalone allocation contract.
        if value
            .entries_memory
            .as_ref()
            .is_some_and(|memory| memory.bytes() < bytes)
        {
            return Err(IndexError::ResourceLimited);
        }
        value.entries.sort_unstable_by_key(|entry| entry.owner);
        if value
            .entries
            .windows(2)
            .any(|pair| matches!(pair, [left, right] if left.owner == right.owner))
        {
            return Err(IndexError::InvalidInput(
                "lexical eligibility repeats an owner".into(),
            ));
        }
        Ok(value)
    }

    pub(super) fn view(&self) -> ViewRef {
        self.view
    }

    pub(super) fn entries(&self) -> &[SemanticLexicalEligibleSource] {
        &self.entries
    }

    pub(super) fn allows_overlay(&self, owner: OwnerKey, revision: u64) -> bool {
        self.entries
            .binary_search_by_key(&owner, |entry| entry.owner)
            .is_ok_and(|index| {
                self.entries
                    .get(index)
                    .is_some_and(|entry| entry.source == NativeTextHitSource::Overlay { revision })
            })
    }

    pub(super) fn overlay_count(&self) -> Result<u32, IndexError> {
        u32::try_from(
            self.entries
                .iter()
                .filter(|entry| matches!(entry.source, NativeTextHitSource::Overlay { .. }))
                .count(),
        )
        .map_err(|_error| IndexError::ResourceLimited)
    }
}
