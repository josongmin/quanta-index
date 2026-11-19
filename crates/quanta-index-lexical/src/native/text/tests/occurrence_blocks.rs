//! Product-path bootstrap and delta oracle for owner-local occurrence blocks.

use super::*;
use quanta_index_core::index::{
    OrdinalMapEdit, OwnerMutation, OwnerMutationDigest, StagedMapRoots, owner_mutation_value,
    ports::{MapDiffRange, MapDiffVisitor},
};

type Rows = BTreeMap<Vec<u8>, MapValue>;

struct DirectoryFaultCursor<C> {
    inner: C,
    fault: u8,
    first: Option<OrdinalMapEdit>,
    sent_late: bool,
}

impl<C: quanta_index_core::index::ports::MapEditCursor>
    quanta_index_core::index::ports::MapEditCursor for DirectoryFaultCursor<C>
{
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        let next = self.inner.next_edit(budget)?;
        if let Some(edit) = next {
            if self.first.is_none() {
                self.first = Some(edit.clone());
                if self.fault == 1 {
                    return Ok(Some(OrdinalMapEdit::unscoped(
                        0,
                        MapEdit {
                            key: edit.key().to_vec(),
                            expected: Some(reference(254).into()),
                            next: edit.next()?,
                        },
                    )?));
                }
                if matches!(self.fault, 4 | 5) {
                    let Some(MapValue::Inline(descriptor)) = edit.next()? else {
                        return Err(IndexError::CorruptObject(
                            "directory selector is not inline".into(),
                        ));
                    };
                    let child = descriptor.single_child()?.ok_or(IndexError::Conflict)?;
                    let data = if self.fault == 4 {
                        [2, 0, 1]
                    } else {
                        [1, 0, 3]
                    };
                    return Ok(Some(OrdinalMapEdit::unscoped(
                        edit.root(),
                        MapEdit {
                            key: edit.key().to_vec(),
                            expected: edit.expected()?,
                            next: Some(MapValue::inline(&data, Some(child))?),
                        },
                    )?));
                }
            } else if self.fault == 2 && !self.sent_late {
                self.sent_late = true;
                return Ok(self.first.clone());
            }
            return Ok(Some(edit));
        }
        if self.fault == 3 && !self.sent_late {
            self.sent_late = true;
            return Ok(Some(OrdinalMapEdit::unscoped(
                1,
                MapEdit::objects(b"late".to_vec(), None, Some(reference(253))),
            )?));
        }
        Ok(None)
    }
}

#[derive(Default)]
pub(super) struct CanonicalObjects {
    stored: Mutex<BTreeMap<ObjectRef, StoredObject>>,
    batch_inputs: AtomicUsize,
    block_inputs: AtomicUsize,
    wrong_second_block_output: AtomicBool,
    get_calls: AtomicUsize,
    retained_calls: AtomicUsize,
    refuse_retained: AtomicBool,
    deny_next_get: AtomicBool,
    bounded_install_memory: AtomicBool,
}

impl CanonicalObjects {
    pub(super) fn registration(
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

    pub(super) fn seed(
        &self,
        payload: Vec<u8>,
        children: Vec<ObjectRef>,
    ) -> Result<ObjectRef, IndexError> {
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
            get_calls: AtomicUsize::new(0),
            retained_calls: AtomicUsize::new(0),
            refuse_retained: AtomicBool::new(false),
            deny_next_get: AtomicBool::new(false),
            bounded_install_memory: AtomicBool::new(
                self.bounded_install_memory.load(Ordering::Relaxed),
            ),
        })
    }
}

impl ImmutableObjectPort for CanonicalObjects {
    fn install_batch_memory_bound(
        &self,
        _shape: quanta_index_core::index::ports::ObjectInstallMemoryShape,
    ) -> Result<Option<quanta_index_core::index::MemoryCreditLimits>, IndexError> {
        Ok(self
            .bounded_install_memory
            .load(Ordering::Relaxed)
            .then_some(quanta_index_core::index::MemoryCreditLimits {
                temporary_data_bytes: 0,
                retained_data_bytes: 0,
                max_live_temporary_leases: 0,
                max_live_retained_leases: 0,
            }))
    }

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
        let _old = self.get_calls.fetch_add(1, Ordering::Relaxed);
        if self.deny_next_get.swap(false, Ordering::Relaxed) {
            return Err(IndexError::ResourceLimited);
        }
        self.stored
            .lock()
            .map_err(|_error| IndexError::Storage("test object lock".into()))?
            .get(&reference)
            .cloned()
            .ok_or_else(|| IndexError::CorruptObject("test object missing".into()))
    }

    fn read_retained(
        &self,
        object: &StoredObject,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        budget.checkpoint()?;
        let _old = self.retained_calls.fetch_add(1, Ordering::Relaxed);
        if self.refuse_retained.load(Ordering::Relaxed) {
            return Err(IndexError::ResourceLimited);
        }
        self.get(object.registration.reference, budget)
    }
}

#[derive(Default)]
struct CanonicalMaps {
    roots: Mutex<BTreeMap<ObjectRef, Rows>>,
    next: AtomicU64,
    applies: AtomicUsize,
    malformed_directory_scan: AtomicBool,
    late_directory_scan: AtomicUsize,
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
        if self.malformed_directory_scan.load(Ordering::Relaxed) {
            return Ok(MapPage::new(Vec::new(), true, Vec::new(), None));
        }
        if self.late_directory_scan.load(Ordering::Relaxed) != 0 {
            let position = self.late_directory_scan.fetch_add(1, Ordering::Relaxed);
            if position >= 2 {
                return Ok(MapPage::new(Vec::new(), false, Vec::new(), None));
            }
        }
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
                prior.root() > edit.root()
                    || (prior.root() == edit.root() && prior.key() >= edit.key())
            }) {
                return Err(IndexError::Conflict);
            }
            let map = rows
                .get_mut(
                    usize::try_from(edit.root()).map_err(|_error| IndexError::ResourceLimited)?,
                )
                .ok_or(IndexError::Conflict)?;
            if map.get(edit.key()).copied() != edit.expected()? {
                return Err(IndexError::Conflict);
            }
            match edit.next()? {
                Some(next) => {
                    let _old = map.insert(edit.key().to_vec(), next);
                }
                None => {
                    let _old = map.remove(edit.key());
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
        source_repo_id: None,
        language: None,
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

fn packed_pair(first: &[u8], last: &[u8]) -> Result<Vec<u8>, IndexError> {
    let mut wire = b"QITXOC01".to_vec();
    wire.extend_from_slice(&2_u16.to_be_bytes());
    wire.extend_from_slice(&1_u16.to_be_bytes());
    wire.extend_from_slice(&0_u32.to_be_bytes());
    for key in [first, last] {
        wire.extend_from_slice(
            &u16::try_from(key.len())
                .map_err(|_| IndexError::ResourceLimited)?
                .to_be_bytes(),
        );
        wire.extend_from_slice(key);
        wire.extend_from_slice(&0_u16.to_be_bytes());
    }
    wire.extend_from_slice(&1_u16.to_be_bytes());
    wire.extend_from_slice(&0_u32.to_be_bytes());
    Ok(wire)
}

struct DenySecondPageRow {
    memory: Arc<TokenMemory>,
    retained: AtomicUsize,
}

impl quanta_index_core::index::MemoryScopePort for DenySecondPageRow {
    fn reserve(
        &self,
        bytes: u64,
        lifetime: quanta_index_core::index::MemoryLifetime,
    ) -> Result<quanta_index_core::index::MemoryLease, IndexError> {
        if lifetime == quanta_index_core::index::MemoryLifetime::Retained
            && self.retained.fetch_add(1, Ordering::Relaxed) == 2
        {
            return Err(IndexError::ResourceLimited);
        }
        token_memory_grant(&self.memory, bytes)
    }
}

#[test]
fn directory_cursor_borrows_previous_fence_under_tight_memory() -> Result<(), IndexError> {
    let objects = CanonicalObjects::default();
    let maps = CanonicalMaps::default();
    let first = stored_term_key("alpha", CaseMode::Sensitive)?;
    let second = stored_term_key("bravo", CaseMode::Sensitive)?;
    let payload = encode(&serde_json::json!({"positions": (0_u32..128).collect::<Vec<_>>()}))?;
    let parse_bytes = u64::try_from(payload.len())
        .ok()
        .and_then(|bytes| bytes.checked_mul(3))
        .ok_or(IndexError::ResourceLimited)?;
    let leaf = objects.seed(payload, Vec::new())?;
    let descriptor = MapValue::inline(&[2, 0, 1], Some(leaf))?;
    let root = maps
        .keep(BTreeMap::from([
            (first.clone(), descriptor),
            (second.clone(), descriptor),
        ]))?
        .0
        .ok_or(IndexError::ResourceLimited)?;
    // The adapter supplies map pages independently. The cursor can fund one
    // leaf parse, but cannot retain a private fence copy alongside it.
    let memory = Arc::new(TokenMemory {
        ceiling: parse_bytes,
        ..TokenMemory::default()
    });
    let mut work = integration_budget()?;
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        BatchOutputMemoryScope(Arc::clone(&memory)),
    ))));
    let mut cursor = entry_map::directory::Cursor::open(&objects, &maps, root);
    for (key, has_more) in [(first, true), (second, false)] {
        let page = cursor.next_page(1, &mut work)?;
        check_eq!(page.entries.as_slice(), &[(key, leaf.into())]);
        check_eq!(page.has_more, has_more);
    }
    let eof = cursor.next_page(1, &mut work)?;
    check!(eof.entries.is_empty());
    check!(!eof.has_more);
    check_eq!(objects.get_calls.load(Ordering::Relaxed), 2);
    drop((eof, cursor));
    check_eq!(memory.peak.load(Ordering::Relaxed), parse_bytes);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(work.first_denial().is_none());
    Ok(())
}

