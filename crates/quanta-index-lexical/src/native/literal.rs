//! Owner-local byte-literal index for the native kernel. The query capability is
//! explicitly case-sensitive byte literals; token/ranking/regex routes stay separate.
use super::edit_spool::LiteralEditSpool;
use super::literal_contribution::LoadedContribution;
use quanta_index_core::index::{
    AttemptContext, BaseMember, DeltaInput, Digest, IndexError, InputBinding, MapRoot,
    MemberBinding, MemoryLease, MemoryLifetime, ObjectBatchInput, ObjectChildren, ObjectRef,
    OwnerKey, PlanContext, PlanDecision, SourceDelta, SourceInputCursor, SourceRecord,
    SourceRecordOwner, StagedObjectReferences, StoredObject, TicketRecord, WorkBudget, WorkUsage,
    member_map_key,
    ports::{ImmutableObjectPort, IndexBuildPort, IndexFuture, PersistentMapPort},
};
#[cfg(test)]
use serde::Serialize;
use sha2::{Digest as _, Sha256};

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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Posting {
    root: MapRoot,
    count: u64,
}
quanta_index_contract::native_serde_struct!(Posting {
    root: MapRoot,
    count: u64,
});

fn load_posting(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<Posting, IndexError> {
    budget.checkpoint()?;
    let stored = objects.get(reference, budget)?;
    budget.checkpoint()?;
    if stored.registration.reference != reference || stored.registration.children.len() != 1 {
        return Err(IndexError::CorruptObject(
            "invalid literal posting object or child count".into(),
        ));
    }
    let posting = super::metadata::decode_fixed::<Posting>(&stored.payload, budget)?;
    let Some(root) = posting.root.0 else {
        return Err(IndexError::CorruptObject(
            "empty posting retained in gram index".into(),
        ));
    };
    if posting.count == 0 || stored.registration.children.as_slice() != [root] {
        return Err(IndexError::CorruptObject(
            "literal posting count or root edge mismatch".into(),
        ));
    }
    Ok(posting)
}

fn load_literal_record(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<SourceRecordOwner, IndexError> {
    budget.checkpoint()?;
    let source = objects.get(reference, budget)?;
    budget.checkpoint()?;
    if source.registration.reference != reference || source.registration.children.len() != 1 {
        return Err(IndexError::CorruptObject(
            "literal source identity or child count mismatch".into(),
        ));
    }
    let record =
        SourceRecordOwner::decode_json(&source.payload, budget, MemoryLifetime::Temporary)?;
    if source.registration.children.as_slice() != [record.bytes] {
        return Err(IndexError::CorruptObject(
            "literal source edges mismatch".into(),
        ));
    }
    Ok(record)
}

fn load_literal_body(
    record: &SourceRecord,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<StoredObject, IndexError> {
    budget.checkpoint()?;
    let body = objects.get(record.bytes, budget)?;
    budget.checkpoint()?;
    if body.registration.reference != record.bytes
        || !body.registration.children.is_empty()
        || u64::try_from(body.payload.len()).map_err(|_error| IndexError::ResourceLimited)?
            != record.byte_len
    {
        return Err(IndexError::CorruptObject(
            "literal source body identity, edges or length mismatch".into(),
        ));
    }
    Ok(body)
}

struct PendingObject<K> {
    key: K,
    prior: Option<ObjectRef>,
    input: Option<ObjectBatchInput>,
    _payload_memory: Option<MemoryLease>,
    _children_memory: Option<MemoryLease>,
}

struct LiteralBatch<K> {
    rows: super::text_memory::PreparedRows<PendingObject<K>>,
}

impl<K: Copy> LiteralBatch<K> {
    fn new(maximum: usize) -> Result<Self, IndexError> {
        Ok(Self {
            rows: super::text_memory::PreparedRows::new(maximum)?,
        })
    }

    fn push(
        &mut self,
        key: K,
        prior: Option<ObjectRef>,
        payload: super::text_memory::PreparedPayload,
        child: ObjectRef,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        let memory = budget.reserve_memory(
            u64::try_from(std::mem::size_of::<ObjectRef>())
                .map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut children = Vec::new();
        children
            .try_reserve_exact(1)
            .map_err(|_error| IndexError::ResourceLimited)?;
        children.push(child);
        let (bytes, payload_memory) = payload.into_parts();
        self.rows.push(
            PendingObject {
                key,
                prior,
                input: Some(ObjectBatchInput {
                    payload: bytes,
                    children,
                }),
                _payload_memory: payload_memory,
                _children_memory: memory,
            },
            budget,
        )
    }

    fn install(
        &mut self,
        context: &mut AttemptContext<'_>,
    ) -> Result<StagedObjectReferences, IndexError> {
        context.budget.checkpoint()?;
        if self.rows.as_slice().iter().any(|row| row.input.is_none()) {
            return Err(IndexError::BackendViolation(
                "literal batch inputs were already consumed".into(),
            ));
        }
        let bytes = self
            .rows
            .len()
            .checked_mul(std::mem::size_of::<ObjectBatchInput>())
            .ok_or(IndexError::ResourceLimited)?;
        let _memory = if bytes == 0 {
            None
        } else {
            context.budget.reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?
        };
        let mut inputs = Vec::new();
        inputs
            .try_reserve_exact(self.rows.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        for row in self.rows.as_mut_slice() {
            inputs.push(
                row.input
                    .take()
                    .ok_or_else(|| IndexError::BackendViolation("literal input missing".into()))?,
            );
        }
        // Row grants outlive this separate input array, including install errors.
        context.put_batch(&inputs)
    }
}

#[derive(Clone, Debug)]
pub struct LiteralPlan {
    base: Option<MemberBinding>,
    context: PlanContext,
    input: DeltaInput<SourceDelta>,
}

#[derive(Clone, Debug)]
pub struct LiteralCandidate {
    pub binding: MemberBinding,
}

#[cfg(test)]
fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, IndexError> {
    serde_json::to_vec(value).map_err(|error| IndexError::InvalidInput(error.to_string()))
}
fn owner_key(owner: OwnerKey) -> [u8; 32] {
    let mut key = [0; 32];
    for (target, byte) in key
        .iter_mut()
        .zip(owner.producer.0.iter().chain(owner.stable_id.0.iter()))
    {
        *target = *byte;
    }
    key
}
fn roots(root: Root) -> ObjectChildren {
    match [root.owners.0, root.grams.0] {
        [None, None] => ObjectChildren::Empty,
        [Some(value), None] | [None, Some(value)] => ObjectChildren::One([value]),
        [Some(first), Some(second)] => ObjectChildren::Two([first, second]),
    }
}
fn flush_contributions(
    context: &mut AttemptContext<'_>,
    pending: &mut LiteralBatch<[u8; 32]>,
    spool: &LiteralEditSpool,
) -> Result<(), IndexError> {
    if pending.rows.len() == 0 {
        return Ok(());
    }
    context.budget.checkpoint()?;
    let count = pending.rows.len();
    context.budget.charge(WorkUsage {
        bytes: u64::try_from(count)
            .map_err(|_conversion_error| IndexError::ResourceLimited)?
            .checked_mul(256)
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let references = pending.install(context)?;
    if references.len() != pending.rows.len() {
        return Err(IndexError::BackendViolation(
            "contribution batch lost ordinal output".into(),
        ));
    }
    for (row, next) in pending.rows.as_slice().iter().zip(references) {
        let prior = row.prior;
        if prior != Some(next) {
            spool.edit(1, &row.key, prior, Some(next), context.budget)?;
        }
    }
    pending.rows.clear();
    Ok(())
}

impl IndexBuildPort for NativeLiteralIndex {
    type Delta = SourceDelta;
    type Plan = LiteralPlan;
    type Candidate = LiteralCandidate;

    fn resource_demand(&self) -> quanta_index_core::index::effort::ResourceNeed {
        quanta_index_core::index::effort::ResourceNeed::default()
    }

    fn validate_instance(
        &self,
        instance: &quanta_index_core::index::InstanceSpec,
    ) -> Result<(), IndexError> {
        if instance.activation != quanta_index_core::index::GroupActivation::CoreRequired
            || instance.input_domain != quanta_index_core::index::InputDomain::SourceLocal
        {
            return Err(IndexError::InvalidInput(
                "adapter requires a core source-local instance".into(),
            ));
        }
        Ok(())
    }

    fn plan(
        &self,
        base: BaseMember,
        input: DeltaInput<SourceDelta>,
        context: &PlanContext,
        budget: &mut WorkBudget,
    ) -> Result<PlanDecision<LiteralPlan>, IndexError> {
        budget.checkpoint()?;
        let (base, input) = match (base, input) {
            (BaseMember::Absent, DeltaInput::Bootstrap)
                if context.input.root == context.source.owners =>
            {
                (None, DeltaInput::Bootstrap)
            }
            (BaseMember::Present(member), DeltaInput::Patch(delta)) => {
                if member.instance != context.instance.instance
                    || member.format != context.instance.format
                    || member.input.schema != context.input.schema
                    || member.input.policy != context.input.policy
                    || member.input.root != delta.prior.owners
                    || delta.next != context.source
                    || context.input.root != delta.next.owners
                {
                    return Err(IndexError::Conflict);
                }
                (Some(*member), DeltaInput::Patch(delta))
            }
            (
                BaseMember::Absent | BaseMember::Present(_),
                DeltaInput::Bootstrap | DeltaInput::Patch(_),
            ) => return Ok(PlanDecision::RebuildRequired),
        };
        Ok(PlanDecision::Incremental(LiteralPlan {
            base,
            context: context.clone(),
            input,
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
                    let stored = context.objects.get(base.artifact, context.budget)?;
                    super::metadata::decode_fixed::<Root>(&stored.payload, context.budget)?
                }
                None => Root::default(),
            };
            if !matches!(&plan.input, DeltaInput::Bootstrap)
                || plan.context.source.owners.0.is_some()
            {
                // Compile the bounded source cursor before installing map roots.
                // Shared grams receive one posting edit batch, not an intermediate
                // root per owner; initial indexing benefits from the same path.
                let mut changes = SourceInputCursor::new(
                    plan.context.source,
                    plan.input.clone(),
                    context.maps,
                    context.objects,
                )?;
                let mut next_change = changes.next_change(context.budget)?;
                if next_change.is_none()
                    && let (Some(base), DeltaInput::Patch(delta)) = (&plan.base, &plan.input)
                {
                    if delta.prior.owners != delta.next.owners {
                        return Err(IndexError::CorruptObject(
                            "empty native literal patch changed owner root".into(),
                        ));
                    }
                    return Ok(LiteralCandidate {
                        binding: MemberBinding {
                            instance: plan.context.instance.instance,
                            format: plan.context.instance.format,
                            artifact: base.artifact,
                            input: plan.context.input,
                            logical_commitment: base.logical_commitment,
                        },
                    });
                }
                let spool = LiteralEditSpool::new(context.scratch, context.budget)?;
                let mut pending_contributions = LiteralBatch::new(CONTRIBUTION_BATCH_OBJECTS)?;
                let mut pending_contribution_wire_bytes = 0u64;
                let marker = context.put(b"native-literal-owner-membership", &[])?;
                while let Some(change) = next_change {
                    context.budget.checkpoint()?;
                    let owner = owner_key(change.owner);
                    let prior_ref =
                        context
                            .maps
                            .lookup_object(root.owners, &owner, context.budget)?;
                    let prior = prior_ref
                        .map(|reference| {
                            super::literal_contribution::load(
                                reference,
                                context.objects,
                                context.budget,
                            )
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
                                super::literal_grams::PreparedGrams::copy(
                                    &old.grams,
                                    context.budget,
                                )?
                            }
                            _ => super::literal_grams::build(
                                &context.objects.get(record.bytes, context.budget)?.payload,
                                context.budget,
                            )?,
                        };
                        Some(LoadedContribution::new(reference, grams))
                    } else {
                        None
                    };
                    let prior_grams = prior.as_ref().map_or(&[][..], |value| &value.grams);
                    let next_grams = next.as_ref().map_or(&[][..], |value| &value.grams);
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
                            &owner,
                            was.then_some(marker),
                            now.then_some(marker),
                            context.budget,
                        )?;
                    }
                    match next {
                        Some(value) if prior.as_deref() != Some(&*value) => {
                            let estimated_bytes = u64::try_from(value.grams.len())
                                .map_err(|_conversion_error| IndexError::ResourceLimited)?
                                .checked_mul(16)
                                .and_then(|bytes| bytes.checked_add(768))
                                .ok_or(IndexError::ResourceLimited)?;
                            context.budget.charge(WorkUsage {
                                bytes: estimated_bytes,
                                ..WorkUsage::default()
                            })?;
                            let payload = super::text_memory::encode(&*value, context.budget)?;
                            let wire_bytes = u64::try_from(payload.as_slice().len())
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
                            pending_contributions.push(
                                owner,
                                prior_ref,
                                payload,
                                value.record,
                                context.budget,
                            )?;
                            pending_contribution_wire_bytes = pending_contribution_wire_bytes
                                .checked_add(wire_bytes)
                                .ok_or(IndexError::ResourceLimited)?;
                            if pending_contributions.rows.len() == CONTRIBUTION_BATCH_OBJECTS {
                                flush_contributions(context, &mut pending_contributions, &spool)?;
                                pending_contribution_wire_bytes = 0;
                            }
                        }
                        None if prior_ref.is_some() => {
                            spool.edit(1, &owner, prior_ref, None, context.budget)?;
                        }
                        Some(_) | None => {}
                    }
                    next_change = changes.next_change(context.budget)?;
                }
                flush_contributions(context, &mut pending_contributions, &spool)?;
                let mut after_gram = None;
                loop {
                    let grams = spool.gram_page(after_gram, context.budget)?;
                    if grams.len() == 0 {
                        break;
                    }
                    after_gram = grams.as_slice().last().copied();
                    context.budget.checkpoint()?;
                    context.budget.charge(WorkUsage {
                        bytes: u64::try_from(POSTING_BATCH_ROOTS)
                            .map_err(|_conversion_error| IndexError::ResourceLimited)?
                            .checked_mul(512)
                            .ok_or(IndexError::ResourceLimited)?,
                        ..WorkUsage::default()
                    })?;
                    let mut metadata = super::text_memory::PreparedRows::new(POSTING_BATCH_ROOTS)?;
                    let mut bases = super::text_memory::PreparedRows::new(POSTING_BATCH_ROOTS)?;
                    for gram in grams.as_slice() {
                        let gram = *gram;
                        let old_wrapper =
                            context
                                .maps
                                .lookup_object(root.grams, &gram, context.budget)?;
                        let posting = old_wrapper
                            .map(|reference| {
                                load_posting(reference, context.objects, context.budget)
                            })
                            .transpose()?
                            .unwrap_or_default();
                        let count = spool.cardinality(gram, posting.count, context.budget)?;
                        metadata.push((gram, old_wrapper, count), context.budget)?;
                        bases.push(posting.root, context.budget)?;
                    }
                    let roots = context.apply_stream(
                        bases.as_slice(),
                        &mut spool.posting_cursor(grams.as_slice()),
                    )?;
                    if roots.len() != metadata.len() {
                        return Err(IndexError::BackendViolation(
                            "map batch lost posting output".into(),
                        ));
                    }
                    let mut wrappers = LiteralBatch::new(POSTING_BATCH_ROOTS)?;
                    for ((gram, old_wrapper, count), new_root) in
                        metadata.as_slice().iter().copied().zip(roots)
                    {
                        if new_root.0.is_none() != (count == 0) {
                            return Err(IndexError::CorruptObject(
                                "posting cardinality and root disagree".into(),
                            ));
                        }
                        if let Some(reference) = new_root.0 {
                            let payload = super::text_memory::encode(
                                &Posting {
                                    root: new_root,
                                    count,
                                },
                                context.budget,
                            )?;
                            wrappers.push(gram, old_wrapper, payload, reference, context.budget)?;
                        } else {
                            spool.edit(0, &gram, old_wrapper, None, context.budget)?;
                        }
                    }
                    let wrapper_refs = wrappers.install(context)?;
                    if wrapper_refs.len() != wrappers.rows.len() {
                        return Err(IndexError::BackendViolation(
                            "object batch lost posting wrapper".into(),
                        ));
                    }
                    for (row, next) in wrappers.rows.as_slice().iter().zip(wrapper_refs) {
                        spool.edit(0, &row.key, row.prior, Some(next), context.budget)?;
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
            let payload = super::text_memory::encode(&root, context.budget)?;
            let artifact = context.put(payload.as_slice(), roots(root).as_slice())?;
            Ok(LiteralCandidate {
                binding: MemberBinding {
                    instance: plan.context.instance.instance,
                    format: plan.context.instance.format,
                    artifact,
                    input: plan.context.input,
                    logical_commitment: Digest(Sha256::digest(payload.as_slice()).into()),
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
            let root = super::metadata::decode_fixed::<Root>(&stored.payload, context.budget)?;
            if stored.registration.children.as_slice() != roots(root).as_slice()
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

#[derive(Debug)]
pub struct LiteralHit {
    pub owner: OwnerKey,
    pub record: SourceRecord,
    record_memory: Option<MemoryLease>,
}

impl LiteralHit {
    /// The producer admits the owned path before constructing an escaping hit.
    #[must_use]
    pub fn from_parts(owner: OwnerKey, record: SourceRecord, memory: Option<MemoryLease>) -> Self {
        Self {
            owner,
            record,
            record_memory: memory,
        }
    }

    #[must_use]
    pub fn into_parts(self) -> (OwnerKey, SourceRecord, Option<MemoryLease>) {
        (self.owner, self.record, self.record_memory)
    }
}

impl PartialEq for LiteralHit {
    fn eq(&self, other: &Self) -> bool {
        self.owner == other.owner && self.record == other.record
    }
}
impl Eq for LiteralHit {}

#[derive(Debug)]
pub struct LiteralPage {
    pub hits: Vec<LiteralHit>,
    pub after: Option<Vec<u8>>,
    /// Unexamined candidates remain. This is a continuation guarantee, not a
    /// claim that another candidate will satisfy the exact byte predicate.
    pub has_more: bool,
    hit_memory: Option<MemoryLease>,
    after_memory: Option<MemoryLease>,
}

/// One handoff of canonical page data and its two allocation charges.
pub type LiteralPageParts = (
    Vec<LiteralHit>,
    Option<Vec<u8>>,
    bool,
    Option<MemoryLease>,
    Option<MemoryLease>,
);

impl LiteralPage {
    /// Input backing and any returned cursor must already have retained admission.
    #[must_use]
    pub fn from_parts(
        hits: Vec<LiteralHit>,
        after: Option<Vec<u8>>,
        has_more: bool,
        hit_memory: Option<MemoryLease>,
        after_memory: Option<MemoryLease>,
    ) -> Self {
        Self {
            hits,
            after,
            has_more,
            hit_memory,
            after_memory,
        }
    }

    #[must_use]
    pub fn into_parts(self) -> LiteralPageParts {
        (
            self.hits,
            self.after,
            self.has_more,
            self.hit_memory,
            self.after_memory,
        )
    }
}

impl PartialEq for LiteralPage {
    fn eq(&self, other: &Self) -> bool {
        self.hits == other.hits && self.after == other.after && self.has_more == other.has_more
    }
}
impl Eq for LiteralPage {}

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
        let member_key = member_map_key(member.instance);
        let member_ref = maps
            .lookup_object(ticket.view.members, &member_key, budget)?
            .ok_or(IndexError::SnapshotUnavailable)?;
        let stored_member = objects.get(member_ref, budget)?;
        let published =
            quanta_index_core::index::decode_member_binding(&stored_member.payload, budget)?;
        if &published != member || input.root != ticket.view.source.owners {
            return Err(IndexError::Conflict);
        }
        if member.input != input || literal.is_empty() || limit == 0 || limit > 1024 {
            return Err(IndexError::InvalidInput(
                "invalid literal query binding or page".into(),
            ));
        }
        let artifact = objects.get(member.artifact, budget)?;
        let root = super::metadata::decode_fixed::<Root>(&artifact.payload, budget)?;
        if !artifact
            .registration
            .children
            .iter()
            .copied()
            .eq([root.owners.0, root.grams.0].into_iter().flatten())
            || Digest(Sha256::digest(&artifact.payload).into()) != member.logical_commitment
        {
            return Err(IndexError::CorruptObject(
                "literal member commitment mismatch".into(),
            ));
        }
        // Sort one admitted contiguous gram array. WorkUsage charges work;
        // it cannot substitute for the live backing owned in this scope.
        let gram_count = literal.len().saturating_sub(2);
        let gram_bytes = gram_count
            .checked_mul(std::mem::size_of::<[u8; 3]>())
            .ok_or(IndexError::ResourceLimited)?;
        let gram_memory = query_memory(gram_bytes, MemoryLifetime::Temporary, budget)?;
        let mut grams = Vec::new();
        grams
            .try_reserve_exact(gram_count)
            .map_err(|_error| IndexError::ResourceLimited)?;
        for window in literal.windows(3) {
            budget.checkpoint()?;
            grams
                .push(<[u8; 3]>::try_from(window).map_err(|_error| {
                    IndexError::BackendViolation("invalid byte window".into())
                })?);
        }
        grams.sort_unstable();
        grams.dedup();
        if grams.len() > 262_144 {
            return Err(IndexError::ResourceLimited);
        }
        let posting_bytes = grams
            .len()
            .checked_mul(std::mem::size_of::<Posting>())
            .ok_or(IndexError::ResourceLimited)?;
        let _posting_memory = query_memory(posting_bytes, MemoryLifetime::Temporary, budget)?;
        let mut posting_roots = Vec::new();
        posting_roots
            .try_reserve_exact(grams.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        for gram in &grams {
            budget.charge(WorkUsage {
                mutations: 1,
                bytes: 128,
                ..WorkUsage::default()
            })?;
            let Some(reference) = maps.lookup_object(root.grams, gram, budget)? else {
                return Ok(LiteralPage::from_parts(Vec::new(), None, false, None, None));
            };
            let posting = load_posting(reference, objects, budget)?;
            posting_roots.push(posting);
        }
        drop(grams);
        drop(gram_memory);
        posting_roots.sort_unstable_by_key(|posting| posting.count);
        let candidates = posting_roots
            .first()
            .map_or(root.owners, |posting| posting.root);
        let mut cursor = after.map(owner_cursor).transpose()?;
        let limit = usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?;
        let hit_bytes = limit
            .checked_mul(std::mem::size_of::<LiteralHit>())
            .ok_or(IndexError::ResourceLimited)?;
        let hit_memory = query_memory(hit_bytes, MemoryLifetime::Retained, budget)?;
        let mut hits = Vec::new();
        hits.try_reserve_exact(limit)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut result = LiteralPage::from_parts(hits, None, false, hit_memory, None);
        loop {
            let page = maps.scan(
                candidates,
                cursor.as_ref().map(<[u8; 32]>::as_slice),
                64,
                budget,
            )?;
            for (position, (key, _)) in page.entries.iter().enumerate() {
                cursor = Some(owner_cursor(key)?);
                let mut present = true;
                for posting in &posting_roots {
                    if maps.lookup_object(posting.root, key, budget)?.is_none() {
                        present = false;
                        break;
                    }
                }
                if !present {
                    continue;
                }
                let reference = maps
                    .lookup_object(root.owners, key, budget)?
                    .ok_or_else(|| IndexError::CorruptObject("posting without owner".into()))?;
                let record_reference =
                    super::literal_contribution::load_record(reference, objects, budget)?;
                let record = load_literal_record(record_reference, objects, budget)?;
                let stored_body = load_literal_body(&record, objects, budget)?;
                let body = &stored_body.payload;
                if body.windows(literal.len()).any(|window| window == literal) {
                    let owner = decode_owner(key)?;
                    let (record, memory) = record.into_retained_parts(budget)?;
                    result
                        .hits
                        .push(LiteralHit::from_parts(owner, record, memory));
                    if result.hits.len() == limit {
                        result.has_more = page.has_more
                            || position
                                .checked_add(1)
                                .is_some_and(|next| next < page.entries.len());
                        if result.has_more {
                            let key = cursor.ok_or_else(|| {
                                IndexError::BackendViolation("literal cursor absent".into())
                            })?;
                            // The key becomes owned output only after retained admission.
                            result.after_memory =
                                query_memory(key.len(), MemoryLifetime::Retained, budget)?;
                            result.after = Some(key.to_vec());
                        }
                        return Ok(result);
                    }
                }
            }
            if !page.has_more {
                return Ok(result);
            }
            if page.entries.is_empty() {
                return Err(IndexError::BackendViolation(
                    "nonprogressing map cursor".into(),
                ));
            }
        }
    }
}

fn owner_cursor(bytes: &[u8]) -> Result<[u8; 32], IndexError> {
    bytes
        .try_into()
        .map_err(|_error| IndexError::CorruptObject("invalid owner cursor".into()))
}

fn query_memory(
    bytes: usize,
    lifetime: MemoryLifetime,
    budget: &WorkBudget,
) -> Result<Option<MemoryLease>, IndexError> {
    if bytes == 0 {
        return Ok(None);
    }
    budget.reserve_memory(
        u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
        lifetime,
    )
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

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::task::{Context, Poll, Waker};
    use std::time::{Duration, Instant};

    use quanta_index_core::index::{
        AttemptId, AttemptRef, CollectionId, GroupActivation, IndexInstanceId, IndexKind,
        InputDomain, InstanceSpec, MapBatchRequest, MapBatchWrite, MapEdit, MapPage, MapWrite,
        OperationKey, OwnerMutationDigest, OwnerMutationSet, ProducerId, PublicationGroupId,
        RegisteredObject, SchemaId, ScratchAllocation, ScratchRequest, ScratchSpacePort, SlotId,
        SourceCut, StorageFormatId, StoredObject, StreamId, TenantId, WorkLimits, WorkspaceKey,
        WorktreeId,
        ports::{MapEditCursor, ObjectCustodyPort, ObjectWriteSink},
    };

    use super::*;

    const SOURCE_WIRE: &[u8] = br#"{"path":"src/lib.rs","bytes":[7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7],"byte_len":3,"text":"Excluded"}"#;

    #[test]
    fn literal_source_readers_bind_record_and_leaf_body_before_predicate() -> Result<(), IndexError>
    {
        let reference = ObjectRef(Digest([9; 32]));
        let body_ref = ObjectRef(Digest([7; 32]));
        let foreign = ObjectRef(Digest([8; 32]));
        let objects = posting_objects(reference, vec![body_ref], SOURCE_WIRE)?;
        let record = load_literal_record(reference, &objects, &mut posting_budget()?)?;
        if record.path != "src/lib.rs" || record.bytes != body_ref || record.byte_len != 3 {
            return Err(IndexError::BackendViolation(
                "literal source fixed gold changed".into(),
            ));
        }
        for (returned, children) in [
            (foreign, vec![body_ref]),
            (reference, vec![]),
            (reference, vec![body_ref, body_ref]),
            (reference, vec![foreign]),
        ] {
            let objects = posting_objects(returned, children, SOURCE_WIRE)?;
            if !matches!(
                load_literal_record(reference, &objects, &mut posting_budget()?),
                Err(IndexError::CorruptObject(_))
            ) || objects.installs.load(Ordering::Relaxed) != 0
            {
                return Err(IndexError::BackendViolation(
                    "literal source object accepted invalid identity or edges".into(),
                ));
            }
        }
        let mut objects = posting_objects(body_ref, vec![], b"abc")?;
        objects.artifact = body_ref;
        let body = load_literal_body(&record, &objects, &mut posting_budget()?)?;
        if body.payload.as_slice() != b"abc" {
            return Err(IndexError::BackendViolation(
                "literal body fixed gold changed".into(),
            ));
        }
        for (returned, children, bytes) in [
            (foreign, vec![], b"abc".as_slice()),
            (body_ref, vec![reference], b"abc".as_slice()),
            (body_ref, vec![], b"ab".as_slice()),
        ] {
            let mut objects = posting_objects(returned, children, bytes)?;
            objects.artifact = body_ref;
            if !matches!(
                load_literal_body(&record, &objects, &mut posting_budget()?),
                Err(IndexError::CorruptObject(_))
            ) || objects.installs.load(Ordering::Relaxed) != 0
            {
                return Err(IndexError::BackendViolation(
                    "literal body accepted invalid identity, edges or length".into(),
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn literal_source_readers_keep_cancel_first_cause_before_and_after_get()
    -> Result<(), IndexError> {
        let reference = ObjectRef(Digest([9; 32]));
        let body_ref = ObjectRef(Digest([7; 32]));
        let record = SourceRecordOwner::decode_json(
            SOURCE_WIRE,
            &posting_budget()?,
            MemoryLifetime::Temporary,
        )?;
        for body in [false, true] {
            for after_read in [false, true] {
                let mut objects =
                    posting_objects(ObjectRef(Digest([8; 32])), vec![reference], b"invalid")?;
                objects.cancel_after_read = after_read;
                if body {
                    objects.artifact = body_ref;
                }
                let mut budget = posting_budget()?;
                if !after_read {
                    budget.request().cancel_handle().cancel();
                }
                let result = if body {
                    load_literal_body(&record, &objects, &mut budget).map(|_body| ())
                } else {
                    load_literal_record(reference, &objects, &mut budget).map(|_record| ())
                };
                if !matches!(result, Err(IndexError::Cancelled))
                    || objects.reads.load(Ordering::Relaxed) != usize::from(after_read)
                    || objects.installs.load(Ordering::Relaxed) != 0
                {
                    return Err(IndexError::BackendViolation(
                        "literal source read renewed original control".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    const POSTING_WIRE: &[u8] =
        br#"{"root":[7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7],"count":3}"#;

    fn posting_objects(
        returned: ObjectRef,
        children: Vec<ObjectRef>,
        wire: &[u8],
    ) -> Result<NoWriteObjects, IndexError> {
        Ok(NoWriteObjects {
            artifact: ObjectRef(Digest([9; 32])),
            stored: StoredObject::new(
                RegisteredObject::unscoped(
                    returned,
                    u64::try_from(wire.len()).map_err(|_error| IndexError::ResourceLimited)?,
                    children,
                ),
                wire.to_vec(),
                None,
            ),
            installs: AtomicUsize::new(0),
            reads: AtomicUsize::new(0),
            cancel_after_read: false,
        })
    }

    fn posting_budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    #[test]
    fn posting_reader_preserves_fixed_wire_and_refuses_wrong_identity_or_edges()
    -> Result<(), IndexError> {
        let reference = ObjectRef(Digest([9; 32]));
        let root = ObjectRef(Digest([7; 32]));
        let foreign = ObjectRef(Digest([8; 32]));
        let objects = posting_objects(reference, vec![root], POSTING_WIRE)?;
        let posting = load_posting(reference, &objects, &mut posting_budget()?)?;
        if posting
            != (Posting {
                root: MapRoot(Some(root)),
                count: 3,
            })
            || objects.reads.load(Ordering::Relaxed) != 1
            || objects.installs.load(Ordering::Relaxed) != 0
        {
            return Err(IndexError::BackendViolation(
                "posting fixed gold changed".into(),
            ));
        }
        for (returned, children) in [
            (foreign, vec![root]),
            (reference, vec![]),
            (reference, vec![root, root]),
            (reference, vec![root, foreign]),
            (reference, vec![foreign]),
        ] {
            let objects = posting_objects(returned, children, POSTING_WIRE)?;
            if !matches!(
                load_posting(reference, &objects, &mut posting_budget()?),
                Err(IndexError::CorruptObject(_))
            ) || objects.reads.load(Ordering::Relaxed) != 1
                || objects.installs.load(Ordering::Relaxed) != 0
            {
                return Err(IndexError::BackendViolation(
                    "invalid posting object was accepted".into(),
                ));
            }
        }
        for wire in [
            br#"{"root":null,"count":3}"#.as_slice(),
            br#"{"root":null,"count":0}"#.as_slice(),
            br#"{"root":[7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7],"count":0}"#.as_slice(),
            br#"{"root":null}"#.as_slice(),
            br#"{"root":null,"count":3,"extra":0}"#.as_slice(),
        ] {
            let objects = posting_objects(reference, vec![root], wire)?;
            if !matches!(load_posting(reference, &objects, &mut posting_budget()?), Err(IndexError::CorruptObject(_))) {
                return Err(IndexError::BackendViolation("empty or malformed posting was accepted".into()));
            }
        }
        Ok(())
    }

    #[test]
    fn posting_reader_preserves_original_control_before_and_after_get() -> Result<(), IndexError> {
        let reference = ObjectRef(Digest([9; 32]));
        let root = ObjectRef(Digest([7; 32]));
        for after_read in [false, true] {
            // Even a corrupt object must not replace a cancellation's first cause.
            let mut objects = posting_objects(ObjectRef(Digest([8; 32])), vec![], b"invalid")?;
            objects.cancel_after_read = after_read;
            let mut budget = posting_budget()?;
            if !after_read {
                budget.request().cancel_handle().cancel();
            }
            if !matches!(
                load_posting(reference, &objects, &mut budget),
                Err(IndexError::Cancelled)
            ) || objects.reads.load(Ordering::Relaxed) != usize::from(after_read)
            {
                return Err(IndexError::BackendViolation(
                    "posting reader renewed original control".into(),
                ));
            }
        }
        let objects = posting_objects(reference, vec![root], POSTING_WIRE)?;
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            Instant::now()
                .checked_sub(Duration::from_secs(1))
                .ok_or(IndexError::ResourceLimited)?,
        );
        if !matches!(
            load_posting(reference, &objects, &mut budget),
            Err(IndexError::ResourceLimited)
        ) || objects.reads.load(Ordering::Relaxed) != 0
        {
            return Err(IndexError::BackendViolation(
                "posting reader ignored original deadline".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn posting_reader_refuses_parser_admission_without_installing_state() -> Result<(), IndexError>
    {
        let reference = ObjectRef(Digest([9; 32]));
        let root = ObjectRef(Digest([7; 32]));
        let objects = posting_objects(reference, vec![root], POSTING_WIRE)?;
        let mut budget = posting_budget()?;
        let calls = Arc::new(AtomicUsize::new(0));
        budget.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
            RefuseMetadataScope {
                expected: u64::try_from(POSTING_WIRE.len())
                    .map_err(|_error| IndexError::ResourceLimited)?
                    * 8
                    + 512,
                calls: Arc::clone(&calls),
            },
        ))));
        if !matches!(
            load_posting(reference, &objects, &mut budget),
            Err(IndexError::ResourceLimited)
        ) || calls.load(Ordering::Relaxed) != 1
            || objects.installs.load(Ordering::Relaxed) != 0
        {
            return Err(IndexError::BackendViolation(
                "posting reader bypassed parser admission".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn literal_inline_keys_and_edges_preserve_order_aliases_and_nulls() -> Result<(), IndexError> {
        let owner = OwnerKey {
            producer: ProducerId(*b"0123456789abcdef"),
            stable_id: quanta_index_core::index::StableOwnerId(*b"FEDCBA9876543210"),
        };
        if owner_key(owner).as_slice() != b"0123456789abcdefFEDCBA9876543210" {
            return Err(IndexError::BackendViolation(
                "literal owner key differs from gold".into(),
            ));
        }
        let first = ObjectRef(Digest([7; 32]));
        let second = ObjectRef(Digest([3; 32]));
        for owners in [None, Some(first)] {
            for grams in [None, Some(first), Some(second)] {
                let root = Root {
                    owners: MapRoot(owners),
                    grams: MapRoot(grams),
                };
                let gold: Vec<_> = [owners, grams].into_iter().flatten().collect();
                if roots(root).as_slice() != gold.as_slice() {
                    return Err(IndexError::BackendViolation(
                        "literal edges changed order or aliases".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    #[test]
    fn literal_batch_refusal_precedes_child_and_install_array_birth() -> Result<(), IndexError> {
        let (plan, _member, objects) = empty_patch_case(OwnerMutationDigest::default().finish())?;
        let maps = NoWriteMaps::default();
        let scratch = NoScratch::default();
        let custody = NoCustody;
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let mut batch = LiteralBatch::new(2)?;
        let payload = super::super::text_memory::encode(&Root::default(), &budget)?;
        let calls = Arc::new(AtomicUsize::new(0));
        budget.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
            RefuseMetadataScope {
                expected: 32,
                calls: Arc::clone(&calls),
            },
        ))));
        if !matches!(
            batch.push([1; 32], None, payload, objects.artifact, &budget),
            Err(IndexError::ResourceLimited)
        ) || batch.rows.len() != 0
            || calls.load(Ordering::Relaxed) != 1
        {
            return Err(IndexError::BackendViolation(
                "literal child refusal changed packet".into(),
            ));
        }
        budget.bind_memory_scope(None);
        let payload = super::super::text_memory::encode(&Root::default(), &budget)?;
        batch.push([1; 32], None, payload, objects.artifact, &budget)?;
        let input_calls = Arc::new(AtomicUsize::new(0));
        budget.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
            RefuseMetadataScope {
                expected: u64::try_from(std::mem::size_of::<ObjectBatchInput>())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                calls: Arc::clone(&input_calls),
            },
        ))));
        let mut context = AttemptContext {
            attempt: AttemptRef {
                operation: OperationKey {
                    workspace: WorkspaceKey {
                        tenant: TenantId([10; 16]),
                        collection: CollectionId([11; 16]),
                        worktree: WorktreeId([12; 16]),
                        slot: SlotId([13; 16]),
                    },
                    epoch: 1,
                    producer: ProducerId([14; 16]),
                    stream: StreamId([15; 16]),
                    stream_epoch: 1,
                    sequence: 1,
                },
                attempt: AttemptId([16; 16]),
                fence: 1,
            },
            objects: &objects,
            maps: &maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut budget,
        };
        if !matches!(
            batch.install(&mut context),
            Err(IndexError::ResourceLimited)
        ) || batch.rows.len() != 1
            || batch.rows.as_slice().iter().any(|row| row.input.is_none())
            || input_calls.load(Ordering::Relaxed) != 1
            || objects.installs.load(Ordering::Relaxed) != 0
            || scratch.allocations.load(Ordering::Relaxed) != 0
            || maps.applies.load(Ordering::Relaxed) != 0
        {
            return Err(IndexError::BackendViolation(
                "literal install-array refusal consumed input or wrote state".into(),
            ));
        }
        // Use the original control for the cancellation check, never a fresh deadline.
        context.budget.request().cancel_handle().cancel();
        if !matches!(batch.install(&mut context), Err(IndexError::Cancelled)) || plan.base.is_none()
        {
            return Err(IndexError::BackendViolation(
                "literal batch replaced original control".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn nonempty_bootstrap_requires_exact_source_input_root() -> Result<(), IndexError> {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(IndexError::BackendViolation(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition)))); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(IndexError::BackendViolation(format!($($message)+))); }
            }};
        }

        let source_root = MapRoot(Some(ObjectRef(Digest([1; 32]))));
        let source = SourceCut {
            owners: source_root,
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        };
        let mut context = PlanContext {
            instance: InstanceSpec {
                instance: IndexInstanceId([2; 16]),
                kind: IndexKind([3; 16]),
                group: PublicationGroupId([4; 16]),
                activation: GroupActivation::CoreRequired,
                input_domain: InputDomain::SourceLocal,
                schema: SchemaId([5; 16]),
                policy: Digest([6; 32]),
                format: StorageFormatId([7; 16]),
            },
            source,
            input: InputBinding {
                root: source_root,
                schema: SchemaId([5; 16]),
                policy: Digest([6; 32]),
                context: Digest([8; 32]),
            },
        };
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?;
        let limits = WorkLimits {
            reads: 100,
            writes: 100,
            bytes: 1_000_000,
            mutations: 100,
        };
        let index = NativeLiteralIndex;
        check!(matches!(
            index.plan(
                BaseMember::Absent,
                DeltaInput::Bootstrap,
                &context,
                &mut WorkBudget::new(limits, deadline),
            ),
            Ok(PlanDecision::Incremental(_))
        ));
        context.input.root = MapRoot(None);
        check!(matches!(
            index.plan(
                BaseMember::Absent,
                DeltaInput::Bootstrap,
                &context,
                &mut WorkBudget::new(limits, deadline),
            ),
            Ok(PlanDecision::RebuildRequired)
        ));
        Ok(())
    }

    struct NoWriteObjects {
        artifact: ObjectRef,
        stored: StoredObject,
        installs: AtomicUsize,
        reads: AtomicUsize,
        cancel_after_read: bool,
    }

    struct RefuseMetadataScope {
        expected: u64,
        calls: Arc<AtomicUsize>,
    }

    impl quanta_index_core::index::MemoryScopePort for RefuseMetadataScope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if bytes != self.expected || lifetime != MemoryLifetime::Temporary {
                return Err(IndexError::BackendViolation(
                    "literal root parser requested unexpected custody".into(),
                ));
            }
            let _previous = self.calls.fetch_add(1, Ordering::Relaxed);
            Err(IndexError::ResourceLimited)
        }
    }

    impl ImmutableObjectPort for NoWriteObjects {
        fn identify(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected object identify".into(),
            ))
        }

        fn put(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("unexpected object put".into()))
        }

        fn install(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            let _previous = self.installs.fetch_add(1, Ordering::Relaxed);
            Err(IndexError::BackendViolation(
                "unexpected object install".into(),
            ))
        }

        fn install_batch(
            &self,
            _: &[ObjectBatchInput],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
            let _previous = self.installs.fetch_add(1, Ordering::Relaxed);
            Err(IndexError::BackendViolation(
                "unexpected object batch".into(),
            ))
        }

        fn get(
            &self,
            reference: ObjectRef,
            budget: &mut WorkBudget,
        ) -> Result<StoredObject, IndexError> {
            budget.checkpoint()?;
            if reference == self.artifact {
                let _previous = self.reads.fetch_add(1, Ordering::Relaxed);
                if self.cancel_after_read {
                    budget.request().cancel_handle().cancel();
                }
                Ok(self.stored.clone())
            } else {
                Err(IndexError::CorruptObject("unexpected test object".into()))
            }
        }
    }

    #[derive(Default)]
    struct NoWriteMaps {
        applies: AtomicUsize,
        empty_diff: Option<(MapRoot, MapRoot)>,
    }

    impl PersistentMapPort for NoWriteMaps {
        fn visit_diff(
            &self,
            before: MapRoot,
            after: MapRoot,
            range: quanta_index_core::index::ports::MapDiffRange<'_>,
            _: &mut dyn quanta_index_core::index::ports::MapDiffVisitor,
            budget: &mut WorkBudget,
        ) -> Result<bool, IndexError> {
            budget.checkpoint()?;
            if self.empty_diff == Some((before, after))
                && range.prefix.is_empty()
                && range.after.is_none()
                && range.limit == 64
            {
                return Ok(false);
            }
            Err(IndexError::BackendViolation(
                "unexpected test structural diff".into(),
            ))
        }

        fn lookup_object(
            &self,
            _: MapRoot,
            _: &[u8],
            _: &mut WorkBudget,
        ) -> Result<Option<ObjectRef>, IndexError> {
            Err(IndexError::BackendViolation("unexpected map lookup".into()))
        }
        fn lookup(
            &self,
            root: MapRoot,
            key: &[u8],
            budget: &mut WorkBudget,
        ) -> Result<Option<quanta_index_core::index::MapValue>, IndexError> {
            self.lookup_object(root, key, budget)
                .map(|value| value.map(Into::into))
        }

        fn scan(
            &self,
            root: MapRoot,
            _: Option<&[u8]>,
            _: u32,
            budget: &mut WorkBudget,
        ) -> Result<MapPage, IndexError> {
            budget.checkpoint()?;
            if root.0.is_some() {
                return Err(IndexError::BackendViolation("unexpected map scan".into()));
            }
            Ok(MapPage::new(Vec::new(), false, Vec::new(), None))
        }

        fn apply(
            &self,
            _: MapRoot,
            _: &[MapEdit],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<MapWrite, IndexError> {
            let _previous = self.applies.fetch_add(1, Ordering::Relaxed);
            Err(IndexError::BackendViolation("unexpected map apply".into()))
        }

        fn apply_batch(
            &self,
            _: &[MapBatchRequest],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<MapBatchWrite, IndexError> {
            let _previous = self.applies.fetch_add(1, Ordering::Relaxed);
            Err(IndexError::BackendViolation("unexpected map batch".into()))
        }

        fn apply_stream(
            &self,
            _: &[MapRoot],
            _: &mut dyn MapEditCursor,
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::StagedMapRoots, IndexError> {
            let _previous = self.applies.fetch_add(1, Ordering::Relaxed);
            Err(IndexError::BackendViolation("unexpected map stream".into()))
        }
    }

    #[derive(Default)]
    struct NoScratch {
        allocations: AtomicUsize,
    }

    impl ScratchSpacePort for NoScratch {
        fn allocate(
            &self,
            _: ScratchRequest,
            _: &mut WorkBudget,
        ) -> Result<Box<dyn ScratchAllocation>, IndexError> {
            let _previous = self.allocations.fetch_add(1, Ordering::Relaxed);
            Err(IndexError::BackendViolation(
                "unexpected scratch allocation".into(),
            ))
        }
    }

    struct NoCustody;

    impl ObjectCustodyPort for NoCustody {
        fn reserve_objects(&self, _: AttemptRef, _: &[RegisteredObject]) -> Result<(), IndexError> {
            Ok(())
        }

        fn register_objects(
            &self,
            _: AttemptRef,
            _: &[RegisteredObject],
        ) -> Result<(), IndexError> {
            Ok(())
        }
    }

    fn ready<T>(mut future: IndexFuture<'_, T>) -> Result<T, IndexError> {
        let mut context = Context::from_waker(Waker::noop());
        match Future::poll(future.as_mut(), &mut context) {
            Poll::Ready(result) => result,
            Poll::Pending => Err(IndexError::BackendViolation(
                "native literal test future unexpectedly pending".into(),
            )),
        }
    }

    fn empty_patch_case(
        digest: Digest,
    ) -> Result<(LiteralPlan, MemberBinding, NoWriteObjects), IndexError> {
        let source = SourceCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        };
        let current = PlanContext {
            instance: InstanceSpec {
                instance: IndexInstanceId([2; 16]),
                kind: IndexKind([3; 16]),
                group: PublicationGroupId([4; 16]),
                activation: GroupActivation::CoreRequired,
                input_domain: InputDomain::SourceLocal,
                schema: SchemaId([5; 16]),
                policy: Digest([6; 32]),
                format: StorageFormatId([7; 16]),
            },
            source,
            input: InputBinding {
                root: source.owners,
                schema: SchemaId([5; 16]),
                policy: Digest([6; 32]),
                context: Digest([8; 32]),
            },
        };
        let artifact = ObjectRef(Digest([9; 32]));
        let payload = encode(&Root::default())?;
        let member = MemberBinding {
            instance: current.instance.instance,
            format: current.instance.format,
            artifact,
            input: current.input,
            logical_commitment: Digest(Sha256::digest(&payload).into()),
        };
        let objects = NoWriteObjects {
            artifact,
            stored: StoredObject::new(
                RegisteredObject::unscoped(
                    artifact,
                    u64::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?,
                    Vec::new(),
                ),
                payload,
                None,
            ),
            installs: AtomicUsize::new(0),
            reads: AtomicUsize::new(0),
            cancel_after_read: false,
        };
        let index = NativeLiteralIndex;
        let changed = OwnerMutationSet {
            root: MapRoot(None),
            count: 0,
            digest,
        };
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?;
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            deadline,
        );
        let PlanDecision::Incremental(plan) = index.plan(
            BaseMember::Present(Box::new(member.clone())),
            DeltaInput::Patch(SourceDelta {
                prior: source,
                next: source,
                changed,
            }),
            &current,
            &mut budget,
        )?
        else {
            return Err(IndexError::BackendViolation(
                "empty patch did not produce an incremental literal plan".into(),
            ));
        };
        Ok((plan, member, objects))
    }

    #[test]
    fn verified_empty_patch_reuses_literal_artifact_without_writes() -> Result<(), IndexError> {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(IndexError::BackendViolation(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition)))); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(IndexError::BackendViolation(format!($($message)+))); }
            }};
        }
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(IndexError::BackendViolation(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!()))); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(IndexError::BackendViolation(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+)))); }
            }};
        }
        macro_rules! check_ne {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left == *right { return Err(IndexError::BackendViolation(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!()))); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left == *right { return Err(IndexError::BackendViolation(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+)))); }
            }};
        }

        let (plan, member, objects) = empty_patch_case(OwnerMutationDigest::default().finish())?;
        let prior_context = plan.context.clone();
        let maps = NoWriteMaps {
            empty_diff: Some((MapRoot(None), MapRoot(Some(ObjectRef(Digest([0x23; 32])))))),
            ..NoWriteMaps::default()
        };
        let scratch = NoScratch::default();
        let custody = NoCustody;
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?;
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            deadline,
        );
        let mut attempt = AttemptContext {
            attempt: AttemptRef {
                operation: OperationKey {
                    workspace: WorkspaceKey {
                        tenant: TenantId([10; 16]),
                        collection: CollectionId([11; 16]),
                        worktree: WorktreeId([12; 16]),
                        slot: SlotId([13; 16]),
                    },
                    epoch: 1,
                    producer: ProducerId([14; 16]),
                    stream: StreamId([15; 16]),
                    stream_epoch: 1,
                    sequence: 1,
                },
                attempt: AttemptId([16; 16]),
                fence: 1,
            },
            objects: &objects,
            maps: &maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut budget,
        };
        let index = NativeLiteralIndex;
        let gold = br#"{"owners":null,"grams":null}"#;
        check_eq!(objects.stored.payload.as_slice(), gold);
        let floor = u64::try_from(gold.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(8)
            .and_then(|bytes| bytes.checked_add(512))
            .ok_or(IndexError::ResourceLimited)?;
        let calls = Arc::new(AtomicUsize::new(0));
        attempt
            .budget
            .bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
                RefuseMetadataScope {
                    expected: floor,
                    calls: Arc::clone(&calls),
                },
            ))));
        check!(matches!(
            ready(index.prepare(plan.clone(), &mut attempt)),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(calls.load(Ordering::Relaxed), 1);
        check_eq!(scratch.allocations.load(Ordering::Relaxed), 0);
        check_eq!(objects.installs.load(Ordering::Relaxed), 0);
        check_eq!(maps.applies.load(Ordering::Relaxed), 0);
        // Continue the existing functional oracle on its original unscoped
        // fixture budget; no rejected grant or cancelled control is renewed.
        attempt.budget.bind_memory_scope(None);
        let candidate = ready(index.prepare(plan, &mut attempt))?;
        check_eq!(candidate.binding, member);
        check_eq!(
            ready(index.verify_and_seal(candidate, &mut attempt))?,
            member
        );
        check_eq!(scratch.allocations.load(Ordering::Relaxed), 0);
        check_eq!(objects.installs.load(Ordering::Relaxed), 0);
        check_eq!(maps.applies.load(Ordering::Relaxed), 0);

        let mut next_context = prior_context.clone();
        next_context.source.frontiers = MapRoot(Some(ObjectRef(Digest([0x21; 32]))));
        next_context.input.context = Digest([0x22; 32]);
        let PlanDecision::Incremental(next_plan) = index.plan(
            BaseMember::Present(Box::new(member.clone())),
            DeltaInput::Patch(SourceDelta {
                prior: prior_context.source,
                next: next_context.source,
                changed: OwnerMutationSet {
                    root: MapRoot(None),
                    count: 0,
                    digest: OwnerMutationDigest::default().finish(),
                },
            }),
            &next_context,
            attempt.budget,
        )?
        else {
            return Err(IndexError::BackendViolation(
                "frontier-only patch was rejected".into(),
            ));
        };
        let rebound = ready(index.prepare(next_plan, &mut attempt))?;
        check_eq!(rebound.binding.artifact, member.artifact);
        check_eq!(
            rebound.binding.logical_commitment,
            member.logical_commitment
        );
        check_eq!(rebound.binding.input, next_context.input);
        check_ne!(rebound.binding, member);
        check_eq!(
            ready(index.verify_and_seal(rebound, &mut attempt))?.input,
            next_context.input
        );
        check_eq!(scratch.allocations.load(Ordering::Relaxed), 0);
        check_eq!(objects.installs.load(Ordering::Relaxed), 0);
        check_eq!(maps.applies.load(Ordering::Relaxed), 0);

        let mut inconsistent_context = prior_context.clone();
        inconsistent_context.source.owners = MapRoot(Some(ObjectRef(Digest([0x23; 32]))));
        inconsistent_context.input.root = inconsistent_context.source.owners;
        let PlanDecision::Incremental(inconsistent_plan) = index.plan(
            BaseMember::Present(Box::new(member)),
            DeltaInput::Patch(SourceDelta {
                prior: prior_context.source,
                next: inconsistent_context.source,
                changed: OwnerMutationSet {
                    root: MapRoot(None),
                    count: 0,
                    digest: OwnerMutationDigest::default().finish(),
                },
            }),
            &inconsistent_context,
            attempt.budget,
        )?
        else {
            return Err(IndexError::BackendViolation(
                "inconsistent patch was rejected before prepare".into(),
            ));
        };
        check!(matches!(
            ready(index.prepare(inconsistent_plan, &mut attempt)),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(scratch.allocations.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn forged_empty_patch_digest_is_refused_before_literal_scratch() -> Result<(), IndexError> {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(IndexError::BackendViolation(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition)))); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(IndexError::BackendViolation(format!($($message)+))); }
            }};
        }
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(IndexError::BackendViolation(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!()))); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(IndexError::BackendViolation(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+)))); }
            }};
        }

        let (plan, _member, objects) = empty_patch_case(Digest([0xff; 32]))?;
        let maps = NoWriteMaps::default();
        let scratch = NoScratch::default();
        let custody = NoCustody;
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?;
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            deadline,
        );
        let mut attempt = AttemptContext {
            attempt: AttemptRef {
                operation: OperationKey {
                    workspace: WorkspaceKey {
                        tenant: TenantId([10; 16]),
                        collection: CollectionId([11; 16]),
                        worktree: WorktreeId([12; 16]),
                        slot: SlotId([13; 16]),
                    },
                    epoch: 1,
                    producer: ProducerId([14; 16]),
                    stream: StreamId([15; 16]),
                    stream_epoch: 1,
                    sequence: 1,
                },
                attempt: AttemptId([16; 16]),
                fence: 1,
            },
            objects: &objects,
            maps: &maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut budget,
        };
        let index = NativeLiteralIndex;
        check!(matches!(
            ready(index.prepare(plan, &mut attempt)),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(scratch.allocations.load(Ordering::Relaxed), 0);
        check_eq!(objects.installs.load(Ordering::Relaxed), 0);
        check_eq!(maps.applies.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
