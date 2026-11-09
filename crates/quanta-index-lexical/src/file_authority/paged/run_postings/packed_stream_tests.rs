use super::packed_stream::{PackedEdits, RecordEdit};
use super::*;

fn interleaved_domain_pack(wide: bool) -> usize {
    let dir = tempfile::tempdir().expect("directory");
    let store = ObjectStore::open(dir.path()).expect("store");
    let binding = crate::lexical_store::AttemptBinding {
        root_incarnation: [1; 16],
        repository_sha256: [2; 32],
        revision_sha256: [3; 32],
        source_event_sha256: [4; 32],
        target_generation: 1,
        base_root_sha256: None,
        operations_sha256: [5; 32],
        format: 1,
    };
    let lease = store.begin_attempt(&binding, &[]).expect("attempt");
    let rows = (0..384_u32)
        .map(|number| {
            let bytes = number.to_be_bytes();
            let gram = [bytes[1], bytes[2], bytes[3]];
            let key = run_page_key(1, gram, u64::from(number % 4) << ID_BUCKET_BITS)
                .expect("page key")
                .to_vec();
            RecordEdit {
                key,
                bytes: format!("body-{number:04}").into_bytes(),
            }
        })
        .collect::<Vec<_>>();
    let source = rows.iter().map(|row| {
        Ok(RecordEdit {
            key: row.key.clone(),
            bytes: row.bytes.clone(),
        })
    });
    let packed = PackedEdits::new(&store, &lease, source);
    let packed = if wide {
        packed.fresh_leaf_lookahead()
    } else {
        packed
    };
    let root = store.map_bootstrap(&lease, packed).expect("map");
    let cold = ObjectStore::open(dir.path()).expect("cold store");
    let mut root_domain = BTreeMap::new();
    for row in &rows {
        let reference = cold
            .map_get(&root, &row.key, None)
            .expect("map lookup")
            .expect("record route");
        assert_eq!(
            cold.read_blob(&reference, 128, None).expect("cold record"),
            row.bytes
        );
        let domain = [
            row.key[0],
            row.key[4],
            row.key[5],
            row.key[6],
            row.key[7],
            row.key[8],
            row.key[9],
            row.key[10],
            row.key[11],
        ];
        if let Some(prior) = root_domain.insert(reference.root.sha256, domain) {
            assert_eq!(prior, domain, "a Record root crossed a restricted bucket");
        }
    }
    root_domain.len()
}

#[test]
fn fresh_leaf_lookahead_packs_interleaved_buckets_without_crossing_boundaries() {
    assert_eq!(interleaved_domain_pack(false), 16);
    assert_eq!(interleaved_domain_pack(true), 4);
}

