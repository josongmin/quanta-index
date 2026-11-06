//! Product-path bootstrap and delta oracle for owner-local occurrence blocks.

use super::*;
use quanta_index_core::index::{
    OrdinalMapEdit, OwnerMutation, OwnerMutationDigest, StagedMapRoots, owner_mutation_value,
    ports::{MapDiffRange, MapDiffVisitor},
};

type Rows = BTreeMap<Vec<u8>, MapValue>;

#[derive(Default)]
struct CanonicalObjects {
    stored: Mutex<BTreeMap<ObjectRef, StoredObject>>,
    batch_inputs: AtomicUsize,
    block_inputs: AtomicUsize,
    wrong_second_block_output: AtomicBool,
}

impl CanonicalObjects {
    fn registration(
        payload: &[u8],
        children: Vec<ObjectRef>,
    ) -> Result<RegisteredObject, IndexError> {
        let payload_len =
            u32::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?;
        let child_count =
            u32::try_from(children.len()).map_err(|_error| IndexError::ResourceLimited)?;
        let wire_len = children
            .len()
            .checked_mul(32)
            .and_then(|bytes| bytes.checked_add(16))
            .and_then(|bytes| bytes.checked_add(payload.len()))
            .ok_or(IndexError::ResourceLimited)?;
        let mut hash = Sha256::new();
        hash.update(b"quanta-index-immutable-object-v1\0");
        hash.update(b"QIOBJ001");
        hash.update(payload_len.to_be_bytes());
        hash.update(child_count.to_be_bytes());
        for child in &children {
            hash.update(child.0.0);
        }
        hash.update(payload);
        Ok(RegisteredObject::unscoped(
            ObjectRef(Digest(hash.finalize().into())),
            u64::try_from(wire_len).map_err(|_error| IndexError::ResourceLimited)?,
            children,
        ))
    }

    fn seed(&self, payload: Vec<u8>, children: Vec<ObjectRef>) -> Result<ObjectRef, IndexError> {
        let registration = Self::registration(&payload, children)?;
        let reference = registration.reference;
        let stored = StoredObject::new(registration, payload, None);
        let mut objects = self
            .stored
            .lock()
            .map_err(|_error| IndexError::Storage("test object lock".into()))?;
        if let Some(prior) = objects.get(&reference) {
            if prior.payload != stored.payload
                || prior.registration.children != stored.registration.children
            {
                return Err(IndexError::BackendViolation(
                    "test object identity collision".into(),
                ));
            }
        } else {
            let _old = objects.insert(reference, stored);
        }
        drop(objects);
        Ok(reference)
    }

    fn reopened(&self) -> Result<Self, IndexError> {
        Ok(Self {
            stored: Mutex::new(
                self.stored
                    .lock()
                    .map_err(|_error| IndexError::Storage("test object lock".into()))?
                    .clone(),
            ),
            batch_inputs: AtomicUsize::new(0),
            block_inputs: AtomicUsize::new(0),
            wrong_second_block_output: AtomicBool::new(false),
        })
    }
}

impl ImmutableObjectPort for CanonicalObjects {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        budget.checkpoint()?;
        Self::registration(payload, children.to_vec())
    }

    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        budget.checkpoint()?;
        let reference = self.seed(payload.to_vec(), children.to_vec())?;
        self.identify(payload, children, budget).and_then(|object| {
            check_eq!(object.reference, reference);
            Ok(object)
        })
    }

    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        let object = self.identify(payload, children, budget)?;
        sink.reserve(&object)?;
        check_eq!(
            self.seed(payload.to_vec(), children.to_vec())?,
            object.reference
        );
        sink.installed(&object)?;
        Ok(object)
    }

    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
        budget.checkpoint()?;
        let mut registered = Vec::new();
        for input in inputs {
            registered.push(self.identify(&input.payload, &input.children, budget)?);
        }
        sink.reserve_batch(&registered)?;
        for (input, object) in inputs.iter().zip(&registered) {
            check_eq!(
                self.seed(input.payload.clone(), input.children.clone())?,
                object.reference
            );
        }
        sink.installed_batch(&registered)?;
        let _old = self.batch_inputs.fetch_add(inputs.len(), Ordering::Relaxed);
        let blocks = inputs
            .iter()
            .filter(|input| input.payload.starts_with(b"QITXOC01"))
            .count();
        let _old = self.block_inputs.fetch_add(blocks, Ordering::Relaxed);
        if blocks >= 2 && self.wrong_second_block_output.load(Ordering::Relaxed) {
            let slot = inputs
                .iter()
                .enumerate()
                .filter(|(_slot, input)| input.payload.starts_with(b"QITXOC01"))
                .nth(1)
                .map(|(slot, _input)| slot)
                .ok_or(IndexError::ResourceLimited)?;
            let second = registered
                .get_mut(slot)
                .ok_or(IndexError::ResourceLimited)?;
            *second = RegisteredObject::unscoped(reference(254), second.bytes, Vec::new());
        }
        Ok(quanta_index_core::index::RegisteredObjectBatch::unscoped(
            registered,
        ))
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        budget.checkpoint()?;
        self.stored
            .lock()
            .map_err(|_error| IndexError::Storage("test object lock".into()))?
            .get(&reference)
            .cloned()
            .ok_or_else(|| IndexError::CorruptObject("test object missing".into()))
    }
}

