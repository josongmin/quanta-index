use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn completed_group_matches_independent_set_with_bounded_work_and_refunds() -> TestResult {
    const MASK: usize = (MAX_INSTALL_BATCH_INPUTS * 2).next_power_of_two() - 1;
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut setup = budget()?;
    let mut objects = (0..MAX_INSTALL_BATCH_INPUTS)
        .map(|id| {
            store.prepare_object(
                &u64::try_from(id)
                    .map_err(|_error| IndexError::ResourceLimited)?
                    .to_be_bytes(),
                &[],
                &mut setup,
            )
        })
        .collect::<Result<Vec<_>, IndexError>>()?;
    let bucket = |reference: ObjectRef| {
        let [first, second, ..] = (reference.0).0;
        usize::from(u16::from_le_bytes([first, second])) & MASK
    };
    let first = objects
        .first()
        .ok_or("empty fixture")?
        .registration
        .reference;
    let mut collisions = Vec::new();
    for id in 10_000_u64..75_536 {
        let candidate = store.prepare_object(&id.to_be_bytes(), &[], &mut setup)?;
        if bucket(candidate.registration.reference) == bucket(first) {
            check!(candidate.registration.reference != first);
            collisions.push(candidate);
            if collisions.len() == 2 {
                break;
            }
        }
    }
    let absent = collisions.pop().ok_or("missing absent collision fixture")?;
    *objects.last_mut().ok_or("empty fixture")? =
        collisions.pop().ok_or("missing collision fixture")?;
    // A full physical group must not require the previous quadratic scan bill.
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 0,
            writes: 0,
            bytes: 128 * 1024,
            mutations: 0,
        },
        Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or(IndexError::ResourceLimited)?,
    );
    let tracker = array_scope(MemoryLifetime::Temporary, Some(0), &mut work);
    let mut group = CompletedGroup::new();
    let mut model = std::collections::BTreeSet::new();
    for (index, object) in objects.iter().enumerate() {
        let reference = object.registration.reference;
        check_eq!(
            group.contains(reference, &objects, &mut work)?,
            model.contains(&reference)
        );
        group.insert(reference, index, &objects, &mut work)?;
        let _inserted = model.insert(reference);
    }
    for object in &objects {
        check_eq!(
            group.contains(object.registration.reference, &objects, &mut work)?,
            model.contains(&object.registration.reference)
        );
    }
    check!(!group.contains(absent.registration.reference, &objects, &mut work)?);
    group.clear(&mut work)?;
    for object in &objects {
        check!(!group.contains(object.registration.reference, &objects, &mut work)?);
    }
    check!(tracker.live.load(Ordering::Relaxed) > 0);
    drop(group);
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);
    let mut denied = budget()?;
    let tracker = array_scope(MemoryLifetime::Temporary, None, &mut denied);
    let first = objects
        .first()
        .ok_or("empty fixture")?
        .registration
        .reference;
    let mut group = CompletedGroup::new();
    check!(matches!(
        group.insert(first, 0, &objects, &mut denied),
        Err(IndexError::ResourceLimited)
    ));
    check!(!group.contains(first, &objects, &mut denied)?);
    drop(group);
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn resident_install_does_not_allocate_the_deferred_group_index() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut setup = budget()?;
    let object = value(&store, b"resident value", &mut setup)?;
    let mut work = budget()?;
    let table_bytes = 2usize
        .checked_mul(MAX_INSTALL_BATCH_INPUTS)
        .and_then(|slots| slots.checked_mul(std::mem::size_of::<usize>()))
        .ok_or(IndexError::ResourceLimited)?;
    let tracker = array_scope(
        MemoryLifetime::Temporary,
        Some(u64::try_from(table_bytes)?),
        &mut work,
    );
    let root = store
        .apply(
            MapRoot(None),
            &[MapEdit::objects(b"a".to_vec(), None, Some(object))],
            &RecordingSink::default(),
            &mut work,
        )?
        .root;
    check_eq!(store.lookup_object(root, b"a", &mut work)?, Some(object));
    check_eq!(tracker.refused.load(Ordering::Relaxed), 0);
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);
    Ok(())
}
