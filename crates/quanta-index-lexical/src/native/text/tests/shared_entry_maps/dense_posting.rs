//! Producer and reader agreement across a physical dense-posting page boundary.

use super::*;
use crate::native::text::dense_posting::wire::{decode_directory, decode_page};
use quanta_index_core::index::{RegisteredObject, RegisteredObjectBatch};

struct RecordingObjects<'a> {
    inner: &'a CanonicalObjects,
    installed: Mutex<Vec<[u8; 8]>>,
    gets: Mutex<Vec<ObjectRef>>,
}

impl<'a> RecordingObjects<'a> {
    fn new(inner: &'a CanonicalObjects) -> Self {
        Self {
            inner,
            installed: Mutex::new(Vec::new()),
            gets: Mutex::new(Vec::new()),
        }
    }

    fn installed(&self) -> Result<Vec<[u8; 8]>, IndexError> {
        self.installed
            .lock()
            .map(|rows| rows.clone())
            .map_err(|_error| IndexError::Storage("installation log lock".into()))
    }

    fn get_count(&self, reference: ObjectRef) -> Result<usize, IndexError> {
        self.gets
            .lock()
            .map(|calls| calls.iter().filter(|called| **called == reference).count())
            .map_err(|_error| IndexError::Storage("object read log lock".into()))
    }
}

impl ImmutableObjectPort for RecordingObjects<'_> {
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
        let installed = self.inner.install(payload, children, sink, budget)?;
        let magic: [u8; 8] = payload
            .get(..8)
            .ok_or_else(|| IndexError::BackendViolation("short installed payload".into()))?
            .try_into()
            .map_err(|_error| IndexError::BackendViolation("installed magic width".into()))?;
        self.installed
            .lock()
            .map_err(|_error| IndexError::Storage("installation log lock".into()))?
            .push(magic);
        Ok(installed)
    }

    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObjectBatch, IndexError> {
        let installed = self.inner.install_batch(inputs, sink, budget)?;
        let mut log = self
            .installed
            .lock()
            .map_err(|_error| IndexError::Storage("installation log lock".into()))?;
        for input in inputs {
            let magic: [u8; 8] = input
                .payload
                .get(..8)
                .ok_or_else(|| IndexError::BackendViolation("short installed payload".into()))?
                .try_into()
                .map_err(|_error| IndexError::BackendViolation("installed magic width".into()))?;
            log.push(magic);
        }
        Ok(installed)
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        let object = self.inner.get(reference, budget)?;
        self.gets
            .lock()
            .map_err(|_error| IndexError::Storage("object read log lock".into()))?
            .push(reference);
        Ok(object)
    }
}

fn owner(id: u16) -> [u8; 32] {
    let mut key = [0; 32];
    key[30..].copy_from_slice(&id.to_be_bytes());
    key
}

fn occurrence(id: u16) -> ObjectRef {
    ObjectRef(Digest(owner(id)))
}

fn apply_dense(
    objects: &RecordingObjects<'_>,
    maps: &ModelMaps,
    prior: EntryMap,
    count: u64,
    original: Vec<OrdinalMapEdit>,
    planned: Vec<OrdinalMapEdit>,
) -> Result<EntryMap, IndexError> {
    let fixture = visitor_fixture()?;
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut budget = entry_budget()?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects,
        maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut budget,
    };
    let roots = entry_patch::apply(
        EntryMapUse::PackedPosting,
        &[prior],
        &[count],
        &mut Rows::new(original),
        |_slot| Ok(Rows::new(planned.clone())),
        &mut context,
        Some(&mut |_slot, _key, _witness, _budget| {
            Err(IndexError::BackendViolation(
                "packed posting must not resolve an occurrence in the producer".into(),
            ))
        }),
    )?;
    roots
        .as_slice()
        .first()
        .copied()
        .ok_or_else(|| IndexError::BackendViolation("packed posting output missing".into()))
}

