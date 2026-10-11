//! Owner-local byte-literal index for the native kernel. The query capability is
//! explicitly case-sensitive byte literals; token/ranking/regex routes stay separate.
use super::edit_spool::LiteralEditSpool;
use quanta_index_core::index::{
    AttemptContext, BaseMember, DeltaInput, Digest, IndexError, InputBinding, MapEdit, MapRoot,
    MemberBinding, ObjectBatchInput, ObjectRef, OwnerKey, PlanContext, PlanDecision,
    SourceChangeCursor, SourceDelta, SourceRecord, TicketRecord, WorkBudget, WorkUsage,
    ports::{ImmutableObjectPort, IndexBuildPort, IndexFuture, PersistentMapPort},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeLiteralIndex;

const POSTING_BATCH_ROOTS: usize = 64;
const CONTRIBUTION_BATCH_OBJECTS: usize = 64;
// The object port admits at most 64 MiB of encoded wire per call. Keep a
// smaller exact-wire window to bound pending contribution memory.
const CONTRIBUTION_BATCH_WIRE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Root {
    owners: MapRoot,
    grams: MapRoot,
}
quanta_index_contract::native_serde_struct!(Root {
    owners: MapRoot,
    grams: MapRoot,
});

#[derive(Clone, Debug, Eq, PartialEq)]
struct Contribution {
    record: ObjectRef,
    grams: Vec<[u8; 3]>,
}
quanta_index_contract::native_serde_struct!(Contribution {
    record: ObjectRef,
    grams: Vec<[u8; 3]>,
});

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Posting {
    root: MapRoot,
    count: u64,
}
quanta_index_contract::native_serde_struct!(Posting {
    root: MapRoot,
    count: u64,
});

struct PendingContribution {
    owner: Vec<u8>,
    prior: Option<ObjectRef>,
    input: ObjectBatchInput,
}

#[derive(Clone, Debug)]
pub struct LiteralPlan {
    base: Option<MemberBinding>,
    context: PlanContext,
    delta: Option<SourceDelta>,
}

#[derive(Clone, Debug)]
pub struct LiteralCandidate {
    pub binding: MemberBinding,
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, IndexError> {
    serde_json::to_vec(value).map_err(|error| IndexError::InvalidInput(error.to_string()))
}
fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, IndexError> {
    serde_json::from_slice(bytes).map_err(|error| IndexError::CorruptObject(error.to_string()))
}
fn owner_key(owner: OwnerKey) -> Vec<u8> {
    let mut key = Vec::with_capacity(32);
    key.extend_from_slice(&owner.producer.0);
    key.extend_from_slice(&owner.stable_id.0);
    key
}
fn roots(root: Root) -> Vec<ObjectRef> {
    [root.owners.0, root.grams.0]
        .into_iter()
        .flatten()
        .collect()
}
fn all_grams(bytes: &[u8], budget: &mut WorkBudget) -> Result<Vec<[u8; 3]>, IndexError> {
    let mut grams = BTreeSet::new();
    for window in bytes.windows(3) {
        budget.checkpoint()?;
        let gram: [u8; 3] = window.try_into().map_err(|_conversion_error| {
            IndexError::BackendViolation("invalid byte window".into())
        })?;
        if grams.insert(gram) {
            if grams.len() > 262_144 {
                return Err(IndexError::ResourceLimited);
            }
            // Conservative admission for tree node, output and mutation-set copies.
            budget.charge(WorkUsage {
                mutations: 1,
                bytes: 128,
                ..WorkUsage::default()
            })?;
        }
    }
    Ok(grams.into_iter().collect())
}

fn flush_contributions(
    context: &mut AttemptContext<'_>,
    pending: &mut Vec<PendingContribution>,
    spool: &LiteralEditSpool,
) -> Result<(), IndexError> {
    if pending.is_empty() {
        return Ok(());
    }
    context.budget.checkpoint()?;
    let count = pending.len();
    context.budget.charge(WorkUsage {
        bytes: u64::try_from(count)
            .map_err(|_conversion_error| IndexError::ResourceLimited)?
            .checked_mul(256)
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let mut inputs = Vec::with_capacity(count);
    let mut metadata = Vec::with_capacity(count);
    for item in pending.drain(..) {
        inputs.push(item.input);
        metadata.push((item.owner, item.prior));
    }
    let references = context.put_batch(&inputs)?;
    if references.len() != metadata.len() {
        return Err(IndexError::BackendViolation(
            "contribution batch lost ordinal output".into(),
        ));
    }
    for ((owner, prior), next) in metadata.into_iter().zip(references) {
        if prior != Some(next) {
            spool.edit(
                1,
                &MapEdit {
                    key: owner,
                    expected: prior,
                    next: Some(next),
                },
                context.budget,
            )?;
        }
    }
    Ok(())
}

impl IndexBuildPort for NativeLiteralIndex {
    type Delta = SourceDelta;
    type Plan = LiteralPlan;
    type Candidate = LiteralCandidate;

    fn plan(
        &self,
        base: BaseMember,
        input: DeltaInput<SourceDelta>,
        context: &PlanContext,
        budget: &mut WorkBudget,
    ) -> Result<PlanDecision<LiteralPlan>, IndexError> {
        budget.checkpoint()?;
        let (base, delta) = match (base, input) {
            (BaseMember::Absent, DeltaInput::Bootstrap) if context.source.owners.0.is_none() => {
                (None, None)
            }
            (BaseMember::Present(member), DeltaInput::Patch(delta)) => {
                if member.instance != context.instance.instance
                    || member.format != context.instance.format
                    || member.input.schema != context.input.schema
                    || member.input.policy != context.input.policy
                    || member.input.root != delta.prior.owners
                {
                    return Err(IndexError::Conflict);
                }
                (Some(*member), Some(delta))
            }
            (
                BaseMember::Absent | BaseMember::Present(_),
                DeltaInput::Bootstrap | DeltaInput::Patch(_),
            ) => return Ok(PlanDecision::RebuildRequired),
        };
        Ok(PlanDecision::Incremental(LiteralPlan {
            base,
            context: context.clone(),
            delta,
        }))
    }

    fn prepare<'a>(
        &'a self,
        plan: LiteralPlan,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, LiteralCandidate> {
        Box::pin(async move {
            let mut root = match &plan.base {
                Some(base) => {
                    decode::<Root>(&context.objects.get(base.artifact, context.budget)?.payload)?
                }
                None => Root::default(),
            };
            if let Some(delta) = plan.delta {
                // Compile the whole bounded delta before installing map roots.
                // Shared grams receive one posting edit batch, not an intermediate
                // root per owner; initial indexing benefits from the same path.
                let spool = LiteralEditSpool::new(context.budget)?;
                let mut pending_contributions = Vec::new();
                let mut pending_contribution_wire_bytes = 0u64;
                let marker = context.put(b"native-literal-owner-membership", &[])?;
                let mut changes = SourceChangeCursor::new(
                    delta.changed,
                    delta.prior,
                    delta.next,
                    context.maps,
                    context.objects,
                );
                while let Some(change) = changes.next_change(context.budget)? {
                    context.budget.checkpoint()?;
                    let owner = owner_key(change.owner);
                    let prior_ref = context.maps.lookup(root.owners, &owner, context.budget)?;
                    let prior = prior_ref
                        .map(|reference| {
                            context
                                .objects
                                .get(reference, context.budget)
                                .and_then(|stored| decode::<Contribution>(&stored.payload))
                        })
                        .transpose()?;
                    if prior.as_ref().map(|value| value.record)
                        != change.prior.as_ref().map(|(reference, _)| *reference)
                    {
                        return Err(IndexError::Conflict);
                    }
                    let next = if let Some((reference, record)) = change.next {
                        let grams = match (&change.prior, &prior) {
                            (Some((_, old_record)), Some(old))
                                if old_record.bytes == record.bytes =>
                            {
                                context.budget.charge(WorkUsage {
                                    bytes: u64::try_from(old.grams.len())
                                        .map_err(|_conversion_error| IndexError::ResourceLimited)?
                                        .checked_mul(128)
                                        .ok_or(IndexError::ResourceLimited)?,
                                    ..WorkUsage::default()
                                })?;
                                old.grams.clone()
                            }
                            _ => all_grams(
                                &context.objects.get(record.bytes, context.budget)?.payload,
                                context.budget,
                            )?,
                        };
                        Some(Contribution {
                            record: reference,
                            grams,
                        })
                    } else {
                        None
                    };
                    let prior_grams = prior.as_ref().map_or(&[][..], |value| &value.grams);
                    let next_grams = next.as_ref().map_or(&[][..], |value| &value.grams);
                    let mut last_prior = None;
                    for gram in prior_grams {
                        if last_prior.is_some_and(|last| last >= gram) {
                            return Err(IndexError::CorruptObject(
                                "contribution grams are not sorted and unique".into(),
                            ));
                        }
                        last_prior = Some(gram);
                    }
                    let mut prior_iter = prior_grams.iter().peekable();
                    let mut next_iter = next_grams.iter().peekable();
                    loop {
                        let change = match (
                            prior_iter.peek().copied().copied(),
                            next_iter.peek().copied().copied(),
                        ) {
                            (None, None) => break,
                            (Some(old), Some(new)) if old == new => {
                                let _old = prior_iter.next();
                                let _new = next_iter.next();
                                continue;
                            }
                            (Some(old), Some(new)) if old < new => {
                                let _old = prior_iter.next();
                                (old, true, false)
                            }
                            (Some(_) | None, Some(new)) => {
                                let _new = next_iter.next();
                                (new, false, true)
                            }
                            (Some(old), None) => {
                                let _old = prior_iter.next();
                                (old, true, false)
                            }
                        };
                        let (gram, was, now) = change;
                        context.budget.charge(WorkUsage {
                            mutations: 1,
                            bytes: 128,
                            ..WorkUsage::default()
                        })?;
                        spool.posting(
                            gram,
                            &MapEdit {
                                key: owner.clone(),
                                expected: was.then_some(marker),
                                next: now.then_some(marker),
                            },
                            context.budget,
                        )?;
                    }
                    match next {
                        Some(value) if prior.as_ref() != Some(&value) => {
                            let estimated_bytes = u64::try_from(value.grams.len())
                                .map_err(|_conversion_error| IndexError::ResourceLimited)?
                                .checked_mul(16)
                                .and_then(|bytes| bytes.checked_add(768))
                                .ok_or(IndexError::ResourceLimited)?;
                            context.budget.charge(WorkUsage {
                                bytes: estimated_bytes,
                                ..WorkUsage::default()
                            })?;
                            let payload = encode(&value)?;
                            let wire_bytes = u64::try_from(payload.len())
                                .map_err(|_conversion_error| IndexError::ResourceLimited)?
                                .checked_add(16 + 32)
                                .ok_or(IndexError::ResourceLimited)?;
                            if wire_bytes > CONTRIBUTION_BATCH_WIRE_BYTES {
                                return Err(IndexError::ResourceLimited);
                            }
                            if pending_contribution_wire_bytes
                                .checked_add(wire_bytes)
                                .ok_or(IndexError::ResourceLimited)?
                                > CONTRIBUTION_BATCH_WIRE_BYTES
                            {
                                flush_contributions(context, &mut pending_contributions, &spool)?;
                                pending_contribution_wire_bytes = 0;
                            }
                            pending_contributions.push(PendingContribution {
                                owner,
                                prior: prior_ref,
                                input: ObjectBatchInput {
                                    payload,
                                    children: vec![value.record],
                                },
                            });
                            pending_contribution_wire_bytes = pending_contribution_wire_bytes
                                .checked_add(wire_bytes)
                                .ok_or(IndexError::ResourceLimited)?;
                            if pending_contributions.len() == CONTRIBUTION_BATCH_OBJECTS {
                                flush_contributions(context, &mut pending_contributions, &spool)?;
                                pending_contribution_wire_bytes = 0;
                            }
                        }
                        None if prior_ref.is_some() => spool.edit(
                            1,
                            &MapEdit {
                                key: owner,
                                expected: prior_ref,
                                next: None,
                            },
                            context.budget,
                        )?,
                        Some(_) | None => {}
                    }
                }
                flush_contributions(context, &mut pending_contributions, &spool)?;
                let mut after_gram = None;
                loop {
                    let grams = spool.gram_page(after_gram, context.budget)?;
                    if grams.is_empty() {
                        break;
                    }
                    after_gram = grams.last().copied();
                    context.budget.checkpoint()?;
                    context.budget.charge(WorkUsage {
                        bytes: u64::try_from(POSTING_BATCH_ROOTS)
                            .map_err(|_conversion_error| IndexError::ResourceLimited)?
                            .checked_mul(512)
                            .ok_or(IndexError::ResourceLimited)?,
                        ..WorkUsage::default()
                    })?;
                    let mut metadata = Vec::with_capacity(POSTING_BATCH_ROOTS);
                    let mut bases = Vec::with_capacity(POSTING_BATCH_ROOTS);
                    for gram in &grams {
                        let gram = *gram;
                        let old_wrapper = context.maps.lookup(root.grams, &gram, context.budget)?;
                        let posting = old_wrapper
                            .map(|reference| {
                                context
                                    .objects
                                    .get(reference, context.budget)
                                    .and_then(|stored| decode::<Posting>(&stored.payload))
                            })
                            .transpose()?
                            .unwrap_or_default();
                        let count = spool.cardinality(gram, posting.count, context.budget)?;
                        metadata.push((gram, old_wrapper, count));
                        bases.push(posting.root);
                    }
                    let roots = context.apply_stream(&bases, &mut spool.posting_cursor(grams))?;
                    if roots.len() != metadata.len() {
                        return Err(IndexError::BackendViolation(
                            "map batch lost posting output".into(),
                        ));
                    }
                    let mut wrappers = Vec::new();
                    let mut wrapper_keys = Vec::new();
                    for ((gram, old_wrapper, count), new_root) in metadata.into_iter().zip(roots) {
                        if new_root.0.is_none() != (count == 0) {
                            return Err(IndexError::CorruptObject(
                                "posting cardinality and root disagree".into(),
                            ));
                        }
                        if let Some(reference) = new_root.0 {
                            wrappers.push(ObjectBatchInput {
                                payload: encode(&Posting {
                                    root: new_root,
                                    count,
                                })?,
                                children: vec![reference],
                            });
                            wrapper_keys.push((gram, old_wrapper));
                        } else {
                            spool.edit(
                                0,
                                &MapEdit {
                                    key: gram.to_vec(),
                                    expected: old_wrapper,
                                    next: None,
                                },
                                context.budget,
                            )?;
                        }
                    }
                    let wrapper_refs = context.put_batch(&wrappers)?;
                    if wrapper_refs.len() != wrapper_keys.len() {
                        return Err(IndexError::BackendViolation(
                            "object batch lost posting wrapper".into(),
                        ));
                    }
                    for ((gram, old_wrapper), next) in wrapper_keys.into_iter().zip(wrapper_refs) {
                        spool.edit(
                            0,
                            &MapEdit {
                                key: gram.to_vec(),
                                expected: old_wrapper,
                                next: Some(next),
                            },
                            context.budget,
                        )?;
                    }
                }
                let roots =
                    context.apply_stream(&[root.grams, root.owners], &mut spool.top_cursor())?;
                let mut roots = roots.into_iter();
                root.grams = roots.next().ok_or_else(|| {
                    IndexError::BackendViolation("gram root batch missing".into())
                })?;
                root.owners = roots.next().ok_or_else(|| {
                    IndexError::BackendViolation("owner root batch missing".into())
                })?;
                if roots.next().is_some() {
                    return Err(IndexError::BackendViolation(
                        "root batch returned extra output".into(),
                    ));
                }
            }
            let payload = encode(&root)?;
            let artifact = context.put(&payload, &roots(root))?;
            Ok(LiteralCandidate {
                binding: MemberBinding {
                    instance: plan.context.instance.instance,
                    format: plan.context.instance.format,
                    artifact,
                    input: plan.context.input,
                    logical_commitment: Digest(Sha256::digest(&payload).into()),
                },
            })
        })
    }

    fn verify_and_seal<'a>(
        &'a self,
        candidate: LiteralCandidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, MemberBinding> {
        Box::pin(async move {
            let stored = context
                .objects
                .get(candidate.binding.artifact, context.budget)?;
            let root = decode::<Root>(&stored.payload)?;
            if stored.registration.children != roots(root)
                || Digest(Sha256::digest(&stored.payload).into())
                    != candidate.binding.logical_commitment
            {
                return Err(IndexError::BackendViolation(
                    "literal root commitment mismatch".into(),
                ));
            }
            Ok(candidate.binding)
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiteralHit {
    pub owner: OwnerKey,
    pub record: SourceRecord,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiteralPage {
    pub hits: Vec<LiteralHit>,
    pub after: Option<Vec<u8>>,
    /// Unexamined candidates remain. This is a continuation guarantee, not a
    /// claim that another candidate will satisfy the exact byte predicate.
    pub has_more: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct LiteralQuery<'a> {
    pub literal: &'a [u8],
    pub after: Option<&'a [u8]>,
    pub limit: u32,
}

pub struct LiteralReadContext<'a> {
    pub ticket: &'a TicketRecord,
    pub input: InputBinding,
    pub objects: &'a dyn ImmutableObjectPort,
    pub maps: &'a dyn PersistentMapPort,
    pub budget: &'a mut WorkBudget,
}

impl NativeLiteralIndex {
    /// Exact immutable-byte verification. Short patterns use a bounded owner scan;
    /// broad searches terminate with `ResourceLimited` rather than false completeness.
    pub fn search(
        &self,
        member: &MemberBinding,
        query: LiteralQuery<'_>,
        context: LiteralReadContext<'_>,
    ) -> Result<LiteralPage, IndexError> {
        let LiteralQuery {
            literal,
            after,
            limit,
        } = query;
        let LiteralReadContext {
            ticket,
            input,
            objects,
            maps,
            budget,
        } = context;
        let mut member_key = b"member\0".to_vec();
        member_key.extend_from_slice(&member.instance.0);
        let member_ref = maps
            .lookup(ticket.view.members, &member_key, budget)?
            .ok_or(IndexError::SnapshotUnavailable)?;
        let published = decode::<MemberBinding>(&objects.get(member_ref, budget)?.payload)?;
        if &published != member || input.root != ticket.view.source.owners {
            return Err(IndexError::Conflict);
        }
        if member.input != input || literal.is_empty() || limit == 0 || limit > 1024 {
            return Err(IndexError::InvalidInput(
                "invalid literal query binding or page".into(),
            ));
        }
        let artifact = objects.get(member.artifact, budget)?;
        let root = decode::<Root>(&artifact.payload)?;
        if artifact.registration.children != roots(root)
            || Digest(Sha256::digest(&artifact.payload).into()) != member.logical_commitment
        {
            return Err(IndexError::CorruptObject(
                "literal member commitment mismatch".into(),
            ));
        }
        let mut posting_roots = Vec::new();
        for gram in all_grams(literal, budget)? {
            let Some(reference) = maps.lookup(root.grams, &gram, budget)? else {
                return Ok(LiteralPage {
                    hits: Vec::new(),
                    after: None,
                    has_more: false,
                });
            };
            let posting = decode::<Posting>(&objects.get(reference, budget)?.payload)?;
            if posting.count == 0 || posting.root.0.is_none() {
                return Err(IndexError::CorruptObject(
                    "empty posting retained in gram index".into(),
                ));
            }
            posting_roots.push(posting);
        }
        posting_roots.sort_unstable_by_key(|posting| posting.count);
        let candidates = posting_roots
            .first()
            .map_or(root.owners, |posting| posting.root);
        let mut cursor = after.map(<[u8]>::to_vec);
        let mut hits = Vec::new();
        loop {
            let page = maps.scan(candidates, cursor.as_deref(), 64, budget)?;
            for (position, (key, _)) in page.entries.iter().enumerate() {
                cursor = Some(key.clone());
                let mut present = true;
                for posting in &posting_roots {
                    if maps.lookup(posting.root, key, budget)?.is_none() {
                        present = false;
                        break;
                    }
                }
                if !present {
                    continue;
                }
                let reference = maps
                    .lookup(root.owners, key, budget)?
                    .ok_or_else(|| IndexError::CorruptObject("posting without owner".into()))?;
                let contribution =
                    decode::<Contribution>(&objects.get(reference, budget)?.payload)?;
                let source = objects.get(contribution.record, budget)?;
                let record = decode::<SourceRecord>(&source.payload)?;
                if source.registration.children.as_slice() != [record.bytes] {
                    return Err(IndexError::CorruptObject(
                        "literal source edges mismatch".into(),
                    ));
                }
                let body = objects.get(record.bytes, budget)?.payload;
                if u64::try_from(body.len())
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?
                    != record.byte_len
                {
                    return Err(IndexError::CorruptObject(
                        "literal source length mismatch".into(),
                    ));
                }
                if body.windows(literal.len()).any(|window| window == literal) {
                    let owner = decode_owner(key)?;
                    hits.push(LiteralHit { owner, record });
                    if hits.len()
                        == usize::try_from(limit)
                            .map_err(|_conversion_error| IndexError::ResourceLimited)?
                    {
                        let has_more = page.has_more
                            || position
                                .checked_add(1)
                                .is_some_and(|next| next < page.entries.len());
                        return Ok(LiteralPage {
                            hits,
                            after: has_more.then_some(cursor).flatten(),
                            has_more,
                        });
                    }
                }
            }
            if !page.has_more {
                return Ok(LiteralPage {
                    hits,
                    after: None,
                    has_more: false,
                });
            }
            if page.entries.is_empty() {
                return Err(IndexError::BackendViolation(
                    "nonprogressing map cursor".into(),
                ));
            }
        }
    }
}

fn decode_owner(bytes: &[u8]) -> Result<OwnerKey, IndexError> {
    let producer = bytes
        .get(..16)
        .ok_or_else(|| IndexError::CorruptObject("truncated owner key".into()))?;
    let stable = bytes
        .get(16..32)
        .filter(|_| bytes.len() == 32)
        .ok_or_else(|| IndexError::CorruptObject("invalid owner key".into()))?;
    Ok(OwnerKey {
        producer: quanta_index_core::index::ProducerId(
            producer.try_into().map_err(|_conversion_error| {
                IndexError::CorruptObject("invalid producer".into())
            })?,
        ),
        stable_id: quanta_index_core::index::StableOwnerId(
            stable
                .try_into()
                .map_err(|_conversion_error| IndexError::CorruptObject("invalid owner".into()))?,
        ),
    })
}
