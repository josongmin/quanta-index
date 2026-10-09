use quanta_index_contract_base::{
    IdentityValidationErrorV1, LogicalGenerationIdentityV1, NativeIdentityCopyErrorV1 as CopyError,
    RepoId, RepositoryRevisionIdentityV1, RevisionId, try_copy_string_into_with_native_birth_v1,
};

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for byte in bytes {
        let _unused = write!(out, "{byte:02x}");
    }
    out
}

#[test]
fn validated_id_constructor_and_serde_refuse_the_same_invalid_forms() {
    for (raw, expected) in [
        ("", IdentityValidationErrorV1::Empty),
        ("bad\u{0000}id", IdentityValidationErrorV1::ControlCharacter),
        ("e\u{301}", IdentityValidationErrorV1::NonCanonical),
    ] {
        assert_eq!(RepoId::new(raw), Err(expected));
        assert_eq!(RevisionId::new(raw), Err(expected));
        let wire = serde_json::to_string(raw).expect("serialize fixture");
        assert!(serde_json::from_str::<RepoId>(&wire).is_err());
        assert!(serde_json::from_str::<RevisionId>(&wire).is_err());
    }
    let over_limit = "é".repeat(257);
    assert_eq!(
        RepoId::new(&over_limit),
        Err(IdentityValidationErrorV1::TooLong)
    );
    assert!(
        serde_json::from_str::<RepoId>(
            &serde_json::to_string(&over_limit).expect("serialize over-limit")
        )
        .is_err()
    );
    for raw in ["%", "/", ".", "..", "MiXeD", "é", &"é".repeat(256)] {
        let id = RepoId::new(raw).expect("valid fixture ID");
        assert_eq!(
            serde_json::from_str::<RepoId>(&serde_json::to_string(&id).expect("serialize id"))
                .expect("roundtrip"),
            id
        );
    }
}

#[test]
fn tuple_framing_separates_legacy_collision_and_digest_domains() {
    let first = RepositoryRevisionIdentityV1::new(
        RepoId::new("a--b").expect("valid"),
        RevisionId::new("c").expect("valid"),
    );
    let second = RepositoryRevisionIdentityV1::new(
        RepoId::new("a").expect("valid"),
        RevisionId::new("b--c").expect("valid"),
    );
    assert_ne!(first.canonical_payload(), second.canonical_payload());
    assert_ne!(first.digest(), second.digest());
    let first_generation = LogicalGenerationIdentityV1::new(first, 0);
    let second_generation = LogicalGenerationIdentityV1::new(second, 0);
    assert_ne!(first_generation.digest(), second_generation.digest());
    assert_ne!(
        first_generation.digest(),
        first_generation.repository_revision().digest()
    );
    assert_eq!(
        first_generation.canonical_payload().get(..4),
        Some(&4_u32.to_be_bytes()[..])
    );
    assert_eq!(
        hex(&first_generation.repository_revision().canonical_payload()),
        "00000004612d2d620000000163"
    );
    assert_eq!(
        hex(&first_generation.repository_revision().digest()),
        "8d8d1cb9bc9e04712e5617b2d5b6bafb1453e4b0ab169e5651fc614c415f4153"
    );
    assert_eq!(
        hex(&first_generation.canonical_payload()),
        "00000004612d2d6200000001630000000000000000"
    );
    assert_eq!(
        hex(&first_generation.digest()),
        "578c84366a18814ec6581d50b59159cf4342a4b207f5cdb6965713909899fefa"
    );
}

#[test]
fn native_string_into_slot_copies_exact_bytes_including_empty_input() {
    for source in ["", "répo/../%", "e\u{301}"] {
        let mut backing = String::new();
        let mut calls = 0;
        try_copy_string_into_with_native_birth_v1(source, &mut backing, |bytes, birth| {
            assert_eq!(bytes, source.len());
            calls += 1;
            Ok::<_, u8>(birth())
        })
        .expect("admitted copy");
        assert_eq!(calls, 1);
        assert_eq!(backing, source);
        assert_eq!(backing.capacity(), source.len());
    }
}

#[test]
fn sealed_id_into_slot_publishes_only_a_complete_copy() {
    let repo = RepoId::new("répo/../%").expect("canonical repo");
    let mut backing = String::new();
    let mut output = None;
    repo.try_clone_into_with_native_birth_v1(&mut backing, &mut output, |_, birth| {
        Ok::<_, u8>(birth())
    })
    .expect("repo copy");
    let copy = output.expect("complete typed repo");
    assert_eq!(copy, repo);
    assert_ne!(copy.as_str().as_ptr(), repo.as_str().as_ptr());
    assert_eq!(backing.capacity(), 0);

    let revision = RevisionId::new("révision/%").expect("canonical revision");
    let mut output = None;
    revision
        .try_clone_into_with_native_birth_v1(&mut backing, &mut output, |_, birth| {
            Ok::<_, u8>(birth())
        })
        .expect("revision copy");
    assert_eq!(output, Some(revision));
    assert_eq!(backing.capacity(), 0);
}