#[derive(Default)]
struct CanonicalMaps {
    roots: Mutex<BTreeMap<ObjectRef, Rows>>,
    next: AtomicU64,
    applies: AtomicUsize,
}

impl CanonicalMaps {
    fn keep(&self, rows: Rows) -> Result<MapRoot, IndexError> {
        if rows.is_empty() {
            return Ok(MapRoot(None));
        }
        let serial = self.next.fetch_add(1, Ordering::Relaxed);
        let mut hash = Sha256::new();
        hash.update(b"occurrence-block-test-map\0");
        hash.update(serial.to_be_bytes());
        let reference = ObjectRef(Digest(hash.finalize().into()));
        let _old = self
            .roots
            .lock()
            .map_err(|_error| IndexError::Storage("test map lock".into()))?
            .insert(reference, rows);
        Ok(MapRoot(Some(reference)))
    }

    fn rows(&self, root: MapRoot) -> Result<Rows, IndexError> {
        match root.0 {
            None => Ok(Rows::new()),
            Some(reference) => self
                .roots
                .lock()
                .map_err(|_error| IndexError::Storage("test map lock".into()))?
                .get(&reference)
                .cloned()
                .ok_or_else(|| IndexError::CorruptObject("test map root missing".into())),
        }
    }
}

impl PersistentMapPort for CanonicalMaps {
    fn lookup(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<MapValue>, IndexError> {
        budget.checkpoint()?;
        Ok(self.rows(root)?.get(key).copied())
    }

    fn scan(
        &self,
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        budget.checkpoint()?;
        let mut page = Vec::new();
        let mut more = false;
        for (key, value) in self.rows(root)? {
            if after.is_some_and(|after| key.as_slice() <= after) {
                continue;
            }
            if page.len() == usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)? {
                more = true;
                break;
            }
            page.push((key, value));
        }
        Ok(MapPage::new(page, more, Vec::new(), None))
    }

    fn visit_diff(
        &self,
        before: MapRoot,
        after: MapRoot,
        range: MapDiffRange<'_>,
        visitor: &mut dyn MapDiffVisitor,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        budget.checkpoint()?;
        let before = self.rows(before)?;
        let after = self.rows(after)?;
        let mut keys = std::collections::BTreeSet::new();
        keys.extend(before.keys().cloned());
        keys.extend(after.keys().cloned());
        let mut seen = 0_usize;
        for key in keys {
            if !key.starts_with(range.prefix)
                || range.after.is_some_and(|after| key.as_slice() <= after)
            {
                continue;
            }
            let prior = before.get(&key).copied();
            let next = after.get(&key).copied();
            if prior == next {
                continue;
            }
            if seen == usize::try_from(range.limit).map_err(|_error| IndexError::ResourceLimited)? {
                return Ok(true);
            }
            visitor.changed(&key, prior, next, budget)?;
            seen = seen.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        Ok(false)
    }

    fn apply(
        &self,
        _: MapRoot,
        _: &[MapEdit],
        _: &dyn ObjectWriteSink,
        _: &mut WorkBudget,
    ) -> Result<MapWrite, IndexError> {
        Err(IndexError::BackendViolation(
            "test expects map stream".into(),
        ))
    }

    fn apply_batch(
        &self,
        _: &[MapBatchRequest],
        _: &dyn ObjectWriteSink,
        _: &mut WorkBudget,
    ) -> Result<MapBatchWrite, IndexError> {
        Err(IndexError::BackendViolation(
            "test expects map stream".into(),
        ))
    }

    fn apply_stream(
        &self,
        bases: &[MapRoot],
        input: &mut dyn MapEditCursor,
        _: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<StagedMapRoots, IndexError> {
        let _old = self.applies.fetch_add(1, Ordering::Relaxed);
        let mut rows = bases
            .iter()
            .map(|root| self.rows(*root))
            .collect::<Result<Vec<_>, _>>()?;
        let mut previous: Option<OrdinalMapEdit> = None;
        while let Some(edit) = input.next_edit(budget)? {
            if previous.as_ref().is_some_and(|prior| {
                prior.root > edit.root
                    || (prior.root == edit.root && prior.edit.key >= edit.edit.key)
            }) {
                return Err(IndexError::Conflict);
            }
            let map = rows
                .get_mut(usize::try_from(edit.root).map_err(|_error| IndexError::ResourceLimited)?)
                .ok_or(IndexError::Conflict)?;
            if map.get(&edit.edit.key).copied() != edit.edit.expected {
                return Err(IndexError::Conflict);
            }
            match edit.edit.next {
                Some(next) => {
                    let _old = map.insert(edit.edit.key.clone(), next);
                }
                None => {
                    let _old = map.remove(&edit.edit.key);
                }
            }
            previous = Some(edit);
        }
        let mut roots = StagedMapRoots::prepare(bases.len(), budget)?;
        for map in rows {
            roots.push(self.keep(map)?)?;
        }
        roots.finish()
    }
}

fn integration_budget() -> Result<WorkBudget, IndexError> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 100_000_000,
            mutations: 100_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(120))
            .ok_or(IndexError::ResourceLimited)?,
    ))
}

