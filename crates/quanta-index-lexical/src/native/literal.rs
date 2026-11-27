//! Owner-local byte-literal index for the native kernel. The query capability is
//! explicitly case-sensitive byte literals; token/ranking/regex routes stay separate.
use super::literal_packed::{self, PackedRoot, SealInput, SealWitness};
use super::literal_work::{load_literal_body, observe_member_work, owner_key};
use quanta_index_core::index::{
    AttemptContext, BaseMember, DeltaInput, Digest, IndexError, InputBinding, MapRoot,
    MemberBinding, MemoryLease, MemoryLifetime, ObjectChildren, ObjectRef, OwnerKey, PlanContext,
    PlanDecision, SourceDelta, SourceInputCursor, SourceRecord, SourceRecordOwner, TicketRecord,
    WorkBudget, WorkUsage, member_map_key, owner_from_map_key, owner_map_key,
    ports::{ImmutableObjectPort, IndexBuildPort, IndexFuture, PersistentMapPort},
};
#[cfg(test)]
use quanta_index_core::index::{ObjectBatchInput, StagedObjectReferences};
#[cfg(test)]
use serde::Serialize;
use sha2::{Digest as _, Sha256};

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeLiteralIndex;

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

#[cfg(test)]
struct PendingObject<K> {
    _key: K,
    _prior: Option<ObjectRef>,
    input: Option<ObjectBatchInput>,
    _payload_memory: Option<MemoryLease>,
    _children_memory: Option<MemoryLease>,
}

#[cfg(test)]
struct LiteralBatch<K> {
    rows: super::text_memory::PreparedRows<PendingObject<K>>,
}

#[cfg(test)]
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
                _key: key,
                _prior: prior,
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

pub struct LiteralCandidate {
    pub binding: MemberBinding,
    seal: Option<SealWitness>,
}

