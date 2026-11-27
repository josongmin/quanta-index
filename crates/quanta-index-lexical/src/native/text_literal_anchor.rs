//! Raw-byte anchoring of `file.contains` on the pinned Literal sibling.
//!
//! `file.contains(n)` matches when `apply_case(nfc(body))` contains
//! `apply_case(nfc(n))`. The Literal sibling indexes raw source byte
//! trigrams. An anchor is a superset of the true matches derived from those
//! grams; every candidate is still verified against source bytes.
//!
//! Contract:
//! - A Text read may use only the pinned Literal sibling over the identical
//!   Source cut: the unique member of the ticket's view whose storage format
//!   is the configured Literal format and whose input root is the view's
//!   Source owners, the same root the Text member binds. No sibling, several
//!   siblings or a different root make the needle unanchorable; it then keeps
//!   the census behaviour.
//! - A needle character `c` of the policy image is *safe* when every raw
//!   scalar whose policy image contains `c` has exactly the image `c`, through
//!   a single NFC scalar that has no canonical decomposition and combining
//!   class 0. Then every occurrence of `c` in the policy image of a body comes
//!   from one raw scalar in `c`'s variant set, at the same position. The table
//!   is generated at build time from the verifier's own `nfc`/`apply_case`
//!   over every Unicode scalar value.
//! - Each run of three consecutive safe characters is a window. A body that
//!   matches contains, for every window, the raw UTF-8 of some variant
//!   combination, and therefore all of that combination's byte trigrams.
//! - A needle without a window whose combinations each have a gram posting of
//!   at most `MAX_UNANCHORED_DOCS` owners, or whose resulting candidate set
//!   exceeds that bound, is unanchored.
//!
//! The window rule only needs a variant source: for each needle character,
//! the raw scalars that can stand for it at its own position. `anchor_needle`
//! uses the `file.contains` inverse above. `anchor_chars` takes any source;
//! `ExactScalars` serves byte-exact matching and `SimpleCaseFold` serves the
//! regex engine's case-insensitive matching of one literal character.

use quanta_index_core::index::{
    IndexError, MemberBinding, MemoryLease, MemoryLifetime, OwnerKey, ProducerId, StableOwnerId,
    StorageFormatId, WorkBudget, WorkUsage, decode_member_binding,
};
use regex_syntax::hir::{ClassUnicode, ClassUnicodeRange};

use super::literal::{GramProbe, LiteralGrams};
#[cfg(test)]
use super::literal_work::owner_key;
use super::text::{CaseMode, TextReadContext};
use super::text_memory::PreparedRows;
use super::text_universe::MAX_UNANCHORED_DOCS;

const MEMBER_KEY_PREFIX: &[u8] = b"member\0";
const MEMBER_KEY_BYTES: usize = 23;
const MEMBER_PAGE: u32 = 64;
/// One merge-join candidate: its raw key, its sweep flag and its kept owner.
const CANDIDATE_ROW_BYTES: usize =
    32 + std::mem::size_of::<bool>() + std::mem::size_of::<OwnerKey>();

/// Inverse of the `file.contains` policy image for one case mode.
pub(super) struct CaseInverse {
    /// Safe image characters with nonidentity preimages, in scalar order.
    variants: &'static [(char, &'static [char])],
    unsafe_chars: &'static [char],
    /// Nonidentity scalars outside `unsafe_chars`. All other safe scalars
    /// have an implicit singleton preimage when absent from `variants`.
    no_identity: &'static [char],
}

include!(concat!(env!("OUT_DIR"), "/literal_case_inverse.rs"));

/// The raw scalars that can stand for one image character.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum Variants<'a> {
    /// Some raw sequence other than one variant scalar can produce it.
    Unsafe,
    /// The character is its own sole raw scalar preimage.
    Identity(char),
    /// Exactly these raw scalars produce it; empty when nothing does.
    Set(&'a [char]),
}

impl CaseInverse {
    pub(super) fn variants(&self, character: char) -> Variants<'_> {
        if self.unsafe_chars.binary_search(&character).is_ok() {
            return Variants::Unsafe;
        }
        if let Ok(index) = self.variants.binary_search_by_key(&character, |row| row.0) {
            return Variants::Set(self.variants[index].1);
        }
        if self.no_identity.binary_search(&character).is_ok() {
            return Variants::Set(&[]);
        }
        Variants::Identity(character)
    }
}

/// The raw scalars an anchor may expand one needle character into.
pub(super) trait CharVariants {
    /// Replace `into` with every raw scalar that can stand for `character`
    /// at its own position, or return `false` when no set of single raw
    /// scalars covers every way to produce it.
    fn expand(&self, character: char, into: &mut Vec<char>) -> bool;
}

impl CharVariants for CaseInverse {
    fn expand(&self, character: char, into: &mut Vec<char>) -> bool {
        into.clear();
        match self.variants(character) {
            Variants::Unsafe => false,
            Variants::Identity(scalar) => {
                into.push(scalar);
                true
            }
            Variants::Set(set) => {
                into.extend_from_slice(set);
                true
            }
        }
    }
}

