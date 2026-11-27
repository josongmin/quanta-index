//! Literals that every reportable match contains, and their anchor on the
//! pinned Literal sibling.
//!
//! A `RequiredLiterals` value is a finite set of members with one guarantee:
//! every non-empty match the query can report contains, as consecutive raw
//! scalars, one expansion of at least one member under the value's variant
//! source (`ExactScalars` or `SimpleCaseFold`). The anchor of the set is the
//! union of its members' window-rule anchors (`anchor_chars`). Each member
//! anchor is a superset of the bodies that contain it, so the union is a
//! superset of the bodies holding a match. One unanchorable member, or a
//! union over `MAX_UNANCHORED_DOCS` owners, leaves the whole set unanchored.
//!
//! The guarantee is about containment in the body. The match engine runs per
//! line, so every match it reports is also a match within the whole body,
//! and a body anchor stays a superset of the bodies with a per-line match.
//!
//! Regex members come from `regex_syntax`'s `Extractor` over the same HIR the
//! engine compiles. Its documented contract (`Extractor::kind`): every match
//! starts with a member of a finite prefix sequence, and ends with a member
//! of a finite suffix sequence. Hence:
//! - An infinite sequence states nothing and is not used.
//! - A finite sequence with an empty member constrains nothing and is not
//!   used.
//! - An empty finite sequence states that the regex never matches; its
//!   anchor is empty.
//! - A member cut at the extractor's length limit may end (prefix) or begin
//!   (suffix) inside a scalar. Every match then also starts (ends) with the
//!   member's longest whole-scalar prefix (suffix), which is used instead.
//! - Inexact members only lose the exact-match property, not containment.
//! - Look-around assertions extract as empty strings, so a reported match
//!   still contains its member.
//! - Under the folded policy each member character is replaced by a
//!   representative of its simple case folding class that folds back to it,
//!   so its `SimpleCaseFold` expansion still covers the original member.

use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, OwnerKey, WorkBudget, WorkUsage,
};
use regex_syntax::hir::Hir;
use regex_syntax::hir::literal::{ExtractKind, Extractor};

use super::literal::LiteralGrams;
use super::text::TextReadContext;
use super::text_literal_anchor::{CharVariants, ExactScalars, SimpleCaseFold, anchor_chars};
use super::text_universe::MAX_UNANCHORED_DOCS;

/// Members at least one of which every reportable match contains.
pub(super) struct RequiredLiterals {
    members: Vec<Vec<char>>,
    /// Members stand for their simple case folding expansions, not only
    /// for their own bytes.
    folded: bool,
    // The members die before the grant that admitted them.
    _memory: Option<MemoryLease>,
}

impl RequiredLiterals {
    /// A literal needle, matched byte-exactly or, when `folded`, under the
    /// regex engine's simple case folding of each of its characters.
    pub(super) fn literal(needle: &str, folded: bool) -> Self {
        Self {
            members: vec![needle.chars().collect()],
            folded,
            _memory: None,
        }
    }

