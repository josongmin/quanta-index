//! Representation-independent map laws plus explicit direct-edge/object-count gold.

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn multi_child_values_preserve_all_85_edges_cas_and_reopen() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut work = budget()?;
    let first = value(&store, b"first real child", &mut work)?;
    let second = value(&store, b"second real child", &mut work)?;
    let children = [first, second, first, second, first];
    let scalar = MapValue::inline(b"fixed", children)?;
    let mut edits = vec![MapEdit {
        key: vec![],
        expected: None,
        next: Some(scalar),
    }];
    for key in 0_u8..16 {
        edits.push(MapEdit {
            key: vec![key << 4],
            expected: None,
            next: Some(scalar),
        });
    }
    let sink = RecordingSink::default();
    let root = store.apply(MapRoot(None), &edits, &sink, &mut work)?.root;
    let stored = store.get(root.0.ok_or("missing root")?, &mut work)?;
    check_eq!(stored.registration.children.as_slice(), children.repeat(17));
    check_eq!(stored.registration.children.len(), 85);
    let reversed = MapValue::inline(b"fixed", [second, first, first, second, first])?;
    check!(matches!(
        store.apply(
            root,
            &[MapEdit {
                key: vec![],
                expected: Some(reversed),
                next: None
            }],
            &sink,
            &mut work
        ),
        Err(IndexError::Conflict)
    ));
    let changed = store
        .apply(
            root,
            &[MapEdit {
                key: vec![],
                expected: Some(scalar),
                next: Some(reversed),
            }],
            &sink,
            &mut work,
        )?
        .root;
    let mut missing = stored.registration.children.as_slice().to_vec();
    let _last = missing.pop();
    let mut extra = stored.registration.children.as_slice().to_vec();
    extra.push(second);
    for invalid in [missing, extra] {
        let bad = store.put(&stored.payload, &invalid, &mut work)?;
        check!(matches!(
            store.read_node(bad.reference, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
    }
    drop(store);
    let reopened = FileObjectStore::open(&directory.0)?;
    check_eq!(reopened.lookup(root, b"", &mut work)?, Some(scalar));
    check_eq!(reopened.lookup(changed, b"", &mut work)?, Some(reversed));
    check_eq!(reopened.scan(root, None, 32, &mut work)?.entries.len(), 17);
    Ok(())
}

fn assert_model(
    store: &FileObjectStore,
    root: MapRoot,
    model: &BTreeMap<Vec<u8>, ObjectRef>,
    work: &mut WorkBudget,
) -> TestResult {
    for (key, value) in model {
        check_eq!(store.lookup_object(root, key, work)?, Some(*value));
    }
    for limit in [1, 7, 64] {
        let mut after = None;
        let mut actual = Vec::new();
        loop {
            let page = store.scan(root, after.as_deref(), limit, work)?;
            check!(page.entries.len() <= usize::try_from(limit)?);
            if page.has_more {
                check!(!page.entries.is_empty());
            }
            after = page.entries.last().map(|(key, _value)| key.clone());
            actual.extend(page.entries.iter().cloned());
            if !page.has_more {
                break;
            }
        }
        check_eq!(
            actual,
            model
                .iter()
                .map(|(key, value)| (key.clone(), (*value).into()))
                .collect::<Vec<_>>()
        );
    }
    Ok(())
}

#[test]
fn sixteen_terminal_siblings_are_one_object_with_direct_value_edges() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut work = budget()?;
    let mut model = BTreeMap::new();
    for key in 0x10_u8..=0x1f {
        let reference = value(&store, &[key], &mut work)?;
        let _prior = model.insert(vec![key], reference);
    }
    let before = physical_file_count(&directory)?;
    let edits = model
        .iter()
        .map(|(key, reference)| MapEdit {
            key: key.clone(),
            expected: None,
            next: Some((*reference).into()),
        })
        .collect::<Vec<_>>();
    let root = store
        .apply(MapRoot(None), &edits, &RecordingSink::default(), &mut work)?
        .root;
    // Sixteen independent keys require exactly one persisted map object.
    check_eq!(physical_file_count(&directory)? - before, 1);
    let stored = store.get(root.0.ok_or("missing map root")?, &mut work)?;
    check_eq!(
        stored.registration.children,
        model.values().copied().collect::<Vec<_>>()
    );
    // Contract bytes: magic, prefix length1, nibble1, no own value,
    // branch maskFFFF, terminal maskFFFF, sixteen empty tails + object-kind tags.
    let mut wire = b"QIMAP004\x00\x01\x01\x00\xff\xff\xff\xff".to_vec();
    wire.extend_from_slice(&[0; 48]);
    check_eq!(stored.payload, wire);
    assert_model(&store, root, &model, &mut work)?;

    let replacement = value(&store, b"replacement", &mut work)?;
    let extended = value(&store, b"prefix extension", &mut work)?;
    let fork = value(&store, b"other branch", &mut work)?;
    let changed = store
        .apply(
            root,
            &[
                MapEdit {
                    key: vec![0x10],
                    expected: (model.get(&vec![0x10]).copied()).map(Into::into),
                    next: None,
                },
                MapEdit {
                    key: vec![0x11],
                    expected: (model.get(&vec![0x11]).copied()).map(Into::into),
                    next: Some((replacement).into()),
                },
                MapEdit {
                    key: vec![0x11, 0],
                    expected: None,
                    next: Some((extended).into()),
                },
                MapEdit {
                    key: vec![0x20],
                    expected: None,
                    next: Some((fork).into()),
                },
            ],
            &RecordingSink::default(),
            &mut work,
        )?
        .root;
    let mut changed_model = model.clone();
    let _prior = changed_model.remove(&vec![0x10]);
    let _prior = changed_model.insert(vec![0x11], replacement);
    let _prior = changed_model.insert(vec![0x11, 0], extended);
    let _prior = changed_model.insert(vec![0x20], fork);
    assert_model(&store, changed, &changed_model, &mut work)?;
    // Old root custody is its immutable graph, including every prior value.
    assert_model(&store, root, &model, &mut work)?;
    drop(store);
    let reopened = FileObjectStore::open(&directory.0)?;
    assert_model(&reopened, root, &model, &mut work)?;
    assert_model(&reopened, changed, &changed_model, &mut work)?;
    Ok(())
}

#[test]
fn terminal_tail_boundary_and_maximum_key_keep_real_indirect_roots() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut work = budget()?;
    let short = value(&store, b"short", &mut work)?;
    let long = value(&store, b"long", &mut work)?;
    let mut keys = [vec![0x10], vec![0x11]];
    keys[0].extend_from_slice(&[0xaa; 128]);
    keys[1].extend_from_slice(&[0xbb; 129]);
    let before = physical_file_count(&directory)?;
    let edits = keys
        .iter()
        .zip([short, long])
        .map(|(key, value)| MapEdit {
            key: key.clone(),
            expected: None,
            next: Some((value).into()),
        })
        .collect::<Vec<_>>();
    let root = store
        .apply(MapRoot(None), &edits, &RecordingSink::default(), &mut work)?
        .root;
    check_eq!(physical_file_count(&directory)? - before, 2);
    let node = store.read_node(root.0.ok_or("missing boundary root")?, &mut work)?;
    check!(
        matches!(node.branches.get(0)?, Some(Branch { prefix, value: BranchValue::Terminal(value) }) if prefix.len() == 256 && *value == short.into())
    );
    check!(matches!(
        node.branches.get(1)?,
        Some(Branch {
            value: BranchValue::Indirect(_),
            ..
        })
    ));
    let model = keys
        .into_iter()
        .zip([short, long])
        .collect::<BTreeMap<_, _>>();
    assert_model(&store, root, &model, &mut work)?;
    // Collapsing to one long key must still return an installed object root.
    let short_key = model.keys().next().ok_or("missing short key")?;
    let one = store
        .apply(
            root,
            &[MapEdit {
                key: short_key.clone(),
                expected: Some((short).into()),
                next: None,
            }],
            &RecordingSink::default(),
            &mut work,
        )?
        .root;
    let one_stored = store.get(one.0.ok_or("missing collapsed root")?, &mut work)?;
    check_eq!(one_stored.registration.children, vec![long]);

    let mut maximum_keys = [vec![0x20], vec![0x21]];
    maximum_keys[0].resize(4096, 0xaa);
    maximum_keys[1].resize(4096, 0xbb);
    let maximum_edits = maximum_keys
        .iter()
        .zip([short, long])
        .map(|(key, value)| MapEdit {
            key: key.clone(),
            expected: None,
            next: Some((value).into()),
        })
        .collect::<Vec<_>>();
    let maximum = store
        .apply(
            MapRoot(None),
            &maximum_edits,
            &RecordingSink::default(),
            &mut work,
        )?
        .root;
    let maximum_model = maximum_keys
        .into_iter()
        .zip([short, long])
        .collect::<BTreeMap<_, _>>();
    assert_model(&store, maximum, &maximum_model, &mut work)?;
    Ok(())
}