#[test]
fn directory_stateless_and_cached_scans_reject_overlapping_blocks() -> Result<(), IndexError> {
    let objects = CanonicalObjects::default();
    let maps = CanonicalMaps::default();
    let a = stored_term_key("a", CaseMode::Sensitive)?;
    let aa = stored_term_key("aa", CaseMode::Sensitive)?;
    let b = stored_term_key("b", CaseMode::Sensitive)?;
    let c = stored_term_key("c", CaseMode::Sensitive)?;
    let cc = stored_term_key("cc", CaseMode::Sensitive)?;
    let d = stored_term_key("d", CaseMode::Sensitive)?;
    let second = objects.seed(packed_pair(&b, &d)?, Vec::new())?;
    for (first_fence, hidden) in [(&c, &b), (&b, &aa)] {
        let first = objects.seed(packed_pair(&a, first_fence)?, Vec::new())?;
        let directory = maps
            .keep(BTreeMap::from([
                (
                    (*first_fence).clone(),
                    MapValue::inline(&[1, 0, 2], Some(first))?,
                ),
                (d.clone(), MapValue::inline(&[1, 0, 2], Some(second))?),
            ]))?
            .0
            .ok_or(IndexError::ResourceLimited)?;
        let root = EntryMap::InventoryDirectory(directory);
        // The fixed descriptor wire is otherwise valid; rejection must come
        // from its overlapping interval, including a touching first key.
        for after in [
            None,
            Some(a.as_slice()),
            Some(first_fence.as_slice()),
            Some(cc.as_slice()),
            Some(d.as_slice()),
        ] {
            let mut work = integration_budget()?;
            check!(matches!(
                root.scan(&objects, &maps, after, 4, &mut work),
                Err(IndexError::CorruptObject(_))
            ));
        }
        for key in [hidden.as_slice(), b.as_slice(), d.as_slice()] {
            let mut work = integration_budget()?;
            check!(matches!(
                root.lookup(&objects, &maps, key, &mut work),
                Err(IndexError::CorruptObject(_))
            ));
        }
        let mut cursor = entry_map::directory::Cursor::open(&objects, &maps, directory);
        let mut work = integration_budget()?;
        check_eq!(cursor.next_page(4, &mut work)?.entries.len(), 2);
        check!(matches!(
            cursor.next_page(4, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
    }
    Ok(())
}

#[test]
fn directory_stateless_interval_checks_accept_contiguous_and_gapped_blocks()
-> Result<(), IndexError> {
    let objects = CanonicalObjects::default();
    let maps = CanonicalMaps::default();
    let a = stored_term_key("a", CaseMode::Sensitive)?;
    let b = stored_term_key("b", CaseMode::Sensitive)?;
    let bb = stored_term_key("bb", CaseMode::Sensitive)?;
    let c = stored_term_key("c", CaseMode::Sensitive)?;
    let d = stored_term_key("d", CaseMode::Sensitive)?;
    let e = stored_term_key("e", CaseMode::Sensitive)?;
    let f = stored_term_key("f", CaseMode::Sensitive)?;
    let first = objects.seed(packed_pair(&a, &b)?, Vec::new())?;
    for (next_first, next_last) in [(&c, &d), (&e, &f)] {
        let second = objects.seed(packed_pair(next_first, next_last)?, Vec::new())?;
        let directory = maps
            .keep(BTreeMap::from([
                (b.clone(), MapValue::inline(&[1, 0, 2], Some(first))?),
                (
                    (*next_last).clone(),
                    MapValue::inline(&[1, 0, 2], Some(second))?,
                ),
            ]))?
            .0
            .ok_or(IndexError::ResourceLimited)?;
        let root = EntryMap::InventoryDirectory(directory);
        for (after, expected) in [
            (
                None,
                vec![
                    a.clone(),
                    b.clone(),
                    (*next_first).clone(),
                    (*next_last).clone(),
                ],
            ),
            (
                Some(a.as_slice()),
                vec![b.clone(), (*next_first).clone(), (*next_last).clone()],
            ),
            (
                Some(b.as_slice()),
                vec![(*next_first).clone(), (*next_last).clone()],
            ),
            (
                Some(bb.as_slice()),
                vec![(*next_first).clone(), (*next_last).clone()],
            ),
            (Some(next_last.as_slice()), Vec::new()),
        ] {
            let mut work = integration_budget()?;
            let page = root.scan(&objects, &maps, after, 4, &mut work)?;
            check_eq!(
                page.entries
                    .iter()
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>(),
                expected
            );
            check!(!page.has_more);
        }
        for (key, expected) in [
            (&a, Some(first)),
            (&b, Some(first)),
            (next_first, Some(second)),
            (next_last, Some(second)),
            (&bb, None),
        ] {
            let mut work = integration_budget()?;
            check_eq!(root.lookup(&objects, &maps, key, &mut work)?, expected);
        }
        // Refusal in the new interval seek returns no page and leaves a fresh
        // stateless retry able to read the same authenticated rows.
        maps.late_directory_scan.store(1, Ordering::Relaxed);
        let mut work = integration_budget()?;
        check!(matches!(
            root.scan(&objects, &maps, Some(b.as_slice()), 4, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
        maps.late_directory_scan.store(0, Ordering::Relaxed);
        let mut work = integration_budget()?;
        check_eq!(
            root.scan(&objects, &maps, Some(b.as_slice()), 4, &mut work)?
                .entries
                .len(),
            2
        );
        let mut cancelled = integration_budget()?;
        cancelled.request().cancel_handle().cancel();
        check!(matches!(
            root.scan(&objects, &maps, Some(b.as_slice()), 4, &mut cancelled),
            Err(IndexError::Cancelled)
        ));
    }
    Ok(())
}

#[test]
fn directory_leaf_fence_checks_packed_successor_interval() -> Result<(), IndexError> {
    let objects = CanonicalObjects::default();
    let maps = CanonicalMaps::default();
    let aa = stored_term_key("aa", CaseMode::Sensitive)?;
    let b = stored_term_key("b", CaseMode::Sensitive)?;
    let c = stored_term_key("c", CaseMode::Sensitive)?;
    let d = stored_term_key("d", CaseMode::Sensitive)?;
    let leaf = objects.seed(br#"{"positions":[0]}"#.to_vec(), Vec::new())?;
    for first in [&b, &c] {
        let packed = objects.seed(packed_pair(first, &d)?, Vec::new())?;
        let directory = maps
            .keep(BTreeMap::from([
                (b.clone(), MapValue::inline(&[2, 0, 1], Some(leaf))?),
                (d.clone(), MapValue::inline(&[1, 0, 2], Some(packed))?),
            ]))?
            .0
            .ok_or(IndexError::ResourceLimited)?;
        let root = EntryMap::InventoryDirectory(directory);
        let mut work = integration_budget()?;
        if first == &b {
            for key in [&aa, &b] {
                let mut work = integration_budget()?;
                check!(matches!(
                    root.lookup(&objects, &maps, key, &mut work),
                    Err(IndexError::CorruptObject(_))
                ));
            }
            check!(matches!(
                root.scan(&objects, &maps, Some(b.as_slice()), 4, &mut work),
                Err(IndexError::CorruptObject(_))
            ));
            let mut cursor = entry_map::directory::Cursor::open(&objects, &maps, directory);
            let mut work = integration_budget()?;
            check_eq!(cursor.next_page(4, &mut work)?.entries.len(), 1);
            check!(matches!(
                cursor.next_page(4, &mut work),
                Err(IndexError::CorruptObject(_))
            ));
        } else {
            check_eq!(root.lookup(&objects, &maps, &b, &mut work)?, Some(leaf));
            let mut work = integration_budget()?;
            check_eq!(root.lookup(&objects, &maps, &c, &mut work)?, Some(packed));
        }
    }
    Ok(())
}

#[test]
fn frozen_cold_small_inventory_borrows_planning_rows() -> Result<(), IndexError> {
    let workspace = visitor_fixture()?.ticket.view.identity.workspace;
    let objects = CanonicalObjects::default();
    let maps = CanonicalMaps::default();
    let scratch = TestScratch::default();
    let mut work = integration_budget()?;
    let spool = TextSpool::new(&scratch, &mut work)?;
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        super::token_spool_batch::CreditOutputMemoryScope {
            memory: Arc::new(TokenMemory {
                ceiling: 100_000_000,
                ..TokenMemory::default()
            }),
            attempts: Arc::new(AtomicUsize::new(0)),
        },
    ))));
    let rows = [([0, b'a'], reference(17)), ([0, b'b'], reference(19))];
    spool.inventory_batch(
        &rows.map(|(key, reference)| {
            (
                InventoryPartition::Ready(0),
                MapEdit::objects(key.to_vec(), None, Some(reference)),
            )
        }),
        &mut work,
    )?;
    let lease = spool.begin_inventory_read(&mut work)?;
    let before = work.usage();
    let mut context = AttemptContext {
        attempt: test_attempt(workspace),
        objects: &objects,
        maps: &maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    let roots = entry_patch::apply_frozen_inventory(
        entry_map::EntryMapUse::ColdInventory,
        &[EntryMap::default()],
        &[2],
        &[None],
        &lease,
        &mut context,
    )?;
    let small = roots
        .as_slice()
        .first()
        .and_then(EntryMap::inline_map)
        .ok_or(IndexError::Conflict)?;
    check_eq!(small.len(), 2);
    for (slot, (key, reference)) in rows.iter().enumerate() {
        check_eq!(small.entry(slot)?, Some((key.as_slice(), *reference)));
    }
    check_eq!(maps.applies.load(Ordering::Relaxed), 0);
    let after = context.budget.usage();
    // Per pass: one 4096-byte query, two 2-byte order keys and two 76-byte
    // original edit digests. Planning and global verification each read both.
    check_eq!(after.reads.checked_sub(before.reads), Some(4));
    check_eq!(after.bytes.checked_sub(before.bytes), Some(8504));

    drop(lease);
    spool.reset_inventories(context.budget)?;
    spool.inventory_batch(
        &[(
            InventoryPartition::Ready(0),
            MapEdit::objects(vec![0, b'a'], Some(reference(17)), Some(reference(19))),
        )],
        context.budget,
    )?;
    let lease = spool.begin_inventory_read(context.budget)?;
    check!(matches!(
        entry_patch::apply_frozen_inventory(
            entry_map::EntryMapUse::ColdInventory,
            &[EntryMap::default()],
            &[1],
            &[None],
            &lease,
            &mut context,
        ),
        Err(IndexError::Conflict)
    ));
    check_eq!(maps.applies.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn frozen_cold_directories_stage_together_and_reopen_by_original_slot() -> Result<(), IndexError> {
    let workspace = visitor_fixture()?.ticket.view.identity.workspace;
    let objects = CanonicalObjects::default();
    objects
        .bounded_install_memory
        .store(true, Ordering::Relaxed);
    let maps = CanonicalMaps::default();
    let scratch = TestScratch::default();
    let mut work = integration_budget()?;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let memory = Arc::new(TokenMemory {
        ceiling: 100_000_000,
        ..TokenMemory::default()
    });
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        super::token_spool_batch::CreditOutputMemoryScope {
            memory,
            attempts: Arc::new(AtomicUsize::new(0)),
        },
    ))));
    let mut context = AttemptContext {
        attempt: test_attempt(workspace),
        objects: &objects,
        maps: &maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    let body = distinct_terms(257);
    let other = body.replace('t', "z");
    let mut writes = OccurrenceWrites::new();
    writes.pack_owners = true;
    let mut confirmed = ConfirmedOccurrences::new(0)?;
    let mut aliases = [None; 3];
    let mut counts = [0; 3];
    for (slot, source) in [(0, body.as_str()), (2, other.as_str())] {
        let ordinal = u32::try_from(slot).map_err(|_width| IndexError::ResourceLimited)?;
        let mut edits = InventoryEdits::new(ordinal)?;
        let prepared = prepare_inventory(
            source,
            &EntryMap::default(),
            ordinal,
            &mut context,
            &spool,
            &mut confirmed,
            &mut edits,
            &mut writes,
        )?;
        let alias = prepared.alias.ok_or(IndexError::Conflict)?;
        check_eq!((alias.terms, alias.anchors), (514, 2));
        check_eq!(alias.layout, entry_patch::ColdAliasLayout::Directory);
        writes.flush(&mut context, &spool, &mut confirmed, &mut edits)?;
        edits.flush(&spool, context.budget)?;
        *aliases.get_mut(slot).ok_or(IndexError::Conflict)? = Some(alias);
        *counts.get_mut(slot).ok_or(IndexError::Conflict)? = alias.anchors;
    }

    let read = spool.begin_inventory_read(context.budget)?;
    let roots = entry_patch::apply_frozen_inventory(
        entry_map::EntryMapUse::ColdInventory,
        &[EntryMap::default(); 3],
        &counts,
        &aliases,
        &read,
        &mut context,
    )?;
    check_eq!(roots.as_slice().get(1), Some(&EntryMap::default()));
    let reopened = objects.reopened()?;
    for (slot, term, foreign) in [(0, "t000", "z000"), (2, "z000", "t000")] {
        let root = *roots.as_slice().get(slot).ok_or(IndexError::Conflict)?;
        let alias = aliases
            .get(slot)
            .copied()
            .flatten()
            .ok_or(IndexError::Conflict)?;
        let root =
            alias.verify_directory(root, &reopened, &maps, context.attempt, context.budget)?;
        let key = stored_term_key(term, CaseMode::Sensitive)?;
        let foreign = stored_term_key(foreign, CaseMode::Sensitive)?;
        check!(
            root.lookup(&reopened, &maps, &key, context.budget)?
                .is_some()
        );
        check!(
            root.lookup(&reopened, &maps, &foreign, context.budget)?
                .is_none()
        );
    }
    check_eq!(maps.applies.load(Ordering::Relaxed), 1);

    // A later root that declares too few anchors must refuse the entire map
    // stream after the first root has already reached its own EOF.
    let before = maps.roots.lock().map_err(|_| IndexError::Conflict)?.len();
    let mut truncated = aliases;
    truncated
        .get_mut(2)
        .and_then(Option::as_mut)
        .ok_or(IndexError::Conflict)?
        .anchors = 1;
    check!(matches!(
        entry_patch::apply_frozen_inventory(
            entry_map::EntryMapUse::ColdInventory,
            &[EntryMap::default(); 3],
            &[2, 0, 1],
            &truncated,
            &read,
            &mut context,
        ),
        Err(IndexError::CorruptObject(message)) if message.contains("original count exceeds")
    ));
    check_eq!(
        maps.roots.lock().map_err(|_| IndexError::Conflict)?.len(),
        before
    );
    check_eq!(maps.applies.load(Ordering::Relaxed), 2);
    Ok(())
}

#[test]
fn cold_inventory_above_block_limit_uses_directory_and_reopens_complete_rows()
-> Result<(), IndexError> {
    let workspace = visitor_fixture()?.ticket.view.identity.workspace;
    let objects = CanonicalObjects::default();
    objects
        .bounded_install_memory
        .store(true, Ordering::Relaxed);
    let maps = CanonicalMaps::default();
    let scratch = TestScratch::default();
    let mut work = integration_budget()?;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let memory = Arc::new(TokenMemory {
        ceiling: 100_000_000,
        ..TokenMemory::default()
    });
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        super::token_spool_batch::CreditOutputMemoryScope {
            memory,
            attempts: Arc::new(AtomicUsize::new(0)),
        },
    ))));
    let mut context = AttemptContext {
        attempt: test_attempt(workspace),
        objects: &objects,
        maps: &maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    let mut edits = InventoryEdits::new(0)?;
    let mut writes = OccurrenceWrites::new();
    writes.pack_owners = true;
    let mut confirmed = ConfirmedOccurrences::new(0)?;
    let prepared = prepare_inventory(
        &distinct_terms(257),
        &EntryMap::default(),
        0,
        &mut context,
        &spool,
        &mut confirmed,
        &mut edits,
        &mut writes,
    )?;
    check!(prepared.term_count > 512);
    check!(prepared.alias.is_some());
    writes.flush(&mut context, &spool, &mut confirmed, &mut edits)?;
    edits.flush(&spool, context.budget)?;
    let roots = entry_patch::apply_with_cold_aliases(
        entry_map::EntryMapUse::ColdInventory,
        &[EntryMap::default()],
        &[prepared
            .alias
            .map_or(prepared.term_count, |alias| alias.anchors)],
        &[prepared.alias],
        &mut spool.inventory_cursor(1)?,
        |slot| spool.inventory_root_cursor(slot),
        &mut context,
        None,
    )?;
    let anchor = *roots
        .as_slice()
        .first()
        .ok_or(IndexError::ResourceLimited)?;
    let root = prepared
        .alias
        .ok_or(IndexError::ResourceLimited)?
        .verify_directory(
            anchor,
            context.objects,
            context.maps,
            context.attempt,
            context.budget,
        )?;
    check!(matches!(root, EntryMap::InventoryDirectory(_)));
    let EntryMap::InventoryDirectory(directory_ref) = root else {
        return Err(IndexError::BackendViolation("cold directory absent".into()));
    };
    let descriptors = maps.rows(MapRoot(Some(directory_ref)))?;
    check!(descriptors.len() >= 2);
    check!(
        descriptors.len()
            < usize::try_from(prepared.term_count).map_err(|_| IndexError::ResourceLimited)?
    );
    check!(
        descriptors
            .values()
            .all(|value| matches!(value, MapValue::Inline(_)))
    );
    check_eq!(prepared.term_count, 514);
    check_eq!(descriptors.len(), 2);
    // The reference page cursor still copies each authenticated key into an
    // owned MapPage. The cold alias visitor can borrow those same 514 keys
    // while retaining its independent key and descriptor seals.
    let mut page_cursor = entry_map::directory::Cursor::open(&objects, &maps, directory_ref);
    let page_gets_before = objects.get_calls.load(Ordering::Relaxed);
    let page_work_before = context.budget.usage();
    let mut page_rows = 0_usize;
    loop {
        let page = page_cursor.next_page(512, context.budget)?;
        check!(!page.entries.is_empty());
        for (key, _) in &page.entries {
            check_eq!(key.len(), 5);
            page_rows = page_rows
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
        }
        if !page.has_more {
            let tail = page_cursor.next_page(512, context.budget)?;
            check!(tail.entries.is_empty() && !tail.has_more);
            break;
        }
    }
    let page_bytes = context
        .budget
        .usage()
        .bytes
        .checked_sub(page_work_before.bytes)
        .ok_or(IndexError::ResourceLimited)?;
    let page_gets = objects
        .get_calls
        .load(Ordering::Relaxed)
        .checked_sub(page_gets_before)
        .ok_or(IndexError::ResourceLimited)?;
    let alias_gets_before = objects.get_calls.load(Ordering::Relaxed);
    let alias_work_before = context.budget.usage();
    check_eq!(
        prepared
            .alias
            .ok_or(IndexError::ResourceLimited)?
            .verify_directory(
                root,
                context.objects,
                context.maps,
                context.attempt,
                context.budget,
            )?,
        root
    );
    let alias_bytes = context
        .budget
        .usage()
        .bytes
        .checked_sub(alias_work_before.bytes)
        .ok_or(IndexError::ResourceLimited)?;
    let alias_gets = objects
        .get_calls
        .load(Ordering::Relaxed)
        .checked_sub(alias_gets_before)
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(page_rows, 514);
    check_eq!((page_gets, alias_gets), (2, 2));
    // 514 key seals at 5+8 bytes plus two descriptor seals at 5+43 bytes,
    // with the page cursor's 514*5-byte owned-key copy removed.
    check_eq!(
        alias_bytes
            .checked_sub(page_bytes)
            .ok_or(IndexError::ResourceLimited)?,
        514 * (5 + 8) + 2 * (5 + 43) - 514 * 5
    );
    let mut blocks = descriptors.iter();
    let (_full_fence, full_descriptor) = blocks.next().ok_or(IndexError::ResourceLimited)?;
    let (_short_fence, short_descriptor) = blocks.next().ok_or(IndexError::ResourceLimited)?;
    let MapValue::Inline(full_inline) = full_descriptor else {
        return Err(IndexError::CorruptObject(
            "full directory descriptor absent".into(),
        ));
    };
    let MapValue::Inline(short_inline) = short_descriptor else {
        return Err(IndexError::CorruptObject(
            "short directory descriptor absent".into(),
        ));
    };
    check_eq!(full_inline.data()?, &[1, 2, 0]);
    check_eq!(short_inline.data()?, &[1, 0, 2]);
    let full_ref = full_descriptor
        .single_child()?
        .ok_or(IndexError::ResourceLimited)?;
    let short_ref = short_descriptor
        .single_child()?
        .ok_or(IndexError::ResourceLimited)?;
    let leaf_key = stored_term_key("zzz", CaseMode::Folded)?;
    let leaf_ref = objects.seed(br#"{"positions":[0]}"#.to_vec(), Vec::new())?;
    let mut extended = descriptors.clone();
    check_eq!(
        extended.insert(
            leaf_key.clone(),
            MapValue::inline(&[2, 0, 1], Some(leaf_ref))?
        ),
        None
    );
    let extended_ref = maps.keep(extended)?.0.ok_or(IndexError::ResourceLimited)?;
    let extended_root = EntryMap::InventoryDirectory(extended_ref);
    let mut block_pages = entry_map::directory::Cursor::open(&objects, &maps, extended_ref);
    check!(matches!(
        block_pages.next_page(513, context.budget),
        Err(IndexError::InvalidInput(_))
    ));
    let full_page = block_pages.next_page(512, context.budget)?;
    check_eq!(full_page.entries.len(), 512);
    check!(full_page.has_more);
    let short_page = block_pages.next_page(512, context.budget)?;
    check_eq!(short_page.entries.len(), 2);
    check!(short_page.has_more);
    let final_page = block_pages.next_page(512, context.budget)?;
    check_eq!(
        final_page.entries.as_slice(),
        &[(leaf_key.clone(), leaf_ref.into())]
    );
    check!(!final_page.has_more);
    drop((full_page, short_page, final_page, block_pages));
    let initial_gets = objects.get_calls.load(Ordering::Relaxed);
    let initial_retained = objects.retained_calls.load(Ordering::Relaxed);
    let mut bulk = MapEntries::new(&objects, &maps, extended_root);
    let mut ordinal = 0_usize;
    for case in [CaseMode::Sensitive, CaseMode::Folded] {
        for term in 0..257_u32 {
            let expected_key = stored_term_key(&format!("t{term:03}"), case)?;
            let row = bulk
                .next(context.budget)?
                .ok_or(IndexError::ResourceLimited)?;
            let (key, reference) = row.object_parts()?;
            check_eq!(key.as_slice(), expected_key.as_slice());
            check_eq!(*reference, if ordinal < 512 { full_ref } else { short_ref });
            ordinal += 1;
        }
    }
    let last = bulk
        .next(context.budget)?
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(last.object_parts()?, (&leaf_key, &leaf_ref));
    check!(bulk.next(context.budget)?.is_none());
    check_eq!(
        objects.retained_calls.load(Ordering::Relaxed) - initial_retained,
        0
    );
    check_eq!(objects.get_calls.load(Ordering::Relaxed) - initial_gets, 3);

    let initial_gets = objects.get_calls.load(Ordering::Relaxed);
    let initial_retained = objects.retained_calls.load(Ordering::Relaxed);
    let mut narrow = entry_map::directory::Cursor::open(&objects, &maps, extended_ref);
    for _page in 0..8 {
        let page = narrow.next_page(64, context.budget)?;
        check_eq!(page.entries.len(), 64);
        check!(page.has_more);
    }
    check_eq!(
        objects.retained_calls.load(Ordering::Relaxed) - initial_retained,
        7
    );
    check_eq!(objects.get_calls.load(Ordering::Relaxed) - initial_gets, 8);
    check_eq!(narrow.next_page(64, context.budget)?.entries.len(), 2);
    let leaf_page = narrow.next_page(64, context.budget)?;
    check_eq!(
        leaf_page.entries.as_slice(),
        &[(leaf_key.clone(), leaf_ref.into())]
    );
    check!(!leaf_page.has_more);
    check!(narrow.next_page(64, context.budget)?.entries.is_empty());

    let mut refused = entry_map::directory::Cursor::open(&objects, &maps, extended_ref);
    check_eq!(refused.next_page(64, context.budget)?.entries.len(), 64);
    objects.refuse_retained.store(true, Ordering::Relaxed);
    check!(matches!(
        refused.next_page(64, context.budget),
        Err(IndexError::ResourceLimited)
    ));
    objects.refuse_retained.store(false, Ordering::Relaxed);
    check_eq!(refused.next_page(64, context.budget)?.entries.len(), 64);
    let mut cancelled = entry_map::directory::Cursor::open(&objects, &maps, extended_ref);
    check_eq!(cancelled.next_page(64, context.budget)?.entries.len(), 64);
    let mut cancelled_work = integration_budget()?;
    cancelled_work.request().cancel_handle().cancel();
    check!(matches!(
        cancelled.next_page(64, &mut cancelled_work),
        Err(IndexError::Cancelled)
    ));

    let memory = Arc::new(TokenMemory {
        ceiling: 100_000_000,
        ..TokenMemory::default()
    });
    let denial = Arc::new(DenySecondPageRow {
        memory: Arc::clone(&memory),
        retained: AtomicUsize::new(0),
    });
    let mut denied_work = integration_budget()?;
    denied_work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(
        denial.clone(),
    )));
    let mut partial = entry_map::directory::Cursor::open(&objects, &maps, extended_ref);
    check!(matches!(
        partial.next_page(2, &mut denied_work),
        Err(IndexError::ResourceLimited)
    ));
    let mut retry_work = integration_budget()?;
    retry_work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(denial)));
    let retried = partial.next_page(2, &mut retry_work)?;
    check_eq!(
        retried.entries.as_slice(),
        &[
            (
                stored_term_key("t000", CaseMode::Sensitive)?,
                full_ref.into()
            ),
            (
                stored_term_key("t001", CaseMode::Sensitive)?,
                full_ref.into()
            ),
        ]
    );
    drop((retried, partial));
    check_eq!(memory.live.load(Ordering::Relaxed), 0);

    let mut boundary = entry_map::directory::Cursor::open(&objects, &maps, extended_ref);
    let mut first_work = integration_budget()?;
    check_eq!(boundary.next_page(512, &mut first_work)?.entries.len(), 512);
    objects.deny_next_get.store(true, Ordering::Relaxed);
    let mut denied_work = integration_budget()?;
    check!(matches!(
        boundary.next_page(512, &mut denied_work),
        Err(IndexError::ResourceLimited)
    ));
    let denied_gets = objects.get_calls.load(Ordering::Relaxed);
    let mut retry_work = integration_budget()?;
    check_eq!(boundary.next_page(512, &mut retry_work)?.entries.len(), 2);
    check!(objects.get_calls.load(Ordering::Relaxed) > denied_gets);
    objects.deny_next_get.store(true, Ordering::Relaxed);
    let mut denied_work = integration_budget()?;
    check!(matches!(
        boundary.next_page(512, &mut denied_work),
        Err(IndexError::ResourceLimited)
    ));
    let denied_gets = objects.get_calls.load(Ordering::Relaxed);
    let mut retry_work = integration_budget()?;
    check_eq!(
        boundary.next_page(512, &mut retry_work)?.entries.as_slice(),
        &[(leaf_key.clone(), leaf_ref.into())]
    );
    check!(objects.get_calls.load(Ordering::Relaxed) > denied_gets);

    maps.malformed_directory_scan.store(true, Ordering::Relaxed);
    check!(
        MapEntries::new(&objects, &maps, root)
            .next(context.budget)
            .is_err()
    );
    maps.malformed_directory_scan
        .store(false, Ordering::Relaxed);
    maps.late_directory_scan.store(1, Ordering::Relaxed);
    let mut late_cursor = MapEntries::new(&objects, &maps, root);
    let mut late_refused = false;
    for _ in 0..=prepared.term_count {
        match late_cursor.next(context.budget) {
            Ok(Some(_row)) => {}
            Err(_) => {
                late_refused = true;
                break;
            }
            Ok(None) => break,
        }
    }
    check!(late_refused);
    maps.late_directory_scan.store(1, Ordering::Relaxed);
    check!(
        root.scan(&objects, &maps, None, 4096, context.budget)
            .is_err()
    );
    maps.late_directory_scan.store(0, Ordering::Relaxed);
    let (first_fence, first_value) = descriptors
        .iter()
        .next()
        .ok_or(IndexError::ResourceLimited)?;
    let first_child = first_value
        .single_child()?
        .ok_or(IndexError::ResourceLimited)?;
    let mut bad_fence = descriptors.clone();
    let _prior = bad_fence.remove(first_fence);
    let mut changed_fence = first_fence.clone();
    changed_fence.push(0);
    let _prior = bad_fence.insert(changed_fence, *first_value);
    let bad_fence_root = maps.keep(bad_fence)?.0.ok_or(IndexError::ResourceLimited)?;
    check!(
        MapEntries::new(
            &objects,
            &maps,
            EntryMap::InventoryDirectory(bad_fence_root)
        )
        .next(context.budget)
        .is_err()
    );
    let mut bad_count = descriptors.clone();
    let _prior = bad_count.insert(
        first_fence.clone(),
        MapValue::inline(&[1, 0, 2], Some(first_child))?,
    );
    let bad_count_root = maps.keep(bad_count)?.0.ok_or(IndexError::ResourceLimited)?;
    check!(
        MapEntries::new(
            &objects,
            &maps,
            EntryMap::InventoryDirectory(bad_count_root)
        )
        .next(context.budget)
        .is_err()
    );
    let mut bad_child = descriptors.clone();
    let MapValue::Inline(first_inline) = first_value else {
        return Err(IndexError::BackendViolation(
            "directory descriptor not inline".into(),
        ));
    };
    let _prior = bad_child.insert(
        first_fence.clone(),
        MapValue::inline(first_inline.data()?, Some(reference(254)))?,
    );
    let bad_child_root = maps.keep(bad_child)?.0.ok_or(IndexError::ResourceLimited)?;
    check!(
        MapEntries::new(
            &objects,
            &maps,
            EntryMap::InventoryDirectory(bad_child_root)
        )
        .next(context.budget)
        .is_err()
    );
    let mut cursor = MapEntries::new(&objects, &maps, root);
    let mut count = 0_u64;
    let mut prior = None;
    while let Some(row) = cursor.next(context.budget)? {
        let (key, reference) = row.object_parts()?;
        check!(
            prior
                .as_ref()
                .is_none_or(|old: &Vec<u8>| old.as_slice() < key.as_slice())
        );
        check_eq!(
            root.lookup(&objects, &maps, key, context.budget)?,
            Some(*reference)
        );
        prior = Some(key.clone());
        count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    }
    check_eq!(count, prepared.term_count);
    // A descriptor's kind and term count are sealed DATA even when its
    // occurrence child stays identical to the original selector.
    for fault in 1..=5 {
        let mut bad = DirectoryFaultCursor {
            inner: spool.inventory_cursor(1)?,
            fault,
            first: None,
            sent_late: false,
        };
        check!(
            entry_patch::apply_with_cold_aliases(
                entry_map::EntryMapUse::ColdInventory,
                &[EntryMap::default()],
                &[prepared
                    .alias
                    .map_or(prepared.term_count, |alias| alias.anchors)],
                &[prepared.alias],
                &mut bad,
                |slot| spool.inventory_root_cursor(slot),
                &mut context,
                None,
            )
            .is_err()
        );
    }
    check_eq!(
        root.scan(&objects, &maps, prior.as_deref(), 64, context.budget)?
            .entries
            .len(),
        0
    );
    Ok(())
}