fn source_record(
    objects: &CanonicalObjects,
    recipe: TextRecipe,
    body: &str,
) -> Result<ObjectRef, IndexError> {
    let bytes = objects.seed(body.as_bytes().to_vec(), Vec::new())?;
    let record = SourceRecord {
        path: "a".into(),
        bytes,
        byte_len: u64::try_from(body.len()).map_err(|_error| IndexError::ResourceLimited)?,
        text: SourceTextAdmission::Utf8 {
            policy: recipe.admission_policy,
        },
    };
    objects.seed(encode(&record)?, vec![bytes])
}

fn source_root(
    maps: &CanonicalMaps,
    owner: OwnerKey,
    record: ObjectRef,
) -> Result<MapRoot, IndexError> {
    maps.keep(Rows::from([(owner_map_key(owner).to_vec(), record.into())]))
}

fn prepare(
    index: &NativeTextIndex,
    base: BaseMember,
    input: DeltaInput<SourceDelta>,
    source: MapRoot,
    objects: &CanonicalObjects,
    maps: &CanonicalMaps,
    workspace: WorkspaceKey,
) -> Result<TextCandidate, IndexError> {
    let plan_context = context(index.recipe(), source);
    let mut plan_work = integration_budget()?;
    let PlanDecision::Incremental(plan) = index.plan(base, input, &plan_context, &mut plan_work)?
    else {
        return Err(IndexError::BackendViolation(
            "product-path plan was not incremental".into(),
        ));
    };
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = integration_budget()?;
    ready(index.prepare(
        plan,
        &mut AttemptContext {
            attempt: test_attempt(workspace),
            objects,
            maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        },
    ))
}

fn ticket_for(
    maps: &CanonicalMaps,
    objects: &CanonicalObjects,
    source: MapRoot,
    member: &MemberBinding,
) -> Result<TicketRecord, IndexError> {
    let children = [Some(member.artifact), source.0]
        .into_iter()
        .flatten()
        .collect();
    let member_ref = objects.seed(encode(member)?, children)?;
    let members = maps.keep(Rows::from([(
        member_map_key(member.instance).to_vec(),
        member_ref.into(),
    )]))?;
    let mut ticket = visitor_fixture()?.ticket;
    ticket.view.source = super::source(source);
    ticket.view.members = members;
    Ok(ticket)
}

fn inventory_ref(
    member: &MemberBinding,
    owner: OwnerKey,
    term: &str,
    objects: &CanonicalObjects,
    maps: &CanonicalMaps,
) -> Result<ObjectRef, IndexError> {
    inventory_ref_case(member, owner, term, CaseMode::Sensitive, objects, maps)
}