/// Byte-exact matching: a character stands only for its own UTF-8.
pub(super) struct ExactScalars;

impl CharVariants for ExactScalars {
    fn expand(&self, character: char, into: &mut Vec<char>) -> bool {
        into.clear();
        into.push(character);
        true
    }
}

/// Case-insensitive regex matching of one literal character: the regex
/// translator turns it into exactly the Unicode simple case folding class of
/// that character, built by this same `regex_syntax` call.
pub(super) struct SimpleCaseFold;

impl CharVariants for SimpleCaseFold {
    fn expand(&self, character: char, into: &mut Vec<char>) -> bool {
        into.clear();
        let mut class = ClassUnicode::new([ClassUnicodeRange::new(character, character)]);
        if class.try_case_fold_simple().is_err() {
            return false;
        }
        for range in class.iter() {
            into.extend(range.start()..=range.end());
        }
        true
    }
}

pub(super) fn case_inverse(case: CaseMode) -> &'static CaseInverse {
    match case {
        CaseMode::Sensitive => &SENSITIVE,
        CaseMode::Folded => &FOLDED,
    }
}

/// The pinned Literal sibling of `text_member`, or `None` when the view does
/// not carry exactly one such member over the identical Source cut.
pub(super) fn pinned_literal_sibling(
    format: StorageFormatId,
    text_member: &MemberBinding,
    context: &mut TextReadContext<'_>,
) -> Result<Option<LiteralGrams>, IndexError> {
    let sources = context.ticket.view.source.owners;
    if text_member.input.root != sources {
        return Ok(None);
    }
    let mut sibling: Option<MemberBinding> = None;
    let mut ambiguous = false;
    let mut after: Option<Vec<u8>> = None;
    loop {
        context.budget.checkpoint()?;
        let page = context.maps.scan(
            context.ticket.view.members,
            after.as_deref(),
            MEMBER_PAGE,
            context.budget,
        )?;
        for (key, value) in &page.entries {
            if after
                .as_deref()
                .is_some_and(|prior| prior >= key.as_slice())
            {
                return Err(IndexError::CorruptObject(
                    "view member map is not strictly ordered".into(),
                ));
            }
            after = Some(key.clone());
            if key.len() != MEMBER_KEY_BYTES || !key.starts_with(MEMBER_KEY_PREFIX) {
                continue;
            }
            let reference = value.object_ref()?;
            let stored = context.objects.get(reference, context.budget)?;
            let member = decode_member_binding(&stored.payload, context.budget)?;
            if stored.registration.reference != reference
                || key.get(MEMBER_KEY_PREFIX.len()..) != Some(member.instance.0.as_slice())
                || !stored.registration.children.iter().copied().eq([
                    Some(member.artifact),
                    member.input.root.0,
                ]
                .into_iter()
                .flatten())
            {
                return Err(IndexError::CorruptObject(
                    "view member differs from its key or edges".into(),
                ));
            }
            if member.format != format || member.instance == text_member.instance {
                continue;
            }
            if sibling.is_some() {
                ambiguous = true;
            }
            sibling = Some(member);
        }
        if !page.has_more {
            break;
        }
        if page.entries.is_empty() {
            return Err(IndexError::BackendViolation(
                "nonprogressing member map cursor".into(),
            ));
        }
    }
    let Some(sibling) = sibling.filter(|member| !ambiguous && member.input.root == sources) else {
        return Ok(None);
    };
    LiteralGrams::open(&sibling, sources, context.objects, context.budget).map(Some)
}

/// Owners that may contain one needle, sorted and at most
/// `MAX_UNANCHORED_DOCS`.
pub(super) struct LiteralAnchor {
    owners: Vec<OwnerKey>,
    _memory: Option<MemoryLease>,
}

impl LiteralAnchor {
    pub(super) fn len(&self) -> Result<u64, IndexError> {
        u64::try_from(self.owners.len()).map_err(|_width| IndexError::ResourceLimited)
    }

    pub(super) fn contains(&self, owner: OwnerKey) -> bool {
        self.owners.binary_search(&owner).is_ok()
    }

    pub(super) fn owners(&self) -> &[OwnerKey] {
        &self.owners
    }
}

struct Combination {
    grams: Vec<[u8; 3]>,
}

struct Window {
    combinations: Vec<Combination>,
    _memory: Option<MemoryLease>,
}

fn owner_from_key(key: [u8; 32]) -> OwnerKey {
    let mut producer = [0_u8; 16];
    let mut stable = [0_u8; 16];
    for (target, byte) in producer.iter_mut().chain(stable.iter_mut()).zip(key) {
        *target = byte;
    }
    OwnerKey {
        producer: ProducerId(producer),
        stable_id: StableOwnerId(stable),
    }
}

