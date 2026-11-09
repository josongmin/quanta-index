use super::page_wire;
use super::*;

fn row(id: u64, digest: u8) -> RunPostingRow {
    RunPostingRow {
        logical_id: id,
        segment_sha256: [digest; 32],
    }
}

fn fixture_page() -> RunPostingPage {
    RunPostingPage::new(2, *b"abc", 1, vec![row(256, 7), row(257, 7)]).expect("fixture page")
}

fn manual_v1() -> Vec<u8> {
    let mut wire = vec![
        0x85, 1, 2, 0x83, 0x18, b'a', 0x18, b'b', 0x18, b'c', 1, 0x82,
    ];
    for id in [0_u8, 1_u8] {
        wire.extend_from_slice(&[0x82, 0x19, 1, id, 0x98, 32]);
        wire.extend_from_slice(&[7; 32]);
    }
    wire
}

fn manual_v2() -> Vec<u8> {
    let mut wire = vec![
        0x86, 2, 2, 0x83, 0x18, b'a', 0x18, b'b', 0x18, b'c', 1, 0x81, 0x98, 32,
    ];
    wire.extend_from_slice(&[7; 32]);
    wire.extend_from_slice(&[0x82, 0x82, 0x19, 1, 0, 0, 0x82, 0x19, 1, 1, 0]);
    wire
}

#[test]
fn manual_v1_and_v2_fixtures_decode_to_one_page_without_reencoding_v1() {
    let page = fixture_page();
    assert_eq!(
        page_wire::encode_legacy_v1_for_test(&page).expect("v1"),
        manual_v1()
    );
    assert_eq!(page.encode().expect("v2"), manual_v2());
    assert_eq!(
        RunPostingPage::decode(&manual_v1()).expect("legacy page"),
        page
    );
    assert_eq!(
        RunPostingPage::decode(&manual_v2()).expect("new page"),
        page
    );
    assert_ne!(manual_v1(), manual_v2());
}

#[test]
fn v2_rejects_noncanonical_or_unsafe_dictionary_and_rows() {
    fn candidate(
        version: u8,
        surface: u8,
        bucket: u64,
        dictionary: Vec<[u8; 32]>,
        rows: Vec<(u64, u8)>,
    ) -> Vec<u8> {
        encode_cbor(
            &(version, surface, *b"abc", bucket, dictionary, rows),
            "malformed run page",
        )
        .expect("candidate encoding")
    }
    let valid = candidate(2, 2, 1, vec![[7; 32]], vec![(256, 0), (257, 0)]);
    assert_eq!(valid, manual_v2());
    for bytes in [
        candidate(3, 2, 1, vec![[7; 32]], vec![(256, 0)]),
        candidate(2, 3, 1, vec![[7; 32]], vec![(256, 0)]),
        candidate(2, 2, 0, vec![[7; 32]], vec![(256, 0)]),
        candidate(2, 2, 1, vec![[7; 32]], vec![(257, 0), (256, 0)]),
        candidate(2, 2, 1, vec![[7; 32]], vec![(256, 0), (256, 0)]),
        candidate(2, 2, 1, vec![[7; 32], [7; 32]], vec![(256, 0), (257, 1)]),
        candidate(2, 2, 1, vec![[8; 32], [7; 32]], vec![(256, 0), (257, 1)]),
        candidate(2, 2, 1, vec![[7; 32], [8; 32]], vec![(256, 0)]),
        candidate(2, 2, 1, vec![[7; 32]], vec![(256, 1)]),
        candidate(
            2,
            2,
            1,
            vec![[7; 32]],
            (256..513).map(|id| (id, 0)).collect(),
        ),
        candidate(2, 2, 1, vec![[7; 32]; 257], vec![(256, 0)]),
    ] {
        assert!(RunPostingPage::decode(&bytes).is_err());
    }
    let mut trailing = valid.clone();
    trailing.push(0);
    assert!(RunPostingPage::decode(&trailing).is_err());
    let mut noncanonical = valid;
    drop(noncanonical.splice(1..2, [0x18, 2]));
    assert!(RunPostingPage::decode(&noncanonical).is_err());
}