fn inventory_ref_case(
    member: &MemberBinding,
    owner: OwnerKey,
    term: &str,
    case: CaseMode,
    objects: &CanonicalObjects,
    maps: &CanonicalMaps,
) -> Result<ObjectRef, IndexError> {
    let mut work = integration_budget()?;
    let root = read_root(member, objects, &mut work)?;
    let contribution = read_contribution(
        maps.lookup(root.owners, &owner_key(owner), &mut work)?
            .ok_or_else(|| IndexError::CorruptObject("owner contribution absent".into()))?,
        objects,
        &mut work,
    )?;
    let key = stored_term_key(term, case)?;
    contribution
        .inventory
        .lookup(objects, maps, &key, &mut work)?
        .ok_or_else(|| IndexError::CorruptObject("inventory term absent".into()))
}

fn term_hit(
    index: &NativeTextIndex,
    member: &MemberBinding,
    ticket: &TicketRecord,
    term: &str,
    objects: &CanonicalObjects,
    maps: &CanonicalMaps,
) -> Result<(u64, Vec<u32>), IndexError> {
    term_hit_case(
        index,
        member,
        ticket,
        term,
        CaseMode::Sensitive,
        objects,
        maps,
    )
}

fn term_hit_case(
    index: &NativeTextIndex,
    member: &MemberBinding,
    ticket: &TicketRecord,
    term: &str,
    case: CaseMode,
    objects: &CanonicalObjects,
    maps: &CanonicalMaps,
) -> Result<(u64, Vec<u32>), IndexError> {
    let mut result = Vec::new();
    let mut work = integration_budget()?;
    let scan = index.visit_term(
        member,
        case,
        term,
        TextReadContext {
            ticket,
            objects,
            maps,
            budget: &mut work,
        },
        |hit| {
            result.push((hit.tf, hit.positions.to_vec()));
            Ok(())
        },
    )?;
    check_eq!(scan.df, 1);
    match result.as_slice() {
        [one] => Ok(one.clone()),
        _ => Err(IndexError::BackendViolation(
            "expected one complete term hit".into(),
        )),
    }
}

#[test]
fn bootstrap_block_and_delta_leaf_keep_term_specific_counts_and_old_ticket()
-> Result<(), IndexError> {
    let recipe = TextRecipe::new(Digest([61; 32]));
    let owner = OwnerKey {
        producer: ProducerId([62; 16]),
        stable_id: StableOwnerId([63; 16]),
    };
    let objects = CanonicalObjects::default();
    let maps = CanonicalMaps::default();
    let first_record = source_record(&objects, recipe, "alpha beta beta")?;
    let first_source = source_root(&maps, owner, first_record)?;
    let index = NativeTextIndex::new(recipe);
    let workspace = visitor_fixture()?.ticket.view.identity.workspace;
    let first = prepare(
        &index,
        BaseMember::Absent,
        DeltaInput::Bootstrap,
        first_source,
        &objects,
        &maps,
        workspace,
    )?;
    let old_ticket = ticket_for(&maps, &objects, first_source, &first.binding)?;
    let alpha_old = inventory_ref(&first.binding, owner, "alpha", &objects, &maps)?;
    let beta_old = inventory_ref(&first.binding, owner, "beta", &objects, &maps)?;
    check_eq!(alpha_old, beta_old);
    check!(
        objects
            .get(alpha_old, &mut integration_budget()?)?
            .payload
            .starts_with(b"QITXOC01")
    );
    check_eq!(
        term_hit(
            &index,
            &first.binding,
            &old_ticket,
            "alpha",
            &objects,
            &maps
        )?,
        (1, vec![0])
    );
    check_eq!(
        term_hit(&index, &first.binding, &old_ticket, "beta", &objects, &maps)?,
        (2, vec![1, 2])
    );

    let next_record = source_record(&objects, recipe, "alpha beta beta beta")?;
    let next_source = source_root(&maps, owner, next_record)?;
    let mutation = OwnerMutation {
        owner,
        expected: Some(first_record),
        next: Some(next_record),
    };
    let mut commitment = OwnerMutationDigest::default();
    commitment.push(&mutation)?;
    let changed_root = maps.keep(Rows::from([(
        owner_map_key(owner).to_vec(),
        owner_mutation_value(&mutation)?,
    )]))?;
    let second = prepare(
        &index,
        BaseMember::Present(Box::new(first.binding.clone())),
        DeltaInput::Patch(SourceDelta {
            prior: super::source(first_source),
            next: super::source(next_source),
            changed: OwnerMutationSet {
                root: changed_root,
                count: 1,
                digest: commitment.finish(),
            },
        }),
        next_source,
        &objects,
        &maps,
        workspace,
    )?;
    let current_ticket = ticket_for(&maps, &objects, next_source, &second.binding)?;
    let alpha_current = inventory_ref(&second.binding, owner, "alpha", &objects, &maps)?;
    let beta_current = inventory_ref(&second.binding, owner, "beta", &objects, &maps)?;
    check_eq!(alpha_current, alpha_old);
    check!(beta_current != beta_old);
    check!(
        objects
            .get(beta_current, &mut integration_budget()?)?
            .payload
            .starts_with(b"{\"positions\":")
    );
    let reopened = objects.reopened()?;
    check_eq!(
        term_hit(
            &index,
            &first.binding,
            &old_ticket,
            "alpha",
            &reopened,
            &maps
        )?,
        (1, vec![0])
    );
    check_eq!(
        term_hit(
            &index,
            &first.binding,
            &old_ticket,
            "beta",
            &reopened,
            &maps
        )?,
        (2, vec![1, 2])
    );
    check_eq!(
        term_hit(
            &index,
            &second.binding,
            &current_ticket,
            "alpha",
            &reopened,
            &maps
        )?,
        (1, vec![0])
    );
    check_eq!(
        term_hit(
            &index,
            &second.binding,
            &current_ticket,
            "beta",
            &reopened,
            &maps
        )?,
        (3, vec![1, 2, 3])
    );
    Ok(())
}