#[test]
fn deleting_pack_members_keeps_the_surviving_whole_inode_in_the_run_ledger() {
    let dir = tempfile::tempdir().expect("directory");
    let store = ObjectStore::open(dir.path()).expect("store");
    let binding = crate::lexical_store::AttemptBinding {
        root_incarnation: [1; 16],
        repository_sha256: [2; 32],
        revision_sha256: [3; 32],
        source_event_sha256: [4; 32],
        target_generation: 1,
        base_root_sha256: None,
        operations_sha256: [5; 32],
        format: 1,
    };
    let lease = store.begin_attempt(&binding, &[]).expect("attempt");
    let left = RunPostingPage::new(
        2,
        *b"abc",
        0,
        vec![RunPostingRow {
            logical_id: 1,
            segment_sha256: [9; 32],
        }],
    )
    .expect("left");
    let right = RunPostingPage::new(
        2,
        *b"xyz",
        0,
        vec![RunPostingRow {
            logical_id: 1,
            segment_sha256: [9; 32],
        }],
    )
    .expect("right");
    let edits = [&left, &right]
        .into_iter()
        .map(|page| {
            (
                page.key().expect("key").to_vec(),
                RunPostingPageEdit {
                    prior: None,
                    next: Some(page.clone()),
                },
            )
        })
        .collect();
    let (mut prior, _) = apply_run_page_edits(&store, &lease, None, 1, edits).expect("initial run");
    let references = store
        .put_record_group(
            &lease,
            &[
                left.encode().expect("left bytes"),
                right.encode().expect("right bytes"),
            ],
        )
        .expect("shared root");
    assert_eq!(references[0].root, references[1].root);
    prior.pages = store
        .map_apply(
            &lease,
            &prior.pages,
            [&left, &right]
                .into_iter()
                .zip(&references)
                .map(|(page, reference)| {
                    Ok(MapEdit {
                        key: page.key().expect("key").to_vec(),
                        value: Some(reference.clone()),
                    })
                }),
        )
        .expect("packed map");
    prior.page_record_encoded_bytes = references[0]
        .root
        .bytes
        .checked_mul(2)
        .expect("initial conservative charge");
    let (target, _) = apply_run_page_edits(
        &store,
        &lease,
        Some(&prior),
        1,
        BTreeMap::from([(
            left.key().expect("left key").to_vec(),
            RunPostingPageEdit {
                prior: Some((references[0].clone(), left)),
                next: None,
            },
        )]),
    )
    .expect("delete one member");
    assert_eq!(target.pages.entries, 1);
    let remaining = store
        .map_get(&target.pages, &right.key().expect("right key"), None)
        .expect("lookup")
        .expect("remaining");
    let digest = remaining
        .root
        .sha256
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let path = dir
        .path()
        .join("lexical-object-pool/objects")
        .join(digest.get(..2).expect("shard"))
        .join(digest);
    let actual_inode_bytes = std::fs::metadata(path).expect("physical extent").len();
    assert_eq!(target.page_record_encoded_bytes, actual_inode_bytes);
    assert!(
        actual_inode_bytes
            > remaining
                .bytes
                .checked_add(22)
                .expect("standalone member width")
    );
    let cold = ObjectStore::open(dir.path()).expect("cold store");
    assert_eq!(
        cold.read_blob(&remaining, 32768, None)
            .expect("remaining bytes"),
        right.encode().expect("right bytes")
    );
}

#[test]
fn thousands_of_logical_records_use_bounded_roots_and_survive_cold_map_reads() {
    let dir = tempfile::tempdir().expect("directory");
    let store = ObjectStore::open(dir.path())
        .expect("store")
        .metered_clone();
    let binding = crate::lexical_store::AttemptBinding {
        root_incarnation: [1; 16],
        repository_sha256: [2; 32],
        revision_sha256: [3; 32],
        source_event_sha256: [4; 32],
        target_generation: 1,
        base_root_sha256: None,
        operations_sha256: [5; 32],
        format: 1,
    };
    let lease = store.begin_attempt(&binding, &[]).expect("attempt");
    let rows = (0_u32..4096).map(|id| {
        Ok(RecordEdit {
            key: id.to_be_bytes().to_vec(),
            bytes: format!("record-{id:04}").into_bytes(),
        })
    });
    let map = store
        .with_adoption_batch(&lease, |scoped| {
            scoped.map_bootstrap(&lease, PackedEdits::new(scoped, &lease, rows))
        })
        .expect("map");
    let cold = ObjectStore::open(dir.path()).expect("cold").metered_clone();
    let mut cursor = cold.map_cursor(&map, &[], None, None).expect("cursor");
    let mut roots = BTreeSet::new();
    for id in 0_u32..4096 {
        let (key, reference) = cursor.next().expect("row").expect("present");
        assert_eq!(key, id.to_be_bytes());
        assert_eq!(
            cold.read_blob(&reference, 11, None).expect("logical bytes"),
            format!("record-{id:04}").as_bytes()
        );
        let _inserted = roots.insert(reference.root.sha256);
    }
    assert!(cursor.next().expect("end").is_none());
    assert!(
        roots.len() <= 43,
        "4096 logical values must fit within 43 physical Record roots"
    );
    assert!(
        lease
            .write_cost_snapshot()
            .expect("physical cost")
            .new_objects
            < 200
    );
}