#[test]
fn legacy_prior_delta_and_mixed_merge_keep_original_blob_bytes_after_cold_open() {
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
    let old = RunPostingPage::new(2, *b"abc", 0, vec![row(1, 7)]).expect("legacy logical page");
    let key = old.key().expect("key").to_vec();
    let (mut base, _) = apply_run_page_edits(
        &store,
        &lease,
        None,
        1,
        BTreeMap::from([(
            key.clone(),
            RunPostingPageEdit {
                prior: None,
                next: Some(old.clone()),
            },
        )]),
    )
    .expect("base run");
    let v1 = page_wire::encode_legacy_v1_for_test(&old).expect("legacy wire");
    let v1_ref = store
        .put_blob(&lease, ObjectKind::Record, &[], &v1)
        .expect("legacy stored page");
    base.pages = store
        .map_apply(
            &lease,
            &base.pages,
            std::iter::once(Ok(MapEdit {
                key: key.clone(),
                value: Some(v1_ref.clone()),
            })),
        )
        .expect("replace page reference");
    base.page_record_encoded_bytes = v1_ref.root.bytes;
    base.validate().expect("legacy base root");
    let next = RunPostingPage::new(2, *b"abc", 0, vec![row(1, 7), row(2, 8)]).expect("next page");
    let (delta, _) = apply_run_page_edits(
        &store,
        &lease,
        Some(&base),
        1,
        BTreeMap::from([(
            key.clone(),
            RunPostingPageEdit {
                prior: Some((v1_ref.clone(), old.clone())),
                next: Some(next.clone()),
            },
        )]),
    )
    .expect("mixed v1/v2 delta");
    let delta_ref = store
        .map_get(&delta.pages, &key, None)
        .expect("delta map")
        .expect("delta ref");
    assert_eq!(delta.page_record_encoded_bytes, delta_ref.root.bytes);
    assert_eq!(
        store
            .read_blob(&v1_ref, 32 * 1024, None)
            .expect("old bytes"),
        v1
    );
    assert_eq!(
        store
            .read_blob(&delta_ref, 32 * 1024, None)
            .expect("new bytes"),
        next.encode().expect("v2")
    );

    let other = RunPostingPage::new(2, *b"abc", 0, vec![row(3, 9)]).expect("other page");
    let (second, _) = apply_run_page_edits(
        &store,
        &lease,
        None,
        2,
        BTreeMap::from([(
            key.clone(),
            RunPostingPageEdit {
                prior: None,
                next: Some(other),
            },
        )]),
    )
    .expect("second run");
    let charged = run_merge_map_stage_bytes(&[&base.pages, &second.pages]).expect("stage charge");
    let old_three_frame_budget = charged
        .checked_sub(u64::try_from(2 * MAX_POSTING_PAGE_BYTES).expect("page bound"))
        .expect("five-frame charge");
    assert!(
        merge_run_posting_roots(
            &store,
            &lease,
            3,
            &[base.clone(), second.clone()],
            old_three_frame_budget,
        )
        .is_err()
    );
    let (merged, _) = merge_run_posting_roots(&store, &lease, 3, &[base, second], charged)
        .expect("mixed v1/v2 merge at exact staging charge");
    drop(lease);
    drop(store);
    let cold = ObjectStore::open(dir.path()).expect("cold store");
    assert!(merged.is_block_layout());
    let merged_key = old.key().expect("merged page key");
    let (merged_ref, merged_page) = merged
        .page_at(&cold, &merged_key, None)
        .expect("merged lookup")
        .expect("merged page");
    let merged_bytes = cold
        .read_blob(&merged_ref, 32 * 1024, None)
        .expect("merged block bytes");
    assert_eq!(merged_page.rows, vec![row(1, 7), row(3, 9)]);
    let mut canonical = immutable_block::ImmutableRunBlockBuilder::new();
    assert!(matches!(
        canonical
            .try_push(
                merged_key,
                &merged_page.encode().expect("canonical v2 page")
            )
            .expect("canonical block entry"),
        immutable_block::PushResult::Added
    ));
    assert_eq!(
        merged_bytes,
        canonical.finish().expect("canonical new block")
    );
    assert_eq!(
        cold.read_blob(&v1_ref, 32 * 1024, None).expect("cold v1"),
        v1
    );
}