#[test]
fn bootstrap_capacity_singleton_uses_original_leaf_wire() -> Result<(), IndexError> {
    let recipe = TextRecipe::new(Digest([71; 32]));
    let owner = OwnerKey {
        producer: ProducerId([72; 16]),
        stable_id: StableOwnerId([73; 16]),
    };
    let objects = CanonicalObjects::default();
    let maps = CanonicalMaps::default();
    // A 2050-position vector exceeds the complete 8192-byte block envelope.
    // Both canonical case keys must use the existing leaf wire.
    let body = "solo ".repeat(2050);
    let record = source_record(&objects, recipe, &body)?;
    let source = source_root(&maps, owner, record)?;
    let index = NativeTextIndex::new(recipe);
    let workspace = visitor_fixture()?.ticket.view.identity.workspace;
    let candidate = prepare(
        &index,
        BaseMember::Absent,
        DeltaInput::Bootstrap,
        source,
        &objects,
        &maps,
        workspace,
    )?;
    let ticket = ticket_for(&maps, &objects, source, &candidate.binding)?;
    let sensitive = inventory_ref(&candidate.binding, owner, "solo", &objects, &maps)?;
    check!(
        objects
            .get(sensitive, &mut integration_budget()?)?
            .payload
            .starts_with(b"{\"positions\":")
    );
    let folded_key = stored_term_key("solo", CaseMode::Folded)?;
    let mut work = integration_budget()?;
    let root = read_root(&candidate.binding, &objects, &mut work)?;
    let contribution = read_contribution(
        maps.lookup(root.owners, &owner_key(owner), &mut work)?
            .ok_or_else(|| IndexError::CorruptObject("owner contribution absent".into()))?,
        &objects,
        &mut work,
    )?;
    let folded = contribution
        .inventory
        .lookup(&objects, &maps, &folded_key, &mut work)?
        .ok_or_else(|| IndexError::CorruptObject("folded term absent".into()))?;
    check!(
        objects
            .get(folded, &mut work)?
            .payload
            .starts_with(b"{\"positions\":")
    );
    check_eq!(
        term_hit(&index, &candidate.binding, &ticket, "solo", &objects, &maps)?,
        (2050, (0_u32..2050).collect::<Vec<_>>())
    );
    Ok(())
}

