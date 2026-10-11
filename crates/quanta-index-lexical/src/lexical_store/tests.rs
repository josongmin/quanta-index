use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::Arc;

use super::{AttemptBinding, BlobRef, MapEdit, MapRoot, ObjectKind, ObjectStore};

fn binding() -> AttemptBinding {
    AttemptBinding {
        root_incarnation: [1; 16],
        repository_sha256: [2; 32],
        revision_sha256: [5; 32],
        source_event_sha256: [3; 32],
        target_generation: 1,
        base_root_sha256: None,
        operations_sha256: [4; 32],
        format: 1,
    }
}

fn key(number: u32) -> Vec<u8> {
    number.to_be_bytes().to_vec()
}

#[test]
fn decoded_map_page_cache_preserves_lookup_oracle_and_rejects_foreign_descriptor() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ObjectStore::open(dir.path()).expect("object store");
    let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
    let first = store
        .put_blob(&lease, ObjectKind::Record, &[], b"first")
        .expect("first value");
    let second = store
        .put_blob(&lease, ObjectKind::Record, &[], b"second")
        .expect("second value");
    let expected = (0..300_u32)
        .map(|number| {
            (
                key(number),
                if number % 2 == 0 {
                    first.clone()
                } else {
                    second.clone()
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let root = store
        .map_bootstrap(
            &lease,
            expected.iter().map(|(name, value)| {
                Ok(MapEdit {
                    key: name.clone(),
                    value: Some(value.clone()),
                })
            }),
        )
        .expect("map bootstrap");
    let query = store.metered_clone();
    let descriptor = root.page.as_ref().expect("root page");
    let decoded = query
        .load_page_shared(descriptor, None)
        .expect("first decoded page");
    let first_reads = query.read_meter_snapshot();
    let again = query
        .load_page_shared(descriptor, None)
        .expect("retained decoded page");
    assert!(Arc::ptr_eq(&decoded, &again));
    assert_eq!(query.read_meter_snapshot(), first_reads);
    for (name, value) in &expected {
        assert_eq!(
            query.map_get(&root, name, None).expect("lookup"),
            Some(value.clone())
        );
    }
    assert_eq!(
        query
            .map_get(&root, &key(300), None)
            .expect("absent lookup"),
        None
    );
    let mut foreign = root.clone();
    let changed = foreign.page.as_mut().expect("foreign page");
    changed.entries = changed.entries.saturating_add(1);
    foreign.entries = foreign.entries.saturating_add(1);
    assert!(matches!(
        query.map_get(&foreign, &key(0), None),
        Err(quanta_index_core::CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
            ..
        })
    ));
}

#[test]
fn blob_blocks_and_dependency_closure_are_authenticated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ObjectStore::open(dir.path()).expect("object store");
    let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
    let small = store
        .put_blob(&lease, ObjectKind::Record, &[], b"record")
        .expect("small blob");
    let body = vec![b'x'; 150_000];
    let large = store
        .put_blob(
            &lease,
            ObjectKind::FileBody,
            std::slice::from_ref(&small.root),
            &body,
        )
        .expect("large blob");
    assert_eq!(
        store.read_blob(&large, 150_000, None).expect("roundtrip"),
        body
    );
    let metered = store.metered_clone();
    let mut cursor = metered.blob_cursor(&large, 150_000).expect("blob cursor");
    let mut streamed = Vec::new();
    let mut charged = 0_u64;
    while let Some((chunk, physical_bytes)) = cursor.next(&metered, None).expect("next block") {
        assert!(chunk.len() <= 32 * 1024);
        streamed.extend_from_slice(&chunk);
        charged = charged.saturating_add(physical_bytes);
    }
    assert_eq!(streamed, body);
    assert_eq!(metered.read_meter_snapshot().0, charged);
    assert!(metered.read_meter_snapshot().1 > 1);
    let mut closure = BTreeSet::new();
    store
        .walk_closure(std::slice::from_ref(&large.root), None, &mut |reference| {
            let _inserted = closure.insert(reference.clone());
            Ok(())
        })
        .expect("closure");
    assert!(closure.contains(&small.root));
    assert!(closure.contains(&large.root));
    let dependencies = store
        .object_dependencies(&large.root, None)
        .expect("dependencies");
    assert!(dependencies.contains(&small.root));

    let mut hex = String::new();
    for byte in large.root.sha256 {
        write!(&mut hex, "{byte:02x}").expect("write digest to String");
    }
    let shard = hex.get(..2).expect("digest prefix");
    let path = dir
        .path()
        .join("lexical-object-pool")
        .join("objects")
        .join(shard)
        .join(&hex);
    let mut encoded = std::fs::read(&path).expect("object bytes");
    let last = encoded.last_mut().expect("object nonempty");
    *last ^= 1;
    std::fs::write(path, encoded).expect("tamper object");
    assert!(store.read_blob(&large, 150_000, None).is_err());
    assert!(metered.read_blob(&large, 150_000, None).is_err());
    assert_eq!(
        metered.fault_reference().expect("fault reference"),
        Some(large.root)
    );
}

#[test]
fn map_bootstrap_delta_absence_cursor_and_changed_key_oracle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ObjectStore::open(dir.path()).expect("object store");
    let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
    let value = store
        .put_blob(&lease, ObjectKind::Record, &[], b"value")
        .expect("value");
    let changed = store
        .put_blob(&lease, ObjectKind::Record, &[], b"changed")
        .expect("changed");
    let rows = (0..500_u32).map(|number| {
        Ok(MapEdit {
            key: key(number),
            value: Some(value.clone()),
        })
    });
    let base = store.map_bootstrap(&lease, rows).expect("bootstrap");
    assert_eq!(base.entries, 500);
    assert_eq!(
        store.map_get(&base, &key(150), None).expect("present"),
        Some(value.clone())
    );
    assert_eq!(store.map_get(&base, &key(700), None).expect("absent"), None);

    let edits = [
        MapEdit {
            key: key(5),
            value: None,
        },
        MapEdit {
            key: key(150),
            value: Some(changed.clone()),
        },
        MapEdit {
            key: key(501),
            value: Some(value.clone()),
        },
    ];
    let updated = store
        .map_apply(&lease, &base, edits.into_iter().map(Ok))
        .expect("delta");
    assert_eq!(updated.entries, 500);
    assert_eq!(
        store.map_get(&updated, &key(5), None).expect("deleted"),
        None
    );
    assert_eq!(
        store.map_get(&updated, &key(150), None).expect("replaced"),
        Some(changed)
    );
    assert_eq!(
        store.map_get(&updated, &key(501), None).expect("inserted"),
        Some(value)
    );
    let mut observed = Vec::new();
    store
        .map_changed_keys(&base, &updated, None, &mut |name, old, new| {
            observed.push((name.to_vec(), old.cloned(), new.cloned()));
            Ok(())
        })
        .expect("changed keys");
    assert_eq!(
        observed
            .iter()
            .map(|(name, _, _)| name.clone())
            .collect::<Vec<_>>(),
        vec![key(5), key(150), key(501)]
    );
    let mut cursor = store
        .map_cursor(&updated, &key(497), Some(&key(502)), None)
        .expect("cursor");
    let mut tail = Vec::new();
    while let Some((name, _value)) = cursor.next().expect("next") {
        tail.push(name);
    }
    assert_eq!(tail, vec![key(497), key(498), key(499), key(501)]);
    let reopened = ObjectStore::open(dir.path()).expect("reopen");
    assert!(
        reopened
            .map_get(&updated, &key(150), None)
            .expect("cold get")
            .is_some()
    );
}

