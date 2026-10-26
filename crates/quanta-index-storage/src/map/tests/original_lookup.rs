use super::*;
use crate::map::original_lookup::OriginalLookup;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn batch_lookup_matches_independent_values_order_absence_and_refusals() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut setup = budget()?;
    let object = value(&store, b"mixed batch object", &mut setup)?;
    let mut model = BTreeMap::new();
    let _prior = model.insert(Vec::new(), object.into());
    for id in 0_u16..48 {
        let mut key = vec![61; 36];
        key.extend_from_slice(&id.to_be_bytes());
        let _prior = model.insert(key, MapValue::inline(&id.to_be_bytes(), [])?);
    }
    let edits = model
        .iter()
        .map(|(key, value)| MapEdit {
            key: key.clone(),
            expected: None,
            next: Some(*value),
        })
        .collect::<Vec<_>>();
    let root = store
        .apply(MapRoot(None), &edits, &RecordingSink::default(), &mut setup)?
        .root;
    let mut queries = model.keys().rev().cloned().collect::<Vec<_>>();
    queries.extend([Vec::new(), vec![61; 37], vec![255; MAX_KEY_BYTES]]);
    let keys = queries.iter().map(Vec::as_slice).collect::<Vec<_>>();
    let mut values = vec![None; keys.len()];
    let mut scalar = budget()?;
    let mut batch = budget()?;
    for key in &keys {
        check_eq!(
            store.lookup(root, key, &mut scalar)?,
            model.get(*key).copied()
        );
    }
    store.lookup_batch(root, &keys, &mut values, &mut batch)?;
    check_eq!(
        values,
        queries
            .iter()
            .map(|key| model.get(key).copied())
            .collect::<Vec<_>>()
    );
    check!(batch.usage().reads < scalar.usage().reads);

    let mut shape = budget()?;
    let mut sentinel = [Some(object.into())];
    check!(matches!(
        store.lookup_batch(root, &keys, &mut sentinel, &mut shape),
        Err(IndexError::InvalidInput(_))
    ));
    check_eq!(sentinel, [Some(object.into())]);
    check_eq!(shape.usage(), WorkUsage::default());
    let oversized = vec![b'q'; MAX_KEY_BYTES + 1];
    let mut provisional = [None; 2];
    check!(matches!(
        store.lookup_batch(root, &[b"", &oversized], &mut provisional, &mut shape),
        Err(IndexError::InvalidInput(_))
    ));
    check_eq!(shape.usage().writes, 0);
    check_eq!(shape.usage().mutations, 0);
    let mut cancelled = budget()?;
    cancelled.request().cancel_handle().cancel();
    check!(matches!(
        store.lookup_batch(root, &keys, &mut values, &mut cancelled),
        Err(IndexError::Cancelled)
    ));
    check_eq!(cancelled.usage(), WorkUsage::default());
    let mut denied = budget()?;
    let tracker = array_scope(MemoryLifetime::Temporary, None, &mut denied);
    check!(matches!(
        store.lookup_batch(root, &keys, &mut values, &mut denied),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn original_path_matches_model_and_avoids_repeated_shared_ancestor_reads() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut setup = budget()?;
    let old = value(&store, b"original-value", &mut setup)?;
    let new = value(&store, b"replacement-value", &mut setup)?;
    let model = (0_u16..1024)
        .map(|suffix| {
            let mut key = vec![42; 36];
            key.extend_from_slice(&suffix.to_be_bytes());
            (key, old)
        })
        .collect::<BTreeMap<_, _>>();
    let edits = model
        .iter()
        .map(|(key, value)| MapEdit::objects(key.clone(), None, Some(*value)))
        .collect::<Vec<_>>();
    let sink = RecordingSink::default();
    let root = store.apply(MapRoot(None), &edits, &sink, &mut setup)?.root;
    let mut queries = model.keys().cloned().collect::<Vec<_>>();
    queries.extend([vec![], vec![0], vec![42; 36], vec![255; 38]]);
    queries.sort();
    queries.dedup();
    let mut plain = budget()?;
    let mut work = budget()?;
    let tracker = array_scope(MemoryLifetime::Temporary, Some(0), &mut work);
    let mut lookup = OriginalLookup::new();
    for key in &queries {
        let expected = model.get(key).copied().map(Into::into);
        let nibbles = key_nibbles(key, &plain)?;
        check_eq!(store.lookup_inner(root, &nibbles, &mut plain)?, expected);
        let nibbles = key_nibbles(key, &work)?;
        check_eq!(lookup.lookup(&store, root, &nibbles, &mut work)?, expected);
    }
    let reads = work.usage().reads;
    let avoided = plain
        .usage()
        .reads
        .checked_sub(reads)
        .ok_or("read count grew")?;
    // Even before counting shared children, the immutable root needs only one
    // object read instead of one per key. This is work eliminated, not rebilled.
    check!(avoided >= u64::try_from(queries.len().checked_sub(1).ok_or("no queries")?)?);
    let first = model.first_key_value().ok_or("empty model")?.0;
    let replacement = store
        .apply(
            root,
            &[MapEdit::objects(first.clone(), Some(old), Some(new))],
            &sink,
            &mut setup,
        )?
        .root;
    for (base, expected) in [
        (replacement, Some(new)),
        (MapRoot(None), None),
        (root, Some(old)),
    ] {
        let key = key_nibbles(first, &work)?;
        check_eq!(
            lookup.lookup(&store, base, &key, &mut work)?,
            expected.map(Into::into)
        );
    }
    check!(tracker.live.load(Ordering::Relaxed) > 0);
    work.request().cancel_handle().cancel();
    let before = work.usage().reads;
    check!(matches!(
        lookup.lookup(&store, root, &[], &mut work),
        Err(IndexError::Cancelled)
    ));
    check_eq!(work.usage().reads, before);
    drop(lookup);
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn original_path_preserves_maximum_keys_prefix_values_and_absent_terminals() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut work = budget()?;
    let value = value(&store, b"long-key-value", &mut work)?;
    let mut last = vec![51; MAX_KEY_BYTES];
    *last.last_mut().ok_or("long key missing")? = 52;
    let model = [Vec::new(), vec![51], vec![51; MAX_KEY_BYTES], last]
        .into_iter()
        .map(|key| (key, value))
        .collect::<BTreeMap<_, _>>();
    let edits = model
        .keys()
        .map(|key| MapEdit::objects(key.clone(), None, Some(value)))
        .collect::<Vec<_>>();
    let root = store
        .apply(MapRoot(None), &edits, &RecordingSink::default(), &mut work)?
        .root;
    let mut lookup = OriginalLookup::new();
    let mut queries = model.keys().cloned().collect::<Vec<_>>();
    queries.extend([
        vec![51; 2],
        vec![50; MAX_KEY_BYTES],
        vec![53; MAX_KEY_BYTES],
    ]);
    queries.sort();
    for key in queries {
        let nibbles = key_nibbles(&key, &work)?;
        check_eq!(
            lookup.lookup(&store, root, &nibbles, &mut work)?,
            model.get(&key).copied().map(Into::into)
        );
    }
    Ok(())
}

#[test]
fn original_path_checks_new_child_corruption_and_allocation_before_retention() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut setup = budget()?;
    let value = value(&store, b"sibling-value", &mut setup)?;
    let keys = [0_u8, 1].map(|first| {
        let mut key = vec![first];
        key.extend_from_slice(&[9 + first; 512]);
        key
    });
    let edits = keys
        .iter()
        .map(|key| MapEdit::objects(key.clone(), None, Some(value)))
        .collect::<Vec<_>>();
    let root = store
        .apply(MapRoot(None), &edits, &RecordingSink::default(), &mut setup)?
        .root;
    let node = store.read_node(root.0.ok_or("root missing")?, &mut setup)?;
    let sibling = node.branches.get(1)?.ok_or("second branch missing")?;
    let BranchValue::Indirect(corrupt) = sibling.value else {
        return Err("expected long indirect child".into());
    };
    check!(
        node.branches
            .get(0)?
            .is_some_and(|first| first.value != sibling.value)
    );
    let mut work = budget()?;
    let mut lookup = OriginalLookup::new();
    let first = key_nibbles(&keys[0], &work)?;
    check_eq!(
        lookup.lookup(&store, root, &first, &mut work)?,
        Some(value.into())
    );
    fs::write(store.path_for(corrupt), b"broken")?;
    let second = key_nibbles(&keys[1], &work)?;
    check!(matches!(
        lookup.lookup(&store, root, &second, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    let mut denied = budget()?;
    let tracker = array_scope(MemoryLifetime::Temporary, None, &mut denied);
    let mut lookup = OriginalLookup::new();
    check!(matches!(
        lookup.lookup(&store, root, &first, &mut denied),
        Err(IndexError::ResourceLimited)
    ));
    drop(lookup);
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);
    check_eq!(denied.usage().writes, 0);
    check_eq!(denied.usage().mutations, 0);
    Ok(())
}
