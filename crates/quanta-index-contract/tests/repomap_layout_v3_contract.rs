use quanta_index_contract::{
    ArtifactContentDigestV1, ArtifactIdentityV1, CandidateObjectDigestV1,
    CanonicalRepoMapCodecErrorV1, QuarantineIncidentV1, QuarantineObservationEvidenceV1,
    QuarantineReasonCodeV1, RepoMapCandidateCommitmentsV1, RepoMapCandidateEnvelopeV1,
    StateRootUuidCommitmentV1,
};
use quanta_index_contract_base::{
    LogicalGenerationIdentityV1, RepoId, RepositoryRevisionIdentityV1, RevisionId,
};

use std::fmt::Write as _;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(
        String::with_capacity(bytes.len().saturating_mul(2)),
        |mut out, byte| {
            let _written = write!(out, "{byte:02x}");
            out
        },
    )
}

fn candidate() -> Result<RepoMapCandidateEnvelopeV1, Box<dyn std::error::Error>> {
    let identity = LogicalGenerationIdentityV1::new(
        RepositoryRevisionIdentityV1::new(RepoId::new("repo/%")?, RevisionId::new("É/..")?),
        25,
    );
    let payload = b"compiled payload".to_vec();
    let artifact = ArtifactIdentityV1::new(
        identity.clone(),
        ArtifactContentDigestV1::for_payload(&payload),
        u64::try_from(payload.len())?,
    );
    Ok(RepoMapCandidateEnvelopeV1::new(
        identity,
        RepoMapCandidateCommitmentsV1 {
            producer_manifest: [1; 32],
            producer_authority: [2; 32],
            compiled_graph: [3; 32],
            schema: [4; 32],
            projection_profile: [5; 32],
        },
        artifact,
        payload,
    )?)
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "assert-based #[test] body; TestResult exists only so the setup steps can use `?`"
)]
fn candidate_canonical_bytes_bind_artifact_and_distinct_digest_domains() -> TestResult {
    let candidate = candidate()?;
    let bytes = candidate.encode_canonical()?;
    assert_eq!(bytes.first(), Some(&0xab_u8));
    assert_eq!(
        RepoMapCandidateEnvelopeV1::decode_canonical(&bytes)?,
        candidate
    );
    assert_eq!(
        CandidateObjectDigestV1::for_canonical_envelope(&bytes),
        candidate.object_digest()?
    );
    assert_ne!(
        candidate.object_digest()?.as_bytes(),
        candidate.commitment()?.as_bytes()
    );
    assert_eq!(
        hex(&candidate.artifact().encode_canonical()?),
        concat!(
            "a7000101667265706f2f250265c3892f2e2e03181904737265706f6d61702e",
            "636f6d70696c65642e76310558202e56d279ad279e93e3b77d96e46db897f6e5",
            "a6ca224bff7ce892efe32fcc838b0610"
        )
    );
    assert_eq!(
        candidate.artifact().commitment()?.to_wire_string(),
        "sha256:d220a1bb915dc567cacbbe30e718d25e187ba0e585d74dc1d04bcbab2efc2724"
    );
    assert_eq!(
        candidate.object_digest()?.to_wire_string(),
        "sha256:b71d459cbaee7360125a53e9d2d6bd0aa1a2881b46f401b64a3477c523279192"
    );
    assert_eq!(
        candidate.commitment()?.to_wire_string(),
        "sha256:e2ad2405a5532f9d050370899061ff4a9824d3563d9b9322f6fa958bb4df68fe"
    );
    assert!(
        CandidateObjectDigestV1::from_wire_str(
            "SHA256:b71d459cbaee7360125a53e9d2d6bd0aa1a2881b46f401b64a3477c523279192"
        )
        .is_err()
    );
    assert!(
        CandidateObjectDigestV1::from_wire_str(
            "sha256:B71d459cbaee7360125a53e9d2d6bd0aa1a2881b46f401b64a3477c523279192"
        )
        .is_err()
    );

    let mut noncanonical_integer = bytes.clone();
    drop(noncanonical_integer.splice(1..2, [0x18, 0]));
    assert_eq!(
        RepoMapCandidateEnvelopeV1::decode_canonical(&noncanonical_integer),
        Err(CanonicalRepoMapCodecErrorV1::NonCanonicalInteger)
    );
    let mut trailing = bytes;
    trailing.push(0);
    assert_eq!(
        RepoMapCandidateEnvelopeV1::decode_canonical(&trailing),
        Err(CanonicalRepoMapCodecErrorV1::TrailingBytes)
    );
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "assert-based #[test] body; TestResult exists only so the setup steps can use `?`"
)]
fn quarantine_incident_retains_raw_invalid_path_evidence_and_sequence() -> TestResult {
    let evidence = QuarantineObservationEvidenceV1::new(
        vec![
            Vec::new(),
            b"..".to_vec(),
            b"has/slash".to_vec(),
            vec![0, 0xff],
        ],
        None,
        None,
        None,
        QuarantineReasonCodeV1::NonCanonicalSourceAddress,
        StateRootUuidCommitmentV1::for_uuid_bytes([7; 16]),
    )?;
    assert_eq!(
        QuarantineIncidentV1::new(0, 11, evidence.clone()),
        Err(CanonicalRepoMapCodecErrorV1::InvalidValue(
            "quarantine_sequence"
        ))
    );
    let incident = QuarantineIncidentV1::new(1, 11, evidence)?;
    let bytes = incident.encode_canonical()?;
    assert_eq!(
        hex(&bytes),
        concat!(
            "aa00010101020b038440422e2e496861732f736c6173684200ff04f605f606f60709085820",
            "f35a602a633f2d26713cc767595939e8b721f57f5ebb0342d85d3b1373ee8192",
            "09582042ba0b2f0945b20dd648911ec57ef93721c09065737c84b9771f1afdafd41d89"
        )
    );
    assert_eq!(
        incident.digest()?.to_wire_string(),
        "sha256:2b28731af88150159ffbe4ffeaa7d443a5852d502a9639a099518c5e0de260f7"
    );
    assert_eq!(QuarantineIncidentV1::decode_canonical(&bytes)?, incident);
    assert_eq!(
        incident.digest()?,
        QuarantineIncidentV1::decode_canonical(&bytes)?.digest()?
    );
    let mut detached_evidence = bytes.clone();
    let evidence_digest_offset = detached_evidence
        .windows(3)
        .position(|window| window == [0x08, 0x58, 0x20])
        .expect("incident evidence digest field")
        .saturating_add(3);
    *detached_evidence
        .get_mut(evidence_digest_offset)
        .ok_or("incident evidence digest offset in bounds")? ^= 1;
    assert_eq!(
        QuarantineIncidentV1::decode_canonical(&detached_evidence),
        Err(CanonicalRepoMapCodecErrorV1::EvidenceBindingMismatch)
    );
    for mutation in [
        replace_first(&bytes, &[0xaa], &[0xa9]),
        replace_first(&bytes, &[0xaa], &[0xab]),
        replace_first(&bytes, &[0x04, 0xf6], &[0x03, 0xf6]),
        replace_first(&bytes, &[0x04, 0xf6], &[0x0a, 0xf6]),
        replace_first(&bytes, &[0xaa], &[0xbf]),
        replace_first(&bytes, &[0x05, 0xf6], &[0x05, 0xf9, 0, 0]),
    ] {
        assert!(QuarantineIncidentV1::decode_canonical(&mutation).is_err());
    }
    Ok(())
}

