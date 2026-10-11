//! Independent oracles for the empty-root Native bootstrap and its later deltas.

use std::collections::BTreeMap;

use quanta_index_contract::{
    ManifestGeneration, RepoId, RepoRelativePath, RevisionId, SourceFileCoverage, SourceFileKey,
    SourceFileRevision, SymbolCoverage, SymbolNameSourcePolicyV1,
};
use quanta_index_core::CoreError;
use quanta_index_core::RequestBudgetV1;
use sha2::{Digest as _, Sha256};

use super::paged_bootstrap::{BootstrapMap, NativeBootstrapMaps};
use super::{
    AddedTextDoc, NativeAuthority, NativeSourceEdit, prepare_compaction, publish_delta,
    publish_delta_prepared, source_ids,
};
use crate::lexical_store::{AttemptBinding, BlobRef, MapRoot, ObjectKind, ObjectStore};

fn binding(generation: u64) -> AttemptBinding {
    AttemptBinding {
        root_incarnation: [1; 16],
        repository_sha256: [2; 32],
        revision_sha256: [5; 32],
        source_event_sha256: [3; 32],
        target_generation: generation,
        base_root_sha256: (generation != 1).then_some([8; 32]),
        operations_sha256: [4; 32],
        format: 1,
    }
}

fn key(number: u32) -> Vec<u8> {
    number.to_be_bytes().to_vec()
}

fn staged_rows(first: u32, end: u32, value: &BlobRef) -> BTreeMap<Vec<u8>, Option<BlobRef>> {
    (first..end)
        .map(|number| (key(number), Some(value.clone())))
        .collect()
}

fn assert_rows(store: &ObjectStore, root: &MapRoot, expected: &BTreeMap<Vec<u8>, BlobRef>) {
    let mut actual = BTreeMap::new();
    store
        .map_scan(root, b"", None, None, &mut |name, value| {
            let previous = actual.insert(name.to_vec(), value.clone());
            assert!(previous.is_none(), "authenticated map repeated a key");
            Ok(())
        })
        .expect("authenticated map scan");
    assert_eq!(&actual, expected);
    assert_eq!(
        root.entries,
        u64::try_from(expected.len()).expect("row count")
    );
}