fn check_inline_owner_inventory(
    member: &MemberBinding,
    owner: OwnerKey,
    record: ObjectRef,
    tokens: u64,
    tag: u8,
    objects: &CanonicalObjects,
    maps: &CanonicalMaps,
) -> Result<ObjectRef, IndexError> {
    let mut work = integration_budget()?;
    let root = read_root(member, objects, &mut work)?;
    let value = maps
        .lookup(root.owners, &owner_key(owner), &mut work)?
        .ok_or_else(|| IndexError::CorruptObject("owner contribution absent".into()))?;
    let MapValue::Inline(inline) = value else {
        return Err(IndexError::BackendViolation(
            "dense owner used a separate contribution object".into(),
        ));
    };
    let logical = read_contribution(value, objects, &mut work)?;
    let inventory = match (tag, logical.inventory) {
        (0x80, EntryMap::Block(reference))
        | (0x81, EntryMap::Tree(reference))
        | (0x82, EntryMap::OccurrenceBlock(reference))
        | (0x83, EntryMap::InventoryDirectory(reference)) => reference,
        _ => {
            return Err(IndexError::CorruptObject(
                "unexpected inventory shape".into(),
            ));
        }
    };
    let mut bytes = [tag; 9];
    bytes
        .get_mut(1..)
        .ok_or(IndexError::ResourceLimited)?
        .copy_from_slice(&tokens.to_be_bytes());
    check_eq!(inline.data()?, &bytes);
    check_eq!(inline.children()?, &[record, inventory]);
    check_eq!(logical.record, record);
    check_eq!(logical.indexed_tokens, tokens);
    let wrapper = CanonicalObjects::registration(&encode(&logical)?, vec![record, inventory])?;
    check!(
        !objects
            .stored
            .lock()
            .map_err(|_| IndexError::Storage("test object lock".into()))?
            .contains_key(&wrapper.reference)
    );
    Ok(inventory)
}