#[test]
fn dropped_attempt_is_durable_and_changed_retry_binding_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ObjectStore::open(dir.path()).expect("object store");
    let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
    let root = store
        .put_blob(&lease, ObjectKind::Record, &[], b"prepared")
        .expect("prepared")
        .root;
    drop(lease);
    let reopened = ObjectStore::open(dir.path()).expect("reopen");
    assert!(reopened.adopted_roots().expect("adopted").contains(&root));
    let mut changed = binding();
    changed.operations_sha256 = [9; 32];
    assert!(reopened.begin_attempt(&changed, &[]).is_err());
    let replay = reopened
        .begin_attempt(&binding(), &[])
        .expect("same attempt retry");
    replay.discard_unsealed().expect("retire attempt");
    assert!(
        reopened
            .adopted_roots()
            .expect("adopted after retire")
            .is_empty()
    );
}

#[test]
fn empty_map_and_bulk_deletions_have_proven_absence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ObjectStore::open(dir.path()).expect("object store");
    let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
    let empty = MapRoot::default();
    assert_eq!(
        store.map_get(&empty, b"missing", None).expect("empty map"),
        None
    );
    let value: BlobRef = store
        .put_blob(&lease, ObjectKind::Record, &[], b"v")
        .expect("value");
    let base = store
        .map_bootstrap(
            &lease,
            (0..200_u32).map(|number| {
                Ok(MapEdit {
                    key: key(number),
                    value: Some(value.clone()),
                })
            }),
        )
        .expect("bootstrap");
    let deleted = store
        .map_apply(
            &lease,
            &base,
            (0..200_u32).map(|number| {
                Ok(MapEdit {
                    key: key(number),
                    value: None,
                })
            }),
        )
        .expect("delete all");
    assert_eq!(deleted, empty);
    let mut count = 0_usize;
    store
        .map_scan(&deleted, b"", None, None, &mut |_key, _value| {
            count = count.saturating_add(1);
            Ok(())
        })
        .expect("empty scan");
    assert_eq!(count, 0);
}

