//! Cold term-page producer and sealed anchor stream oracles.

use super::super::{term_dictionary, term_dictionary_write};
use super::*;
use quanta_index_core::index::MemoryScope;

struct PageBatchProbe<'a> {
    inner: &'a super::occurrence_blocks::CanonicalObjects,
    installed: AtomicUsize,
    calls: AtomicUsize,
    fail_after_first: bool,
}

impl ImmutableObjectPort for PageBatchProbe<'_> {
    fn install_batch_memory_bound(
        &self,
        shape: quanta_index_core::index::ports::ObjectInstallMemoryShape,
    ) -> Result<Option<quanta_index_core::index::MemoryCreditLimits>, IndexError> {
        self.inner.install_batch_memory_bound(shape)
    }

    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.inner.identify(payload, children, budget)
    }

    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.inner.put(payload, children, budget)
    }

    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.inner.install(payload, children, sink, budget)
    }

    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
        let _old = self.calls.fetch_add(1, Ordering::Relaxed);
        if self.fail_after_first {
            if inputs.len() < 2 {
                return Err(IndexError::BackendViolation(
                    "term page batch did not contain both pages".into(),
                ));
            }
            let _first = self.inner.install_batch(&inputs[..1], sink, budget)?;
            let _old = self.installed.fetch_add(1, Ordering::Relaxed);
            return Err(IndexError::BackendViolation(
                "injected second term page install".into(),
            ));
        }
        let result = self.inner.install_batch(inputs, sink, budget)?;
        let _old = self.installed.fetch_add(inputs.len(), Ordering::Relaxed);
        Ok(result)
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        self.inner.get(reference, budget)
    }
}

fn funded_writer_budget() -> Result<(WorkBudget, Arc<TokenMemory>), IndexError> {
    let memory = Arc::new(TokenMemory {
        ceiling: 64 * 1024 * 1024,
        ..TokenMemory::default()
    });
    let mut work = packet_budget()?;
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(BatchOutputMemoryScope(
        Arc::clone(&memory),
    )))));
    Ok((work, memory))
}

fn stage_128_terms(
    spool: &TextSpool,
    work: &mut WorkBudget,
    expected_first: Option<ObjectRef>,
) -> Result<(), IndexError> {
    for case in [0_u8, 1_u8] {
        for page in 0..2 {
            let mut edits = Vec::new();
            for ordinal in (page * 32)..((page + 1) * 32) {
                let mut key = vec![case];
                key.extend_from_slice(format!("k{ordinal:03}").as_bytes());
                let expected = (case == 0 && ordinal == 0)
                    .then_some(expected_first)
                    .flatten();
                edits.push((0, MapEdit::objects(key, expected, Some(reference(7)))));
            }
            spool.top_batch(&edits, work)?;
        }
    }
    spool.top_batch(
        &[(1, MapEdit::objects(vec![3; 32], None, Some(reference(9))))],
        work,
    )
}

// A row has a two-byte key length, a 1000-byte key, kind/data-length/slot.
// Sixteen rows use 16 + 16 * 1005 = 16096 bytes; the seventeenth cannot fit.
fn wire_limited_key(ordinal: usize) -> Vec<u8> {
    let mut key = vec![0];
    key.extend_from_slice(format!("k{ordinal:03}").as_bytes());
    key.resize(1000, b'x');
    key
}

fn stage_two_wire_pages(spool: &TextSpool, work: &mut WorkBudget) -> Result<(), IndexError> {
    let edits = (0..32)
        .map(|ordinal| {
            (
                0,
                MapEdit::objects(wire_limited_key(ordinal), None, Some(reference(7))),
            )
        })
        .collect::<Vec<_>>();
    spool.top_batch(&edits, work)
}

fn seed_wire_full_prior_page(
    objects: &super::occurrence_blocks::CanonicalObjects,
    maps: &mut TestMaps,
    work: &mut WorkBudget,
) -> Result<(MapRoot, ObjectRef), IndexError> {
    let mut draft = term_dictionary::PageDraft::new(work)?;
    for ordinal in 0..16 {
        check!(draft.try_push(
            &wire_limited_key(ordinal),
            MapValue::Object(reference(7)),
            work
        )?);
    }
    check!(!draft.try_push(&wire_limited_key(16), MapValue::Object(reference(7)), work)?);
    let page = draft.finish(work)?;
    check_eq!(page.input.payload.len(), 16096);
    let page_ref = objects
        .put(&page.input.payload, &page.input.children, work)?
        .reference;
    let root = MapRoot(Some(reference(201)));
    maps.add(root, page.fence.as_slice().to_vec(), page_ref)?;
    Ok((root, page_ref))
}

