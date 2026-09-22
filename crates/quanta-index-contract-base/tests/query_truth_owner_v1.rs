//! P05 owner tests (integration surface): the typed outcome window
//! invariants and the authenticated cursor envelope, exercised through the
//! public contract API only.
//!
//! Registered `query-truth-owner-v1` target. Mirrors the lib-level `DoD`
//! matrix: outcome honesty (no capped-to-exact promotion), zero-row
//! provenance, and cursor mint/encode/decode with the full tamper matrix
//! (signature, key, expiry).

#![forbid(unsafe_code)]

use quanta_index_contract_base::{
    CandidateCountV1, CoverageV1, CursorBindingV2, CursorEnvelopeV2, CursorKeyV2, CursorRouteV2,
    CursorTtlPolicyV2, EmptyProvenanceV2, ExaminedUniverseV1, ExecutionOutcomeV2,
    ExhaustionProofV1, GenerationPin, ManifestGeneration, QueryResultWindowV2, RepoId, RevisionId,
};

fn binding(repo: &str) -> CursorBindingV2 {
    CursorBindingV2 {
        route: CursorRouteV2::Lexical,
        pin: GenerationPin::new(
            RepoId::new(repo).expect("static fixture identity"),
            RevisionId::new("rev-a").expect("static fixture identity"),
            ManifestGeneration::new(7),
        ),
        plan_digest: [1; 32],
        query_digest: [2; 32],
        constraints_digest: [3; 32],
        order: "rank".to_string(),
        cap: 10,
        aux_epochs: Vec::new(),
    }
}

#[test]
fn a_capped_outcome_never_becomes_exact() {
    // Capped admission without an exhaustion proof constructs a window
    // whose has_more stays unknown — never a proven exhaustion.
    let capped = QueryResultWindowV2::new(
        10,
        CandidateCountV1::AtLeast(10),
        ExecutionOutcomeV2::CappedUnknown { cap: 10 },
        CoverageV1::new(ExaminedUniverseV1::Unknown, None, Vec::new()),
        None,
    )
    .expect("capped window is constructible");
    assert_eq!(capped.has_more(), None);

    // The exact-exhausted outcome requires an exhaustion proof on the
    // coverage; without one the constructor refuses.
    assert!(
        QueryResultWindowV2::new(
            10,
            CandidateCountV1::AtLeast(10),
            ExecutionOutcomeV2::ExactExhausted,
            CoverageV1::new(ExaminedUniverseV1::Unknown, None, Vec::new()),
            None,
        )
        .is_err(),
        "exact exhaustion without a proof refuses"
    );

    // With a probe proof the same window is honestly exhausted.
    let exhausted = QueryResultWindowV2::new(
        10,
        CandidateCountV1::Exact(10),
        ExecutionOutcomeV2::ExactExhausted,
        CoverageV1::new(
            ExaminedUniverseV1::Exact(10),
            Some(ExhaustionProofV1::ProbeExhausted { fetched: 10 }),
            Vec::new(),
        ),
        None,
    )
    .expect("proof-carrying exhaustion is constructible");
    assert_eq!(exhausted.has_more(), Some(false));
}

#[test]
fn a_zero_row_window_must_state_its_empty_provenance() {
    assert!(
        QueryResultWindowV2::new(
            0,
            CandidateCountV1::Exact(0),
            ExecutionOutcomeV2::ExactExhausted,
            CoverageV1::new(
                ExaminedUniverseV1::Exact(0),
                Some(ExhaustionProofV1::ExactCount { total: 0 }),
                Vec::new(),
            ),
            None,
        )
        .is_err(),
        "a zero-row window without provenance refuses"
    );
    let empty = QueryResultWindowV2::new(
        0,
        CandidateCountV1::Exact(0),
        ExecutionOutcomeV2::ExactExhausted,
        CoverageV1::new(
            ExaminedUniverseV1::Exact(0),
            Some(ExhaustionProofV1::ExactCount { total: 0 }),
            Vec::new(),
        ),
        Some(EmptyProvenanceV2::AvailableEmpty),
    )
    .expect("provenance-carrying empty window is constructible");
    assert_eq!(empty.has_more(), Some(false));
}

#[test]
fn a_cursor_round_trips_and_every_tamper_fails_closed() {
    let key = CursorKeyV2::new(1, [7_u8; 32]);
    let now = 1_000_u64;
    let ttl = CursorTtlPolicyV2::standard();
    let envelope = CursorEnvelopeV2::mint(
        binding("repo-a"),
        "boundary-1",
        &key,
        now,
        now + ttl.default_secs,
    )
    .expect("mint");
    let token = envelope.encode(&key).expect("encode");
    let decoded = CursorEnvelopeV2::decode(token.as_str(), &key, now).expect("decode verifies");
    assert!(decoded.matches(&binding("repo-a"), 1));

    // A different key cannot verify the token.
    let other_key = CursorKeyV2::new(1, [8_u8; 32]);
    assert!(CursorEnvelopeV2::decode(token.as_str(), &other_key, now).is_err());
    // Expiry fails before anything else.
    assert!(CursorEnvelopeV2::decode(token.as_str(), &key, now + 10_000).is_err());
    // A tampered body byte breaks the signature. Flip a character in the
    // middle of the token: a trailing-character flip can decode to the
    // same canonical bytes, a mid-token flip cannot.
    let mut tampered = token.into_bytes();
    let middle = tampered.len().checked_div(2).expect("non-empty token");
    let Some(slot) = tampered.get_mut(middle) else {
        panic!("tamper index in range");
    };
    *slot = if *slot == b'A' { b'B' } else { b'A' };
    let tampered = String::from_utf8(tampered).expect("base64url is ascii");
    assert!(CursorEnvelopeV2::decode(tampered.as_str(), &key, now).is_err());
    // A different binding (another repo) does not match.
    assert!(!decoded.matches(&binding("repo-b"), 1));
}