#[test]
fn bootstrap_sorts_cross_cohort_keys_into_five_independent_roots() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ObjectStore::open(dir.path()).expect("object store");
    let lease = store.begin_attempt(&binding(1), &[]).expect("attempt");
    let value = store
        .put_blob(&lease, ObjectKind::Record, &[], b"fixed-value")
        .expect("value");
    let mut staged = NativeBootstrapMaps::new().expect("scratch");
    for (first, end) in [(4100, 6210), (0, 2000), (2000, 4100)] {
        staged
            .stage(BootstrapMap::GramRoutes, staged_rows(first, end, &value))
            .expect("out-of-order gram cohort");
    }
    for (first, end) in [(96, 193), (0, 96)] {
        staged
            .stage(BootstrapMap::Current, staged_rows(first, end, &value))
            .expect("out-of-order current cohort");
        staged
            .stage(BootstrapMap::TermRoutes, staged_rows(first, end, &value))
            .expect("out-of-order term cohort");
    }
    staged
        .stage(BootstrapMap::Membership, staged_rows(7, 10, &value))
        .expect("membership cohort");
    staged
        .stage(BootstrapMap::Segments, staged_rows(1, 2, &value))
        .expect("segment cohort");
    let roots = staged.finish(&store, &lease).expect("five roots");
    let expected = |first, end| {
        (first..end)
            .map(|number| (key(number), value.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    assert_rows(&store, &roots.current, &expected(0, 193));
    assert_rows(&store, &roots.membership, &expected(7, 10));
    assert_rows(&store, &roots.segments, &expected(1, 2));
    assert_rows(&store, &roots.gram_routes, &expected(0, 6210));
    assert_rows(&store, &roots.term_routes, &expected(0, 193));
    assert_eq!(roots.current.page.as_ref().expect("current page").height, 1);
    assert_eq!(
        roots
            .gram_routes
            .page
            .as_ref()
            .expect("gram route page")
            .height,
        2,
        "6,210 keys cross both the 96-row leaf and 64-child branch boundaries"
    );
    for number in [0, 95, 96, 6143, 6144, 6209] {
        assert_eq!(
            store
                .map_get(&roots.gram_routes, &key(number), None)
                .expect("boundary lookup"),
            Some(value.clone())
        );
    }
}

#[test]
fn bootstrap_rejects_duplicate_and_deletion_without_partial_cohort() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ObjectStore::open(dir.path()).expect("object store");
    let lease = store.begin_attempt(&binding(1), &[]).expect("attempt");
    let value = store
        .put_blob(&lease, ObjectKind::Record, &[], b"fixed-value")
        .expect("value");
    let mut staged = NativeBootstrapMaps::new().expect("scratch");
    staged
        .stage(BootstrapMap::Current, staged_rows(100, 101, &value))
        .expect("first cohort");
    let mut duplicate = staged_rows(50, 51, &value);
    let previous = duplicate.insert(key(100), Some(value.clone()));
    assert!(previous.is_none());
    assert!(matches!(
        staged.stage(BootstrapMap::Current, duplicate),
        Err(CoreError::InvalidContract(message)) if message.contains("duplicate Native bootstrap map key")
    ));
    let mut deletion = staged_rows(10, 11, &value);
    let previous = deletion.insert(key(20), None);
    assert!(previous.is_none());
    assert!(matches!(
        staged.stage(BootstrapMap::Current, deletion),
        Err(CoreError::InvalidContract(message)) if message.contains("bootstrap contains deletion")
    ));
    let roots = staged.finish(&store, &lease).expect("prior cohort only");
    let expected = std::iter::once((key(100), value)).collect();
    assert_rows(&store, &roots.current, &expected);
    for empty in [
        &roots.membership,
        &roots.segments,
        &roots.gram_routes,
        &roots.term_routes,
    ] {
        assert_eq!(empty, &MapRoot::default());
    }
}

#[test]
fn empty_bootstrap_and_scratch_lifetime_are_explicit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ObjectStore::open(dir.path()).expect("object store");
    let lease = store.begin_attempt(&binding(1), &[]).expect("attempt");
    let staged = NativeBootstrapMaps::new().expect("scratch");
    let scratch = staged.scratch_path().to_path_buf();
    assert!(scratch.exists());
    let roots = staged.finish(&store, &lease).expect("empty roots");
    for empty in [
        &roots.current,
        &roots.membership,
        &roots.segments,
        &roots.gram_routes,
        &roots.term_routes,
    ] {
        assert_eq!(empty, &MapRoot::default());
    }
    drop(staged);
    assert!(!scratch.exists(), "scratch survives its owning helper");
}

fn source(name: &str) -> SourceFileKey {
    SourceFileKey {
        source_repo_id: RepoId::new("source-repo").expect("repo"),
        repo_relative_path: RepoRelativePath::new(format!("src/{name}.rs")),
    }
}

fn coverage(file: &SourceFileKey, revision: &str, text: &str) -> SourceFileCoverage {
    SourceFileCoverage {
        source: SourceFileRevision {
            file: file.clone(),
            revision_id: RevisionId::new(revision).expect("revision"),
            source_sha256: Sha256::digest(text.as_bytes()).into(),
        },
        language: quanta_index_contract::lex::LanguageCode::new("rust").expect("language"),
        producer_policy_sha256: [1; 32],
        symbol_name_source_policy: SymbolNameSourcePolicyV1::Unspecified,
        unit_set_sha256: [2; 32],
        text_admitted: true,
        symbols: SymbolCoverage::Complete { symbol_count: 0 },
    }
}