#[test]
fn cold_multiblock_alias_stages_two_selectors_and_reopens_full_directory() -> Result<(), IndexError>
{
    assert_cold_multiblock_alias("t256")?;
    assert_cold_multiblock_alias(&"x".repeat(32))
}

fn assert_cold_multiblock_alias(last_term: &str) -> Result<(), IndexError> {
    let workspace = visitor_fixture()?.ticket.view.identity.workspace;
    let objects = CanonicalObjects::default();
    objects
        .bounded_install_memory
        .store(true, Ordering::Relaxed);
    let maps = CanonicalMaps::default();
    let scratch = TestScratch::default();
    let mut work = integration_budget()?;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let memory = Arc::new(TokenMemory {
        ceiling: 100_000_000,
        ..TokenMemory::default()
    });
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        super::token_spool_batch::CreditOutputMemoryScope {
            memory,
            attempts: Arc::new(AtomicUsize::new(0)),
        },
    ))));
    let mut context = AttemptContext {
        attempt: test_attempt(workspace),
        objects: &objects,
        maps: &maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    let mut edits = InventoryEdits::new(0)?;
    let mut writes = OccurrenceWrites::new();
    writes.pack_owners = true;
    let mut confirmed = ConfirmedOccurrences::new(0)?;
    let prepared = prepare_inventory(
        &format!("{} {last_term}", distinct_terms(256)),
        &EntryMap::default(),
        0,
        &mut context,
        &spool,
        &mut confirmed,
        &mut edits,
        &mut writes,
    )?;
    check!(prepared.changed);
    check_eq!(prepared.term_count, 514);
    writes.flush(&mut context, &spool, &mut confirmed, &mut edits)?;
    edits.flush(&spool, context.budget)?;
    let alias = prepared.alias.ok_or_else(|| {
        IndexError::BackendViolation("multi-block cold alias witness absent".into())
    })?;
    check_eq!(alias.terms, prepared.term_count);
    check_eq!(alias.anchors, 2);
    check_eq!(alias.layout, entry_patch::ColdAliasLayout::Directory);
    let mut staged = spool.inventory_cursor(1)?;
    let mut staged_refs = [None; 2];
    for (slot, (fence, data)) in [
        (stored_term_key("t254", CaseMode::Folded)?, [1, 2, 0]),
        (stored_term_key(last_term, CaseMode::Folded)?, [1, 0, 2]),
    ]
    .into_iter()
    .enumerate()
    {
        let row =
            quanta_index_core::index::ports::MapEditCursor::next_edit(&mut staged, context.budget)?
                .ok_or(IndexError::ResourceLimited)?;
        check_eq!(row.root(), 0);
        check_eq!(row.key(), fence.as_slice());
        check_eq!(row.expected()?, None);
        let Some(MapValue::Inline(value)) = row.next()? else {
            return Err(IndexError::CorruptObject(
                "directory selector is not inline".into(),
            ));
        };
        check_eq!(value.data()?, data.as_slice());
        staged_refs[slot] = value.single_child()?;
    }
    check!(
        quanta_index_core::index::ports::MapEditCursor::next_edit(&mut staged, context.budget)?
            .is_none()
    );
    check_eq!(staged_refs[0], Some(alias.reference));
    check!(staged_refs[1].is_some() && staged_refs[1] != staged_refs[0]);
    let selector_rows = 2;
    let roots = entry_patch::apply_with_cold_aliases(
        entry_map::EntryMapUse::ColdInventory,
        &[EntryMap::default()],
        &[selector_rows],
        &[Some(alias)],
        &mut spool.inventory_cursor(1)?,
        |slot| spool.inventory_root_cursor(slot),
        &mut context,
        None,
    )?;
    let anchor = roots
        .as_slice()
        .first()
        .ok_or(IndexError::ResourceLimited)?;
    let root = alias.verify_directory(
        *anchor,
        context.objects,
        context.maps,
        context.attempt,
        context.budget,
    )?;
    let EntryMap::InventoryDirectory(directory) = root else {
        return Err(IndexError::CorruptObject(
            "multi-block alias did not seal a directory".into(),
        ));
    };
    let descriptors = maps.rows(MapRoot(Some(directory)))?;
    check_eq!(descriptors.len(), 2);
    let mut block_refs = descriptors
        .values()
        .map(MapValue::single_child)
        .collect::<Result<Vec<_>, _>>()?;
    let second = block_refs
        .pop()
        .flatten()
        .ok_or(IndexError::ResourceLimited)?;
    let first = block_refs
        .pop()
        .flatten()
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!([Some(first), Some(second)], staged_refs);
    let reopened = objects.reopened()?;
    let mut rows = MapEntries::new(&reopened, &maps, root);
    let mut ordinal = 0_usize;
    for case in [CaseMode::Sensitive, CaseMode::Folded] {
        for term in 0..257_u32 {
            let expected_key = stored_term_key(
                &if term == 256 {
                    last_term.to_owned()
                } else {
                    format!("t{term:03}")
                },
                case,
            )?;
            if term == 256 {
                check_eq!(expected_key.len(), last_term.len() + 1);
            }
            let row = rows
                .next(context.budget)?
                .ok_or(IndexError::ResourceLimited)?;
            let (key, reference) = row.object_parts()?;
            check_eq!(key.as_slice(), expected_key.as_slice());
            check_eq!(*reference, if ordinal < 512 { first } else { second });
            ordinal = ordinal.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
    }
    check_eq!(ordinal, 514);
    check!(rows.next(context.budget)?.is_none());
    let mut changed_payload = objects.get(second, context.budget)?.payload.clone();
    let position_byte = changed_payload
        .last_mut()
        .ok_or(IndexError::ResourceLimited)?;
    *position_byte = (*position_byte).wrapping_add(1);
    let changed_ref = objects.seed(changed_payload, Vec::new())?;
    check!(changed_ref != second);
    let changed_checked = crate::native::text_occurrence::block::CheckedBlock::parse(
        objects.get(changed_ref, context.budget)?,
        context.budget,
    )?;
    check_eq!(changed_checked.term_count(), 2);
    check_eq!(
        changed_checked.key(0)?,
        stored_term_key("t255", CaseMode::Folded)?.as_slice()
    );
    check_eq!(
        changed_checked.key(1)?,
        stored_term_key(last_term, CaseMode::Folded)?.as_slice()
    );
    drop(changed_checked);
    let mut changed_descriptors = descriptors.clone();
    let (last_fence, last_value) = descriptors
        .iter()
        .last()
        .ok_or(IndexError::ResourceLimited)?;
    let MapValue::Inline(last_inline) = last_value else {
        return Err(IndexError::ResourceLimited);
    };
    let _prior = changed_descriptors.insert(
        last_fence.clone(),
        MapValue::inline(last_inline.data()?, Some(changed_ref))?,
    );
    let changed_directory = maps
        .keep(changed_descriptors)?
        .0
        .ok_or(IndexError::ResourceLimited)?;
    check!(matches!(
        alias.verify_directory(
            EntryMap::InventoryDirectory(changed_directory),
            context.objects,
            context.maps,
            context.attempt,
            context.budget,
        ),
        Err(IndexError::CorruptObject(_))
    ));
    let mut cancelled = integration_budget()?;
    cancelled.request().cancel_handle().cancel();
    check!(matches!(
        alias.verify_directory(
            root,
            context.objects,
            context.maps,
            context.attempt,
            &mut cancelled,
        ),
        Err(IndexError::Cancelled)
    ));
    Ok(())
}

#[test]
fn cold_key_seal_accepts_canonical_maximum_for_both_block_shapes() -> Result<(), IndexError> {
    let mut budget = integration_budget()?;
    let mut short = entry_map::occurrence::ColdInventoryKeySeal::new();
    short.push(b"\0a", &mut budget)?;
    check_eq!(
        short.finish(),
        Digest([
            0x2e, 0x70, 0x2c, 0x20, 0xf0, 0x91, 0x24, 0x43, 0xa5, 0x95, 0xf7, 0x0e, 0x17, 0xce,
            0x3b, 0xd8, 0xe1, 0xf6, 0x0d, 0x21, 0xdf, 0x7d, 0x53, 0x6e, 0xa6, 0x3b, 0x7e, 0x4b,
            0x28, 0xfd, 0xb3, 0x36,
        ])
    );
    let mut key = vec![b'x'; 1_024];
    key[0] = 0;
    check_term_key(&key)?;
    let mut single = entry_map::occurrence::ColdInventoryKeySeal::new();
    single.push(&key, &mut budget)?;
    key.push(b'x');
    check!(matches!(
        check_term_key(&key),
        Err(IndexError::CorruptObject(_))
    ));
    check!(matches!(
        single.push(&key, &mut budget),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn cold_one_block_long_keys_use_one_authenticated_directory_anchor() -> Result<(), IndexError> {
    // Nine distinct 32-byte ASCII identifiers become 18 canonical 33-byte
    // case keys. This fixed source fits one QITXOC01 block under 8 KiB.
    let names = (0_u32..9)
        .map(|ordinal| format!("id{ordinal:02}{}", "x".repeat(28)))
        .collect::<Vec<_>>();
    let body = names.join(" ");
    let workspace = visitor_fixture()?.ticket.view.identity.workspace;
    let objects = CanonicalObjects::default();
    objects
        .bounded_install_memory
        .store(true, Ordering::Relaxed);
    let maps = CanonicalMaps::default();
    let scratch = TestScratch::default();
    let mut work = integration_budget()?;
    let memory = Arc::new(TokenMemory {
        ceiling: 100_000_000,
        ..TokenMemory::default()
    });
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        super::token_spool_batch::CreditOutputMemoryScope {
            memory,
            attempts: Arc::new(AtomicUsize::new(0)),
        },
    ))));
    let spool = TextSpool::new(&scratch, &mut work)?;
    let mut context = AttemptContext {
        attempt: test_attempt(workspace),
        objects: &objects,
        maps: &maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    let mut edits = InventoryEdits::new(0)?;
    let mut writes = OccurrenceWrites::new();
    writes.pack_owners = true;
    let mut confirmed = ConfirmedOccurrences::new(0)?;
    let prepared = prepare_inventory(
        &body,
        &EntryMap::default(),
        0,
        &mut context,
        &spool,
        &mut confirmed,
        &mut edits,
        &mut writes,
    )?;
    check!(prepared.changed);
    check_eq!(prepared.term_count, 18);
    // Existing fallback emits 18 original edits; the new path must emit the
    // first key once and verify the whole block against its source seal.
    check_eq!(writes.fanout_rows, 1);
    let alias = prepared.alias.ok_or_else(|| {
        IndexError::BackendViolation("one-block long-key alias witness absent".into())
    })?;
    check_eq!(alias.anchors, 1);
    check_eq!(
        alias.key_digest,
        Digest([
            0x96, 0x26, 0x7c, 0x84, 0xe3, 0xbd, 0x48, 0x8f, 0xc3, 0x69, 0x6c, 0x00, 0x54, 0x11,
            0x6c, 0xdf, 0x86, 0x8c, 0x26, 0xb2, 0xc2, 0x2b, 0x80, 0x47, 0x0d, 0xef, 0x42, 0xd6,
            0x33, 0xd1, 0x7e, 0xd8,
        ])
    );
    writes.flush(&mut context, &spool, &mut confirmed, &mut edits)?;
    edits.flush(&spool, context.budget)?;
    let mut original = spool.inventory_cursor(1)?;
    let anchor =
        quanta_index_core::index::ports::MapEditCursor::next_edit(&mut original, context.budget)?
            .ok_or(IndexError::ResourceLimited)?;
    check_eq!(anchor.root(), 0);
    check_eq!(anchor.expected()?, None);
    check_eq!(
        anchor.key(),
        stored_term_key(&names[8], CaseMode::Folded)?.as_slice()
    );
    let Some(MapValue::Inline(descriptor)) = anchor.next()? else {
        return Err(IndexError::CorruptObject(
            "long-key directory selector is not inline".into(),
        ));
    };
    check_eq!(descriptor.data()?, &[1, 0, 18]);
    check_eq!(descriptor.single_child()?, Some(alias.reference));
    check!(
        quanta_index_core::index::ports::MapEditCursor::next_edit(&mut original, context.budget,)?
            .is_none()
    );
    let roots = entry_patch::apply_with_cold_aliases(
        entry_map::EntryMapUse::ColdInventory,
        &[EntryMap::default()],
        &[1],
        &[Some(alias)],
        &mut spool.inventory_cursor(1)?,
        |slot| spool.inventory_root_cursor(slot),
        &mut context,
        None,
    )?;
    let root = *roots
        .as_slice()
        .first()
        .ok_or(IndexError::ResourceLimited)?;
    let root = alias.verify_directory(
        root,
        context.objects,
        context.maps,
        context.attempt,
        context.budget,
    )?;
    let EntryMap::InventoryDirectory(directory) = root else {
        return Err(IndexError::CorruptObject(
            "one-block long-key alias did not seal a directory".into(),
        ));
    };
    let descriptors = maps.rows(MapRoot(Some(directory)))?;
    check_eq!(descriptors.len(), 1);
    check_eq!(
        descriptors.get(&stored_term_key(&names[8], CaseMode::Folded)?),
        Some(&MapValue::inline(&[1, 0, 18], Some(alias.reference))?)
    );
    let block = descriptors
        .values()
        .next()
        .ok_or(IndexError::ResourceLimited)?
        .single_child()?
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(block, alias.reference);
    check_eq!(objects.block_inputs.load(Ordering::Relaxed), 1);
    let reopened = objects.reopened()?;
    let mut rows = MapEntries::new(&reopened, &maps, root);
    let mut reader = crate::native::text_occurrence::OccurrenceReader::uncached()?;
    for case in [CaseMode::Sensitive, CaseMode::Folded] {
        for (ordinal, name) in names.iter().enumerate() {
            let expected = stored_term_key(name, case)?;
            check_eq!(expected.len(), 33);
            let row = rows
                .next(context.budget)?
                .ok_or(IndexError::ResourceLimited)?;
            let (key, reference) = row.object_parts()?;
            check_eq!(key.as_slice(), expected.as_slice());
            check_eq!(*reference, block);
            let occurrence = reader.load_packed(block, &expected, &reopened, context.budget)?;
            check_eq!(
                occurrence.positions.0.as_slice(),
                &[u32::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?]
            );
        }
    }
    check!(rows.next(context.budget)?.is_none());
    let mut foreign = context.attempt;
    foreign.fence = foreign
        .fence
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    check!(matches!(
        alias.verify_directory(root, &reopened, &maps, foreign, context.budget),
        Err(IndexError::FenceLost)
    ));
    let mut cancelled = integration_budget()?;
    cancelled.request().cancel_handle().cancel();
    check!(matches!(
        alias.verify_directory(root, &reopened, &maps, context.attempt, &mut cancelled),
        Err(IndexError::Cancelled)
    ));
    Ok(())
}

#[test]
fn cold_single_block_stages_one_anchor_with_long_nonfirst_key() -> Result<(), IndexError> {
    let workspace = visitor_fixture()?.ticket.view.identity.workspace;
    for (body, terms, staged_rows) in [
        (distinct_terms(256), 512, 1),
        (format!("{} {}", distinct_terms(6), "x".repeat(32)), 14, 1),
    ] {
        let objects = CanonicalObjects::default();
        let maps = CanonicalMaps::default();
        let scratch = TestScratch::default();
        let mut work = integration_budget()?;
        let spool = TextSpool::new(&scratch, &mut work)?;
        let mut context = AttemptContext {
            attempt: test_attempt(workspace),
            objects: &objects,
            maps: &maps,
            custody: &TestCustody,
            scratch: &scratch,
            budget: &mut work,
        };
        let mut edits = InventoryEdits::new(0)?;
        let mut writes = OccurrenceWrites::new();
        writes.pack_owners = true;
        let mut confirmed = ConfirmedOccurrences::new(0)?;
        let prepared = prepare_inventory(
            &body,
            &EntryMap::default(),
            0,
            &mut context,
            &spool,
            &mut confirmed,
            &mut edits,
            &mut writes,
        )?;
        check!(prepared.changed);
        check_eq!(prepared.term_count, terms);
        check_eq!(writes.fanout_rows, staged_rows);
        check_eq!(prepared.alias.is_some(), staged_rows == 1);
        writes.flush(&mut context, &spool, &mut confirmed, &mut edits)?;
        let roots = entry_patch::apply(
            entry_map::EntryMapUse::ColdInventory,
            &[EntryMap::default()],
            &[staged_rows],
            &mut spool.inventory_cursor(1)?,
            |slot| spool.inventory_root_cursor(slot),
            &mut context,
            None,
        )?;
        let root = *roots
            .as_slice()
            .first()
            .ok_or(IndexError::ResourceLimited)?;
        if let Some(alias) = prepared.alias {
            check_eq!(alias.terms, terms);
            check_eq!(
                alias
                    .visit_inline(
                        EntryMap::OccurrenceBlock(alias.reference),
                        &root,
                        context.objects,
                        context.attempt,
                        context.budget,
                        |_, _, _, _| Ok(()),
                    )?
                    .0,
                EntryMap::OccurrenceBlock(alias.reference)
            );
        } else {
            check!(matches!(root, EntryMap::OccurrenceBlock(_)));
        }
    }
    Ok(())
}

#[test]
fn small_ascii_cold_buffer_matches_sql_wire_and_uses_sql_without_optional_memory()
-> Result<(), IndexError> {
    // Repeated and unique tokens cross multiple TokenBatch packets. The two
    // paths must produce the same installed block, not merely the same count.
    let body = format!("{} alpha beta alpha", distinct_terms(64));
    let workspace = visitor_fixture()?.ticket.view.identity.workspace;
    let mut observed = Vec::new();
    for direct in [false, true] {
        let objects = CanonicalObjects::default();
        let maps = CanonicalMaps::default();
        let scratch = TestScratch::default();
        let mut work = integration_budget()?;
        let spool = TextSpool::new(&scratch, &mut work)?;
        let memory = Arc::new(TokenMemory {
            ceiling: 100_000_000,
            ..TokenMemory::default()
        });
        if direct {
            work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
                BatchOutputMemoryScope(Arc::clone(&memory)),
            ))));
        }
        let mut context = AttemptContext {
            attempt: test_attempt(workspace),
            objects: &objects,
            maps: &maps,
            custody: &TestCustody,
            scratch: &scratch,
            budget: &mut work,
        };
        let mut edits = InventoryEdits::new(0)?;
        let mut writes = OccurrenceWrites::new();
        writes.pack_owners = true;
        let mut confirmed = ConfirmedOccurrences::new(0)?;
        let prepared = prepare_inventory(
            &body,
            &EntryMap::default(),
            0,
            &mut context,
            &spool,
            &mut confirmed,
            &mut edits,
            &mut writes,
        )?;
        check_eq!(
            spool.token_term_page(None, context.budget)?.is_empty(),
            direct
        );
        check!(prepared.alias.is_some());
        writes.flush(&mut context, &spool, &mut confirmed, &mut edits)?;
        let roots = entry_patch::apply(
            entry_map::EntryMapUse::ColdInventory,
            &[EntryMap::default()],
            &[1],
            &mut spool.inventory_cursor(1)?,
            |slot| spool.inventory_root_cursor(slot),
            &mut context,
            None,
        )?;
        let root = roots
            .as_slice()
            .first()
            .ok_or(IndexError::ResourceLimited)?;
        let alias = prepared.alias.ok_or(IndexError::ResourceLimited)?;
        let expanded = alias
            .visit_inline(
                EntryMap::OccurrenceBlock(alias.reference),
                root,
                context.objects,
                context.attempt,
                context.budget,
                |_, _, _, _| Ok(()),
            )?
            .0;
        let payload = objects
            .get(alias.reference, context.budget)?
            .payload
            .clone();
        observed.push((
            prepared.indexed_tokens,
            prepared.term_count,
            expanded,
            payload,
        ));
        drop(spool);
    }
    check_eq!(observed.first(), observed.get(1));
    Ok(())
}