#[test]
fn branch_split_and_repartition_preserve_independent_key_diff() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ObjectStore::open(dir.path()).expect("object store");
    let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
    let value = store
        .put_blob(&lease, ObjectKind::Record, &[], b"v")
        .expect("value");
    let base = store
        .map_bootstrap(
            &lease,
            (0..6200_u32).map(|number| {
                Ok(MapEdit {
                    key: key(number),
                    value: Some(value.clone()),
                })
            }),
        )
        .expect("bootstrap across branch fanout");
    let updated = store
        .map_apply(
            &lease,
            &base,
            [
                MapEdit {
                    key: key(0),
                    value: None,
                },
                MapEdit {
                    key: key(6199),
                    value: None,
                },
                MapEdit {
                    key: key(6200),
                    value: Some(value),
                },
            ]
            .into_iter()
            .map(Ok),
        )
        .expect("repartition delta");
    assert_eq!(updated.entries, 6199);
    let mut changed = Vec::new();
    store
        .map_changed_keys(&base, &updated, None, &mut |name, _old, _new| {
            changed.push(name.to_vec());
            Ok(())
        })
        .expect("independent diff");
    assert_eq!(changed, vec![key(0), key(6199), key(6200)]);
    assert!(
        store
            .map_get(&updated, &key(3100), None)
            .expect("middle remains")
            .is_some()
    );
}