fn windows(
    image: &[char],
    variants: &dyn CharVariants,
    budget: &mut WorkBudget,
) -> Result<PreparedRows<Window>, IndexError> {
    let window_capacity = image.len().saturating_sub(2);
    let mut windows = PreparedRows::new(window_capacity.max(1))?;
    if window_capacity == 0 {
        return Ok(windows);
    }
    // Deduplicate the character windows before allocating variant rows.
    let mut keys = PreparedRows::with_capacity(window_capacity, budget)?;
    for triple in image.windows(3) {
        budget.checkpoint()?;
        let key = <[char; 3]>::try_from(triple).map_err(|_width| {
            IndexError::BackendViolation("literal character window width differs".into())
        })?;
        keys.push_preallocated(key)?;
    }
    keys.as_mut_slice().sort_unstable();
    let mut previous = None;
    let mut buffer = String::new();
    let (mut first, mut second, mut third) = (Vec::new(), Vec::new(), Vec::new());
    for key in keys.as_slice().iter().copied() {
        budget.checkpoint()?;
        if previous.replace(key) == Some(key) {
            continue;
        }
        let [first_char, second_char, third_char] = key;
        if !(variants.expand(first_char, &mut first)
            && variants.expand(second_char, &mut second)
            && variants.expand(third_char, &mut third))
        {
            continue;
        }
        let combination_capacity = first
            .len()
            .checked_mul(second.len())
            .and_then(|count| count.checked_mul(third.len()))
            .ok_or(IndexError::ResourceLimited)?;
        // Three UTF-8 scalars have at most twelve bytes and ten trigrams.
        // Fund both the combination vector and all of its gram vectors for
        // the lifetime of this window before allocating either one.
        let bytes_per_combination = std::mem::size_of::<Combination>()
            .checked_add(10 * std::mem::size_of::<[u8; 3]>())
            .ok_or(IndexError::ResourceLimited)?;
        let combination_bytes = combination_capacity
            .checked_mul(bytes_per_combination)
            .ok_or(IndexError::ResourceLimited)?;
        let combination_memory = if combination_bytes == 0 {
            None
        } else {
            budget.reserve_memory(
                u64::try_from(combination_bytes).map_err(|_width| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?
        };
        let mut combinations = Vec::new();
        combinations
            .try_reserve_exact(combination_capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if combinations.capacity() > combination_capacity {
            return Err(IndexError::ResourceLimited);
        }
        for a in &first {
            for b in &second {
                for c in &third {
                    buffer.clear();
                    buffer.push(*a);
                    buffer.push(*b);
                    buffer.push(*c);
                    budget.charge(WorkUsage {
                        bytes: u64::try_from(buffer.len())
                            .map_err(|_width| IndexError::ResourceLimited)?,
                        ..WorkUsage::default()
                    })?;
                    let mut unique = [[0_u8; 3]; 10];
                    let mut gram_count = 0;
                    for gram in buffer.as_bytes().windows(3) {
                        let gram = <[u8; 3]>::try_from(gram).map_err(|_width| {
                            IndexError::BackendViolation("literal gram width".into())
                        })?;
                        if !unique[..gram_count].contains(&gram) {
                            unique[gram_count] = gram;
                            gram_count += 1;
                        }
                    }
                    let mut grams = Vec::new();
                    grams
                        .try_reserve_exact(gram_count)
                        .map_err(|_error| IndexError::ResourceLimited)?;
                    if grams.capacity() > gram_count {
                        return Err(IndexError::ResourceLimited);
                    }
                    grams.extend_from_slice(&unique[..gram_count]);
                    combinations.push(Combination { grams });
                }
            }
        }
        windows.push(
            Window {
                combinations,
                _memory: combination_memory,
            },
            budget,
        )?;
    }
    Ok(windows)
}

/// The candidate owners of `file.contains(needle)` from the sibling's gram
/// postings, or `None` when the needle is not anchorable.
pub(super) fn anchor_needle(
    grams: LiteralGrams,
    needle: &str,
    case: CaseMode,
    context: &mut TextReadContext<'_>,
) -> Result<Option<LiteralAnchor>, IndexError> {
    let plan = plan_needle(grams, needle, case, None, context)?;
    finish_anchor(grams, plan, context)
}

/// A pure body conjunction can use any one complete literal superset. Probe
/// singleton suppliers cheaply, then compare window bounds without allocating
/// an owner set for the broad terms. Every conjunct is still verified on Source.
pub(super) fn anchor_body_conjunction(
    grams: LiteralGrams,
    needles: &[Option<&str>],
    case: CaseMode,
    context: &mut TextReadContext<'_>,
) -> Result<Option<(usize, LiteralAnchor)>, IndexError> {
    let needles = needles.iter().enumerate().map(|(index, needle)| {
        needle.map(|needle| (index, needle)).ok_or_else(|| {
            IndexError::BackendViolation("native file.contains source missing".into())
        })
    });
    // A dense term written first must not force full posting counts before
    // the planner has tested the other terms for a selective window.
    let mut probe = 1;
    loop {
        let mut best: Option<(usize, &str, u64)> = None;
        for entry in needles.clone() {
            let (index, needle) = entry?;
            let limit = best.map_or(probe, |(_, _, bound)| bound.saturating_sub(1));
            if let Some(plan) = plan_needle(grams, needle, case, Some(limit), context)? {
                if plan.bound <= 1 {
                    return finish_anchor(grams, Some(plan), context)
                        .map(|anchor| anchor.map(|anchor| (index, anchor)));
                }
                best = Some((index, needle, plan.bound));
            }
        }
        if let Some((index, needle, bound)) = best {
            // Re-plan only the winner with the tight bound; the estimate pass
            // does not retain per-needle Unicode windows or posting statistics.
            let plan =
                plan_needle(grams, needle, case, Some(bound), context)?.ok_or_else(|| {
                    IndexError::CorruptObject("literal anchor bound changed during planning".into())
                })?;
            return finish_anchor(grams, Some(plan), context)
                .map(|anchor| anchor.map(|anchor| (index, anchor)));
        }
        if probe == MAX_UNANCHORED_DOCS {
            break;
        }
        probe = probe.saturating_mul(8).min(MAX_UNANCHORED_DOCS);
    }

    // Folded variants may have a loose sum of posting sizes even though
    // intersecting every required window yields an admissible owner set.
    for entry in needles {
        let (index, needle) = entry?;
        if let Some(anchor) = anchor_needle(grams, needle, case, context)? {
            return Ok(Some((index, anchor)));
        }
    }
    Ok(None)
}

fn plan_needle(
    grams: LiteralGrams,
    needle: &str,
    case: CaseMode,
    probe_owner_limit: Option<u64>,
    context: &mut TextReadContext<'_>,
) -> Result<Option<LiteralAnchorPlan>, IndexError> {
    context.budget.checkpoint()?;
    let needle_bytes = u64::try_from(needle.len()).map_err(|_width| IndexError::ResourceLimited)?;
    // The policy image of the needle, exactly as the verifier computes it.
    context.budget.charge(WorkUsage {
        bytes: needle_bytes
            .checked_mul(8)
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let _image_memory = context.budget.reserve_memory(
        needle_bytes
            .checked_mul(8)
            .ok_or(IndexError::ResourceLimited)?
            .max(1),
        MemoryLifetime::Temporary,
    )?;
    let normalized = crate::normalize::nfc(needle);
    let image: Vec<char> = crate::normalize::apply_case(normalized.as_ref(), case)
        .chars()
        .collect();
    plan_anchor(
        grams,
        &image,
        case_inverse(case),
        probe_owner_limit,
        context,
    )
}

/// The candidate owners of `needle` under `variants`, or `None`.
///
/// Every owner whose body contains a raw expansion of the needle under
/// `variants` is a candidate; `None` means the needle is not anchorable. The
/// caller charges and holds the needle itself.
///
/// The result is the exact set of owners that, for every window, hold every
/// gram of at least one of its combinations. It is computed as a merge over
/// sorted postings:
/// - The chosen window is the one with the smallest bound, the sum over its
///   combinations of their rarest gram's count (see [`plan_anchor`]).
/// - Its combinations' rarest postings are read once each, in order, and
///   their union is the candidate stream.
/// - The candidates are filtered window by window, in needle order. Within a
///   window each combination, in order, sweeps the candidates no earlier
///   combination admitted, ascending, through one monotone [`GramProbe`] per
///   gram, so each of those postings' blocks is read at most once per sweep
///   and only when it holds a candidate. Only one combination's probes, at
///   most ten grams and their current blocks, are live at a time, whatever
///   the needle length.
pub(super) fn anchor_chars(
    grams: LiteralGrams,
    needle: &[char],
    variants: &dyn CharVariants,
    context: &mut TextReadContext<'_>,
) -> Result<Option<LiteralAnchor>, IndexError> {
    let plan = plan_anchor(grams, needle, variants, None, context)?;
    finish_anchor(grams, plan, context)
}

struct LiteralAnchorPlan {
    windows: PreparedRows<Window>,
    counts: GramCounts,
    chosen: usize,
    bound: u64,
}

// Posting probes cap counts at MAX_UNANCHORED_DOCS + 1.
const UNCOUNTED_GRAM: u16 = u16::MAX;

struct GramCounts {
    keys: Vec<[u8; 3]>,
    values: PreparedRows<u16>,
    _key_memory: Option<MemoryLease>,
}

impl GramCounts {
    fn new(windows: &[Window], budget: &WorkBudget) -> Result<Self, IndexError> {
        let combinations = windows.iter().flat_map(|window| &window.combinations);
        let slots = combinations
            .clone()
            .try_fold(0_usize, |slots, combination| {
                budget.checkpoint()?;
                slots
                    .checked_add(combination.grams.len())
                    .ok_or(IndexError::ResourceLimited)
            })?;
        if slots == 0 {
            return Ok(Self {
                keys: Vec::new(),
                values: PreparedRows::new(1)?,
                _key_memory: None,
            });
        }
        let bytes = slots
            .checked_mul(std::mem::size_of::<[u8; 3]>())
            .ok_or(IndexError::ResourceLimited)?;
        let key_memory = budget.reserve_memory(
            u64::try_from(bytes).map_err(|_width| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut keys = Vec::new();
        keys.try_reserve_exact(slots)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if keys.capacity() > slots {
            return Err(IndexError::ResourceLimited);
        }
        for combination in combinations {
            budget.checkpoint()?;
            keys.extend_from_slice(&combination.grams);
        }
        keys.sort_unstable();
        keys.dedup();
        budget.checkpoint()?;
        let mut values = PreparedRows::with_capacity(keys.len(), budget)?;
        for _ in &keys {
            values.push_preallocated(UNCOUNTED_GRAM)?;
        }
        Ok(Self {
            keys,
            values,
            _key_memory: key_memory,
        })
    }

    fn get(&self, gram: &[u8; 3]) -> Result<usize, IndexError> {
        self.keys.binary_search(gram).map_err(|_missing| {
            IndexError::BackendViolation("literal anchor gram count missing".into())
        })
    }

    fn count(
        &mut self,
        gram: &[u8; 3],
        probe: impl FnOnce() -> Result<u64, IndexError>,
    ) -> Result<u64, IndexError> {
        let index = self.get(gram)?;
        let entry = self.values.as_mut_slice().get_mut(index).ok_or_else(|| {
            IndexError::BackendViolation("literal anchor gram count missing".into())
        })?;
        if *entry == UNCOUNTED_GRAM {
            *entry = u16::try_from(probe()?)
                .ok()
                .filter(|count| *count != UNCOUNTED_GRAM)
                .ok_or(IndexError::ResourceLimited)?;
        }
        Ok(u64::from(*entry))
    }

    fn rarest(&self, combination: &Combination) -> Result<Option<([u8; 3], u64)>, IndexError> {
        let mut rarest = None;
        for gram in &combination.grams {
            let index = self.get(gram)?;
            let count = self
                .values
                .as_slice()
                .get(index)
                .copied()
                .filter(|count| *count != UNCOUNTED_GRAM)
                .ok_or_else(|| {
                    IndexError::BackendViolation("literal anchor gram count missing".into())
                })?;
            let count = u64::from(count);
            if rarest.is_none_or(|(_, best)| count < best) {
                rarest = Some((*gram, count));
            }
        }
        Ok(rarest)
    }
}

fn plan_anchor(
    grams: LiteralGrams,
    needle: &[char],
    variants: &dyn CharVariants,
    probe_owner_limit: Option<u64>,
    context: &mut TextReadContext<'_>,
) -> Result<Option<LiteralAnchorPlan>, IndexError> {
    context.budget.checkpoint()?;
    let windows = windows(needle, variants, context.budget)?;
    let owner_limit = probe_owner_limit.unwrap_or(MAX_UNANCHORED_DOCS);
    let cap = owner_limit
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    let mut counts = GramCounts::new(windows.as_slice(), context.budget)?;
    // The bound of a window is the sum over its combinations of their
    // rarest gram; a window is usable only when each such gram is bounded.
    let mut chosen: Option<(usize, u64)> = None;
    for (ordinal, window) in windows.as_slice().iter().enumerate() {
        let mut bound = 0_u64;
        let mut bounded = true;
        for combination in &window.combinations {
            let mut rarest = cap;
            for gram in &combination.grams {
                let count = counts.count(gram, || {
                    grams.count_capped(*gram, cap, context.objects, context.maps, context.budget)
                })?;
                rarest = rarest.min(count);
            }
            if rarest > owner_limit {
                bounded = false;
                break;
            }
            bound = bound
                .checked_add(rarest)
                .ok_or(IndexError::ResourceLimited)?;
            if probe_owner_limit.is_some_and(|limit| bound > limit) {
                bounded = false;
                break;
            }
        }
        if bounded && chosen.is_none_or(|(_, best)| bound < best) {
            chosen = Some((ordinal, bound));
        }
    }
    let Some((chosen, bound)) = chosen else {
        return Ok(None);
    };
    Ok(Some(LiteralAnchorPlan {
        windows,
        counts,
        chosen,
        bound,
    }))
}

fn finish_anchor(
    grams: LiteralGrams,
    plan: Option<LiteralAnchorPlan>,
    context: &mut TextReadContext<'_>,
) -> Result<Option<LiteralAnchor>, IndexError> {
    let Some(LiteralAnchorPlan {
        windows,
        counts,
        chosen,
        bound,
    }) = plan
    else {
        return Ok(None);
    };
    let window = windows
        .as_slice()
        .get(chosen)
        .ok_or_else(|| IndexError::BackendViolation("literal anchor window lost".into()))?;
    let capacity = usize::try_from(bound).map_err(|_width| IndexError::ResourceLimited)?;
    // Each candidate row is held as a raw key, a sweep flag and a kept
    // owner key at once.
    let row_bytes = capacity
        .checked_mul(CANDIDATE_ROW_BYTES)
        .ok_or(IndexError::ResourceLimited)?;
    let memory = if row_bytes == 0 {
        None
    } else {
        context.budget.reserve_memory(
            u64::try_from(row_bytes).map_err(|_width| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
    };
    let mut candidates: Vec<[u8; 32]> = Vec::new();
    candidates
        .try_reserve_exact(capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    for combination in &window.combinations {
        let Some((rarest, mut remaining)) = counts.rarest(combination)? else {
            continue;
        };
        if remaining == 0 {
            continue;
        }
        grams.owners(
            rarest,
            context.objects,
            context.maps,
            context.budget,
            |owner, _budget| {
                remaining = remaining.checked_sub(1).ok_or_else(|| {
                    IndexError::CorruptObject("literal posting exceeds its counted owners".into())
                })?;
                if candidates.len() == candidates.capacity() {
                    return Err(IndexError::CorruptObject(
                        "literal anchor exceeds its counted owners".into(),
                    ));
                }
                candidates.push(owner);
                Ok(())
            },
        )?;
        if remaining != 0 {
            return Err(IndexError::CorruptObject(
                "literal posting has fewer owners than counted".into(),
            ));
        }
    }
    candidates.sort_unstable();
    candidates.dedup();
    let mut admitted: Vec<bool> = Vec::new();
    admitted
        .try_reserve_exact(candidates.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    for window in windows.as_slice() {
        if candidates.is_empty() {
            break;
        }
        admitted.clear();
        admitted.resize(candidates.len(), false);
        for combination in &window.combinations {
            let mut probes: Vec<GramProbe> = Vec::new();
            probes
                .try_reserve_exact(combination.grams.len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            probes.extend(combination.grams.iter().map(|gram| grams.probe(*gram)));
            let mut pending = false;
            for (owner, admitted) in candidates.iter().zip(admitted.iter_mut()) {
                if *admitted {
                    continue;
                }
                context.budget.checkpoint()?;
                let mut all = true;
                for probe in &mut probes {
                    if !probe.contains(*owner, context.objects, context.maps, context.budget)? {
                        all = false;
                        break;
                    }
                }
                *admitted = all;
                pending |= !all;
            }
            // The probes and the blocks they hold end with their sweep.
            drop(probes);
            if !pending {
                break;
            }
        }
        let mut flags = admitted.iter().copied();
        candidates.retain(|_owner| flags.next().is_some_and(|admitted| admitted));
    }
    if u64::try_from(candidates.len()).map_err(|_width| IndexError::ResourceLimited)?
        > MAX_UNANCHORED_DOCS
    {
        return Ok(None);
    }
    let mut owners = Vec::new();
    owners
        .try_reserve_exact(candidates.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    owners.extend(candidates.iter().map(|owner| owner_from_key(*owner)));
    owners.sort_unstable();
    Ok(Some(LiteralAnchor {
        owners,
        _memory: memory,
    }))
}

/// The per-owner probing finish, kept as the exactness oracle for the
/// merge-join anchor: identical result sets on every needle.
#[cfg(test)]
pub(super) fn anchor_chars_probing(
    grams: LiteralGrams,
    needle: &[char],
    variants: &dyn CharVariants,
    context: &mut TextReadContext<'_>,
) -> Result<Option<LiteralAnchor>, IndexError> {
    let plan = plan_anchor(grams, needle, variants, None, context)?;
    finish_anchor_probing(grams, plan, context)
}

#[cfg(test)]
fn finish_anchor_probing(
    grams: LiteralGrams,
    plan: Option<LiteralAnchorPlan>,
    context: &mut TextReadContext<'_>,
) -> Result<Option<LiteralAnchor>, IndexError> {
    let Some(LiteralAnchorPlan {
        windows,
        counts,
        chosen,
        bound,
    }) = plan
    else {
        return Ok(None);
    };
    let window = windows
        .as_slice()
        .get(chosen)
        .ok_or_else(|| IndexError::BackendViolation("literal anchor window lost".into()))?;
    let capacity = usize::try_from(bound).map_err(|_width| IndexError::ResourceLimited)?;
    let row_bytes = capacity
        .checked_mul(std::mem::size_of::<OwnerKey>())
        .ok_or(IndexError::ResourceLimited)?;
    let memory = if row_bytes == 0 {
        None
    } else {
        context.budget.reserve_memory(
            u64::try_from(row_bytes).map_err(|_width| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
    };
    let mut owners = Vec::new();
    owners
        .try_reserve_exact(capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if owners.capacity() > capacity {
        return Err(IndexError::ResourceLimited);
    }
    let mut membership = super::literal_blocks::MembershipCache::default();
    for combination in &window.combinations {
        let Some((rarest, mut remaining)) = counts.rarest(combination)? else {
            continue;
        };
        if remaining == 0 {
            continue;
        }
        // Append this posting in the funded owner buffer. Finish reading it
        // before probing the other grams so no posting cursor stays pinned.
        let start = owners.len();
        grams.owners(
            rarest,
            context.objects,
            context.maps,
            context.budget,
            |owner, _budget| {
                remaining = remaining.checked_sub(1).ok_or_else(|| {
                    IndexError::CorruptObject("literal posting exceeds its counted owners".into())
                })?;
                if owners.len() == owners.capacity() {
                    return Err(IndexError::CorruptObject(
                        "literal anchor exceeds its counted owners".into(),
                    ));
                }
                owners.push(owner_from_key(owner));
                Ok(())
            },
        )?;
        if remaining != 0 {
            return Err(IndexError::CorruptObject(
                "literal posting has fewer owners than counted".into(),
            ));
        }
        // The validated posting already proves membership in its own gram.
        // ASCII windows have just that gram and need no point lookups here.
        if combination.grams.len() > 1 {
            retain_owners_from(&mut owners, start, |owner| {
                let remaining_grams = combination
                    .grams
                    .iter()
                    .copied()
                    .filter(|&gram| gram != rarest);
                contains_grams(
                    grams,
                    &mut membership,
                    remaining_grams,
                    owner_key(owner),
                    context,
                )
            })?;
        }
    }
    owners.sort_unstable();
    owners.dedup();
    // Every other window filters the chosen window's candidates.
    retain_owners_from(&mut owners, 0, |owner| {
        let key = owner_key(owner);
        for (ordinal, other) in windows.as_slice().iter().enumerate() {
            if ordinal == chosen {
                continue;
            }
            let mut any = false;
            for combination in &other.combinations {
                if contains_grams(
                    grams,
                    &mut membership,
                    combination.grams.iter().copied(),
                    key,
                    context,
                )? {
                    any = true;
                    break;
                }
            }
            if !any {
                return Ok(false);
            }
        }
        Ok(true)
    })?;
    if u64::try_from(owners.len()).map_err(|_width| IndexError::ResourceLimited)?
        > MAX_UNANCHORED_DOCS
    {
        return Ok(None);
    }
    Ok(Some(LiteralAnchor {
        owners,
        _memory: memory,
    }))
}

/// Compact only the newly appended owners, retaining the funded allocation.
#[cfg(test)]
fn retain_owners_from(
    owners: &mut Vec<OwnerKey>,
    start: usize,
    mut keep: impl FnMut(OwnerKey) -> Result<bool, IndexError>,
) -> Result<(), IndexError> {
    let mut kept = start;
    for index in start..owners.len() {
        let owner = *owners.get(index).ok_or_else(|| {
            IndexError::BackendViolation("literal candidate cursor exceeds owner buffer".into())
        })?;
        if keep(owner)? {
            *owners.get_mut(kept).ok_or_else(|| {
                IndexError::BackendViolation("literal compaction exceeds owner buffer".into())
            })? = owner;
            kept = kept.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
    }
    owners.truncate(kept);
    Ok(())
}

#[cfg(test)]
fn contains_grams(
    grams: LiteralGrams,
    membership: &mut super::literal_blocks::MembershipCache,
    required: impl IntoIterator<Item = [u8; 3]>,
    owner: [u8; 32],
    context: &mut TextReadContext<'_>,
) -> Result<bool, IndexError> {
    for gram in required {
        if !grams.contains(
            membership,
            gram,
            owner,
            context.objects,
            context.maps,
            context.budget,
        )? {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::index::{MemoryLeaseOwner, MemoryScope, MemoryScopePort, WorkLimits};
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use unicode_normalization::char::{canonical_combining_class, decompose_canonical};

    struct BoundedMemory(u64);

    impl MemoryScopePort for BoundedMemory {
        fn reserve(&self, bytes: u64, _: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if bytes > self.0 {
                return Err(IndexError::ResourceLimited);
            }
            Ok(MemoryLease::new(bytes, Arc::new(Self(self.0))))
        }
    }

    impl MemoryLeaseOwner for BoundedMemory {
        fn release(&self, _: u64) {}
    }

    #[test]
    fn unicode_expansion_funds_combination_storage_before_allocation() {
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 0,
                writes: 0,
                bytes: 1_000,
                mutations: 0,
            },
            Instant::now() + Duration::from_secs(30),
        );
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(BoundedMemory(64)))));
        let result = windows(
            &['k', 'k', 'k'],
            case_inverse(CaseMode::Folded),
            &mut budget,
        );
        assert!(matches!(result, Err(IndexError::ResourceLimited)));

        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(BoundedMemory(4096)))));
        let admitted = windows(
            &['k', 'k', 'k'],
            case_inverse(CaseMode::Folded),
            &mut budget,
        )
        .expect("fund the folded trigram expansion");
        assert_eq!(admitted.len(), 1);
        assert_eq!(
            admitted
                .as_slice()
                .first()
                .map(|window| window.combinations.len()),
            Some(27)
        );

        let repeated = windows(&['k'; 128], case_inverse(CaseMode::Folded), &mut budget)
            .expect("duplicate trigrams need only one funded window");
        assert_eq!(repeated.len(), 1);

        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(BoundedMemory(0)))));
        assert!(GramCounts::new(&[], &budget).is_ok());
        assert!(matches!(
            GramCounts::new(admitted.as_slice(), &budget),
            Err(IndexError::ResourceLimited)
        ));

        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(BoundedMemory(4096)))));
        let mut counts = GramCounts::new(admitted.as_slice(), &budget).expect("fund gram counts");
        assert_eq!(counts.count(b"kkk", || Ok(0)).expect("count empty gram"), 0);
        assert_eq!(
            counts
                .count(b"kkk", || Err(IndexError::ResourceLimited))
                .expect("reuse the counted zero"),
            0
        );
        assert_eq!(
            counts
                .count(b"KKK", || Ok(MAX_UNANCHORED_DOCS + 1))
                .expect("preserve a capped count"),
            MAX_UNANCHORED_DOCS + 1
        );
    }

    fn image(text: &str, case: CaseMode) -> String {
        crate::normalize::apply_case(crate::normalize::nfc(text).as_ref(), case).into_owned()
    }

    /// Completeness and soundness over every Unicode scalar.
    ///
    /// A raw scalar's image either touches only unsafe characters or is
    /// exactly one safe character whose variant set lists that scalar, and
    /// every listed variant maps back onto its character.
    #[test]
    fn inverse_table_is_complete_and_sound_for_both_policies() {
        let mut buffer = [0_u8; 4];
        for case in [CaseMode::Sensitive, CaseMode::Folded] {
            let inverse = case_inverse(case);
            let mut unsafe_oracle = BTreeSet::new();
            for raw in (0..=u32::from(char::MAX)).filter_map(char::from_u32) {
                let normalized = crate::normalize::nfc(raw.encode_utf8(&mut buffer));
                let raw_image = crate::normalize::apply_case(&normalized, case);
                let mut normalized_chars = normalized.chars();
                let single_starter = normalized_chars.next().is_some_and(|scalar| {
                    let mut decomposition_len = 0;
                    decompose_canonical(scalar, |_| decomposition_len += 1);
                    decomposition_len == 1 && canonical_combining_class(scalar) == 0
                }) && normalized_chars.next().is_none();
                if !single_starter || raw_image.chars().count() != 1 {
                    unsafe_oracle.extend(raw_image.chars());
                }
                for character in raw_image.chars() {
                    match inverse.variants(character) {
                        Variants::Unsafe => {}
                        Variants::Identity(identity) => {
                            assert_eq!(raw_image.chars().count(), 1, "{raw:?} -> {raw_image:?}");
                            assert_eq!(identity, raw);
                        }
                        Variants::Set(set) => {
                            assert_eq!(raw_image.chars().count(), 1, "{raw:?} -> {raw_image:?}");
                            assert!(set.contains(&raw), "{raw:?} missing from {character:?}");
                        }
                    }
                }
            }
            assert!(inverse.unsafe_chars.iter().copied().eq(unsafe_oracle));
            for (character, set) in inverse.variants {
                for variant in *set {
                    assert_eq!(
                        image(variant.encode_utf8(&mut buffer), case),
                        character.to_string()
                    );
                }
            }
        }
    }

    #[test]
    fn inverse_table_fixed_unicode_cases() {
        let sensitive = case_inverse(CaseMode::Sensitive);
        let folded = case_inverse(CaseMode::Folded);
        // KELVIN SIGN normalizes to K and folds to k.
        assert_eq!(sensitive.variants('K'), Variants::Set(&['K', '\u{212A}']));
        assert_eq!(folded.variants('k'), Variants::Set(&['K', 'k', '\u{212A}']));
        // GREEK QUESTION MARK normalizes to a semicolon.
        assert_eq!(sensitive.variants(';'), Variants::Set(&[';', '\u{37E}']));
        // Dotted capital I folds to i + U+0307: both are unsafe when folded.
        assert_eq!(folded.variants('i'), Variants::Unsafe);
        assert_eq!(folded.variants('\u{307}'), Variants::Unsafe);
        assert_eq!(sensitive.variants('i'), Variants::Identity('i'));
        assert_eq!(folded.variants('K'), Variants::Set(&[]));
        assert_eq!(sensitive.variants('\u{212A}'), Variants::Set(&[]));
        // A composed letter can come from a decomposed raw sequence.
        assert_eq!(sensitive.variants('é'), Variants::Unsafe);
        assert_eq!(folded.variants('é'), Variants::Unsafe);
        assert_eq!(sensitive.variants('e'), Variants::Identity('e'));
        // Combining marks can be reordered by NFC.
        assert_eq!(sensitive.variants('\u{301}'), Variants::Unsafe);
        // Uncased non-ASCII letters keep a single variant.
        assert_eq!(folded.variants('ж'), Variants::Set(&['Ж', 'ж']));
        assert_eq!(sensitive.variants('中'), Variants::Identity('中'));
    }
}
