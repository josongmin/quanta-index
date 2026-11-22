//! Ordered, term-bounded path candidates. Layered folded terms merge the
//! explicit folded aliases with identical sensitive terms by owner.

use quanta_index_core::index::{
    IndexError, MapPageCursor, MapPageEntry, MapRoot, ObjectRef, OwnerKey, WorkBudget,
    ports::PersistentMapPort,
};
use quanta_index_lq_text_normalizer::CaseMode;

use super::{
    MAX_KEY_BYTES, PAGE, PathPostingValue, decode_owner, decode_posting_value, term_prefix,
    validate_page,
};
use crate::native::text_memory::decode_case_term_key;

type Candidate = (OwnerKey, ObjectRef);

struct RangeCursor {
    root: MapRoot,
    value: PathPostingValue,
    prefix: [u8; MAX_KEY_BYTES],
    prefix_len: usize,
    after: Option<MapPageEntry>,
    page: MapPageCursor,
    more_after_page: bool,
    complete: bool,
}

impl RangeCursor {
    fn new(
        root: MapRoot,
        term_key: &[u8],
        value: PathPostingValue,
        alternate_sensitive: bool,
    ) -> Result<Self, IndexError> {
        let mut prefix = [0u8; MAX_KEY_BYTES];
        let prefix_len = term_prefix(term_key, &mut prefix)?;
        if alternate_sensitive {
            // The length includes the case tag; only that tag changes.
            *prefix.get_mut(2).ok_or(IndexError::ResourceLimited)? = 0;
        }
        Ok(Self {
            root,
            value,
            prefix,
            prefix_len,
            after: None,
            page: MapPageCursor::default(),
            more_after_page: true,
            complete: false,
        })
    }