#[test]
fn map_diff_handles_root_height_changes_in_both_directions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ObjectStore::open(dir.path()).expect("object store");
    let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
    let value = store
        .put_blob(&lease, ObjectKind::Record, &[], b"height-boundary")
        .expect("value");
    let base = store
        .map_bootstrap(
            &lease,
            (0..96_u32).map(|number| {
                Ok(MapEdit {
                    key: key(number),
                    value: Some(value.clone()),
                })
            }),
        )
        .expect("single full leaf");
    let grown = store
        .map_apply(
            &lease,
            &base,
            std::iter::once(Ok(MapEdit {
                key: key(96),
                value: Some(value.clone()),
            })),
        )
        .expect("grow root");
    assert_eq!(base.page.as_ref().expect("base page").height, 0);
    assert_eq!(grown.page.as_ref().expect("grown page").height, 1);
    let mut added = Vec::new();
    store
        .map_changed_keys(&base, &grown, None, &mut |name, before, after| {
            added.push((name.to_vec(), before.cloned(), after.cloned()));
            Ok(())
        })
        .expect("height growth diff");
    assert_eq!(added, vec![(key(96), None, Some(value.clone()))]);
    let mut removed = Vec::new();
    store
        .map_changed_keys(&grown, &base, None, &mut |name, before, after| {
            removed.push((name.to_vec(), before.cloned(), after.cloned()));
            Ok(())
        })
        .expect("reverse height diff");
    assert_eq!(removed, vec![(key(96), Some(value), None)]);
}

#[test]
fn repartitioned_diff_matches_fixed_btree_oracle_and_skips_shared_branches() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ObjectStore::open(dir.path()).expect("object store");
    let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
    let original = store
        .put_blob(&lease, ObjectKind::Record, &[], b"original")
        .expect("original value");
    let replacement = store
        .put_blob(&lease, ObjectKind::Record, &[], b"replacement")
        .expect("replacement value");
    let source = (0..12_500_u32)
        .map(|number| (number, original.clone()))
        .collect::<BTreeMap<_, _>>();
    let base = store
        .map_bootstrap(
            &lease,
            source.iter().map(|(number, value)| {
                Ok(MapEdit {
                    key: key(*number),
                    value: Some(value.clone()),
                })
            }),
        )
        .expect("multi-branch bootstrap");
    let boundary = store
        .map_apply(
            &lease,
            &base,
            std::iter::once(Ok(MapEdit {
                key: key(0),
                value: None,
            })),
        )
        .expect("first-key deletion");
    let metered = store.metered_clone();
    let mut boundary_changes = Vec::new();
    metered
        .map_changed_keys(&base, &boundary, None, &mut |name, before, after| {
            boundary_changes.push((name.to_vec(), before.cloned(), after.cloned()));
            Ok(())
        })
        .expect("boundary diff");
    assert_eq!(boundary_changes, vec![(key(0), Some(original), None)]);
    let (_, diff_reads) = metered.read_meter_snapshot();
    let baseline = store.metered_clone();
    for root in [&base, &boundary] {
        baseline
            .map_scan(root, b"", None, None, &mut |_name, _value| Ok(()))
            .expect("full authenticated walk");
    }
    let (_, full_reads) = baseline.read_meter_snapshot();
    assert!(diff_reads.saturating_mul(3) < full_reads.saturating_mul(2));

    let mut edits = BTreeMap::<u32, Option<BlobRef>>::new();
    for number in [0, 95, 96, 6143, 6144, 12_499, 12_500] {
        let _previous = edits.insert(
            number,
            if number % 3 == 0 {
                None
            } else {
                Some(replacement.clone())
            },
        );
    }
    let mut seed = 0x9e37_79b9_u32;
    for _ in 0..48 {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let number = seed % 12_750;
        let _previous = edits.insert(
            number,
            if number.is_multiple_of(5) {
                None
            } else {
                Some(replacement.clone())
            },
        );
    }
    let mut expected_after = source.clone();
    for (number, value) in &edits {
        match value {
            Some(value) => {
                let _previous = expected_after.insert(*number, value.clone());
            }
            None => {
                let _previous = expected_after.remove(number);
            }
        }
    }
    let updated = store
        .map_apply(
            &lease,
            &base,
            edits.iter().map(|(number, value)| {
                Ok(MapEdit {
                    key: key(*number),
                    value: value.clone(),
                })
            }),
        )
        .expect("fixed-seed edits");
    let keys = source
        .keys()
        .chain(expected_after.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    let expected = keys
        .into_iter()
        .filter_map(|number| {
            let before = source.get(&number).cloned();
            let after = expected_after.get(&number).cloned();
            (before != after).then(|| (key(number), before, after))
        })
        .collect::<Vec<_>>();
    let mut observed = Vec::new();
    store
        .map_changed_keys(&base, &updated, None, &mut |name, before, after| {
            observed.push((name.to_vec(), before.cloned(), after.cloned()));
            Ok(())
        })
        .expect("fixed-seed diff");
    assert_eq!(observed, expected);

    let base_root = base.page.as_ref().expect("base root");
    let changed_root = boundary.page.as_ref().expect("changed root");
    let base_children = store
        .object_dependencies(&base_root.object, None)
        .expect("base root children")
        .into_iter()
        .collect::<BTreeSet<_>>();
    let shared = store
        .object_dependencies(&changed_root.object, None)
        .expect("changed root children")
        .into_iter()
        .find(|child| base_children.contains(child))
        .expect("unmodified child branch");
    let mut hex = String::new();
    for byte in shared.sha256 {
        write!(&mut hex, "{byte:02x}").expect("write digest to String");
    }
    let digest_prefix = hex.get(..2).expect("digest prefix");
    let path = dir
        .path()
        .join("lexical-object-pool")
        .join("objects")
        .join(digest_prefix)
        .join(&hex);
    let mut bytes = std::fs::read(&path).expect("shared branch bytes");
    let last = bytes.last_mut().expect("nonempty shared branch");
    *last ^= 1;
    std::fs::write(&path, bytes).expect("corrupt shared branch");
    let fresh = store.metered_clone();
    let refusal = fresh.map_changed_keys(&base, &boundary, None, &mut |_name, _, _| Ok(()));
    assert!(matches!(
        refusal,
        Err(quanta_index_core::CoreError::Typed { .. })
    ));
    let expired = quanta_index_core::RequestBudgetV1::until(std::time::Instant::now());
    let cancelled = store.map_changed_keys(&base, &base, Some(&expired), &mut |_name, _, _| Ok(()));
    assert!(cancelled.is_err());
}

