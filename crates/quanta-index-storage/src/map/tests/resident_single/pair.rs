use super::*;

#[test]
fn resident_pair_matches_scalar_gold_and_preserves_both_old_roots() -> TestResult {
    let directory = TestDir::new();
    fs::DirBuilder::new().mode(0o700).create(&directory.0)?;
    let scratch = Arc::new(CountScratchSpace {
        inner: crate::FileScratchSpace::open_standalone(
            directory.0.join("scratch"),
            16 * 1024 * 1024,
        )?,
        allocations: AtomicU64::new(0),
    });
    let store = FileObjectStore::open_with_scratch(directory.0.join("objects"), scratch.clone())?;
    let mut work = budget()?;
    let old = value(&store, b"pair-old", &mut work)?;
    let new = value(&store, b"pair-new", &mut work)?;
    let sink = RecordingSink::default();
    let baseline = BTreeMap::from([(b"a".to_vec(), old), (b"b".to_vec(), old)]);
    let setup = baseline
        .keys()
        .map(|key| MapEdit::objects(key.clone(), None, Some(old)))
        .collect::<Vec<_>>();
    let base = store.apply(MapRoot(None), &setup, &sink, &mut work)?.root;
    let cases = [
        [
            MapEdit::objects(b"a".to_vec(), Some(old), Some(new)),
            MapEdit::objects(b"b".to_vec(), Some(old), None),
        ],
        [
            MapEdit::objects(b"a".to_vec(), Some(old), Some(new)),
            MapEdit::objects(b"a".to_vec(), Some(old), Some(new)),
        ],
        [
            MapEdit::objects(b"a".to_vec(), Some(old), Some(old)),
            MapEdit::objects(b"b".to_vec(), Some(old), Some(new)),
        ],
        [
            MapEdit::objects(Vec::new(), None, Some(new)),
            MapEdit::objects(vec![b'q'; 64], None, Some(new)),
        ],
        [
            MapEdit::objects(vec![b'q'; 65], None, Some(new)),
            MapEdit::objects(b"b".to_vec(), Some(old), Some(new)),
        ],
    ];
    for edits in cases {
        let gold = [
            store
                .apply(base, std::slice::from_ref(&edits[0]), &sink, &mut work)?
                .root,
            store
                .apply(base, std::slice::from_ref(&edits[1]), &sink, &mut work)?
                .root,
        ];
        let before = scratch.allocated();
        let reservations = sink.1.load(Ordering::Relaxed);
        let mut input = TestEditCursor(
            edits
                .iter()
                .cloned()
                .enumerate()
                .map(|(slot, edit)| {
                    OrdinalMapEdit::unscoped(u32::try_from(slot).expect("two fixed roots"), edit)
                })
                .collect::<Vec<_>>()
                .into_iter(),
        );
        let result = store.apply_stream(&[base, base], &mut input, &sink, &mut work)?;
        check_eq!(result.as_slice(), gold.as_slice());
        let fallback = u64::from(edits.iter().any(|edit| edit.key.len() > 64));
        check_eq!(
            scratch.allocated(),
            before
                .checked_add(fallback)
                .ok_or("scratch count overflow")?
        );
        if edits[0] == edits[1] {
            check_eq!(
                sink.1.load(Ordering::Relaxed),
                reservations.checked_add(1).ok_or("reservation overflow")?
            );
        }
        assert_model(&store, base, &baseline, &mut work)?;
        for (slot, edit) in edits.iter().enumerate() {
            let mut model = baseline.clone();
            if let Some(next) = edit.next {
                let _old = model.insert(edit.key.clone(), next.object_ref()?);
            } else {
                let _old = model.remove(&edit.key);
            }
            assert_model(
                &store,
                *result.as_slice().get(slot).ok_or("pair root missing")?,
                &model,
                &mut work,
            )?;
        }
    }
    for bases in [
        [MapRoot(None); 2],
        [MapRoot(None), base],
        [base, MapRoot(None)],
    ] {
        let edits = [
            MapEdit::objects(b"new-left".to_vec(), None, Some(new)),
            MapEdit::objects(b"new-right".to_vec(), None, Some(new)),
        ];
        let gold = [
            store
                .apply(bases[0], std::slice::from_ref(&edits[0]), &sink, &mut work)?
                .root,
            store
                .apply(bases[1], std::slice::from_ref(&edits[1]), &sink, &mut work)?
                .root,
        ];
        let before = scratch.allocated();
        let mut input = TestEditCursor(
            edits
                .into_iter()
                .enumerate()
                .map(|(slot, edit)| {
                    OrdinalMapEdit::unscoped(u32::try_from(slot).expect("two fixed roots"), edit)
                })
                .collect::<Vec<_>>()
                .into_iter(),
        );
        let roots = store.apply_stream(&bases, &mut input, &sink, &mut work)?;
        check_eq!(roots.as_slice(), gold.as_slice());
        check_eq!(scratch.allocated(), before);
        assert_model(&store, base, &baseline, &mut work)?;
    }
    assert_clean_scratch(&directory)?;
    Ok(())
}

#[test]
fn resident_pair_requires_eof_and_replays_a_non_pair_without_installing_prefix() -> TestResult {
    struct TailFailure(std::vec::IntoIter<OrdinalMapEdit>);
    impl MapEditCursor for TailFailure {
        fn next_edit(
            &mut self,
            _budget: &mut WorkBudget,
        ) -> Result<Option<OrdinalMapEdit>, IndexError> {
            self.0
                .next()
                .map(Some)
                .ok_or_else(|| IndexError::CorruptObject("invalid pair cursor tail".into()))
        }
    }
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut work = budget()?;
    let value = value(&store, b"pair-tail", &mut work)?;
    let sink = RecordingSink::default();
    let before = physical_file_count(&directory)?;
    let rows = (0..2)
        .map(|root| {
            OrdinalMapEdit::unscoped(root, MapEdit::objects(b"a".to_vec(), None, Some(value)))
        })
        .collect::<Vec<_>>();
    let mut broken = TailFailure(rows.clone().into_iter());
    check!(matches!(
        store.apply_stream(&[MapRoot(None); 2], &mut broken, &sink, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    let mut invalid = rows;
    invalid.push(OrdinalMapEdit::unscoped(
        2,
        MapEdit::objects(b"b".to_vec(), None, Some(value)),
    ));
    let mut input = TestEditCursor(invalid.into_iter());
    check!(matches!(
        store.apply_stream(&[MapRoot(None); 2], &mut input, &sink, &mut work),
        Err(IndexError::InvalidInput(_))
    ));
    check_eq!(sink.1.load(Ordering::Relaxed), 0);
    check_eq!(sink.2.load(Ordering::Relaxed), 0);
    check_eq!(physical_file_count(&directory)?, before);
    assert_clean_scratch(&directory)?;
    Ok(())
}