#[test]
fn prepared_native_compaction_replays_three_debts_and_discards_stale_selection() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ObjectStore::open(dir.path()).expect("object store");
    let budget = RequestBudgetV1::unbounded();
    let keys = (1..=14)
        .map(|id| source(&format!("doc-{id}")))
        .collect::<Vec<_>>();
    // Three of these 700 KiB bodies cannot share one 2 MiB segment. Seven
    // two-document segments leave five debts after the initial two are paid.
    let texts = (1..=14)
        .map(|id| format!("needle{id} {}", " ".repeat(700_000)))
        .collect::<Vec<_>>();
    let edits = keys
        .iter()
        .zip(&texts)
        .map(|(key, text)| NativeSourceEdit::Upsert(coverage(key, "r1", text)))
        .collect::<Vec<_>>();
    let added = keys
        .iter()
        .zip(&texts)
        .enumerate()
        .map(|(index, (key, text))| AddedTextDoc {
            doc_id: u64::try_from(index + 1).expect("small ID"),
            source_file: key.clone(),
            candidate_id: format!("candidate-{}", index + 1),
            text: text.clone(),
        })
        .collect::<Vec<_>>();
    let first_lease = store
        .begin_attempt(&binding(1), &[])
        .expect("first attempt");
    let (initial, _) = publish_delta(
        &store,
        &first_lease,
        None,
        &edits,
        &[],
        &added,
        false,
        ManifestGeneration::new(1),
        [9; 32],
    )
    .expect("initial root");
    assert_eq!(initial.segments.entries, 7);
    first_lease
        .finish_published(&initial.root_objects())
        .expect("first roots");

    let retired = [1_u64, 3, 5, 7, 9, 11, 13];
    let deletes = retired
        .iter()
        .map(|id| {
            NativeSourceEdit::Delete(
                keys.get(usize::try_from(*id - 1).expect("small key"))
                    .expect("fixture key")
                    .clone(),
            )
        })
        .collect::<Vec<_>>();
    let second_lease = store
        .begin_attempt(&binding(2), &initial.root_objects())
        .expect("second attempt");
    let (mut root, first_receipt) = publish_delta(
        &store,
        &second_lease,
        Some(&initial),
        &deletes,
        &retired,
        &[],
        false,
        ManifestGeneration::new(2),
        [9; 32],
    )
    .expect("retire seven source documents");
    assert_eq!(first_receipt.compaction_rederived_docs, 2);
    assert_eq!(root.compaction_due.entries, 5);
    second_lease
        .finish_published(&root.root_objects())
        .expect("second roots");
    let debt_base = root.clone();
    assert!(
        prepare_compaction(&store, &root, 0, &[], None)
            .expect("bounded skip")
            .is_none()
    );

    for generation in 3..=5 {
        let prepared = prepare_compaction(&store, &root, 62 * 1024 * 1024, &[], None)
            .expect("pure preparation")
            .expect("queued debt");
        let prior = root.clone();
        if generation == 3 {
            let mut baseline_binding = binding(3);
            baseline_binding.source_event_sha256 = [99; 32];
            let baseline_lease = store
                .begin_attempt(&baseline_binding, &prior.root_objects())
                .expect("baseline attempt");
            let (baseline, _) = publish_delta(
                &store,
                &baseline_lease,
                Some(&prior),
                &[],
                &[],
                &[],
                false,
                ManifestGeneration::new(3),
                [9; 32],
            )
            .expect("inline baseline");
            baseline_lease
                .finish_published(&baseline.root_objects())
                .expect("baseline roots");
            let lease = store
                .begin_attempt(&binding(generation), &prior.root_objects())
                .expect("prepared attempt");
            let (next, receipt) = publish_delta_prepared(
                &store,
                &lease,
                Some(&prior),
                &[],
                &[],
                &[],
                false,
                ManifestGeneration::new(generation),
                [9; 32],
                Some(prepared),
            )
            .expect("prepared publication");
            assert_eq!(next, baseline, "pure preparation changed the physical root");
            assert!(receipt.compaction_rederived_docs > 0);
            lease
                .finish_published(&next.root_objects())
                .expect("prepared roots");
            root = next;
        } else {
            let lease = store
                .begin_attempt(&binding(generation), &prior.root_objects())
                .expect("prepared attempt");
            let (next, receipt) = publish_delta_prepared(
                &store,
                &lease,
                Some(&prior),
                &[],
                &[],
                &[],
                false,
                ManifestGeneration::new(generation),
                [9; 32],
                Some(prepared),
            )
            .expect("prepared publication");
            assert!(receipt.compaction_rederived_docs > 0);
            lease
                .finish_published(&next.root_objects())
                .expect("prepared roots");
            root = next;
        }
        let query = NativeAuthority::open(store.clone(), root.clone())
            .expect("current authority")
            .scoped_query();
        assert_eq!(
            query
                .intersect_grams(&[*b"nee"], false, &budget)
                .expect("survivor query"),
            vec![2, 4, 6, 8, 10, 12, 14]
        );
    }
    assert_eq!(root.compaction_due.entries, 0);
    let old = NativeAuthority::open(store.clone(), initial)
        .expect("old authority")
        .scoped_query();
    assert_eq!(
        old.intersect_grams(&[*b"nee"], false, &budget)
            .expect("old pin")
            .len(),
        14
    );

    let stale = prepare_compaction(&store, &debt_base, 62 * 1024 * 1024, &[], None)
        .expect("stale preparation")
        .expect("old debt");
    let drift_lease = store
        .begin_attempt(&binding(9), &debt_base.root_objects())
        .expect("drift attempt");
    let (drifted, _) = publish_delta_prepared(
        &store,
        &drift_lease,
        Some(&debt_base),
        &[NativeSourceEdit::Delete(
            keys.get(5).expect("fixture key").clone(),
        )],
        &[6],
        &[],
        false,
        ManifestGeneration::new(9),
        [9; 32],
        Some(stale),
    )
    .expect("stale packet discarded");
    let drift_query = NativeAuthority::open(store.clone(), drifted.clone())
        .expect("drift authority")
        .scoped_query();
    assert_eq!(
        drift_query
            .intersect_grams(&[*b"nee"], false, &budget)
            .expect("drift query"),
        vec![2, 4, 8, 10, 12, 14]
    );
    drift_lease
        .finish_published(&drifted.root_objects())
        .expect("drift roots");

    // A retired body's absence must not turn an otherwise valid deletion
    // into a preparation failure. The final-live writer owns that decision.
    let row = super::paged_writer::read_current(&store, &debt_base, 6)
        .expect("prior current")
        .expect("live ID 6");
    let mut name = String::new();
    for byte in row.body.root.sha256 {
        use std::fmt::Write as _;
        write!(&mut name, "{byte:02x}").expect("hex digest");
    }
    let path = dir
        .path()
        .join("lexical-object-pool")
        .join("objects")
        .join(name.get(..2).expect("digest prefix"))
        .join(&name);
    std::fs::remove_file(path).expect("remove selected old body");
    assert!(
        prepare_compaction(&store, &debt_base, 62 * 1024 * 1024, &[6], None)
            .expect("retiring body skipped")
            .is_none()
    );
    assert!(prepare_compaction(&store, &debt_base, 62 * 1024 * 1024, &[], None).is_err());
    let cancelled = RequestBudgetV1::unbounded();
    cancelled.cancel_handle().cancel();
    assert!(
        matches!(prepare_compaction(&store, &debt_base, 0, &[], Some(&cancelled)),
        Err(CoreError::Typed { code, .. }) if code == quanta_index_core::REQUEST_CANCELLED_CODE)
    );
}