#[test]
fn gc_refuses_stale_mark_after_adoption_and_deletes_only_unmarked_orphan() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ObjectStore::open(dir.path()).expect("object store");
    let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
    let old_epoch = store.gc_epoch().expect("mark epoch");
    let blob = store
        .put_blob(&lease, ObjectKind::Record, &[], b"orphan")
        .expect("blob");
    assert!(
        !store
            .gc_sweep_batch(old_epoch, &[blob.root.sha256], &BTreeSet::new())
            .expect("stale sweep")
    );
    assert_eq!(
        store.read_blob(&blob, 64, None).expect("protected blob"),
        b"orphan"
    );
    lease.discard_unsealed().expect("discard attempt");
    let current_epoch = store.gc_epoch().expect("fresh epoch");
    assert!(
        store
            .gc_sweep_batch(current_epoch, &[blob.root.sha256], &BTreeSet::new())
            .expect("orphan sweep")
    );
    assert!(store.read_blob(&blob, 64, None).is_err());
}

#[test]
fn serving_open_is_read_only_and_limited_scan_stops_early() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(ObjectStore::open_existing(dir.path()).is_err());
    assert!(!dir.path().join("lexical-object-pool").exists());
    let store = ObjectStore::open(dir.path()).expect("writer open");
    let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
    let value = store
        .put_blob(&lease, ObjectKind::Record, &[], b"v")
        .expect("value");
    let root = store
        .map_bootstrap(
            &lease,
            (0..500_u32).map(|number| {
                Ok(MapEdit {
                    key: key(number),
                    value: Some(value.clone()),
                })
            }),
        )
        .expect("bootstrap");
    let serving = ObjectStore::open_existing(dir.path()).expect("existing reader");
    let rows = serving
        .map_scan_limit(&root, &key(250), None, 2, None)
        .expect("bounded scan");
    assert_eq!(
        rows.into_iter().map(|(name, _)| name).collect::<Vec<_>>(),
        vec![key(250), key(251)]
    );
}
