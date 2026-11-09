use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn scoped_scalar_reads_keep_values_logical_usage_and_original_refusals() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut setup = budget()?;
    let first = value(&store, b"first", &mut setup)?;
    let second = value(&store, b"second", &mut setup)?;
    let edits = (0_u16..48)
        .map(|number| MapEdit::objects(number.to_be_bytes().to_vec(), None, Some(first)))
        .collect::<Vec<_>>();
    let root = store
        .apply(MapRoot(None), &edits, &RecordingSink::default(), &mut setup)?
        .root;
    let changed = store
        .apply(
            root,
            &[MapEdit::objects(
                7_u16.to_be_bytes().to_vec(),
                Some(first),
                Some(second),
            )],
            &RecordingSink::default(),
            &mut setup,
        )?
        .root;
    let mut plain = budget()?;
    let mut scoped_work = budget()?;
    let tracker = array_scope(MemoryLifetime::Retained, Some(0), &mut scoped_work);
    let scoped = store
        .scoped_reads(&scoped_work)?
        .ok_or("FileObjectStore omitted its scoped reader")?;
    check!(scoped.scoped_reads(&scoped_work)?.is_none());
    for (selected, key) in [
        (root, 7_u16.to_be_bytes()),
        (root, 8_u16.to_be_bytes()),
        (root, 7_u16.to_be_bytes()),
        (changed, 7_u16.to_be_bytes()),
        (root, 7_u16.to_be_bytes()),
        (MapRoot(None), 7_u16.to_be_bytes()),
        (root, 255_u16.to_be_bytes()),
    ] {
        check_eq!(
            scoped.lookup(selected, &key, &mut scoped_work)?,
            store.lookup(selected, &key, &mut plain)?
        );
        check_eq!(scoped_work.usage(), plain.usage());
    }
    check!(tracker.live.load(Ordering::Relaxed) > 0);
    let malformed = MapRoot(Some(value(&store, b"not a map node", &mut setup)?));
    check!(matches!(
        scoped.lookup(malformed, b"key", &mut scoped_work),
        Err(IndexError::CorruptObject(_))
    ));
    scoped_work.request().cancel_handle().cancel();
    let before = scoped_work.usage();
    check!(matches!(
        scoped.lookup(root, &7_u16.to_be_bytes(), &mut scoped_work),
        Err(IndexError::Cancelled)
    ));
    check_eq!(scoped_work.usage(), before);
    drop(scoped);
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);

    let mut refused = budget()?;
    let refused_tracker = array_scope(MemoryLifetime::Temporary, None, &mut refused);
    check!(matches!(
        store.scoped_reads(&refused),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(refused_tracker.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn scoped_reader_reuses_only_its_own_successful_physical_gets() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut setup = budget()?;
    let object = value(&store, b"physical-read-witness", &mut setup)?;
    let root = store
        .apply(
            MapRoot(None),
            &[MapEdit::objects(b"key".to_vec(), None, Some(object))],
            &RecordingSink::default(),
            &mut setup,
        )?
        .root;
    let mut work = budget()?;
    let scoped = super::super::scoped_reads::ScopedReads::new(&store, &work)?;
    check_eq!(scoped.lookup(root, b"key", &mut work)?, Some(object.into()));
    let physical = scoped.physical_gets();
    let logical = work.usage().reads;
    check!(physical > 0);
    check_eq!(scoped.lookup(root, b"key", &mut work)?, Some(object.into()));
    check_eq!(scoped.physical_gets(), physical);
    check!(work.usage().reads > logical);
    drop(scoped);
    let fresh = super::super::scoped_reads::ScopedReads::new(&store, &work)?;
    check_eq!(fresh.lookup(root, b"key", &mut work)?, Some(object.into()));
    check_eq!(fresh.physical_gets(), physical);
    Ok(())
}

#[test]
fn scoped_batch_keeps_path_and_data_reuse_across_pages() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut setup = budget()?;
    let first = value(&store, b"batch-first", &mut setup)?;
    let second = value(&store, b"batch-second", &mut setup)?;
    let edits = (0_u8..48)
        .map(|id| MapEdit::objects(vec![id], None, Some(first)))
        .collect::<Vec<_>>();
    let root = store
        .apply(MapRoot(None), &edits, &RecordingSink::default(), &mut setup)?
        .root;
    let keys = (0_u8..48).map(|id| vec![id]).collect::<Vec<_>>();
    let mut scalar_work = budget()?;
    let scalar = super::super::scoped_reads::ScopedReads::new(&store, &scalar_work)?;
    for key in &keys {
        check_eq!(scalar.lookup(root, key, &mut scalar_work)?, Some(first.into()));
    }
    let scalar_gets = scalar.physical_gets();
    check!(scalar_gets > 0);

    let mut batch_work = budget()?;
    let batch = super::super::scoped_reads::ScopedReads::new(&store, &batch_work)?;
    for page in keys.chunks(24) {
        let inputs = page.iter().map(Vec::as_slice).collect::<Vec<_>>();
        let mut values = [None; 24];
        batch.lookup_batch(root, &inputs, &mut values, &mut batch_work)?;
        check!(values.iter().all(|value| *value == Some(first.into())));
    }
    let batch_gets = batch.physical_gets();
    check!(batch_gets > 0);
    check!(batch_gets <= scalar_gets);
    check!(batch_work.usage().reads < scalar_work.usage().reads);

    let mut repeated = [None];
    batch.lookup_batch(
        root,
        &[keys.get(47).ok_or("last key absent")?.as_slice()],
        &mut repeated,
        &mut batch_work,
    )?;
    check_eq!(repeated, [Some(first.into())]);
    check_eq!(batch.physical_gets(), batch_gets);

    let changed = store
        .apply(
            root,
            &[MapEdit::objects(vec![7], Some(first), Some(second))],
            &RecordingSink::default(),
            &mut setup,
        )?
        .root;
    let mut replaced = [None];
    batch.lookup_batch(changed, &[b"\x07".as_slice()], &mut replaced, &mut batch_work)?;
    check_eq!(replaced, [Some(second.into())]);
    batch.lookup_batch(root, &[b"\x07".as_slice()], &mut replaced, &mut batch_work)?;
    check_eq!(replaced, [Some(first.into())]);

    let before_cancel = batch.physical_gets();
    let mut cancelled = budget()?;
    cancelled.request().cancel_handle().cancel();
    let mut untouched = [Some(second.into())];
    check_eq!(
        batch.lookup_batch(root, &[b"\x07".as_slice()], &mut untouched, &mut cancelled),
        Err(IndexError::Cancelled)
    );
    check_eq!(untouched, [Some(second.into())]);
    check_eq!(cancelled.usage(), WorkUsage::default());
    check_eq!(batch.physical_gets(), before_cancel);
    Ok(())
}

#[test]
fn scoped_paged_scan_reuses_authenticated_nodes_without_changing_pages_or_work() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut setup = budget()?;
    let object = value(&store, b"paged-scan-value", &mut setup)?;
    let edits = (0_u8..3)
        .map(|key| MapEdit::objects(vec![key], None, Some(object)))
        .collect::<Vec<_>>();
    let root = store
        .apply(MapRoot(None), &edits, &RecordingSink::default(), &mut setup)?
        .root;
    let mut plain = budget()?;
    let mut scoped_work = budget()?;
    let tracker = array_scope(MemoryLifetime::Retained, Some(0), &mut scoped_work);
    {
        let scoped = super::super::scoped_reads::ScopedReads::new(&store, &scoped_work)?;
        let mut after = None;
        for key in 0_u8..3 {
            let expected = store.scan(root, after.as_deref(), 1, &mut plain)?;
            let actual = scoped.scan(root, after.as_deref(), 1, &mut scoped_work)?;
            check_eq!(actual, expected);
            check_eq!(actual.entries.as_slice(), &[(vec![key], object.into())]);
            check_eq!(actual.has_more, key < 2);
            check_eq!(scoped_work.usage(), plain.usage());
            // Three inline terminal keys share one authenticated map node.
            check_eq!(scoped.physical_gets(), 1);
            after = actual.entries.last().map(|(key, _value)| key.clone());
        }
        let expected = store.scan(root, None, 1, &mut plain)?;
        let repeated = scoped.scan(root, None, 1, &mut scoped_work)?;
        check_eq!(repeated, expected);
        check_eq!(scoped_work.usage(), plain.usage());
        check_eq!(scoped.physical_gets(), 1);
        drop(repeated);
        drop(expected);

        let mut unfunded = WorkBudget::new(
            WorkLimits {
                reads: 0,
                writes: u64::MAX,
                bytes: u64::MAX,
                mutations: u64::MAX,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        check_eq!(
            scoped.scan(root, None, 1, &mut unfunded),
            Err(IndexError::ResourceLimited)
        );
        check_eq!(unfunded.usage().reads, 0);
        check_eq!(scoped.physical_gets(), 1);
        let mut cancelled = budget()?;
        cancelled.request().cancel_handle().cancel();
        check_eq!(
            scoped.scan(root, None, 1, &mut cancelled),
            Err(IndexError::Cancelled)
        );
        check_eq!(cancelled.usage(), WorkUsage::default());
        check_eq!(scoped.physical_gets(), 1);
        check!(tracker.live.load(Ordering::Relaxed) > 0);
    }
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);

    let mut fresh_work = budget()?;
    let fresh = super::super::scoped_reads::ScopedReads::new(&store, &fresh_work)?;
    check_eq!(fresh.physical_gets(), 0);
    let page = fresh.scan(root, None, 1, &mut fresh_work)?;
    check_eq!(page.entries.as_slice(), &[(vec![0], object.into())]);
    check_eq!(fresh.physical_gets(), 1);
    Ok(())
}

#[test]
fn scoped_multilevel_scan_preserves_page_boundaries_and_logical_work() -> TestResult {
    const KEYS: usize = 130;
    const PAGE_SIZE: usize = 7;

    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut setup = budget()?;
    let object = value(&store, b"multilevel-scan-value", &mut setup)?;
    let edits = (0..KEYS)
        .map(|key| {
            Ok(MapEdit::objects(
                u16::try_from(key)?.to_be_bytes().to_vec(),
                None,
                Some(object),
            ))
        })
        .collect::<Result<Vec<_>, std::num::TryFromIntError>>()?;
    let root = store
        .apply(MapRoot(None), &edits, &RecordingSink::default(), &mut setup)?
        .root;
    let mut plain = budget()?;
    let mut scoped_work = budget()?;
    let scoped = super::super::scoped_reads::ScopedReads::new(&store, &scoped_work)?;
    let page_limit = u32::try_from(PAGE_SIZE)?;
    let mut after = None;
    for start in (0..KEYS).step_by(PAGE_SIZE) {
        let expected = store.scan(root, after.as_deref(), page_limit, &mut plain)?;
        let actual = scoped.scan(root, after.as_deref(), page_limit, &mut scoped_work)?;
        let end = start.saturating_add(PAGE_SIZE).min(KEYS);
        let golden = (start..end)
            .map(|key| {
                Ok((
                    u16::try_from(key)?.to_be_bytes().to_vec(),
                    MapValue::from(object),
                ))
            })
            .collect::<Result<Vec<_>, std::num::TryFromIntError>>()?;
        check_eq!(actual, expected);
        check_eq!(actual.entries, golden);
        check_eq!(actual.has_more, end < KEYS);
        check_eq!(scoped_work.usage(), plain.usage());
        after = actual.entries.last().map(|(key, _value)| key.clone());
    }
    check!(scoped.physical_gets() > 1);
    let expected = store.scan(root, after.as_deref(), page_limit, &mut plain)?;
    let actual = scoped.scan(root, after.as_deref(), page_limit, &mut scoped_work)?;
    check!(actual.entries.is_empty());
    check!(!actual.has_more);
    check_eq!(actual, expected);
    check_eq!(scoped_work.usage(), plain.usage());
    Ok(())
}