#[test]
fn late_admission_refusal_retains_backing_and_the_exact_noncopy_cause() {
    let repo = RepoId::new("repo/test").expect("canonical repo");
    let cause = Box::new(23_u8);
    let cause_address = std::ptr::from_ref(cause.as_ref());
    let mut backing = String::new();
    let mut output = None;
    let result =
        repo.try_clone_into_with_native_birth_v1(&mut backing, &mut output, |bytes, birth| {
            assert_eq!(bytes, 9);
            assert!(birth());
            Err::<bool, _>(cause)
        });
    match result {
        Err(CopyError::Admission(cause)) => {
            assert_eq!(std::ptr::from_ref(cause.as_ref()), cause_address);
            assert_eq!(*cause, 23);
        }
        other => panic!("expected original admission cause, got {other:?}"),
    }
    assert!(output.is_none());
    assert!(backing.is_empty());
    assert_eq!(backing.capacity(), 9);

    let revision = RevisionId::new("rev/test").expect("canonical revision");
    let mut backing = String::new();
    let mut output = None;
    assert_eq!(
        revision.try_clone_into_with_native_birth_v1(&mut backing, &mut output, |_, birth| {
            assert!(birth());
            Err::<bool, _>(31_u8)
        }),
        Err(CopyError::Admission(31))
    );
    assert!(output.is_none());
    assert_eq!(backing.capacity(), 8);
}

#[test]
fn admission_before_birth_leaves_both_external_slots_empty() {
    let repo = RepoId::new("repo/test").expect("canonical repo");
    let mut backing = String::new();
    let mut output = None;
    assert_eq!(
        repo.try_clone_into_with_native_birth_v1(&mut backing, &mut output, |_, _| {
            Err::<bool, _>(7_u8)
        }),
        Err(CopyError::Admission(7))
    );
    assert!(output.is_none());
    assert_eq!(backing.capacity(), 0);
}

#[test]
fn invalid_native_callback_keeps_any_born_backing_in_the_caller() {
    for report in [false, true] {
        let mut backing = String::new();
        assert_eq!(
            try_copy_string_into_with_native_birth_v1("repo/test", &mut backing, |_, _| {
                Ok::<_, u8>(report)
            }),
            Err(CopyError::InvalidNativeProducer)
        );
        assert_eq!(backing.capacity(), 0);
    }
    for repeated in [false, true] {
        let mut backing = String::new();
        assert_eq!(
            try_copy_string_into_with_native_birth_v1("repo/test", &mut backing, |_, birth| {
                assert!(birth());
                if repeated {
                    assert!(birth(), "repeat keeps first physical receipt");
                }
                Ok::<_, u8>(repeated)
            }),
            Err(CopyError::InvalidNativeProducer)
        );
        assert!(backing.is_empty());
        assert_eq!(backing.capacity(), 9);
    }
}

#[test]
fn populated_external_slots_are_preserved_without_invoking_admission() {
    let repo = RepoId::new("repo/test").expect("canonical repo");
    let prior = RepoId::new("repo/prior").expect("canonical prior repo");
    let mut output = Some(prior);
    let mut backing = String::new();
    assert_eq!(
        repo.try_clone_into_with_native_birth_v1(&mut backing, &mut output, |_, _| {
            panic!("occupied typed slot must refuse before admission")
        }),
        Err(CopyError::<u8>::InvalidNativeProducer)
    );
    assert_eq!(output.as_ref().map(RepoId::as_str), Some("repo/prior"));
    assert_eq!(backing.capacity(), 0);

    for mut backing in [String::from("prior"), String::with_capacity(9)] {
        let prior_address = backing.as_ptr();
        let prior_capacity = backing.capacity();
        let prior_value = backing.clone();
        let mut output = None;
        assert_eq!(
            repo.try_clone_into_with_native_birth_v1(&mut backing, &mut output, |_, _| {
                panic!("occupied backing must refuse before admission")
            }),
            Err(CopyError::<u8>::InvalidNativeProducer)
        );
        assert!(output.is_none());
        assert_eq!(backing.as_ptr(), prior_address);
        assert_eq!(backing.capacity(), prior_capacity);
        assert_eq!(backing, prior_value);
    }
}
