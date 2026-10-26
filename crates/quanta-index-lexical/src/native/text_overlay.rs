//! Bounded, request-local text contributions for a selected overlay snapshot.
//! The durable member remains immutable; no source or index object is published.

use super::text::{
    MAX_POSITIONS_PER_TERM, NativeTextIndex, PreparedPositions, Root, TextReadContext, TextRecipe,
    TextStatistics, selected_overlay_prior, validate_overlay_term_membership,
    visit_indexed_token_batches,
};
use super::text_memory::{PreparedKey, PreparedRows};
use quanta_index_core::index::{
    Digest, IndexError, MemberBinding, OwnerKey, ViewRef, WorkBudget, WorkUsage,
};
use sha2::{Digest as _, Sha256};

pub(super) const MAX_TEXT_OVERLAY_DOCUMENTS: usize = 256;
pub(super) const MAX_TEXT_OVERLAY_DOCUMENT_BYTES: usize = 1024 * 1024;
pub(super) const MAX_TEXT_OVERLAY_TOTAL_BYTES: usize = 16 * 1024 * 1024;
// The owner tokenizer permits up to 16 MiB of NFC output for one source.
const MAX_TEXT_OVERLAY_TOKEN_ROWS: usize = 2 * 16 * 1024 * 1024;

/// Sorted, unique owners captured from one atomic overlay snapshot. `None`
/// deletes or masks an owner; `Some("")` is an admitted empty document.
#[derive(Clone, Copy)]
pub struct TextOverlayInput<'a> {
    pub owner: OwnerKey,
    pub revision: u64,
    pub text: Option<&'a str>,
}

struct OccurrenceRow {
    key: PreparedKey,
    position: u32,
}

struct OverlayTerm {
    key: PreparedKey,
    positions: PreparedPositions,
}

struct OverlayOwner {
    owner: OwnerKey,
    revision: u64,
    old_inventory: Option<super::text::EntryMap>,
    new_indexed_tokens: Option<u64>,
    body_digest: Option<Digest>,
    terms: PreparedRows<OverlayTerm>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextOverlayDocument {
    pub owner: OwnerKey,
    pub revision: u64,
    pub indexed_tokens: u64,
}

#[derive(Clone, Copy)]
pub struct TextOverlayTermHit<'a> {
    pub document: TextOverlayDocument,
    pub positions: &'a [u32],
}

impl TextOverlayTermHit<'_> {
    #[must_use]
    pub fn tf(self) -> usize {
        self.positions.len()
    }
}

/// Owns every ephemeral posting allocation and grant for one request stage.
pub struct TextOverlayCorpus {
    owners: PreparedRows<OverlayOwner>,
    statistics: TextStatistics,
    view: ViewRef,
    member: MemberBinding,
    root: Root,
    recipe: TextRecipe,
}

impl TextOverlayCorpus {
    pub fn validate_binding(
        &self,
        view: ViewRef,
        member: &MemberBinding,
    ) -> Result<(), IndexError> {
        if self.view != view || &self.member != member {
            return Err(IndexError::Conflict);
        }
        Ok(())
    }

    #[must_use]
    pub const fn statistics(&self) -> TextStatistics {
        self.statistics
    }

    #[must_use]
    pub fn masks(&self, owner: OwnerKey) -> bool {
        self.find_owner(owner).is_some()
    }

    #[must_use]
    pub fn document(&self, owner: OwnerKey) -> Option<TextOverlayDocument> {
        let entry = self.find_owner(owner)?;
        Some(TextOverlayDocument {
            owner,
            revision: entry.revision,
            indexed_tokens: entry.new_indexed_tokens?,
        })
    }

