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