    /// The independently sound required-literal sets of a regex: prefix
    /// members first, then suffix members. Either may be absent.
    ///
    /// `hir` must be the HIR the match engine compiles, and `folded` its
    /// case policy.
    pub(super) fn of_regex(
        hir: &Hir,
        folded: bool,
        budget: &mut WorkBudget,
    ) -> Result<Vec<Self>, IndexError> {
        let mut sets = Vec::new();
        for kind in [ExtractKind::Prefix, ExtractKind::Suffix] {
            budget.checkpoint()?;
            let prefix = kind.is_prefix();
            let mut extractor = Extractor::new();
            let _configured = extractor.kind(kind);
            let sequence = extractor.extract(hir);
            let Some(literals) = sequence.literals() else {
                continue;
            };
            let bytes = literals
                .iter()
                .try_fold(0_usize, |total, literal| total.checked_add(literal.len()))
                .and_then(|total| total.checked_mul(std::mem::size_of::<char>()))
                .ok_or(IndexError::ResourceLimited)?;
            let bytes = u64::try_from(bytes).map_err(|_width| IndexError::ResourceLimited)?;
            budget.charge(WorkUsage {
                bytes,
                ..WorkUsage::default()
            })?;
            let memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
            let mut members: Vec<Vec<char>> = Vec::new();
            members
                .try_reserve_exact(literals.len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            let mut constrained = true;
            for literal in literals {
                let whole = whole_scalars(literal.as_bytes(), prefix);
                if whole.is_empty() {
                    constrained = false;
                    break;
                }
                let member: Vec<char> = if folded {
                    whole.chars().map(fold_representative).collect()
                } else {
                    whole.chars().collect()
                };
                if !members.contains(&member) {
                    members.push(member);
                }
            }
            if constrained {
                sets.push(Self {
                    members,
                    folded,
                    _memory: memory,
                });
            }
        }
        Ok(sets)
    }
}

/// The longest whole-scalar prefix (or suffix) of a member's bytes.
fn whole_scalars(bytes: &[u8], prefix: bool) -> &str {
    if prefix {
        let end = match std::str::from_utf8(bytes) {
            Ok(text) => return text,
            Err(error) => error.valid_up_to(),
        };
        return match bytes.get(..end).map(std::str::from_utf8) {
            Some(Ok(text)) => text,
            _ => "",
        };
    }
    for skip in 0..=bytes.len() {
        if let Some(Ok(text)) = bytes.get(skip..).map(std::str::from_utf8) {
            return text;
        }
    }
    ""
}

/// The smallest scalar of `character`'s simple case folding class whose own
/// class contains `character`, or `character` itself.
fn fold_representative(character: char) -> char {
    let mut class = Vec::new();
    let mut back = Vec::new();
    if !SimpleCaseFold.expand(character, &mut class) {
        return character;
    }
    class
        .into_iter()
        .filter(|candidate| {
            SimpleCaseFold.expand(*candidate, &mut back) && back.contains(&character)
        })
        .chain(std::iter::once(character))
        .min()
        .unwrap_or(character)
}

/// Sorted owners that may hold a match, at most `MAX_UNANCHORED_DOCS`.
pub(super) struct RequiredAnchor {
    owners: Vec<OwnerKey>,
    _memory: Option<MemoryLease>,
}

impl RequiredAnchor {
    pub(super) fn owners(&self) -> &[OwnerKey] {
        &self.owners
    }
}

/// The union of the members' anchors, or `None` when any member is
/// unanchorable or the union exceeds `MAX_UNANCHORED_DOCS`.
pub(super) fn anchor_required(
    grams: LiteralGrams,
    required: &RequiredLiterals,
    context: &mut TextReadContext<'_>,
) -> Result<Option<RequiredAnchor>, IndexError> {
    let variants: &dyn CharVariants = if required.folded {
        &SimpleCaseFold
    } else {
        &ExactScalars
    };
    let bound =
        usize::try_from(MAX_UNANCHORED_DOCS).map_err(|_width| IndexError::ResourceLimited)?;
    let mut owners: Vec<OwnerKey> = Vec::new();
    let mut memory = None;
    for member in &required.members {
        context.budget.checkpoint()?;
        let Some(anchor) = anchor_chars(grams, member, variants, context)? else {
            return Ok(None);
        };
        let total = owners
            .len()
            .checked_add(anchor.owners().len())
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = total
            .checked_mul(std::mem::size_of::<OwnerKey>())
            .ok_or(IndexError::ResourceLimited)?;
        // The grant covers the union before deduplication shrinks it.
        let lease = context.budget.reserve_memory(
            u64::try_from(bytes).map_err(|_width| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        owners
            .try_reserve_exact(anchor.owners().len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        owners.extend_from_slice(anchor.owners());
        memory = lease;
        owners.sort_unstable();
        owners.dedup();
        if owners.len() > bound {
            return Ok(None);
        }
    }
    Ok(Some(RequiredAnchor {
        owners,
        _memory: memory,
    }))
}