    pub fn validates_body(
        &self,
        owner: OwnerKey,
        revision: u64,
        body: &str,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        let Some(entry) = self.find_owner(owner) else {
            return Ok(false);
        };
        if entry.revision != revision {
            return Ok(false);
        }
        let Some(expected) = entry.body_digest else {
            return Ok(false);
        };
        budget.charge(WorkUsage {
            bytes: u64::try_from(body.len()).map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        Ok(Digest(Sha256::digest(body).into()) == expected)
    }

    pub fn documents(&self) -> impl Iterator<Item = TextOverlayDocument> + '_ {
        self.owners.as_slice().iter().filter_map(|entry| {
            entry
                .new_indexed_tokens
                .map(|indexed_tokens| TextOverlayDocument {
                    owner: entry.owner,
                    revision: entry.revision,
                    indexed_tokens,
                })
        })
    }

    #[must_use]
    pub fn term_hit(&self, owner: OwnerKey, key: &[u8]) -> Option<TextOverlayTermHit<'_>> {
        let entry = self.find_owner(owner)?;
        let indexed_tokens = entry.new_indexed_tokens?;
        let index = match entry
            .terms
            .as_slice()
            .binary_search_by(|term| term.key.as_slice().cmp(key))
        {
            Ok(index) => index,
            Err(_insertion_point) => return None,
        };
        let term = entry.terms.as_slice().get(index)?;
        Some(TextOverlayTermHit {
            document: TextOverlayDocument {
                owner,
                revision: entry.revision,
                indexed_tokens,
            },
            positions: term.positions.as_slice(),
        })
    }