    fn next(
        &mut self,
        maps: &dyn PersistentMapPort,
        budget: &mut WorkBudget,
    ) -> Result<Option<Candidate>, IndexError> {
        loop {
            budget.checkpoint()?;
            if self.complete {
                return Ok(None);
            }
            if let Some(entry) = self.page.next() {
                let (key, value) = entry.get()?;
                let prefix = self
                    .prefix
                    .get(..self.prefix_len)
                    .ok_or(IndexError::ResourceLimited)?;
                if !key.starts_with(prefix) {
                    self.complete = true;
                    return Ok(None);
                }
                if key.len()
                    != self
                        .prefix_len
                        .checked_add(32)
                        .ok_or(IndexError::ResourceLimited)?
                {
                    return Err(IndexError::CorruptObject(
                        "native text path posting key width".into(),
                    ));
                }
                let owner = decode_owner(
                    key.get(self.prefix_len..)
                        .ok_or(IndexError::ResourceLimited)?,
                )?;
                let source = decode_posting_value(self.value, value)?;
                self.after = Some(entry);
                return Ok(Some((owner, source)));
            }
            if self.after.is_some() && !self.more_after_page {
                self.complete = true;
                return Ok(None);
            }
            let prefix = self
                .prefix
                .get(..self.prefix_len)
                .ok_or(IndexError::ResourceLimited)?;
            let after = self
                .after
                .as_ref()
                .map(MapPageEntry::key)
                .transpose()?
                .unwrap_or(prefix);
            let page = maps.scan(self.root, Some(after), PAGE, budget)?;
            if page.entries.is_empty() {
                if page.has_more {
                    return Err(IndexError::BackendViolation(
                        "native text path scan returned empty continuation".into(),
                    ));
                }
                self.complete = true;
                return Ok(None);
            }
            validate_page(&page, after)?;
            self.more_after_page = page.has_more;
            self.page = page.cursor();
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Selection {
    Primary,
    Secondary,
    Both,
    End,
}

fn select(
    primary: Option<Candidate>,
    secondary: Option<Candidate>,
) -> Result<Selection, IndexError> {
    Ok(match (primary, secondary) {
        (Some((left, _)), Some((right, _))) if left < right => Selection::Primary,
        (Some((left, _)), Some((right, _))) if left > right => Selection::Secondary,
        (Some((_, left)), Some((_, right))) => {
            if left != right {
                return Err(IndexError::CorruptObject(
                    "native layered path aliases disagree on pinned source".into(),
                ));
            }
            Selection::Both
        }
        (Some(_), None) => Selection::Primary,
        (None, Some(_)) => Selection::Secondary,
        (None, None) => Selection::End,
    })
}

/// One or two ordered ranges yielding each owner once. Both ranges are bounded
/// to the same normalized term bytes; the second differs only in its case tag.
pub(in crate::native) struct PathTermCursor {
    primary: RangeCursor,
    secondary: Option<RangeCursor>,
    primary_head: Option<Candidate>,
    secondary_head: Option<Candidate>,
    last_owner: Option<OwnerKey>,
}

impl PathTermCursor {
    pub(in crate::native) fn new(
        root: MapRoot,
        term_key: &[u8],
        value: PathPostingValue,
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        let (case, _text) = decode_case_term_key(term_key)?;
        let merge = value == PathPostingValue::SourceCutDigest && case == CaseMode::Folded;
        Ok(Self {
            primary: RangeCursor::new(root, term_key, value, false)?,
            secondary: merge
                .then(|| RangeCursor::new(root, term_key, value, true))
                .transpose()?,
            primary_head: None,
            secondary_head: None,
            last_owner: None,
        })
    }

    pub(in crate::native) fn next(
        &mut self,
        maps: &dyn PersistentMapPort,
        budget: &mut WorkBudget,
    ) -> Result<Option<(OwnerKey, ObjectRef)>, IndexError> {
        budget.checkpoint()?;
        if self.primary_head.is_none() {
            self.primary_head = self.primary.next(maps, budget)?;
        }
        if let Some(secondary) = self.secondary.as_mut()
            && self.secondary_head.is_none()
        {
            self.secondary_head = secondary.next(maps, budget)?;
        }
        let selected = match select(self.primary_head, self.secondary_head)? {
            Selection::Primary => self.primary_head.take(),
            Selection::Secondary => self.secondary_head.take(),
            Selection::Both => {
                self.secondary_head = None;
                self.primary_head.take()
            }
            Selection::End => None,
        };
        if let Some((owner, _source)) = selected {
            if self.last_owner.is_some_and(|last| owner <= last) {
                return Err(IndexError::CorruptObject(
                    "native text path owners are not strictly ordered".into(),
                ));
            }
            self.last_owner = Some(owner);
        }
        Ok(selected)
    }

    /// Replay from the root so the candidate cursor remains untouched.
    pub(in crate::native) fn df(
        &self,
        maps: &dyn PersistentMapPort,
        budget: &mut WorkBudget,
    ) -> Result<u64, IndexError> {
        let term_key = self
            .primary
            .prefix
            .get(2..self.primary.prefix_len)
            .ok_or(IndexError::ResourceLimited)?;
        let mut replay = Self::new(self.primary.root, term_key, self.primary.value, budget)?;
        let mut count = 0u64;
        while replay.next(maps, budget)?.is_some() {
            count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::index::{Digest, ProducerId, StableOwnerId};

    fn candidate(stable: u8, digest: u8) -> Candidate {
        (
            OwnerKey {
                producer: ProducerId([0; 16]),
                stable_id: StableOwnerId([stable; 16]),
            },
            ObjectRef(Digest([digest; 32])),
        )
    }

    #[test]
    fn folded_aliases_select_unique_owners_in_order() -> Result<(), IndexError> {
        assert_eq!(
            select(Some(candidate(1, 7)), Some(candidate(2, 8)))?,
            Selection::Primary
        );
        assert_eq!(
            select(Some(candidate(3, 7)), Some(candidate(2, 8)))?,
            Selection::Secondary
        );
        assert_eq!(
            select(Some(candidate(2, 7)), Some(candidate(2, 7)))?,
            Selection::Both
        );
        assert_eq!(select(None, None)?, Selection::End);
        Ok(())
    }

    #[test]
    fn folded_aliases_reject_different_sources_for_one_owner() {
        assert!(select(Some(candidate(2, 7)), Some(candidate(2, 8))).is_err());
    }
}
