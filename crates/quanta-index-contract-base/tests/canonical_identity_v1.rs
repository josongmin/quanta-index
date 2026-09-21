use quanta_index_contract_base::{
    IdentityValidationErrorV1, LogicalGenerationIdentityV1, RepoId, RepositoryRevisionIdentityV1,
    RevisionId,
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
    assert_eq!(RepoId::new(&over_limit), Err(IdentityValidationErrorV1::TooLong));
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
    assert_ne!(first_generation.digest(), first_generation.repository_revision().digest());
    assert_eq!(first_generation.canonical_payload().get(..4), Some(&4_u32.to_be_bytes()[..]));
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