fn evidence(
    path: Vec<Vec<u8>>,
    size: Option<u64>,
    raw: Option<[u8; 32]>,
    address: Option<[u8; 32]>,
    reason: QuarantineReasonCodeV1,
) -> Result<QuarantineObservationEvidenceV1, CanonicalRepoMapCodecErrorV1> {
    QuarantineObservationEvidenceV1::new(
        path,
        size,
        raw.map(quanta_index_contract::QuarantinePayloadDigestV1::from_bytes),
        address.map(CandidateObjectDigestV1::from_bytes),
        reason,
        StateRootUuidCommitmentV1::for_uuid_bytes([7; 16]),
    )
}

#[expect(
    clippy::expect_used,
    reason = "fixture helper: every caller passes a needle the golden fixture provably contains"
)]
fn replace_first(bytes: &[u8], old: &[u8], new: &[u8]) -> Vec<u8> {
    let offset = bytes
        .windows(old.len())
        .position(|window| window == old)
        .expect("the fixture contains the targeted canonical field");
    let mut mutated = bytes.to_vec();
    drop(mutated.splice(
        offset..offset.saturating_add(old.len()),
        new.iter().copied(),
    ));
    mutated
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "assert-based #[test] body; TestResult exists only so the setup steps can use `?`"
)]
fn canonical_cbor_rejects_schema_and_type_mutations() -> TestResult {
    let bytes = evidence(
        vec![b"file".to_vec()],
        None,
        None,
        None,
        QuarantineReasonCodeV1::SecureIoUnavailable,
    )?
    .encode_canonical()?;
    let malformed = [
        replace_first(&bytes, &[0xa7], &[0xa8]), // unknown extra entry count
        replace_first(&bytes, &[0xa7], &[0xa6]), // missing entry count
        replace_first(&bytes, &[0x03, 0xf6], &[0x02, 0xf6]), // duplicate key
        replace_first(&bytes, &[0x03, 0xf6], &[0x04, 0xf6]), // out-of-order key
        replace_first(&bytes, &[0x03, 0xf6], &[0x07, 0xf6]), // unknown key
        replace_first(&bytes, &[0xa7], &[0xbf]), // indefinite map
        replace_first(&bytes, &[0x81, 0x44], &[0x9f, 0x44]), // indefinite array
        replace_first(&bytes, &[0x02, 0xf6], &[0x02, 0xf5]), // boolean nullable
        replace_first(&bytes, &[0x02, 0xf6], &[0x02, 0xc0, 0xf6]), // tag nullable
        replace_first(&bytes, &[0x02, 0xf6], &[0x02, 0xf9, 0, 0]), // float nullable
        replace_first(&bytes, &[0x03, 0xf6], &[0x03, 0x00]), // uint in nullable digest
        replace_first(&bytes, &[0x01, 0x81], &[0x01, 0xf6]), // null path array
        replace_first(&bytes, &[0x05, 0x08], &[0x05, 0x18, 0x08]), // non-shortest reason
    ];
    for (index, mutation) in malformed.iter().enumerate() {
        assert!(
            QuarantineObservationEvidenceV1::decode_canonical(mutation).is_err(),
            "malformed CBOR case {index} accepted"
        );
    }
    let invalid_reason = replace_first(&bytes, &[0x05, 0x08], &[0x05, 0x01]);
    assert_eq!(
        QuarantineObservationEvidenceV1::decode_canonical(&invalid_reason),
        Err(CanonicalRepoMapCodecErrorV1::InvalidValue(
            "quarantine_address_digest_prerequisite"
        ))
    );
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "assert-based #[test] body; TestResult exists only so the setup steps can use `?`"
)]
fn candidate_decoder_rejects_artifact_payload_and_schema_mutations() -> TestResult {
    let candidate = candidate()?;
    let bytes = candidate.encode_canonical()?;
    let artifact = candidate.artifact().encode_canonical()?;
    let mut wrong_content = artifact.clone();
    let digest_offset = wrong_content
        .windows(3)
        .position(|window| window == [0x05, 0x58, 0x20])
        .expect("artifact content digest field")
        .saturating_add(3);
    *wrong_content
        .get_mut(digest_offset)
        .ok_or("artifact content digest offset in bounds")? ^= 1;
    let bad_digest = replace_first(&bytes, &artifact, &wrong_content);
    assert_eq!(
        RepoMapCandidateEnvelopeV1::decode_canonical(&bad_digest),
        Err(CanonicalRepoMapCodecErrorV1::ArtifactBindingMismatch)
    );
    let wrong_size = replace_first(&artifact, &[0x06, 0x10], &[0x06, 0x0f]);
    let bad_size = replace_first(&bytes, &artifact, &wrong_size);
    assert_eq!(
        RepoMapCandidateEnvelopeV1::decode_canonical(&bad_size),
        Err(CanonicalRepoMapCodecErrorV1::ArtifactBindingMismatch)
    );
    let mut bad_payload = bytes.clone();
    let payload_offset = bad_payload
        .windows(b"compiled payload".len())
        .position(|window| window == b"compiled payload")
        .expect("candidate compiled payload field");
    *bad_payload
        .get_mut(payload_offset)
        .ok_or("candidate compiled payload offset in bounds")? ^= 1;
    assert_eq!(
        RepoMapCandidateEnvelopeV1::decode_canonical(&bad_payload),
        Err(CanonicalRepoMapCodecErrorV1::ArtifactBindingMismatch)
    );
    let wrong_identity = replace_first(&artifact, b"repo/%", b"repo/&");
    let bad_identity = replace_first(&bytes, &artifact, &wrong_identity);
    assert_eq!(
        RepoMapCandidateEnvelopeV1::decode_canonical(&bad_identity),
        Err(CanonicalRepoMapCodecErrorV1::ArtifactBindingMismatch)
    );
    for mutation in [
        replace_first(&bytes, &[0xab], &[0xaa]),
        replace_first(&bytes, &[0xab], &[0xac]),
        replace_first(&bytes, &[0x01, 0x66], &[0x00, 0x66]),
        replace_first(&bytes, &[0x01, 0x66], &[0x0b, 0x66]),
        replace_first(&bytes, &[0xab], &[0xbf]),
        replace_first(&bytes, &[0x09, 0x81], &[0x09, 0x9f]),
        replace_first(&bytes, &[0x03, 0x18, 0x19], &[0x03, 0xc0, 0x18, 0x19]),
    ] {
        assert!(RepoMapCandidateEnvelopeV1::decode_canonical(&mutation).is_err());
    }
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "assert-based #[test] body; TestResult exists only so the setup steps can use `?`"
)]
fn quarantine_evidence_rejects_reason_prerequisite_violations() -> TestResult {
    use QuarantineReasonCodeV1 as Reason;
    let valid = || vec![b"objects".to_vec(), b"file".to_vec()];
    for invalid in [
        vec![],
        vec![Vec::new()],
        vec![b".".to_vec()],
        vec![b"..".to_vec()],
        vec![b"a/b".to_vec()],
        vec![b"a\0b".to_vec()],
    ] {
        assert_eq!(
            evidence(
                invalid.clone(),
                None,
                None,
                None,
                Reason::SecureIoUnavailable
            ),
            Err(CanonicalRepoMapCodecErrorV1::InvalidValue(
                "quarantine_raw_path_reason"
            ))
        );
        let preserved = evidence(invalid, None, None, None, Reason::NonCanonicalSourceAddress)?;
        assert_eq!(
            QuarantineObservationEvidenceV1::decode_canonical(&preserved.encode_canonical()?)?,
            preserved
        );
        assert!(
            evidence(
                vec![b"..".to_vec()],
                Some(1),
                None,
                None,
                Reason::NonCanonicalSourceAddress
            )
            .is_err()
        );
    }
    assert!(
        evidence(
            valid(),
            None,
            Some([1; 32]),
            None,
            Reason::NonCanonicalSourceAddress
        )
        .is_err()
    );
    for (raw, address) in [(None, None), (Some([1; 32]), None), (None, Some([2; 32]))] {
        assert!(
            evidence(
                valid(),
                Some(1),
                raw,
                address,
                Reason::AddressDigestMismatch
            )
            .is_err()
        );
    }
    assert!(
        evidence(
            valid(),
            Some(1),
            Some([1; 32]),
            Some([1; 32]),
            Reason::AddressDigestMismatch
        )
        .is_err()
    );
    assert!(
        evidence(
            valid(),
            None,
            Some([1; 32]),
            Some([2; 32]),
            Reason::AddressDigestMismatch
        )
        .is_err()
    );
    assert!(
        evidence(
            valid(),
            Some(1),
            Some([1; 32]),
            Some([2; 32]),
            Reason::AddressDigestMismatch
        )
        .is_ok()
    );
    for reason in [
        Reason::NonCanonicalEnvelope,
        Reason::EnvelopeDecodeFailed,
        Reason::LogicalIdentityMismatch,
    ] {
        assert!(evidence(valid(), Some(1), None, Some([2; 32]), reason).is_err());
        assert!(evidence(valid(), Some(1), Some([1; 32]), None, reason).is_err());
        assert!(evidence(valid(), Some(1), Some([1; 32]), Some([2; 32]), reason).is_err());
        assert!(evidence(valid(), Some(1), Some([1; 32]), Some([1; 32]), reason).is_ok());
    }
    for reason in [
        Reason::UnsafeFilesystemMetadata,
        Reason::SymlinkEncountered,
        Reason::HardlinkEncountered,
        Reason::SecureIoUnavailable,
    ] {
        assert!(evidence(valid(), Some(1), Some([1; 32]), Some([2; 32]), reason).is_err());
        assert!(evidence(valid(), None, None, None, reason).is_ok());
    }
    assert!(evidence(valid(), None, None, None, Reason::NonCanonicalSourceAddress).is_err());
    assert!(
        evidence(
            valid(),
            Some(1),
            Some([1; 32]),
            Some([2; 32]),
            Reason::NonCanonicalSourceAddress
        )
        .is_err()
    );
    assert!(
        evidence(
            valid(),
            Some(1),
            Some([1; 32]),
            None,
            Reason::NonCanonicalSourceAddress
        )
        .is_ok()
    );
    assert!(
        evidence(
            valid(),
            None,
            None,
            None,
            Reason::UnsupportedPersistedFormat
        )
        .is_err()
    );
    assert!(
        evidence(
            valid(),
            Some(1),
            Some([1; 32]),
            Some([2; 32]),
            Reason::UnsupportedPersistedFormat
        )
        .is_ok()
    );
    Ok(())
}