fn seed_prior_page(
    objects: &super::occurrence_blocks::CanonicalObjects,
    maps: &mut TestMaps,
    work: &mut WorkBudget,
) -> Result<(MapRoot, ObjectRef), IndexError> {
    let mut draft = term_dictionary::PageDraft::new(work)?;
    for ordinal in 0..64 {
        let mut key = vec![0];
        key.extend_from_slice(format!("k{ordinal:03}").as_bytes());
        check!(draft.try_push(&key, MapValue::Object(reference(7)), work)?);
    }
    let page = draft.finish(work)?;
    check_eq!(page.fence.as_slice(), b"\0k063");
    let page_ref = objects
        .put(&page.input.payload, &page.input.children, work)?
        .reference;
    let root = MapRoot(Some(reference(200)));
    maps.add(root, page.fence.as_slice().to_vec(), page_ref)?;
    Ok((root, page_ref))
}

#[test]
fn cold_128_short_terms_share_one_byte_bounded_page_then_forward_one_owner()
-> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let inner = super::occurrence_blocks::CanonicalObjects::default();
    let objects = PageBatchProbe {
        inner: &inner,
        installed: AtomicUsize::new(0),
        calls: AtomicUsize::new(0),
        fail_after_first: false,
    };
    let (mut work, memory) = funded_writer_budget()?;
    let scratch = TestScratch::default();
    let spool = TextSpool::new(&scratch, &mut work)?;
    stage_128_terms(&spool, &mut work, None)?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &objects,
        maps: &fixture.maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    term_dictionary_write::prepare(MapRoot(None), &mut context, &spool)?;
    check_eq!(objects.calls.load(Ordering::Relaxed), 1);
    check_eq!(objects.installed.load(Ordering::Relaxed), 1);
    let mut stream = spool.term_anchor_owner_cursor();
    let edit = stream
        .next_edit(context.budget)?
        .ok_or_else(|| IndexError::BackendViolation("cold term page anchor missing".into()))?;
    check_eq!(edit.root(), 0);
    check_eq!(edit.key(), b"\x01k063");
    check_eq!(edit.expected()?, None);
    let page_ref = edit
        .next()?
        .ok_or(IndexError::ResourceLimited)?
        .object_ref()?;
    let page = term_dictionary::CheckedPage::parse(
        context.objects.get(page_ref, context.budget)?,
        page_ref,
        context.budget,
    )?;
    check_eq!(page.len(), 128);
    check_eq!(page.first_key()?, b"\0k000");
    check_eq!(page.last_key()?, b"\x01k063");
    check_eq!(page.entry(32)?.1, MapValue::Object(reference(7)));
    check_eq!(page.entry(64)?.0, b"\x01k000");
    drop(page);
    drop(edit);
    let owner = stream
        .next_edit(context.budget)?
        .ok_or_else(|| IndexError::BackendViolation("term anchor stream lost owner".into()))?;
    check_eq!(owner.root(), 1);
    check_eq!(owner.key(), &[3; 32]);
    check_eq!(owner.expected()?, None);
    check_eq!(owner.next()?, Some(MapValue::Object(reference(9))));
    check!(stream.next_edit(context.budget)?.is_none());
    drop(owner);
    drop(stream);
    drop(context);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn wrong_expected_term_cas_refuses_and_poisoned_anchor_stream_cannot_publish()
-> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let objects = super::occurrence_blocks::CanonicalObjects::default();
    let (mut work, memory) = funded_writer_budget()?;
    let scratch = TestScratch::default();
    let spool = TextSpool::new(&scratch, &mut work)?;
    stage_128_terms(&spool, &mut work, Some(reference(99)))?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &objects,
        maps: &fixture.maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    check!(matches!(
        term_dictionary_write::prepare(MapRoot(None), &mut context, &spool),
        Err(IndexError::Conflict)
    ));
    check!(matches!(
        spool.term_anchor_owner_cursor().next_edit(context.budget),
        Err(IndexError::BackendViolation(_))
    ));
    drop(context);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn second_page_install_failure_poisoned_anchor_stream_cannot_publish() -> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let inner = super::occurrence_blocks::CanonicalObjects::default();
    let objects = PageBatchProbe {
        inner: &inner,
        installed: AtomicUsize::new(0),
        calls: AtomicUsize::new(0),
        fail_after_first: true,
    };
    let (mut work, memory) = funded_writer_budget()?;
    let scratch = TestScratch::default();
    let spool = TextSpool::new(&scratch, &mut work)?;
    stage_two_wire_pages(&spool, &mut work)?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &objects,
        maps: &fixture.maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    check!(matches!(
        term_dictionary_write::prepare(MapRoot(None), &mut context, &spool),
        Err(IndexError::BackendViolation(message))
            if message == "injected second term page install"
    ));
    check_eq!(objects.installed.load(Ordering::Relaxed), 1);
    check!(matches!(
        spool.term_anchor_owner_cursor().next_edit(context.budget),
        Err(IndexError::BackendViolation(_))
    ));
    drop(context);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn warm_replace_first_delete_middle_and_last_moves_upper_fence() -> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    let objects = super::occurrence_blocks::CanonicalObjects::default();
    let (mut work, memory) = funded_writer_budget()?;
    let (prior, old_ref) = seed_prior_page(&objects, &mut fixture.maps, &mut work)?;
    let scratch = TestScratch::default();
    let spool = TextSpool::new(&scratch, &mut work)?;
    spool.top_batch(
        &[
            (
                0,
                MapEdit::objects(b"\0k000".to_vec(), Some(reference(7)), Some(reference(8))),
            ),
            (
                0,
                MapEdit::objects(b"\0k031".to_vec(), Some(reference(7)), None),
            ),
            (
                0,
                MapEdit::objects(b"\0k063".to_vec(), Some(reference(7)), None),
            ),
        ],
        &mut work,
    )?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &objects,
        maps: &fixture.maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    term_dictionary_write::prepare(prior, &mut context, &spool)?;
    let mut stream = spool.term_anchor_owner_cursor();
    let created = stream
        .next_edit(context.budget)?
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(created.root(), 0);
    check_eq!(created.key(), b"\0k062");
    check_eq!(created.expected()?, None);
    let new_ref = created
        .next()?
        .ok_or(IndexError::ResourceLimited)?
        .object_ref()?;
    let new_page = term_dictionary::CheckedPage::parse(
        objects.get(new_ref, context.budget)?,
        new_ref,
        context.budget,
    )?;
    check_eq!(new_page.len(), 62);
    check_eq!(new_page.first_key()?, b"\0k000");
    check_eq!(new_page.last_key()?, b"\0k062");
    check_eq!(
        new_page.lookup(b"\0k000", context.budget)?,
        Some(MapValue::Object(reference(8)))
    );
    check_eq!(new_page.lookup(b"\0k031", context.budget)?, None);
    check_eq!(new_page.lookup(b"\0k063", context.budget)?, None);
    let removed = stream
        .next_edit(context.budget)?
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(removed.root(), 0);
    check_eq!(removed.key(), b"\0k063");
    check_eq!(removed.expected()?, Some(MapValue::Object(old_ref)));
    check_eq!(removed.next()?, None);
    check!(stream.next_edit(context.budget)?.is_none());
    drop(new_page);
    drop(removed);
    drop(created);
    drop(stream);
    drop(context);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn warm_identical_page_skips_install_and_anchor_mutation() -> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    let inner = super::occurrence_blocks::CanonicalObjects::default();
    let (mut work, memory) = funded_writer_budget()?;
    let (prior, _old_ref) = seed_prior_page(&inner, &mut fixture.maps, &mut work)?;
    let objects = PageBatchProbe {
        inner: &inner,
        installed: AtomicUsize::new(0),
        calls: AtomicUsize::new(0),
        fail_after_first: false,
    };
    let scratch = TestScratch::default();
    let spool = TextSpool::new(&scratch, &mut work)?;
    spool.top_batch(
        &[(
            0,
            MapEdit::objects(b"\0k032".to_vec(), Some(reference(7)), Some(reference(7))),
        )],
        &mut work,
    )?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &objects,
        maps: &fixture.maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    term_dictionary_write::prepare(prior, &mut context, &spool)?;
    check_eq!(objects.installed.load(Ordering::Relaxed), 0);
    check!(
        spool
            .term_anchor_owner_cursor()
            .next_edit(context.budget)?
            .is_none()
    );
    drop(context);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn warm_insert_inside_wire_full_page_splits_without_changing_old_anchor_cas()
-> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    let objects = super::occurrence_blocks::CanonicalObjects::default();
    let (mut work, memory) = funded_writer_budget()?;
    let (prior, old_ref) = seed_wire_full_prior_page(&objects, &mut fixture.maps, &mut work)?;
    let scratch = TestScratch::default();
    let spool = TextSpool::new(&scratch, &mut work)?;
    let mut inserted = b"\0k007a".to_vec();
    inserted.resize(1000, b'x');
    spool.top_batch(
        &[(
            0,
            MapEdit::objects(inserted.clone(), None, Some(reference(8))),
        )],
        &mut work,
    )?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &objects,
        maps: &fixture.maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    term_dictionary_write::prepare(prior, &mut context, &spool)?;
    let mut stream = spool.term_anchor_owner_cursor();
    for (last, expected, count, first) in [(14, None, 16, 0), (15, Some(old_ref), 1, 15)] {
        let edit = stream
            .next_edit(context.budget)?
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(edit.root(), 0);
        check_eq!(edit.key(), wire_limited_key(last));
        check_eq!(edit.expected()?, expected.map(MapValue::Object));
        let next = edit
            .next()?
            .ok_or(IndexError::ResourceLimited)?
            .object_ref()?;
        let page = term_dictionary::CheckedPage::parse(
            objects.get(next, context.budget)?,
            next,
            context.budget,
        )?;
        check_eq!(page.len(), count);
        check_eq!(page.first_key()?, wire_limited_key(first));
        check_eq!(page.last_key()?, wire_limited_key(last));
        if count == 16 {
            check_eq!(
                page.lookup(&inserted, context.budget)?,
                Some(MapValue::Object(reference(8)))
            );
            check_eq!(page.lookup(&wire_limited_key(15), context.budget)?, None);
        }
    }
    check!(stream.next_edit(context.budget)?.is_none());
    drop(stream);
    drop(context);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}