    /// Adjust a validated durable term DF using only the selected owners'
    /// admitted contribution inventories. Call after complete durable traversal.
    pub fn adjusted_df(
        &self,
        member: &MemberBinding,
        key: &[u8],
        durable_df: u64,
        context: &mut TextReadContext<'_>,
    ) -> Result<u64, IndexError> {
        self.validate_binding(context.ticket.view.identity, member)?;
        let mut df = durable_df;
        for owner in self.owners.as_slice() {
            context.budget.checkpoint()?;
            if let Some(inventory) = owner.old_inventory {
                let inventory_ref = inventory.lookup(context.maps, key, context.budget)?;
                if validate_overlay_term_membership(
                    self.root,
                    self.recipe,
                    owner.owner,
                    key,
                    inventory_ref,
                    context,
                )? {
                    df = df.checked_sub(1).ok_or_else(|| {
                        IndexError::CorruptObject(
                            "overlay removal exceeds durable term frequency".into(),
                        )
                    })?;
                }
            }
            if owner.new_indexed_tokens.is_some()
                && owner
                    .terms
                    .as_slice()
                    .binary_search_by(|term| term.key.as_slice().cmp(key))
                    .is_ok()
            {
                df = df.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
        }
        if df > self.statistics.docs {
            return Err(IndexError::CorruptObject(
                "overlay term frequency exceeds virtual corpus".into(),
            ));
        }
        Ok(df)
    }

    fn find_owner(&self, owner: OwnerKey) -> Option<&OverlayOwner> {
        let key = owner_bytes(owner);
        let index = match self
            .owners
            .as_slice()
            .binary_search_by(|entry| owner_bytes(entry.owner).cmp(&key))
        {
            Ok(index) => index,
            Err(_insertion_point) => return None,
        };
        self.owners.as_slice().get(index)
    }
}

impl NativeTextIndex {
    /// Build ephemeral postings from the same normalized token batches used by
    /// durable owner construction. The selected member and source are checked
    /// before any old contribution is subtracted.
    pub fn prepare_overlay_corpus(
        &self,
        member: &MemberBinding,
        inputs: &[TextOverlayInput<'_>],
        context: &mut TextReadContext<'_>,
    ) -> Result<TextOverlayCorpus, IndexError> {
        context.budget.checkpoint()?;
        if inputs.len() > MAX_TEXT_OVERLAY_DOCUMENTS {
            return Err(IndexError::ResourceLimited);
        }
        let mut total_bytes = 0usize;
        let mut previous = None;
        for input in inputs {
            let key = owner_bytes(input.owner);
            if previous.is_some_and(|prior| prior >= key) {
                return Err(IndexError::InvalidInput(
                    "overlay owners must be sorted and unique".into(),
                ));
            }
            previous = Some(key);
            if let Some(body) = input.text {
                if body.len() > MAX_TEXT_OVERLAY_DOCUMENT_BYTES {
                    return Err(IndexError::ResourceLimited);
                }
                total_bytes = total_bytes
                    .checked_add(body.len())
                    .filter(|total| *total <= MAX_TEXT_OVERLAY_TOTAL_BYTES)
                    .ok_or(IndexError::ResourceLimited)?;
            }
        }
        let root = self.pinned_root(member, context)?;
        let (_, docs, indexed_tokens) = root.owner_universe();
        let mut statistics = TextStatistics {
            docs,
            indexed_tokens,
        };
        let mut owners = PreparedRows::new(MAX_TEXT_OVERLAY_DOCUMENTS)?;
        for input in inputs {
            context.budget.checkpoint()?;
            let prior = selected_overlay_prior(root, self.recipe(), input.owner, context)?;
            let (old_inventory, old_indexed_tokens) =
                prior.map_or((None, 0), |(inventory, tokens)| (Some(inventory), tokens));
            if prior.is_some() {
                statistics.docs = statistics.docs.checked_sub(1).ok_or_else(|| {
                    IndexError::CorruptObject("overlay removal exceeds durable documents".into())
                })?;
                statistics.indexed_tokens = statistics
                    .indexed_tokens
                    .checked_sub(old_indexed_tokens)
                    .ok_or_else(|| {
                        IndexError::CorruptObject("overlay removal exceeds durable tokens".into())
                    })?;
            }
            let (new_indexed_tokens, body_digest, terms) = if let Some(body) = input.text {
                context.budget.charge(WorkUsage {
                    bytes: u64::try_from(body.len())
                        .map_err(|_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                let digest = Digest(Sha256::digest(body).into());
                let (tokens, terms) = prepare_terms(body, context.budget)?;
                statistics.docs = statistics
                    .docs
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                statistics.indexed_tokens = statistics
                    .indexed_tokens
                    .checked_add(tokens)
                    .ok_or(IndexError::ResourceLimited)?;
                (Some(tokens), Some(digest), terms)
            } else {
                (None, None, PreparedRows::new(MAX_TEXT_OVERLAY_TOKEN_ROWS)?)
            };
            owners.push(
                OverlayOwner {
                    owner: input.owner,
                    revision: input.revision,
                    old_inventory,
                    new_indexed_tokens,
                    body_digest,
                    terms,
                },
                context.budget,
            )?;
        }
        Ok(TextOverlayCorpus {
            owners,
            statistics,
            view: context.ticket.view.identity,
            member: member.clone(),
            root,
            recipe: self.recipe(),
        })
    }
}

fn prepare_terms(
    body: &str,
    budget: &mut WorkBudget,
) -> Result<(u64, PreparedRows<OverlayTerm>), IndexError> {
    let mut occurrences = PreparedRows::new(MAX_TEXT_OVERLAY_TOKEN_ROWS)?;
    let indexed_tokens = visit_indexed_token_batches(body, budget, |batch, budget| {
        for row in batch.rows() {
            let key = PreparedKey::copy(batch.term(row)?, budget)?;
            occurrences.push(
                OccurrenceRow {
                    key,
                    position: row.position(),
                },
                budget,
            )?;
        }
        Ok(())
    })?;
    let levels = usize::try_from(
        usize::BITS
            .checked_sub(occurrences.len().max(1).leading_zeros())
            .ok_or(IndexError::ResourceLimited)?,
    )
    .map_err(|_error| IndexError::ResourceLimited)?;
    let comparisons = occurrences
        .len()
        .checked_mul(levels)
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        mutations: u64::try_from(comparisons).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    occurrences.as_mut_slice().sort_unstable_by(|a, b| {
        a.key
            .as_slice()
            .cmp(b.key.as_slice())
            .then(a.position.cmp(&b.position))
    });
    let mut terms = PreparedRows::new(MAX_TEXT_OVERLAY_TOKEN_ROWS)?;
    for occurrence in occurrences.as_slice() {
        budget.checkpoint()?;
        let same_term = terms
            .as_slice()
            .last()
            .is_some_and(|term: &OverlayTerm| term.key.as_slice() == occurrence.key.as_slice());
        if same_term {
            let term = terms.as_mut_slice().last_mut().ok_or_else(|| {
                IndexError::BackendViolation("overlay term row disappeared".into())
            })?;
            term.positions.push(occurrence.position, budget)?;
        } else {
            let key = PreparedKey::copy(occurrence.key.as_slice(), budget)?;
            let mut positions = PreparedPositions::new(
                usize::try_from(MAX_POSITIONS_PER_TERM)
                    .map_err(|_error| IndexError::ResourceLimited)?,
            )?;
            positions.push(occurrence.position, budget)?;
            terms.push(OverlayTerm { key, positions }, budget)?;
        }
    }
    Ok((indexed_tokens, terms))
}

fn owner_bytes(owner: OwnerKey) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    let (producer, stable_id) = bytes.split_at_mut(16);
    producer.copy_from_slice(&owner.producer.0);
    stable_id.copy_from_slice(&owner.stable_id.0);
    bytes
}