#[test]
fn bootstrap_512_aliases_share_one_physical_block_and_keep_all_selectors() -> Result<(), IndexError>
{
    let recipe = TextRecipe::new(Digest([77; 32]));
    let owner = OwnerKey {
        producer: ProducerId([78; 16]),
        stable_id: StableOwnerId([79; 16]),
    };
    let objects = CanonicalObjects::default();
    let maps = CanonicalMaps::default();
    // Two case surfaces for each of 256 terms yield 512 canonical selectors.
    // The anchor carries one physical ref for all 512 logical selectors.
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
    let inventory = check_inline_owner_inventory(
        &candidate.binding,
        owner,
        record,
        256,
        0x82,
        &objects,
        &maps,
    )?;
    let first = inventory_ref(&candidate.binding, owner, "t000", &objects, &maps)?;
    let last = inventory_ref(&candidate.binding, owner, "t255", &objects, &maps)?;
    check_eq!(inventory, first);
    check_eq!(first, last);
    check!(
        !objects
            .stored
            .lock()
            .map_err(|_| IndexError::Storage("test object lock".into()))?
            .values()
            .any(|object| object.payload.starts_with(b"QITXEM02"))
    );
    check_eq!(
        EntryMap::OccurrenceBlock(inventory).lookup(
            &objects,
            &maps,
            b"\0missing",
            &mut integration_budget()?,
        )?,
        None
    );
    let block = objects.get(first, &mut integration_budget()?)?;
    check!(block.payload.starts_with(b"QITXOC01"));
    check_eq!(block.payload.get(8..10), Some(b"\x02\x00".as_slice()));
    let ticket = ticket_for(&maps, &objects, source, &candidate.binding)?;
    let reopened = objects.reopened()?;
    for ordinal in 0_u32..256 {
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
    let _inventory = check_inline_owner_inventory(
        &candidate.binding,
        owner,
        record,
        258,
        0x83,
        &objects,
        &maps,
    )?;
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
    let next_body = (1_u32..258)
        .map(|ordinal| format!("t{ordinal:03}"))
        .chain(std::iter::once("t257".to_owned()))
        .collect::<Vec<_>>()
        .join(" ");
    let next_record = source_record(&objects, recipe, &next_body)?;
    let next_source = source_root(&maps, owner, next_record)?;
    let mutation = OwnerMutation {
        owner,
        expected: Some(record),
        next: Some(next_record),
    };
    let mut commitment = OwnerMutationDigest::default();
    commitment.push(&mutation)?;
    let changed_root = maps.keep(Rows::from([(
        owner_map_key(owner).to_vec(),
        owner_mutation_value(&mutation)?,
    )]))?;
    let warm = prepare(
        &index,
        BaseMember::Present(Box::new(candidate.binding.clone())),
        DeltaInput::Patch(SourceDelta {
            prior: super::source(source),
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
    let current_ticket = ticket_for(&maps, &objects, next_source, &warm.binding)?;
    let _warm_inventory = check_inline_owner_inventory(
        &warm.binding,
        owner,
        next_record,
        258,
        0x81,
        &objects,
        &maps,
    )?;
    let reopened = objects.reopened()?;
    check_eq!(
        term_hit(
            &index,
            &candidate.binding,
            &ticket,
            "t000",
            &reopened,
            &maps
        )?,
        (1, vec![0])
    );
    check_eq!(
        term_hit(
            &index,
            &candidate.binding,
            &ticket,
            "t257",
            &reopened,
            &maps
        )?,
        (1, vec![257])
    );
    check_eq!(
        term_hit(
            &index,
            &warm.binding,
            &current_ticket,
            "t257",
            &reopened,
            &maps
        )?,
        (2, vec![256, 257])
    );
    check_eq!(
        term_hit(
            &index,
            &warm.binding,
            &current_ticket,
            "t001",
            &reopened,
            &maps
        )?,
        (1, vec![0])
    );
    let mut absent = Vec::new();
    let mut work = integration_budget()?;
    let missing = index.visit_term(
        &warm.binding,
        CaseMode::Sensitive,
        "t000",
        TextReadContext {
            ticket: &current_ticket,
            objects: &reopened,
            maps: &maps,
            budget: &mut work,
        },
        |hit| {
            absent.push(hit.tf);
            Ok(())
        },
    )?;
    check_eq!(missing.df, 0);
    check!(absent.is_empty());
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

mod prepare_read_cost;