#[test]
fn multi_cohort_initial_root_keeps_update_delete_and_clear_all_query_exact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ObjectStore::open(dir.path()).expect("object store");
    let first = source("first");
    let second = source("second");
    let stable = source("stable");
    let replacement = source("replacement");
    let mut large = String::from("alpha ");
    large.push_str(&" ".repeat(2 * 1024 * 1024));
    let budget = RequestBudgetV1::unbounded();
    let first_lease = store
        .begin_attempt(&binding(1), &[])
        .expect("initial attempt");
    let (initial, _) = publish_delta(
        &store,
        &first_lease,
        None,
        &[
            NativeSourceEdit::Upsert(coverage(&first, "r1", &large)),
            NativeSourceEdit::Upsert(coverage(&second, "r1", "beta small")),
            NativeSourceEdit::Upsert(coverage(&stable, "r1", "stable anchor")),
        ],
        &[],
        &[
            AddedTextDoc {
                doc_id: 1,
                source_file: first.clone(),
                candidate_id: "alpha".into(),
                text: large,
            },
            AddedTextDoc {
                doc_id: 2,
                source_file: second.clone(),
                candidate_id: "beta".into(),
                text: "beta small".into(),
            },
            AddedTextDoc {
                doc_id: 3,
                source_file: stable.clone(),
                candidate_id: "stable".into(),
                text: "stable anchor".into(),
            },
        ],
        false,
        ManifestGeneration::new(1),
        [9; 32],
    )
    .expect("multi-cohort initial publication");
    assert_eq!(initial.current.entries, 3);
    assert_eq!(initial.membership.entries, 3);
    assert!(
        initial.next_segment_id >= 3,
        "large first body must split cohorts"
    );
    assert_eq!(
        source_ids(&store, &initial, &first).expect("first membership"),
        vec![1]
    );
    assert_eq!(
        source_ids(&store, &initial, &second).expect("second membership"),
        vec![2]
    );
    assert_eq!(
        source_ids(&store, &initial, &stable).expect("stable membership"),
        vec![3]
    );
    let query = NativeAuthority::open(store.clone(), initial.clone())
        .expect("initial authority")
        .scoped_query();
    assert_eq!(
        query
            .intersect_grams(&[*b"alp"], false, &budget)
            .expect("alpha"),
        vec![1]
    );
    assert_eq!(
        query
            .intersect_grams(&[*b"bet"], false, &budget)
            .expect("beta"),
        vec![2]
    );
    assert_eq!(
        query
            .intersect_grams(&[*b"sta"], false, &budget)
            .expect("stable"),
        vec![3]
    );
    first_lease
        .finish_published(&initial.root_objects())
        .expect("first roots committed");

    let second_lease = store
        .begin_attempt(&binding(2), &initial.root_objects())
        .expect("delta attempt");
    let (changed, _) = publish_delta(
        &store,
        &second_lease,
        Some(&initial),
        &[
            NativeSourceEdit::Delete(first.clone()),
            NativeSourceEdit::Upsert(coverage(&second, "r2", "delta fresh")),
        ],
        &[1, 2],
        &[AddedTextDoc {
            doc_id: 4,
            source_file: second.clone(),
            candidate_id: "delta".into(),
            text: "delta fresh".into(),
        }],
        false,
        ManifestGeneration::new(2),
        [9; 32],
    )
    .expect("update and delete");
    assert_eq!(changed.current.entries, 2);
    assert_eq!(changed.membership.entries, 2);
    assert!(
        source_ids(&store, &changed, &first)
            .expect("deleted membership")
            .is_empty()
    );
    assert_eq!(
        source_ids(&store, &changed, &second).expect("updated membership"),
        vec![4]
    );
    assert_eq!(
        source_ids(&store, &changed, &stable).expect("unchanged membership"),
        vec![3]
    );
    let query = NativeAuthority::open(store.clone(), changed.clone())
        .expect("changed authority")
        .scoped_query();
    assert!(
        query
            .intersect_grams(&[*b"alp"], false, &budget)
            .expect("old alpha")
            .is_empty()
    );
    assert!(
        query
            .intersect_grams(&[*b"bet"], false, &budget)
            .expect("old beta")
            .is_empty()
    );
    assert_eq!(
        query
            .intersect_grams(&[*b"del"], false, &budget)
            .expect("delta"),
        vec![4]
    );
    assert_eq!(
        query
            .intersect_grams(&[*b"sta"], false, &budget)
            .expect("stable after delta"),
        vec![3]
    );
    assert_eq!(
        query
            .read_document(3, &budget)
            .expect("stable document")
            .expect("unchanged source remains")
            .indexed_text,
        "stable anchor"
    );
    assert!(
        query
            .read_document(1, &budget)
            .expect("deleted document")
            .is_none()
    );
    second_lease
        .finish_published(&changed.root_objects())
        .expect("second roots committed");

    let third_lease = store
        .begin_attempt(&binding(3), &changed.root_objects())
        .expect("clear attempt");
    let (cleared, _) = publish_delta(
        &store,
        &third_lease,
        Some(&changed),
        &[NativeSourceEdit::Upsert(coverage(
            &replacement,
            "r1",
            "omega final",
        ))],
        &[],
        &[AddedTextDoc {
            doc_id: 5,
            source_file: replacement.clone(),
            candidate_id: "omega".into(),
            text: "omega final".into(),
        }],
        true,
        ManifestGeneration::new(3),
        [9; 32],
    )
    .expect("clear and republish");
    assert_eq!(cleared.current.entries, 1);
    assert_eq!(cleared.membership.entries, 1);
    assert!(
        source_ids(&store, &cleared, &second)
            .expect("cleared membership")
            .is_empty()
    );
    assert!(
        source_ids(&store, &cleared, &stable)
            .expect("cleared stable membership")
            .is_empty()
    );
    assert_eq!(
        source_ids(&store, &cleared, &replacement).expect("new membership"),
        vec![5]
    );
    let query = NativeAuthority::open(store, cleared)
        .expect("cleared authority")
        .scoped_query();
    assert!(
        query
            .intersect_grams(&[*b"del"], false, &budget)
            .expect("cleared delta")
            .is_empty()
    );
    assert!(
        query
            .intersect_grams(&[*b"sta"], false, &budget)
            .expect("cleared stable")
            .is_empty()
    );
    assert_eq!(
        query
            .intersect_grams(&[*b"ome"], false, &budget)
            .expect("omega"),
        vec![5]
    );
    assert!(
        query
            .read_document(3, &budget)
            .expect("cleared stable document")
            .is_none()
    );
    assert!(
        query
            .read_document(4, &budget)
            .expect("cleared delta document")
            .is_none()
    );
}