#[test]
fn dense_posting_two_pages_reuse_untouched_leaf_and_seal_edits() -> Result<(), IndexError> {
    let backing = CanonicalObjects::default();
    let objects = RecordingObjects::new(&backing);
    let maps = ModelMaps::default();
    let initial = (0_u16..257)
        .map(|id| row(&owner(id), None, Some(occurrence(id))))
        .collect::<Vec<_>>();
    let first = apply_dense(
        &objects,
        &maps,
        EntryMap::default(),
        257,
        initial.clone(),
        initial,
    )?;
    let EntryMap::PostingPages(first_directory_ref) = first else {
        return Err(IndexError::BackendViolation(
            "dense posting did not choose pages".into(),
        ));
    };
    check_eq!(
        objects.installed()?,
        vec![*b"QITXDP01", *b"QITXDP01", *b"QITXDD01"]
    );
    check_eq!(maps.applies.load(Ordering::Relaxed), 0);

    let mut budget = entry_budget()?;
    let first_directory = decode_directory(
        objects.get(first_directory_ref, &mut budget)?,
        first_directory_ref,
        &budget,
    )?;
    check_eq!(first_directory.len(), 2);
    check_eq!(first_directory.total(), 257);
    let (first_fence, first_leaf_ref) = first_directory.descriptor(0)?;
    let (second_fence, second_leaf_ref) = first_directory.descriptor(1)?;
    check_eq!(first_fence.count, 256);
    check_eq!(second_fence.count, 1);
    check_eq!(first_fence.first, owner(0));
    check_eq!(first_fence.last, owner(255));
    check_eq!(second_fence.first, owner(256));
    let first_leaf = decode_page(
        objects.get(first_leaf_ref, &mut budget)?,
        first_leaf_ref,
        first_fence,
        &budget,
    )?;
    check_eq!(first_leaf.len(), 256);
    check_eq!(first_leaf.row(255)?.witness, occurrence(255).0.0);
    check!(first_leaf.object().registration.children.is_empty());

    let untouched_reads_before_warm = objects.get_count(first_leaf_ref)?;
    let touched_reads_before_warm = objects.get_count(second_leaf_ref)?;
    let changed = row(&owner(256), Some(occurrence(256)), Some(occurrence(1000)));
    let warm = apply_dense(
        &objects,
        &maps,
        first,
        257,
        vec![changed.clone()],
        vec![changed],
    )?;
    check_eq!(
        objects.get_count(first_leaf_ref)?,
        untouched_reads_before_warm
    );
    check!(objects.get_count(second_leaf_ref)? > touched_reads_before_warm);
    let EntryMap::PostingPages(warm_directory_ref) = warm else {
        return Err(IndexError::BackendViolation(
            "warm posting lost pages".into(),
        ));
    };
    check!(warm_directory_ref != first_directory_ref);
    check_eq!(
        objects.installed()?,
        vec![
            *b"QITXDP01",
            *b"QITXDP01",
            *b"QITXDD01",
            *b"QITXDP01",
            *b"QITXDD01",
        ]
    );
    let warm_directory = decode_directory(
        objects.get(warm_directory_ref, &mut budget)?,
        warm_directory_ref,
        &budget,
    )?;
    check_eq!(warm_directory.len(), 2);
    check_eq!(warm_directory.descriptor(0)?.1, first_leaf_ref);
    check!(warm_directory.descriptor(1)?.1 != second_leaf_ref);
    let (warm_fence, warm_leaf_ref) = warm_directory.descriptor(1)?;
    let warm_leaf = decode_page(
        objects.get(warm_leaf_ref, &mut budget)?,
        warm_leaf_ref,
        warm_fence,
        &budget,
    )?;
    check_eq!(warm_leaf.row(0)?.witness, occurrence(1000).0.0);
    check!(warm_leaf.object().registration.children.is_empty());

    for id in [0_u16, 255, 256] {
        let expected = if id == 256 {
            occurrence(1000)
        } else {
            occurrence(id)
        };
        let witness = warm
            .posting_lookup(&objects, &maps, &owner(id), 257, &mut budget)?
            .ok_or_else(|| IndexError::BackendViolation("posting lookup missed row".into()))?;
        check!(witness.matches(expected));
    }
    check_eq!(
        warm.posting_lookup(&objects, &maps, &owner(257), 257, &mut budget)?,
        None
    );
    let mut cursor = MapEntries::new(&objects, &maps, warm);
    for id in 0_u16..257 {
        let entry = cursor
            .next(&mut budget)?
            .ok_or_else(|| IndexError::BackendViolation("posting scan ended early".into()))?;
        let (key, value) = entry.get()?;
        check_eq!(key.as_slice(), owner(id).as_slice());
        check!(value.children()?.is_empty());
        let expected = if id == 256 {
            occurrence(1000)
        } else {
            occurrence(id)
        };
        check!(OccurrenceDigestWitness::from_value(*value)?.matches(expected));
    }
    check!(cursor.next(&mut budget)?.is_none());

    let installed_before_refusal = objects.installed()?;
    let wrong_expected = row(&owner(256), Some(occurrence(256)), Some(occurrence(1001)));
    check!(
        apply_dense(
            &objects,
            &maps,
            warm,
            257,
            vec![wrong_expected.clone()],
            vec![wrong_expected],
        )
        .is_err()
    );
    let original = row(&owner(256), Some(occurrence(1000)), Some(occurrence(1001)));
    let changed_plan = row(&owner(256), Some(occurrence(1000)), Some(occurrence(1002)));
    check!(
        apply_dense(
            &objects,
            &maps,
            warm,
            257,
            vec![original],
            vec![changed_plan]
        )
        .is_err()
    );
    check_eq!(objects.installed()?, installed_before_refusal);

    let untouched_reads_before_append = objects.get_count(first_leaf_ref)?;
    let touched_reads_before_append = objects.get_count(warm_leaf_ref)?;
    let append = row(&owner(257), None, Some(occurrence(257)));
    let appended = apply_dense(
        &objects,
        &maps,
        warm,
        258,
        vec![append.clone()],
        vec![append],
    )?;
    check_eq!(
        objects.get_count(first_leaf_ref)?,
        untouched_reads_before_append
    );
    check!(objects.get_count(warm_leaf_ref)? > touched_reads_before_append);
    let EntryMap::PostingPages(appended_ref) = appended else {
        return Err(IndexError::BackendViolation(
            "append lost packed posting".into(),
        ));
    };
    let appended_directory = decode_directory(
        objects.get(appended_ref, &mut budget)?,
        appended_ref,
        &budget,
    )?;
    check_eq!(appended_directory.len(), 2);
    check_eq!(appended_directory.descriptor(0)?.1, first_leaf_ref);
    let (last, _) = appended_directory.descriptor(1)?;
    check_eq!(last.count, 2);
    check_eq!(last.last, owner(257));
    check_eq!(
        objects.installed()?.get(installed_before_refusal.len()..),
        Some([*b"QITXDP01", *b"QITXDD01"].as_slice())
    );
    Ok(())
}
