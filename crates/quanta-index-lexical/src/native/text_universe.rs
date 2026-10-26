//! A finite, ticket-bound admitted-owner universe for unanchored Boolean text.

use super::text::{
    Root, TextReadContext, TextRecipe, ValidatedUniverseOwner, validate_universe_owner,
};
use quanta_index_core::index::{IndexError, MapPageCursor, MapPageEntry, MapRoot};

const OWNER_PAGE: u32 = 64;
pub(super) const MAX_UNANCHORED_DOCS: u64 = 4_096;

/// The source member's owner map, not the broader source map, defines the
/// universe. It includes admitted empty text and excludes unadmitted sources.
pub(super) struct TextOwnerCursor {
    root: Root,
    recipe: TextRecipe,
    owners: MapRoot,
    declared: u64,
    visited: u64,
    declared_tokens: u64,
    visited_tokens: u64,
    after: Option<MapPageEntry>,
    entries: MapPageCursor,
    more: bool,
}

impl TextOwnerCursor {
    pub(super) fn new(root: Root, recipe: TextRecipe) -> Result<Self, IndexError> {
        let (owners, declared, declared_tokens) = root.owner_universe();
        if declared > MAX_UNANCHORED_DOCS {
            return Err(IndexError::ResourceLimited);
        }
        Ok(Self {
            root,
            recipe,
            owners,
            declared,
            visited: 0,
            declared_tokens,
            visited_tokens: 0,
            after: None,
            entries: MapPageCursor::default(),
            more: true,
        })
    }

    pub(super) fn next(
        &mut self,
        context: &mut TextReadContext<'_>,
    ) -> Result<Option<ValidatedUniverseOwner>, IndexError> {
        loop {
            context.budget.checkpoint()?;
            if let Some(entry) = self.entries.next() {
                let (key, contribution) = entry.get()?;
                if key.len() != 32
                    || self
                        .after
                        .as_ref()
                        .map(MapPageEntry::key)
                        .transpose()?
                        .is_some_and(|prior| key.as_slice() <= prior)
                {
                    return Err(IndexError::CorruptObject(
                        "native text owner universe has invalid order or key".into(),
                    ));
                }
                self.visited = self
                    .visited
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                if self.visited > self.declared {
                    return Err(IndexError::CorruptObject(
                        "native text owner universe exceeds declared docs".into(),
                    ));
                }
                let owner =
                    validate_universe_owner(key, *contribution, self.root, self.recipe, context)?;
                self.visited_tokens = self
                    .visited_tokens
                    .checked_add(owner.doc_indexed_tokens)
                    .ok_or(IndexError::ResourceLimited)?;
                if self.visited_tokens > self.declared_tokens {
                    return Err(IndexError::CorruptObject(
                        "native text owner universe exceeds indexed token total".into(),
                    ));
                }
                self.after = Some(entry);
                return Ok(Some(owner));
            }
            if !self.more {
                if self.visited != self.declared || self.visited_tokens != self.declared_tokens {
                    return Err(IndexError::CorruptObject(
                        "native text owner universe differs from declared statistics".into(),
                    ));
                }
                return Ok(None);
            }
            let page = context.maps.scan(
                self.owners,
                self.after.as_ref().map(MapPageEntry::key).transpose()?,
                OWNER_PAGE,
                context.budget,
            )?;
            if page.entries.len()
                > usize::try_from(OWNER_PAGE).map_err(|_error| IndexError::ResourceLimited)?
                || (page.entries.is_empty() && page.has_more)
            {
                return Err(IndexError::BackendViolation(
                    "native text owner universe page exceeded its bound or did not advance".into(),
                ));
            }
            let mut prior = self.after.as_ref().map(MapPageEntry::key).transpose()?;
            for (key, _) in &page.entries {
                if key.len() != 32 || prior.is_some_and(|previous| key.as_slice() <= previous) {
                    return Err(IndexError::CorruptObject(
                        "native text owner universe page is not strictly ordered".into(),
                    ));
                }
                prior = Some(key);
            }
            self.more = page.has_more;
            self.entries = page.cursor();
        }
    }
}