#[test]
fn authenticated_map_objects_refuse_bad_inline_codec_and_prior_format() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut work = budget()?;
    let child = value(&store, b"leaf value", &mut work)?;
    // Two sibling branches0/1 with empty tails and one direct value edge each.
    let valid = b"QIMAP004\x00\x01\x01\x00\x00\x03\x00\x03\x00\x00\x00\x00\x00\x00".to_vec();
    let mut cases = Vec::new();
    let mut flag = valid.clone();
    *flag.get_mut(11).ok_or("codec gold offset missing")? = 2;
    cases.push((flag, vec![child, child]));
    let mut mask = valid.clone();
    *mask.get_mut(15).ok_or("codec gold offset missing")? = 7;
    cases.push((mask, vec![child, child]));
    let mut length = valid.clone();
    *length.get_mut(16).ok_or("codec gold offset missing")? = 1;
    *length.get_mut(17).ok_or("codec gold offset missing")? = 1;
    cases.push((length, vec![child, child]));
    let mut nibble = valid.clone();
    *nibble.get_mut(17).ok_or("codec gold offset missing")? = 1;
    nibble.insert(18, 16);
    cases.push((nibble, vec![child, child]));
    let mut trailing = valid.clone();
    trailing.push(0);
    cases.push((trailing, vec![child, child]));
    cases.push((valid.clone(), vec![child]));
    for (bytes, edges) in cases {
        let registration = store.put(&bytes, &edges, &mut work)?;
        check!(matches!(
            store.read_node(registration.reference, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
    }
    let registration = store.put(&valid, &[child, child], &mut work)?;
    check!(store.read_node(registration.reference, &mut work).is_ok());
    for prior in [
        b"QIMAP001".as_slice(),
        b"QIMAP002".as_slice(),
        b"QIMAP003".as_slice(),
    ] {
        let old = store.put(prior, &[child], &mut work)?;
        check!(matches!(
            store.read_node(old.reference, &mut work),
            Err(IndexError::RebuildRequired)
        ));
    }
    Ok(())
}

#[test]
fn inline_value_gold_cas_edges_and_history_survive_reopen() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut work = budget()?;
    let sink = RecordingSink::default();
    let data = [23; 32];
    let scalar = MapValue::inline(&data, None)?;
    let before = physical_file_count(&directory)?;
    let root = store
        .apply(
            MapRoot(None),
            &[MapEdit {
                key: vec![],
                expected: None,
                next: Some(scalar),
            }],
            &sink,
            &mut work,
        )?
        .root;
    check_eq!(physical_file_count(&directory)? - before, 1);
    let object = store.get(root.0.ok_or("missing scalar root")?, &mut work)?;
    // Independent bytes: empty prefix, present value, zero branch masks,
    // inline tag, exact32 data bytes, absent child. No object identity is invented.
    let mut gold = b"QIMAP004\0\0\x01\0\0\0\0\x01\x20".to_vec();
    gold.extend_from_slice(&data);
    gold.push(0);
    check_eq!(object.payload.as_slice(), gold.as_slice());
    check!(object.registration.children.is_empty());
    check!(matches!(
        store.lookup_object(root, b"", &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    let child = value(&store, b"real child", &mut work)?;
    let compound = MapValue::inline(&data, Some(child))?;
    let next = store
        .apply(
            root,
            &[MapEdit {
                key: vec![],
                expected: Some(scalar),
                next: Some(compound),
            }],
            &sink,
            &mut work,
        )?
        .root;
    check_ne!(root, next);
    let compound_object = store.get(next.0.ok_or("missing compound root")?, &mut work)?;
    check_eq!(compound_object.registration.children.as_slice(), &[child]);
    // Identical inline bytes with a different kind or child cannot satisfy CAS.
    for wrong in [scalar, MapValue::Object(child)] {
        check!(matches!(
            store.apply(
                next,
                &[MapEdit {
                    key: vec![],
                    expected: Some(wrong),
                    next: None
                }],
                &sink,
                &mut work
            ),
            Err(IndexError::Conflict)
        ));
    }
    let mixed = store
        .apply(
            next,
            &[MapEdit {
                key: b"b".to_vec(),
                expected: None,
                next: Some(child.into()),
            }],
            &sink,
            &mut work,
        )?
        .root;
    let page = store.scan(mixed, None, 1, &mut work)?;
    check_eq!(page.entries.as_slice(), &[(vec![], compound)]);
    check!(page.has_more);
    let second_page = store.scan(mixed, Some(b""), 1, &mut work)?;
    check_eq!(
        second_page.entries.as_slice(),
        &[(b"b".to_vec(), child.into())]
    );
    drop(store);
    let reopened = FileObjectStore::open(&directory.0)?;
    check_eq!(reopened.lookup(root, b"", &mut work)?, Some(scalar));
    check_eq!(reopened.lookup(next, b"", &mut work)?, Some(compound));
    check_eq!(reopened.lookup_object(mixed, b"b", &mut work)?, Some(child));
    let removed = reopened
        .apply(
            mixed,
            &[MapEdit {
                key: vec![],
                expected: Some(compound),
                next: None,
            }],
            &sink,
            &mut work,
        )?
        .root;
    check_eq!(reopened.lookup(removed, b"", &mut work)?, None);
    check_eq!(reopened.lookup(next, b"", &mut work)?, Some(compound));
    Ok(())
}

#[test]
fn inline_scalar_codec_requires_exact_shape_and_physical_children() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut work = budget()?;
    let child = value(&store, b"codec child", &mut work)?;
    let valid = b"QIMAP004\0\0\x01\0\0\0\0\x01\x01a\0".to_vec();
    let stored = store.put(&valid, &[], &mut work)?;
    check!(store.read_node(stored.reference, &mut work).is_ok());
    let mut missing_child = valid.clone();
    *missing_child.last_mut().ok_or("missing child flag")? = 1;
    let mut bad_tag = valid.clone();
    *bad_tag.get_mut(15).ok_or("missing value tag")? = 2;
    let mut oversize = valid.clone();
    *oversize.get_mut(16).ok_or("missing value length")? = 33;
    let mut bad_flag = valid.clone();
    *bad_flag.last_mut().ok_or("missing child flag")? = 6;
    let mut trailing = valid.clone();
    trailing.push(0);
    for (bytes, children) in [
        (valid, vec![child]),
        (missing_child, vec![]),
        (bad_tag, vec![]),
        (oversize, vec![]),
        (bad_flag, vec![]),
        (trailing, vec![]),
    ] {
        let stored = store.put(&bytes, &children, &mut work)?;
        check!(matches!(
            store.read_node(stored.reference, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
    }
    Ok(())
}