impl std::fmt::Debug for LiteralCandidate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiteralCandidate")
            .field("binding", &self.binding)
            .field("has_seal", &self.seal.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, IndexError> {
    serde_json::to_vec(value).map_err(|error| IndexError::InvalidInput(error.to_string()))
}

fn roots(root: Root) -> ObjectChildren {
    match [root.owners.0, root.grams.0] {
        [None, None] => ObjectChildren::Empty,
        [Some(value), None] | [None, Some(value)] => ObjectChildren::One([value]),
        [Some(first), Some(second)] => ObjectChildren::Two([first, second]),
    }
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
                // A Literal root binds only the Source owners map. When a Source
                // advance leaves it unchanged (a frontier-only, no-op DELTA),
                // the published root is already the exact output: keep its
                // identity and commitment under the new input binding without
                // invoking prepare or opening a Literal object.
                if quanta_index_core::index::is_canonical_empty_mutation_set(delta.changed)
                    && delta.prior.owners == delta.next.owners
                {
                    return Ok(PlanDecision::Reuse(Box::new(MemberBinding {
                        input: context.input,
                        ..*member
                    })));
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
            // Lanes 40/42 partition this member's prepare/seal work beneath
            // the enclosing BuildPrepare without changing accepted charges.
            observe_member_work(context, 40, |context| {
                let mut prior = None;
                if let Some(base) = &plan.base {
                    let stored = context.objects.get(base.artifact, context.budget)?;
                    let children = if let Some(root) = PackedRoot::decode(&stored.payload)? {
                        if root.sources != base.input.root {
                            return Err(IndexError::CorruptObject(
                                "literal base source differs".into(),
                            ));
                        }
                        prior = Some(root.blocks);
                        root.children()
                    } else {
                        roots(super::metadata::decode_fixed::<Root>(
                            &stored.payload,
                            context.budget,
                        )?)
                    };
                    if stored.registration.reference != base.artifact
                        || stored.registration.children.as_slice() != children.as_slice()
                        || Digest(Sha256::digest(&stored.payload).into()) != base.logical_commitment
                    {
                        return Err(IndexError::CorruptObject(
                            "literal base commitment differs".into(),
                        ));
                    }
                    if let DeltaInput::Patch(delta) = &plan.input
                        && (delta.changed.count == 0 || delta.prior.owners == delta.next.owners)
                    {
                        let mut cursor = SourceInputCursor::new(
                            plan.context.source,
                            plan.input.clone(),
                            context.maps,
                            context.objects,
                        )?;
                        while let Some(change) = cursor.next_change(context.budget)? {
                            if change.prior.as_ref().map(|(reference, _)| reference)
                                != change.next.as_ref().map(|(reference, _)| reference)
                            {
                                return Err(IndexError::CorruptObject(
                                    "unchanged literal cut contains a source edit".into(),
                                ));
                            }
                        }
                        if delta.prior.owners != delta.next.owners {
                            return Err(IndexError::CorruptObject(
                                "empty literal patch changes source".into(),
                            ));
                        }
                        return Ok(LiteralCandidate {
                            binding: MemberBinding {
                                input: plan.context.input,
                                ..base.clone()
                            },
                            seal: None,
                        });
                    }
                    if prior.is_none() && matches!(&plan.input, DeltaInput::Patch(_)) {
                        let mut cursor = SourceInputCursor::new(
                            plan.context.source,
                            plan.input.clone(),
                            context.maps,
                            context.objects,
                        )?;
                        while cursor.next_change(context.budget)?.is_some() {}
                    }
                }

                let input = SealInput {
                    source: plan.context.source,
                    input: if prior.is_some() {
                        plan.input
                    } else {
                        // A legacy artifact stays readable in its pinned view;
                        // the next changed view is explicitly built from the source snapshot.
                        DeltaInput::Bootstrap
                    },
                    prior: prior.unwrap_or_default(),
                };
                let (blocks, mut seal) =
                    literal_packed::publish(&input, plan.context.input, context)?;
                let (artifact, logical_commitment) = PackedRoot {
                    sources: plan.context.source.owners,
                    blocks,
                }
                .install(context)?;
                let binding = MemberBinding {
                    instance: plan.context.instance.instance,
                    format: plan.context.instance.format,
                    artifact,
                    input: plan.context.input,
                    logical_commitment,
                };
                seal.bind_member(&binding);
                Ok(LiteralCandidate {
                    binding,
                    seal: Some(seal),
                })
            })
        })
    }

    fn verify_and_seal<'a>(
        &'a self,
        candidate: LiteralCandidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, MemberBinding> {
        Box::pin(async move {
            observe_member_work(context, 42, |context| {
                let stored = context
                    .objects
                    .get(candidate.binding.artifact, context.budget)?;
                let packed = PackedRoot::decode(&stored.payload)?;
                let children = if let Some(root) = packed {
                    if root.sources != candidate.binding.input.root {
                        return Err(IndexError::CorruptObject(
                            "literal root source differs".into(),
                        ));
                    }
                    root.children()
                } else {
                    roots(super::metadata::decode_fixed::<Root>(
                        &stored.payload,
                        context.budget,
                    )?)
                };
                if stored.registration.reference != candidate.binding.artifact
                    || stored.registration.children.as_slice() != children.as_slice()
                    || Digest(Sha256::digest(&stored.payload).into())
                        != candidate.binding.logical_commitment
                {
                    return Err(IndexError::BackendViolation(
                        "literal root commitment mismatch".into(),
                    ));
                }
                if let Some(input) = candidate.seal.as_ref() {
                    let root = packed.ok_or_else(|| {
                        IndexError::BackendViolation("literal seal needs a block root".into())
                    })?;
                    literal_packed::seal(input, &candidate.binding, root, context)?;
                }
                Ok(candidate.binding)
            })
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
        if let Some(root) = PackedRoot::decode(&artifact.payload)? {
            if root.sources != input.root
                || artifact.registration.reference != member.artifact
                || artifact.registration.children.as_slice() != root.children().as_slice()
                || Digest(Sha256::digest(&artifact.payload).into()) != member.logical_commitment
            {
                return Err(IndexError::CorruptObject(
                    "literal block root commitment mismatch".into(),
                ));
            }
            return search_packed(
                root,
                LiteralQuery {
                    literal,
                    after,
                    limit,
                },
                objects,
                maps,
                budget,
            );
        }
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

/// Read-only gram postings of one authenticated Literal member, used by Text
/// reads to anchor `file.contains` on the pinned Literal sibling.
#[derive(Clone, Copy, Debug)]
pub(super) enum LiteralGrams {
    Tree { grams: MapRoot },
    Packed { blocks: MapRoot },
}

impl LiteralGrams {
    /// Authenticates the member's artifact exactly as `search` does. The
    /// caller has already proved the member is the unique pinned sibling over
    /// `sources`.
    pub(super) fn open(
        member: &MemberBinding,
        sources: MapRoot,
        objects: &dyn ImmutableObjectPort,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        let artifact = objects.get(member.artifact, budget)?;
        if artifact.registration.reference != member.artifact
            || Digest(Sha256::digest(&artifact.payload).into()) != member.logical_commitment
        {
            return Err(IndexError::CorruptObject(
                "literal sibling commitment mismatch".into(),
            ));
        }
        if let Some(root) = PackedRoot::decode(&artifact.payload)? {
            if root.sources != sources
                || artifact.registration.children.as_slice() != root.children().as_slice()
            {
                return Err(IndexError::CorruptObject(
                    "literal sibling block root mismatch".into(),
                ));
            }
            return Ok(Self::Packed {
                blocks: root.blocks,
            });
        }
        let root = super::metadata::decode_fixed::<Root>(&artifact.payload, budget)?;
        if !artifact
            .registration
            .children
            .iter()
            .copied()
            .eq([root.owners.0, root.grams.0].into_iter().flatten())
        {
            return Err(IndexError::CorruptObject(
                "literal sibling tree root mismatch".into(),
            ));
        }
        Ok(Self::Tree { grams: root.grams })
    }

    fn tree_posting(
        grams: MapRoot,
        gram: [u8; 3],
        objects: &dyn ImmutableObjectPort,
        maps: &dyn PersistentMapPort,
        budget: &mut WorkBudget,
    ) -> Result<Option<Posting>, IndexError> {
        maps.lookup_object(grams, &gram, budget)?
            .map(|reference| load_posting(reference, objects, budget))
            .transpose()
    }

    /// The gram's owner count, saturated at `cap`. A packed posting is read
    /// only until `cap` owners have been seen.
    pub(super) fn count_capped(
        self,
        gram: [u8; 3],
        cap: u64,
        objects: &dyn ImmutableObjectPort,
        maps: &dyn PersistentMapPort,
        budget: &mut WorkBudget,
    ) -> Result<u64, IndexError> {
        match self {
            Self::Tree { grams } => Ok(Self::tree_posting(grams, gram, objects, maps, budget)?
                .map_or(0, |posting| posting.count.min(cap))),
            Self::Packed { blocks } => {
                let mut cursor = super::literal_blocks::LiteralGramCursor::new(
                    maps, objects, blocks, gram, None,
                );
                let mut count = 0_u64;
                while count < cap && cursor.next(budget)?.is_some() {
                    count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                }
                Ok(count)
            }
        }
    }

    /// Every owner of the gram in ascending key order. A tree posting must
    /// hold exactly its declared count.
    pub(super) fn owners(
        self,
        gram: [u8; 3],
        objects: &dyn ImmutableObjectPort,
        maps: &dyn PersistentMapPort,
        budget: &mut WorkBudget,
        mut visit: impl FnMut([u8; 32], &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        match self {
            Self::Tree { grams } => {
                let Some(posting) = Self::tree_posting(grams, gram, objects, maps, budget)? else {
                    return Ok(());
                };
                let mut after: Option<[u8; 32]> = None;
                let mut seen = 0_u64;
                loop {
                    let page = maps.scan(
                        posting.root,
                        after.as_ref().map(<[u8; 32]>::as_slice),
                        64,
                        budget,
                    )?;
                    for (key, _) in &page.entries {
                        let key = owner_cursor(key)?;
                        if after.is_some_and(|prior| prior >= key) {
                            return Err(IndexError::CorruptObject(
                                "literal posting owners are not strictly ordered".into(),
                            ));
                        }
                        after = Some(key);
                        seen = seen.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                        visit(key, budget)?;
                    }
                    if !page.has_more {
                        break;
                    }
                    if page.entries.is_empty() {
                        return Err(IndexError::BackendViolation(
                            "nonprogressing map cursor".into(),
                        ));
                    }
                }
                if seen != posting.count {
                    return Err(IndexError::CorruptObject(
                        "literal posting differs from its declared count".into(),
                    ));
                }
                Ok(())
            }
            Self::Packed { blocks } => {
                let mut cursor = super::literal_blocks::LiteralGramCursor::new(
                    maps, objects, blocks, gram, None,
                );
                while let Some(owner) = cursor.next(budget)? {
                    visit(owner, budget)?;
                }
                Ok(())
            }
        }
    }

    /// A monotone membership probe of one gram's posting; see
    /// [`GramProbe::contains`].
    pub(super) fn probe(self, gram: [u8; 3]) -> GramProbe {
        GramProbe(match self {
            Self::Tree { grams } => ProbeState::Tree {
                grams,
                gram,
                posting: TreePosting::Unread,
            },
            Self::Packed { blocks } => {
                ProbeState::Packed(super::literal_blocks::LiteralGramProbe::new(blocks, gram))
            }
        })
    }

    #[cfg(test)]
    pub(super) fn contains(
        self,
        cache: &mut super::literal_blocks::MembershipCache,
        gram: [u8; 3],
        owner: [u8; 32],
        objects: &dyn ImmutableObjectPort,
        maps: &dyn PersistentMapPort,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        match self {
            Self::Tree { grams } => Ok(
                match Self::tree_posting(grams, gram, objects, maps, budget)? {
                    Some(posting) => maps.lookup(posting.root, &owner, budget)?.is_some(),
                    None => false,
                },
            ),
            Self::Packed { blocks } => cache.contains(
                maps,
                objects,
                blocks,
                super::literal_blocks::literal_key(gram, owner),
                budget,
            ),
        }
    }
}

/// Membership of one gram's posting for owners asked in ascending order.
///
/// A packed posting reads each of its blocks at most once; a tree posting
/// loads its posting record once and looks each owner up in its owner map.
pub(super) struct GramProbe(ProbeState);

/// A tree posting record, read on first use.
enum TreePosting {
    Unread,
    Absent,
    Present(Posting),
}

enum ProbeState {
    Tree {
        grams: MapRoot,
        gram: [u8; 3],
        posting: TreePosting,
    },
    Packed(super::literal_blocks::LiteralGramProbe),
}

impl GramProbe {
    pub(super) fn contains(
        &mut self,
        owner: [u8; 32],
        objects: &dyn ImmutableObjectPort,
        maps: &dyn PersistentMapPort,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        match &mut self.0 {
            ProbeState::Tree {
                grams,
                gram,
                posting,
            } => {
                if matches!(posting, TreePosting::Unread) {
                    *posting = LiteralGrams::tree_posting(*grams, *gram, objects, maps, budget)?
                        .map_or(TreePosting::Absent, TreePosting::Present);
                }
                match posting {
                    TreePosting::Present(found) => {
                        Ok(maps.lookup(found.root, &owner, budget)?.is_some())
                    }
                    TreePosting::Unread | TreePosting::Absent => Ok(false),
                }
            }
            ProbeState::Packed(probe) => probe.contains(maps, objects, owner, budget),
        }
    }
}

/// Canonical payloads of a tree-format Literal artifact and gram posting,
/// for sibling fixtures in Text read tests.
#[cfg(test)]
pub(super) fn tree_fixture_payloads(
    owners: MapRoot,
    grams: MapRoot,
) -> Result<Vec<u8>, IndexError> {
    serde_json::to_vec(&Root { owners, grams })
        .map_err(|error| IndexError::InvalidInput(error.to_string()))
}

#[cfg(test)]
pub(super) fn tree_fixture_posting(root: MapRoot, count: u64) -> Result<Vec<u8>, IndexError> {
    serde_json::to_vec(&Posting { root, count })
        .map_err(|error| IndexError::InvalidInput(error.to_string()))
}

struct SourceOwners<'a> {
    maps: &'a dyn PersistentMapPort,
    root: MapRoot,
    after: Option<[u8; 38]>,
    page: quanta_index_core::index::MapPageCursor,
    more: bool,
}

impl SourceOwners<'_> {
    fn next(
        &mut self,
        budget: &mut WorkBudget,
    ) -> Result<Option<([u8; 32], ObjectRef)>, IndexError> {
        loop {
            if let Some(row) = self.page.next() {
                let (key, reference) = row.object_parts()?;
                let owner = owner_from_map_key(key)?;
                let current = owner_map_key(owner);
                if self.after.is_some_and(|after| after >= current) {
                    return Err(IndexError::CorruptObject(
                        "literal owner scan did not advance".into(),
                    ));
                }
                self.after = Some(current);
                return Ok(Some((owner_key(owner), *reference)));
            }
            if !self.more {
                return Ok(None);
            }
            let page = self.maps.scan(
                self.root,
                self.after.as_ref().map(<[u8; 38]>::as_slice),
                64,
                budget,
            )?;
            if page.entries.is_empty() && page.has_more {
                return Err(IndexError::CorruptObject(
                    "literal owner scan has empty continuation".into(),
                ));
            }
            self.more = page.has_more;
            self.page = page.cursor();
        }
    }
}

fn search_packed(
    root: PackedRoot,
    query: LiteralQuery<'_>,
    objects: &dyn ImmutableObjectPort,
    maps: &dyn PersistentMapPort,
    budget: &mut WorkBudget,
) -> Result<LiteralPage, IndexError> {
    use super::literal_blocks::{LiteralGramCursor, contains, literal_key};

    let after = query.after.map(owner_cursor).transpose()?;
    let gram_count = query.literal.len().saturating_sub(2);
    let gram_bytes = gram_count
        .checked_mul(3)
        .ok_or(IndexError::ResourceLimited)?;
    let _gram_memory = query_memory(gram_bytes, MemoryLifetime::Temporary, budget)?;
    let mut grams = Vec::<[u8; 3]>::new();
    grams
        .try_reserve_exact(gram_count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    for gram in query.literal.windows(3) {
        grams.push(
            gram.try_into()
                .map_err(|_error| IndexError::BackendViolation("literal gram width".into()))?,
        );
    }
    grams.sort_unstable();
    grams.dedup();
    let mut smallest = None;
    for gram in &grams {
        let mut cursor = LiteralGramCursor::new(maps, objects, root.blocks, *gram, None);
        let mut count = 0_usize;
        while cursor.next(budget)?.is_some() {
            count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            if smallest.is_some_and(|(_, previous)| count >= previous) {
                break;
            }
        }
        if count == 0 {
            return Ok(LiteralPage::from_parts(Vec::new(), None, false, None, None));
        }
        if smallest.is_none_or(|(_, previous)| count < previous) {
            smallest = Some((*gram, count));
        }
    }
    let mut postings =
        smallest.map(|(gram, _)| LiteralGramCursor::new(maps, objects, root.blocks, gram, after));
    let mut sources = SourceOwners {
        maps,
        root: root.sources,
        after: after
            .as_ref()
            .map(|key| decode_owner(key).map(owner_map_key))
            .transpose()?,
        page: quanta_index_core::index::MapPageCursor::default(),
        more: root.sources.0.is_some(),
    };
    let mut next = |budget: &mut WorkBudget| {
        if let Some(postings) = &mut postings {
            postings.next(budget).map(|row| row.map(|key| (key, None)))
        } else {
            sources
                .next(budget)
                .map(|row| row.map(|(key, reference)| (key, Some(reference))))
        }
    };

    let limit = usize::try_from(query.limit).map_err(|_error| IndexError::ResourceLimited)?;
    let hit_bytes = limit
        .checked_mul(std::mem::size_of::<LiteralHit>())
        .ok_or(IndexError::ResourceLimited)?;
    let hit_memory = query_memory(hit_bytes, MemoryLifetime::Retained, budget)?;
    let mut hits = Vec::new();
    hits.try_reserve_exact(limit)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut result = LiteralPage::from_parts(hits, None, false, hit_memory, None);
    'candidates: while let Some((key, record)) = next(budget)? {
        for gram in &grams {
            if smallest.is_some_and(|(chosen, _)| *gram == chosen) {
                continue;
            }
            if !contains(maps, objects, root.blocks, literal_key(*gram, key), budget)? {
                continue 'candidates;
            }
        }
        let owner = decode_owner(&key)?;
        let reference = match record {
            Some(reference) => reference,
            None => maps
                .lookup_object(root.sources, &owner_map_key(owner), budget)?
                .ok_or_else(|| {
                    IndexError::CorruptObject("literal posting lacks a source owner".into())
                })?,
        };
        let record = load_literal_record(reference, objects, budget)?;
        let body = load_literal_body(&record, objects, budget)?;
        if body
            .payload
            .windows(query.literal.len())
            .any(|window| window == query.literal)
        {
            let (record, memory) = record.into_retained_parts(budget)?;
            result
                .hits
                .push(LiteralHit::from_parts(owner, record, memory));
            if result.hits.len() == limit {
                result.has_more = next(budget)?.is_some();
                if result.has_more {
                    result.after_memory =
                        query_memory(key.len(), MemoryLifetime::Retained, budget)?;
                    result.after = Some(key.to_vec());
                }
                return Ok(result);
            }
        }
    }
    Ok(result)
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
            &mut posting_budget()?,
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
            rows: Vec::new(),
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

    struct LiteralSearchFixture {
        ticket: TicketRecord,
        member: MemberBinding,
        objects: NoWriteObjects,
        maps: NoWriteMaps,
    }

    fn literal_search_fixture(packed: bool) -> Result<LiteralSearchFixture, IndexError> {
        use super::super::literal_blocks::{LiteralBlockBuilder, PushResult, literal_key};
        use super::super::literal_contribution::Contribution;
        use quanta_index_core::index::{
            CatalogView, InputBinding, ReadSessionRef, ReadTicketLease, StableOwnerId, TicketId,
            ViewRef,
        };

        let reference = |byte| ObjectRef(Digest([byte; 32]));
        let source_root = MapRoot(Some(reference(20)));
        let owner_root = MapRoot(Some(reference(21)));
        let gram_root = MapRoot(Some(reference(22)));
        let first_posting_root = MapRoot(Some(reference(23)));
        let second_posting_root = MapRoot(Some(reference(24)));
        let block_root = MapRoot(Some(reference(25)));
        let member_root = MapRoot(Some(reference(26)));
        let mut objects = posting_objects(reference(120), Vec::new(), b"")?;
        objects.artifact = reference(120);
        let mut sources = Vec::new();
        let mut owners = Vec::new();
        let mut postings = Vec::new();
        let mut block_keys = Vec::new();
        // Owners 2 and 6 have both query trigrams without contiguous "abcX".
        // Owner 6 follows the final true hit and tests empty terminal continuation.
        const BODIES: [&[u8]; 6] = [b"abcX", b"abcYbcX", b"abcX", b"zzz", b"abcX", b"abcYbcX"];
        for (ordinal, body) in (1u8..).zip(BODIES) {
            let owner = OwnerKey {
                producer: ProducerId([1; 16]),
                stable_id: StableOwnerId([ordinal; 16]),
            };
            let source_ref = reference(
                40_u8
                    .checked_add(ordinal)
                    .ok_or(IndexError::ResourceLimited)?,
            );
            let contribution_ref = reference(
                60_u8
                    .checked_add(ordinal)
                    .ok_or(IndexError::ResourceLimited)?,
            );
            let body_ref = reference(
                80_u8
                    .checked_add(ordinal)
                    .ok_or(IndexError::ResourceLimited)?,
            );
            let record = SourceRecord {
                path: format!("src/{ordinal}.rs"),
                bytes: body_ref,
                byte_len: u64::try_from(body.len()).map_err(|_| IndexError::ResourceLimited)?,
                text: quanta_index_core::index::SourceTextAdmission::Excluded,
                source_repo_id: None,
                language: None,
            };
            let contribution = Contribution {
                record: source_ref,
                grams: Vec::new(),
            };
            for (reference, payload, children) in [
                (body_ref, body.to_vec(), Vec::new()),
                (source_ref, encode(&record)?, vec![body_ref]),
                (contribution_ref, encode(&contribution)?, vec![source_ref]),
            ] {
                objects.rows.push((
                    reference,
                    StoredObject::new(
                        RegisteredObject::unscoped(
                            reference,
                            u64::try_from(payload.len())
                                .map_err(|_| IndexError::ResourceLimited)?,
                            children,
                        ),
                        payload,
                        None,
                    ),
                ));
            }
            sources.push((owner_map_key(owner).to_vec(), source_ref));
            owners.push((owner_key(owner).to_vec(), contribution_ref));
            if ordinal != 4 {
                postings.push((owner_key(owner).to_vec(), source_ref));
                for gram in [*b"abc", *b"bcX"] {
                    block_keys.push(literal_key(gram, owner_key(owner)));
                }
            }
        }
        block_keys.sort_unstable();
        let mut block_budget = posting_budget()?;
        let mut block = LiteralBlockBuilder::new(&block_budget, None)?;
        for key in block_keys {
            if block.try_push(key, &mut block_budget)? != PushResult::Added {
                return Err(IndexError::BackendViolation(
                    "literal fixture block filled".into(),
                ));
            }
        }
        let block = block.finish()?;
        let block_last = block.last_key;
        let (block_input, _memory) = block.into_batch_input();
        objects.rows.push((
            reference(110),
            StoredObject::new(
                RegisteredObject::unscoped(
                    reference(110),
                    u64::try_from(block_input.payload.len())
                        .map_err(|_| IndexError::ResourceLimited)?,
                    Vec::new(),
                ),
                block_input.payload,
                None,
            ),
        ));
        let mut maps = NoWriteMaps {
            rows: vec![
                (source_root, sources),
                (owner_root, owners),
                (first_posting_root, postings.clone()),
                (second_posting_root, postings),
                (block_root, vec![(block_last.to_vec(), reference(110))]),
                (
                    member_root,
                    vec![(
                        member_map_key(IndexInstanceId([2; 16])).to_vec(),
                        reference(120),
                    )],
                ),
            ],
            ..NoWriteMaps::default()
        };
        let artifact = if packed {
            reference(122)
        } else {
            reference(121)
        };
        let payload = if packed {
            let mut payload = b"QLITBLK2".to_vec();
            for root in [source_root, block_root] {
                payload.push(1);
                payload.extend_from_slice(&root.0.ok_or(IndexError::ResourceLimited)?.0.0);
            }
            payload
        } else {
            let posting = |root| Posting { root, count: 5 };
            for (reference, root) in [
                (reference(100), first_posting_root),
                (reference(101), second_posting_root),
            ] {
                let payload = encode(&posting(root))?;
                objects.rows.push((
                    reference,
                    StoredObject::new(
                        RegisteredObject::unscoped(
                            reference,
                            u64::try_from(payload.len())
                                .map_err(|_| IndexError::ResourceLimited)?,
                            vec![root.0.ok_or(IndexError::ResourceLimited)?],
                        ),
                        payload,
                        None,
                    ),
                ));
            }
            maps.rows.push((
                gram_root,
                vec![
                    (b"abc".to_vec(), reference(100)),
                    (b"bcX".to_vec(), reference(101)),
                ],
            ));
            encode(&Root {
                owners: owner_root,
                grams: gram_root,
            })?
        };
        let member = MemberBinding {
            instance: IndexInstanceId([2; 16]),
            format: StorageFormatId([3; 16]),
            artifact,
            input: InputBinding {
                root: source_root,
                schema: SchemaId([4; 16]),
                policy: Digest([5; 32]),
                context: Digest([6; 32]),
            },
            logical_commitment: Digest(Sha256::digest(&payload).into()),
        };
        let children = if packed {
            vec![
                source_root.0.ok_or(IndexError::ResourceLimited)?,
                block_root.0.ok_or(IndexError::ResourceLimited)?,
            ]
        } else {
            vec![
                owner_root.0.ok_or(IndexError::ResourceLimited)?,
                gram_root.0.ok_or(IndexError::ResourceLimited)?,
            ]
        };
        objects.rows.push((
            artifact,
            StoredObject::new(
                RegisteredObject::unscoped(
                    artifact,
                    u64::try_from(payload.len()).map_err(|_| IndexError::ResourceLimited)?,
                    children,
                ),
                payload,
                None,
            ),
        ));
        let member_payload = encode(&member)?;
        objects.stored = StoredObject::new(
            RegisteredObject::unscoped(
                reference(120),
                u64::try_from(member_payload.len()).map_err(|_| IndexError::ResourceLimited)?,
                vec![artifact],
            ),
            member_payload,
            None,
        );
        let workspace = WorkspaceKey {
            tenant: TenantId([10; 16]),
            collection: CollectionId([11; 16]),
            worktree: WorktreeId([12; 16]),
            slot: SlotId([13; 16]),
        };
        let mut view = CatalogView {
            identity: ViewRef {
                workspace,
                epoch: 1,
                sequence: 1,
                digest: Digest([0; 32]),
            },
            profile: reference(30),
            source: SourceCut {
                owners: source_root,
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            },
            projections: MapRoot(None),
            members: member_root,
            coverage: MapRoot(None),
        };
        view.identity.digest = view
            .identity_digest()
            .map_err(|error| IndexError::CorruptObject(error.to_string()))?;
        Ok(LiteralSearchFixture {
            ticket: TicketRecord {
                lease: ReadTicketLease {
                    id: TicketId([14; 16]),
                    session: ReadSessionRef { id: 1, fence: 1 },
                    fence: 1,
                    expires_at_ms: 10_000,
                },
                view,
            },
            member,
            objects,
            maps,
        })
    }

    #[test]
    fn packed_and_legacy_literal_pages_follow_independent_byte_oracle() -> Result<(), IndexError> {
        use quanta_index_core::index::StableOwnerId;

        for packed in [false, true] {
            let fixture = literal_search_fixture(packed)?;
            for (literal, expected) in [
                (b"abcX".as_slice(), &[1_u8, 3, 5][..]),
                (b"a".as_slice(), &[1_u8, 2, 3, 5, 6][..]),
            ] {
                for limit in [1, 2] {
                    let mut after = None;
                    let mut found = Vec::new();
                    let mut terminal = false;
                    for _page in 0..=expected.len() {
                        let page = NativeLiteralIndex.search(
                            &fixture.member,
                            LiteralQuery {
                                literal,
                                after: after.as_deref(),
                                limit,
                            },
                            LiteralReadContext {
                                ticket: &fixture.ticket,
                                input: fixture.member.input,
                                objects: &fixture.objects,
                                maps: &fixture.maps,
                                budget: &mut posting_budget()?,
                            },
                        )?;
                        if (page.has_more && page.hits.is_empty())
                            || page.hits.len()
                                > usize::try_from(limit).map_err(|_| IndexError::ResourceLimited)?
                        {
                            return Err(IndexError::BackendViolation(
                                "literal fixture page size differs".into(),
                            ));
                        }
                        found.extend(page.hits.iter().map(|hit| hit.owner));
                        if page.has_more {
                            if after.as_deref().is_some_and(|prior| {
                                page.after.as_deref().is_none_or(|next| next <= prior)
                            }) {
                                return Err(IndexError::BackendViolation(
                                    "literal fixture cursor failed to advance".into(),
                                ));
                            }
                            if page.after.as_deref()
                                != page
                                    .hits
                                    .last()
                                    .map(|hit| owner_key(hit.owner))
                                    .as_ref()
                                    .map(<[u8; 32]>::as_slice)
                            {
                                return Err(IndexError::BackendViolation(
                                    "literal fixture cursor differs from last hit".into(),
                                ));
                            }
                            after = page.after;
                        } else {
                            if page.after.is_some() {
                                return Err(IndexError::BackendViolation(
                                    "terminal literal fixture page has a cursor".into(),
                                ));
                            }
                            terminal = true;
                            break;
                        }
                    }
                    if !terminal {
                        return Err(IndexError::BackendViolation(
                            "literal fixture never reached a terminal page".into(),
                        ));
                    }
                    let expected = expected.iter().copied().map(|ordinal| OwnerKey {
                        producer: ProducerId([1; 16]),
                        stable_id: StableOwnerId([ordinal; 16]),
                    });
                    if !found.into_iter().eq(expected) {
                        return Err(IndexError::BackendViolation(format!(
                            "literal fixed byte oracle differs for packed={packed}, limit={limit}"
                        )));
                    }
                }
            }
        }
        Ok(())
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
        rows: Vec<(ObjectRef, StoredObject)>,
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
            let stored = if reference == self.artifact {
                Some(&self.stored)
            } else {
                self.rows
                    .iter()
                    .find(|(key, _)| *key == reference)
                    .map(|(_, stored)| stored)
            };
            if let Some(stored) = stored {
                let _previous = self.reads.fetch_add(1, Ordering::Relaxed);
                if self.cancel_after_read {
                    budget.request().cancel_handle().cancel();
                }
                Ok(stored.clone())
            } else {
                Err(IndexError::CorruptObject("unexpected test object".into()))
            }
        }
    }

    #[derive(Default)]
    struct NoWriteMaps {
        applies: AtomicUsize,
        empty_diff: Option<(MapRoot, MapRoot)>,
        rows: Vec<(MapRoot, Vec<(Vec<u8>, ObjectRef)>)>,
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
            root: MapRoot,
            key: &[u8],
            budget: &mut WorkBudget,
        ) -> Result<Option<ObjectRef>, IndexError> {
            budget.checkpoint()?;
            let entries = self
                .rows
                .iter()
                .find(|(candidate, _)| *candidate == root)
                .ok_or_else(|| IndexError::BackendViolation("unexpected map lookup".into()))?;
            Ok(entries
                .1
                .iter()
                .find(|(candidate, _)| candidate == key)
                .map(|(_, reference)| *reference))
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
            after: Option<&[u8]>,
            limit: u32,
            budget: &mut WorkBudget,
        ) -> Result<MapPage, IndexError> {
            budget.checkpoint()?;
            if let Some((_, entries)) = self.rows.iter().find(|(candidate, _)| *candidate == root) {
                let limit = usize::try_from(limit).map_err(|_| IndexError::ResourceLimited)?;
                let mut remaining = entries
                    .iter()
                    .filter(|(key, _)| after.is_none_or(|cursor| key.as_slice() > cursor));
                let page = remaining
                    .by_ref()
                    .take(limit)
                    .map(|(key, reference)| (key.clone(), (*reference).into()))
                    .collect();
                return Ok(MapPage::new(
                    page,
                    remaining.next().is_some(),
                    Vec::new(),
                    None,
                ));
            }
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
            rows: Vec::new(),
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
        let delta = SourceDelta {
            prior: source,
            next: source,
            changed,
        };
        let plan = match index.plan(
            BaseMember::Present(Box::new(member.clone())),
            DeltaInput::Patch(delta.clone()),
            &current,
            &mut budget,
        )? {
            PlanDecision::Incremental(plan) => plan,
            // Planning keeps the exact published member for a canonical empty
            // set. These tests drive prepare's own empty-patch defence too.
            PlanDecision::Reuse(reused) if *reused == member => LiteralPlan {
                base: Some(member.clone()),
                context: current,
                input: DeltaInput::Patch(delta),
            },
            PlanDecision::Reuse(_)
            | PlanDecision::RebuildRequired
            | PlanDecision::MaintenanceRequired => {
                return Err(IndexError::BackendViolation(
                    "empty patch did not keep the published literal member".into(),
                ));
            }
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
        // A frontier-only patch is decided at plan time: the published
        // Literal root is kept under the new input without prepare or I/O.
        let PlanDecision::Reuse(rebound) = index.plan(
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
                "frontier-only patch did not reuse the literal root".into(),
            ));
        };
        check_eq!(rebound.input, next_context.input);
        check_eq!(
            (
                rebound.instance,
                rebound.format,
                rebound.artifact,
                rebound.logical_commitment
            ),
            (
                member.instance,
                member.format,
                member.artifact,
                member.logical_commitment
            )
        );
        check_ne!(rebound.input, member.input);
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