fn distinct_terms(count: u32) -> String {
    (0_u32..count)
        .map(|ordinal| format!("t{ordinal:03}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn bootstrap_512_aliases_share_one_physical_block_across_sql_packets() -> Result<(), IndexError> {
    let recipe = TextRecipe::new(Digest([77; 32]));
    let owner = OwnerKey {
        producer: ProducerId([78; 16]),
        stable_id: StableOwnerId([79; 16]),
    };
    let objects = CanonicalObjects::default();
    let maps = CanonicalMaps::default();
    // Two case surfaces for each of 256 terms yield 512 canonical selectors.
    // One physical ref must survive eight 64-row inventory SQL packets.
    let record = source_record(&objects, recipe, &distinct_terms(256))?;
    let source = source_root(&maps, owner, record)?;
    let index = NativeTextIndex::new(recipe);
    let workspace = visitor_fixture()?.ticket.view.identity.workspace;
    let candidate = prepare(
        &index,
        BaseMember::Absent,
        DeltaInput::Bootstrap,
        source,
        &objects,
        &maps,
        workspace,
    )?;
    check_eq!(objects.block_inputs.load(Ordering::Relaxed), 1);
    let first = inventory_ref(&candidate.binding, owner, "t000", &objects, &maps)?;
    let last = inventory_ref(&candidate.binding, owner, "t255", &objects, &maps)?;
    check_eq!(first, last);
    let block = objects.get(first, &mut integration_budget()?)?;
    check!(block.payload.starts_with(b"QITXOC01"));
    check_eq!(block.payload.get(8..10), Some(b"\x02\x00".as_slice()));
    let ticket = ticket_for(&maps, &objects, source, &candidate.binding)?;
    let reopened = objects.reopened()?;
    for ordinal in [0_u32, 255] {
        let term = format!("t{ordinal:03}");
        check_eq!(
            term_hit(&index, &candidate.binding, &ticket, &term, &reopened, &maps)?,
            (1, vec![ordinal])
        );
        check_eq!(
            term_hit_case(
                &index,
                &candidate.binding,
                &ticket,
                &term,
                CaseMode::Folded,
                &reopened,
                &maps,
            )?,
            (1, vec![ordinal])
        );
    }
    Ok(())
}

#[test]
fn bootstrap_full_retry_crosses_512_rows_and_keeps_boundary_selectors_exact()
-> Result<(), IndexError> {
    let recipe = TextRecipe::new(Digest([81; 32]));
    let owner = OwnerKey {
        producer: ProducerId([82; 16]),
        stable_id: StableOwnerId([83; 16]),
    };
    let objects = CanonicalObjects::default();
    let maps = CanonicalMaps::default();
    let record = source_record(&objects, recipe, &distinct_terms(258))?;
    let source = source_root(&maps, owner, record)?;
    let index = NativeTextIndex::new(recipe);
    let workspace = visitor_fixture()?.ticket.view.identity.workspace;
    let candidate = prepare(
        &index,
        BaseMember::Absent,
        DeltaInput::Bootstrap,
        source,
        &objects,
        &maps,
        workspace,
    )?;
    check_eq!(objects.block_inputs.load(Ordering::Relaxed), 2);
    let first = inventory_ref(&candidate.binding, owner, "t000", &objects, &maps)?;
    let last = inventory_ref_case(
        &candidate.binding,
        owner,
        "t257",
        CaseMode::Folded,
        &objects,
        &maps,
    )?;
    check!(first != last);
    let ticket = ticket_for(&maps, &objects, source, &candidate.binding)?;
    let reopened = objects.reopened()?;
    for ordinal in 0_u32..258 {
        let term = format!("t{ordinal:03}");
        check_eq!(
            term_hit(&index, &candidate.binding, &ticket, &term, &reopened, &maps)?,
            (1, vec![ordinal])
        );
    }
    check_eq!(
        term_hit_case(
            &index,
            &candidate.binding,
            &ticket,
            "t257",
            CaseMode::Folded,
            &reopened,
            &maps,
        )?,
        (1, vec![257])
    );
    Ok(())
}

#[test]
fn later_wrong_block_output_cannot_publish_planned_prefix() -> Result<(), IndexError> {
    let recipe = TextRecipe::new(Digest([91; 32]));
    let owner = OwnerKey {
        producer: ProducerId([92; 16]),
        stable_id: StableOwnerId([93; 16]),
    };
    let objects = CanonicalObjects::default();
    objects
        .wrong_second_block_output
        .store(true, Ordering::Relaxed);
    let maps = CanonicalMaps::default();
    let record = source_record(&objects, recipe, &distinct_terms(258))?;
    let source = source_root(&maps, owner, record)?;
    let index = NativeTextIndex::new(recipe);
    let workspace = visitor_fixture()?.ticket.view.identity.workspace;
    check!(matches!(
        prepare(
            &index,
            BaseMember::Absent,
            DeltaInput::Bootstrap,
            source,
            &objects,
            &maps,
            workspace,
        ),
        Err(IndexError::BackendViolation(message))
            if message == "native text occurrence identity changed during installation"
    ));
    check_eq!(objects.block_inputs.load(Ordering::Relaxed), 2);
    check_eq!(maps.applies.load(Ordering::Relaxed), 0);
    Ok(())
}
