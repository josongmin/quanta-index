//! Canonical immutable `RepoMap` candidate and quarantine contracts.
//!
//! These codecs intentionally do not implement generic serde. Persisted bytes
//! have one accepted form: the exact canonical CBOR schemas in SEP-21-001.

use core::fmt;

use quanta_index_contract_base::{
    IdentityValidationErrorV1, LogicalGenerationIdentityV1, RepoId, RepositoryRevisionIdentityV1,
    RevisionId,
};

mod cbor;
use cbor::{
    Decoder, decode_digest_wire_string, digest_wire_string, domain_digest, plain_digest,
    push_array_len, push_array_len_checked, push_bytes, push_key, push_map_len,
    push_nullable_digest, push_nullable_uint, push_text, push_uint_pair,
};

const ARTIFACT_IDENTITY_DOMAIN: &str = "quanta-index/artifact-identity/v1";
const CANDIDATE_COMMITMENT_DOMAIN: &str = "quanta-index/repomap-candidate-commitment/v1";
const QUARANTINE_EVIDENCE_DOMAIN: &str = "quanta-index/quarantine-evidence/v1";
const QUARANTINE_INCIDENT_DOMAIN: &str = "quanta-index/quarantine-incident/v1";
const STATE_ROOT_UUID_DOMAIN: &str = "quanta-index/state-root-uuid/v1";
const REPOMAP_ARTIFACT_DOMAIN: &str = "repomap.compiled.v1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CanonicalRepoMapCodecErrorV1 {
    UnexpectedEnd,
    TrailingBytes,
    NonCanonicalInteger,
    WrongType(&'static str),
    InvalidValue(&'static str),
    InvalidUtf8,
    Identity(IdentityValidationErrorV1),
    ArtifactBindingMismatch,
    EvidenceBindingMismatch,
    LengthOutOfRange,
    InvalidDigestText,
}

impl fmt::Display for CanonicalRepoMapCodecErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEnd => formatter.write_str("CANONICAL_CBOR_UNEXPECTED_END"),
            Self::TrailingBytes => formatter.write_str("CANONICAL_CBOR_TRAILING_BYTES"),
            Self::NonCanonicalInteger => {
                formatter.write_str("CANONICAL_CBOR_NON_CANONICAL_INTEGER")
            }
            Self::WrongType(expected) => write!(formatter, "CANONICAL_CBOR_WRONG_TYPE:{expected}"),
            Self::InvalidValue(field) => write!(formatter, "CANONICAL_CBOR_INVALID_VALUE:{field}"),
            Self::InvalidUtf8 => formatter.write_str("CANONICAL_CBOR_INVALID_UTF8"),
            Self::Identity(error) => write!(formatter, "{error}"),
            Self::ArtifactBindingMismatch => {
                formatter.write_str("CANDIDATE_ARTIFACT_BINDING_MISMATCH")
            }
            Self::EvidenceBindingMismatch => {
                formatter.write_str("QUARANTINE_EVIDENCE_BINDING_MISMATCH")
            }
            Self::LengthOutOfRange => formatter.write_str("CANONICAL_CBOR_LENGTH_OUT_OF_RANGE"),
            Self::InvalidDigestText => formatter.write_str("CANONICAL_DIGEST_TEXT_INVALID"),
        }
    }
}

impl std::error::Error for CanonicalRepoMapCodecErrorV1 {}

impl From<IdentityValidationErrorV1> for CanonicalRepoMapCodecErrorV1 {
    fn from(error: IdentityValidationErrorV1) -> Self {
        Self::Identity(error)
    }
}

macro_rules! digest_type {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name([u8; 32]);

        impl $name {
            #[must_use]
            pub const fn from_bytes(bytes: [u8; 32]) -> Self {
                Self(bytes)
            }

            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; 32] {
                &self.0
            }

            #[must_use]
            pub fn to_wire_string(self) -> String {
                digest_wire_string(&self.0)
            }

            pub fn from_wire_str(value: &str) -> Result<Self, CanonicalRepoMapCodecErrorV1> {
                decode_digest_wire_string(value).map(Self)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(digest_wire_string(&self.0).as_str())
            }
        }
    };
}

digest_type!(ArtifactContentDigestV1);
digest_type!(ArtifactIdentityCommitmentV1);
digest_type!(CandidateCommitmentV1);
digest_type!(CandidateObjectDigestV1);
digest_type!(QuarantineObservationDigestV1);
digest_type!(QuarantineIncidentDigestV1);
digest_type!(QuarantinePayloadDigestV1);
digest_type!(StateRootUuidCommitmentV1);

impl ArtifactContentDigestV1 {
    #[must_use]
    pub fn for_payload(payload: &[u8]) -> Self {
        Self(plain_digest(payload))
    }
}

impl CandidateObjectDigestV1 {
    #[must_use]
    pub fn for_canonical_envelope(bytes: &[u8]) -> Self {
        Self(plain_digest(bytes))
    }
}

impl QuarantinePayloadDigestV1 {
    #[must_use]
    pub fn for_payload(payload: &[u8]) -> Self {
        Self(plain_digest(payload))
    }
}

impl StateRootUuidCommitmentV1 {
    #[must_use]
    pub fn for_uuid_bytes(uuid_bytes: [u8; 16]) -> Self {
        Self(domain_digest(STATE_ROOT_UUID_DOMAIN, &uuid_bytes))
    }
}

/// Exact seven-field artifact identity. Its domain token is fixed by V1.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactIdentityV1 {
    logical_identity: LogicalGenerationIdentityV1,
    content_digest: ArtifactContentDigestV1,
    byte_size: u64,
}

impl ArtifactIdentityV1 {
    #[must_use]
    pub const fn new(
        logical_identity: LogicalGenerationIdentityV1,
        content_digest: ArtifactContentDigestV1,
        byte_size: u64,
    ) -> Self {
        Self {
            logical_identity,
            content_digest,
            byte_size,
        }
    }

    #[must_use]
    pub const fn logical_identity(&self) -> &LogicalGenerationIdentityV1 {
        &self.logical_identity
    }

    #[must_use]
    pub const fn content_digest(&self) -> ArtifactContentDigestV1 {
        self.content_digest
    }

    #[must_use]
    pub const fn byte_size(&self) -> u64 {
        self.byte_size
    }

    pub fn encode_canonical(&self) -> Result<Vec<u8>, CanonicalRepoMapCodecErrorV1> {
        let mut bytes = Vec::new();
        encode_artifact(self, &mut bytes)?;
        Ok(bytes)
    }

    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, CanonicalRepoMapCodecErrorV1> {
        let mut decoder = Decoder::new(bytes);
        let artifact = decode_artifact(&mut decoder)?;
        decoder.finish()?;
        if artifact.encode_canonical()?.as_slice() != bytes {
            return Err(CanonicalRepoMapCodecErrorV1::InvalidValue("artifact_canonical_bytes"));
        }
        Ok(artifact)
    }

    pub fn commitment(&self) -> Result<ArtifactIdentityCommitmentV1, CanonicalRepoMapCodecErrorV1> {
        Ok(ArtifactIdentityCommitmentV1(domain_digest(
            ARTIFACT_IDENTITY_DOMAIN,
            self.encode_canonical()?.as_slice(),
        )))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RepoMapCandidateCommitmentsV1 {
    pub producer_manifest: [u8; 32],
    pub producer_authority: [u8; 32],
    pub compiled_graph: [u8; 32],
    pub schema: [u8; 32],
    pub projection_profile: [u8; 32],
}

/// The exact immutable candidate object defined by SEP-21-001.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapCandidateEnvelopeV1 {
    logical_identity: LogicalGenerationIdentityV1,
    commitments: RepoMapCandidateCommitmentsV1,
    artifact: ArtifactIdentityV1,
    compiled_payload: Vec<u8>,
}

impl RepoMapCandidateEnvelopeV1 {
    pub fn new(
        logical_identity: LogicalGenerationIdentityV1,
        commitments: RepoMapCandidateCommitmentsV1,
        artifact: ArtifactIdentityV1,
        compiled_payload: Vec<u8>,
    ) -> Result<Self, CanonicalRepoMapCodecErrorV1> {
        let payload_size = u64::try_from(compiled_payload.len())
            .map_err(|_error| CanonicalRepoMapCodecErrorV1::LengthOutOfRange)?;
        if artifact.logical_identity != logical_identity
            || artifact.content_digest != ArtifactContentDigestV1::for_payload(&compiled_payload)
            || artifact.byte_size != payload_size
        {
            return Err(CanonicalRepoMapCodecErrorV1::ArtifactBindingMismatch);
        }
        Ok(Self {
            logical_identity,
            commitments,
            artifact,
            compiled_payload,
        })
    }

    #[must_use]
    pub const fn logical_identity(&self) -> &LogicalGenerationIdentityV1 {
        &self.logical_identity
    }

    #[must_use]
    pub const fn artifact(&self) -> &ArtifactIdentityV1 {
        &self.artifact
    }

    #[must_use]
    pub fn compiled_payload(&self) -> &[u8] {
        self.compiled_payload.as_slice()
    }

    pub fn encode_canonical(&self) -> Result<Vec<u8>, CanonicalRepoMapCodecErrorV1> {
        let mut bytes = Vec::new();
        push_map_len(&mut bytes, 11);
        push_uint_pair(&mut bytes, 0, 1);
        push_key(&mut bytes, 1);
        push_text(
            &mut bytes,
            self.logical_identity
                .repository_revision()
                .repo_id()
                .as_str(),
        )?;
        push_key(&mut bytes, 2);
        push_text(
            &mut bytes,
            self.logical_identity
                .repository_revision()
                .revision_id()
                .as_str(),
        )?;
        push_uint_pair(&mut bytes, 3, self.logical_identity.generation());
        push_key(&mut bytes, 4);
        push_bytes(&mut bytes, &self.commitments.producer_manifest)?;
        push_key(&mut bytes, 5);
        push_bytes(&mut bytes, &self.commitments.producer_authority)?;
        push_key(&mut bytes, 6);
        push_bytes(&mut bytes, &self.commitments.compiled_graph)?;
        push_key(&mut bytes, 7);
        push_bytes(&mut bytes, &self.commitments.schema)?;
        push_key(&mut bytes, 8);
        push_bytes(&mut bytes, &self.commitments.projection_profile)?;
        push_key(&mut bytes, 9);
        push_array_len(&mut bytes, 1);
        encode_artifact(&self.artifact, &mut bytes)?;
        push_key(&mut bytes, 10);
        push_bytes(&mut bytes, &self.compiled_payload)?;
        Ok(bytes)
    }

    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, CanonicalRepoMapCodecErrorV1> {
        let mut decoder = Decoder::new(bytes);
        decoder.expect_len(5, 11, "candidate_map")?;
        decoder.expect_uint(0, "candidate_key_0")?;
        decoder.expect_uint(1, "candidate_version")?;
        decoder.expect_uint(1, "candidate_key_1")?;
        let repo_id = RepoId::new(decoder.text()?.to_owned())?;
        decoder.expect_uint(2, "candidate_key_2")?;
        let revision_id = RevisionId::new(decoder.text()?.to_owned())?;
        decoder.expect_uint(3, "candidate_key_3")?;
        let generation = decoder.uint()?;
        let logical_identity = LogicalGenerationIdentityV1::new(
            RepositoryRevisionIdentityV1::new(repo_id, revision_id),
            generation,
        );
        decoder.expect_uint(4, "candidate_key_4")?;
        let producer_manifest_commitment = decoder.digest()?;
        decoder.expect_uint(5, "candidate_key_5")?;
        let producer_authority_commitment = decoder.digest()?;
        decoder.expect_uint(6, "candidate_key_6")?;
        let compiled_graph_commitment = decoder.digest()?;
        decoder.expect_uint(7, "candidate_key_7")?;
        let schema_commitment = decoder.digest()?;
        decoder.expect_uint(8, "candidate_key_8")?;
        let projection_profile_commitment = decoder.digest()?;
        decoder.expect_uint(9, "candidate_key_9")?;
        decoder.expect_len(4, 1, "candidate_artifact_array")?;
        let artifact = decode_artifact(&mut decoder)?;
        decoder.expect_uint(10, "candidate_key_10")?;
        let compiled_payload = decoder.bytes()?.to_vec();
        decoder.finish()?;
        let candidate = Self::new(
            logical_identity,
            RepoMapCandidateCommitmentsV1 {
                producer_manifest: producer_manifest_commitment,
                producer_authority: producer_authority_commitment,
                compiled_graph: compiled_graph_commitment,
                schema: schema_commitment,
                projection_profile: projection_profile_commitment,
            },
            artifact,
            compiled_payload,
        )?;
        if candidate.encode_canonical()?.as_slice() != bytes {
            return Err(CanonicalRepoMapCodecErrorV1::InvalidValue("candidate_canonical_bytes"));
        }
        Ok(candidate)
    }

    pub fn commitment(&self) -> Result<CandidateCommitmentV1, CanonicalRepoMapCodecErrorV1> {
        Ok(CandidateCommitmentV1(domain_digest(
            CANDIDATE_COMMITMENT_DOMAIN,
            self.encode_canonical()?.as_slice(),
        )))
    }

    pub fn object_digest(&self) -> Result<CandidateObjectDigestV1, CanonicalRepoMapCodecErrorV1> {
        Ok(CandidateObjectDigestV1::for_canonical_envelope(
            self.encode_canonical()?.as_slice(),
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum QuarantineReasonCodeV1 {
    AddressDigestMismatch = 1,
    NonCanonicalEnvelope = 2,
    EnvelopeDecodeFailed = 3,
    LogicalIdentityMismatch = 4,
    UnsafeFilesystemMetadata = 5,
    SymlinkEncountered = 6,
    HardlinkEncountered = 7,
    SecureIoUnavailable = 8,
    NonCanonicalSourceAddress = 9,
    UnsupportedPersistedFormat = 10,
}

impl QuarantineReasonCodeV1 {
    pub const ALL: [Self; 10] = [
        Self::AddressDigestMismatch,
        Self::NonCanonicalEnvelope,
        Self::EnvelopeDecodeFailed,
        Self::LogicalIdentityMismatch,
        Self::UnsafeFilesystemMetadata,
        Self::SymlinkEncountered,
        Self::HardlinkEncountered,
        Self::SecureIoUnavailable,
        Self::NonCanonicalSourceAddress,
        Self::UnsupportedPersistedFormat,
    ];

    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::AddressDigestMismatch => 1,
            Self::NonCanonicalEnvelope => 2,
            Self::EnvelopeDecodeFailed => 3,
            Self::LogicalIdentityMismatch => 4,
            Self::UnsafeFilesystemMetadata => 5,
            Self::SymlinkEncountered => 6,
            Self::HardlinkEncountered => 7,
            Self::SecureIoUnavailable => 8,
            Self::NonCanonicalSourceAddress => 9,
            Self::UnsupportedPersistedFormat => 10,
        }
    }

    fn from_u64(value: u64) -> Result<Self, CanonicalRepoMapCodecErrorV1> {
        match value {
            1 => Ok(Self::AddressDigestMismatch),
            2 => Ok(Self::NonCanonicalEnvelope),
            3 => Ok(Self::EnvelopeDecodeFailed),
            4 => Ok(Self::LogicalIdentityMismatch),
            5 => Ok(Self::UnsafeFilesystemMetadata),
            6 => Ok(Self::SymlinkEncountered),
            7 => Ok(Self::HardlinkEncountered),
            8 => Ok(Self::SecureIoUnavailable),
            9 => Ok(Self::NonCanonicalSourceAddress),
            10 => Ok(Self::UnsupportedPersistedFormat),
            _ => Err(CanonicalRepoMapCodecErrorV1::InvalidValue("quarantine_reason")),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuarantineObservationEvidenceV1 {
    raw_relative_path_components: Vec<Vec<u8>>,
    observed_byte_size: Option<u64>,
    raw_payload_digest: Option<QuarantinePayloadDigestV1>,
    encoded_address_digest: Option<CandidateObjectDigestV1>,
    reason: QuarantineReasonCodeV1,
    state_root_uuid_commitment: StateRootUuidCommitmentV1,
}

impl QuarantineObservationEvidenceV1 {
    pub fn new(
        raw_relative_path_components: Vec<Vec<u8>>,
        observed_byte_size: Option<u64>,
        raw_payload_digest: Option<QuarantinePayloadDigestV1>,
        encoded_address_digest: Option<CandidateObjectDigestV1>,
        reason: QuarantineReasonCodeV1,
        state_root_uuid_commitment: StateRootUuidCommitmentV1,
    ) -> Result<Self, CanonicalRepoMapCodecErrorV1> {
        let evidence = Self {
            raw_relative_path_components,
            observed_byte_size,
            raw_payload_digest,
            encoded_address_digest,
            reason,
            state_root_uuid_commitment,
        };
        evidence.validate()?;
        Ok(evidence)
    }

    fn validate(&self) -> Result<(), CanonicalRepoMapCodecErrorV1> {
        let valid_raw_path = !self.raw_relative_path_components.is_empty()
            && self.raw_relative_path_components.iter().all(|component| {
                !component.is_empty()
                    && component != b"."
                    && component != b".."
                    && !component.contains(&0)
                    && !component.contains(&b'/')
            });
        if !valid_raw_path && self.reason != QuarantineReasonCodeV1::NonCanonicalSourceAddress {
            return Err(CanonicalRepoMapCodecErrorV1::InvalidValue("quarantine_raw_path_reason"));
        }
        if !valid_raw_path
            && (self.observed_byte_size.is_some()
                || self.raw_payload_digest.is_some()
                || self.encoded_address_digest.is_some())
        {
            return Err(CanonicalRepoMapCodecErrorV1::InvalidValue(
                "quarantine_raw_path_first_stage",
            ));
        }
        if self.raw_payload_digest.is_some() && self.observed_byte_size.is_none() {
            return Err(CanonicalRepoMapCodecErrorV1::InvalidValue(
                "quarantine_raw_digest_without_size",
            ));
        }
        match self.reason {
            QuarantineReasonCodeV1::AddressDigestMismatch => {
                let (Some(raw), Some(address)) =
                    (self.raw_payload_digest, self.encoded_address_digest)
                else {
                    return Err(CanonicalRepoMapCodecErrorV1::InvalidValue(
                        "quarantine_address_digest_prerequisite",
                    ));
                };
                if raw.as_bytes() == address.as_bytes() {
                    return Err(CanonicalRepoMapCodecErrorV1::InvalidValue(
                        "quarantine_address_digest_equal",
                    ));
                }
            }
            QuarantineReasonCodeV1::NonCanonicalEnvelope
            | QuarantineReasonCodeV1::EnvelopeDecodeFailed
            | QuarantineReasonCodeV1::LogicalIdentityMismatch => {
                let (Some(raw), Some(address)) =
                    (self.raw_payload_digest, self.encoded_address_digest)
                else {
                    return Err(CanonicalRepoMapCodecErrorV1::InvalidValue(
                        "quarantine_readable_envelope_prerequisite",
                    ));
                };
                if raw.as_bytes() != address.as_bytes() {
                    return Err(CanonicalRepoMapCodecErrorV1::InvalidValue(
                        "quarantine_address_digest_reason_priority",
                    ));
                }
            }
            QuarantineReasonCodeV1::UnsafeFilesystemMetadata
            | QuarantineReasonCodeV1::SymlinkEncountered
            | QuarantineReasonCodeV1::HardlinkEncountered
            | QuarantineReasonCodeV1::SecureIoUnavailable => {
                if self.raw_payload_digest.is_some() {
                    return Err(CanonicalRepoMapCodecErrorV1::InvalidValue(
                        "quarantine_unreadable_raw_digest",
                    ));
                }
            }
            QuarantineReasonCodeV1::NonCanonicalSourceAddress => {
                if valid_raw_path
                    && (self.raw_payload_digest.is_none() || self.encoded_address_digest.is_some())
                {
                    return Err(CanonicalRepoMapCodecErrorV1::InvalidValue(
                        "quarantine_address_grammar_prerequisite",
                    ));
                }
            }
            QuarantineReasonCodeV1::UnsupportedPersistedFormat => {
                if self.raw_payload_digest.is_none() || self.encoded_address_digest.is_none() {
                    return Err(CanonicalRepoMapCodecErrorV1::InvalidValue(
                        "quarantine_persisted_format_prerequisite",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn encode_canonical(&self) -> Result<Vec<u8>, CanonicalRepoMapCodecErrorV1> {
        self.validate()?;
        let mut bytes = Vec::new();
        push_map_len(&mut bytes, 7);
        push_uint_pair(&mut bytes, 0, 1);
        push_key(&mut bytes, 1);
        push_array_len_checked(&mut bytes, self.raw_relative_path_components.len())?;
        for component in &self.raw_relative_path_components {
            push_bytes(&mut bytes, component)?;
        }
        push_key(&mut bytes, 2);
        push_nullable_uint(&mut bytes, self.observed_byte_size);
        push_key(&mut bytes, 3);
        push_nullable_digest(&mut bytes, self.raw_payload_digest.map(|digest| digest.0))?;
        push_key(&mut bytes, 4);
        push_nullable_digest(&mut bytes, self.encoded_address_digest.map(|digest| digest.0))?;
        push_uint_pair(&mut bytes, 5, u64::from(self.reason.code()));
        push_key(&mut bytes, 6);
        push_bytes(&mut bytes, self.state_root_uuid_commitment.as_bytes())?;
        Ok(bytes)
    }

    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, CanonicalRepoMapCodecErrorV1> {
        let mut decoder = Decoder::new(bytes);
        decoder.expect_len(5, 7, "evidence_map")?;
        decoder.expect_uint(0, "evidence_key_0")?;
        decoder.expect_uint(1, "evidence_version")?;
        decoder.expect_uint(1, "evidence_key_1")?;
        let raw_relative_path_components = decode_raw_path_components(&mut decoder)?;
        decoder.expect_uint(2, "evidence_key_2")?;
        let observed_byte_size = decoder.nullable_uint()?;
        decoder.expect_uint(3, "evidence_key_3")?;
        let raw_payload_digest = decoder.nullable_digest()?.map(QuarantinePayloadDigestV1);
        decoder.expect_uint(4, "evidence_key_4")?;
        let encoded_address_digest = decoder.nullable_digest()?.map(CandidateObjectDigestV1);
        decoder.expect_uint(5, "evidence_key_5")?;
        let reason = QuarantineReasonCodeV1::from_u64(decoder.uint()?)?;
        decoder.expect_uint(6, "evidence_key_6")?;
        let state_root_uuid_commitment = StateRootUuidCommitmentV1(decoder.digest()?);
        decoder.finish()?;
        let evidence = Self::new(
            raw_relative_path_components,
            observed_byte_size,
            raw_payload_digest,
            encoded_address_digest,
            reason,
            state_root_uuid_commitment,
        )?;
        if evidence.encode_canonical()?.as_slice() != bytes {
            return Err(CanonicalRepoMapCodecErrorV1::InvalidValue("evidence_canonical_bytes"));
        }
        Ok(evidence)
    }

    pub fn digest(&self) -> Result<QuarantineObservationDigestV1, CanonicalRepoMapCodecErrorV1> {
        Ok(QuarantineObservationDigestV1(domain_digest(
            QUARANTINE_EVIDENCE_DOMAIN,
            self.encode_canonical()?.as_slice(),
        )))
    }
}

/// Append-only incident envelope. Sequence zero and values above `SQLite`'s
/// positive integer range are rejected by construction and decoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuarantineIncidentV1 {
    sequence: u64,
    observed_at_unix_nanos: u64,
    evidence: QuarantineObservationEvidenceV1,
}

impl QuarantineIncidentV1 {
    pub fn new(
        sequence: u64,
        observed_at_unix_nanos: u64,
        evidence: QuarantineObservationEvidenceV1,
    ) -> Result<Self, CanonicalRepoMapCodecErrorV1> {
        if sequence == 0 || sequence > i64::MAX.unsigned_abs() {
            return Err(CanonicalRepoMapCodecErrorV1::InvalidValue("quarantine_sequence"));
        }
        evidence.validate()?;
        Ok(Self {
            sequence,
            observed_at_unix_nanos,
            evidence,
        })
    }

    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    #[must_use]
    pub const fn evidence(&self) -> &QuarantineObservationEvidenceV1 {
        &self.evidence
    }

    pub fn encode_canonical(&self) -> Result<Vec<u8>, CanonicalRepoMapCodecErrorV1> {
        let evidence_digest = self.evidence.digest()?;
        let mut bytes = Vec::new();
        push_map_len(&mut bytes, 10);
        push_uint_pair(&mut bytes, 0, 1);
        push_uint_pair(&mut bytes, 1, self.sequence);
        push_uint_pair(&mut bytes, 2, self.observed_at_unix_nanos);
        push_key(&mut bytes, 3);
        push_array_len_checked(&mut bytes, self.evidence.raw_relative_path_components.len())?;
        for component in &self.evidence.raw_relative_path_components {
            push_bytes(&mut bytes, component)?;
        }
        push_key(&mut bytes, 4);
        push_nullable_uint(&mut bytes, self.evidence.observed_byte_size);
        push_key(&mut bytes, 5);
        push_nullable_digest(&mut bytes, self.evidence.raw_payload_digest.map(|digest| digest.0))?;
        push_key(&mut bytes, 6);
        push_nullable_digest(
            &mut bytes,
            self.evidence.encoded_address_digest.map(|digest| digest.0),
        )?;
        push_uint_pair(&mut bytes, 7, u64::from(self.evidence.reason.code()));
        push_key(&mut bytes, 8);
        push_bytes(&mut bytes, evidence_digest.as_bytes())?;
        push_key(&mut bytes, 9);
        push_bytes(&mut bytes, self.evidence.state_root_uuid_commitment.as_bytes())?;
        Ok(bytes)
    }

    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, CanonicalRepoMapCodecErrorV1> {
        let mut decoder = Decoder::new(bytes);
        decoder.expect_len(5, 10, "incident_map")?;
        decoder.expect_uint(0, "incident_key_0")?;
        decoder.expect_uint(1, "incident_version")?;
        decoder.expect_uint(1, "incident_key_1")?;
        let sequence = decoder.uint()?;
        decoder.expect_uint(2, "incident_key_2")?;
        let observed_at_unix_nanos = decoder.uint()?;
        decoder.expect_uint(3, "incident_key_3")?;
        let raw_relative_path_components = decode_raw_path_components(&mut decoder)?;
        decoder.expect_uint(4, "incident_key_4")?;
        let observed_byte_size = decoder.nullable_uint()?;
        decoder.expect_uint(5, "incident_key_5")?;
        let raw_payload_digest = decoder.nullable_digest()?.map(QuarantinePayloadDigestV1);
        decoder.expect_uint(6, "incident_key_6")?;
        let encoded_address_digest = decoder.nullable_digest()?.map(CandidateObjectDigestV1);
        decoder.expect_uint(7, "incident_key_7")?;
        let reason = QuarantineReasonCodeV1::from_u64(decoder.uint()?)?;
        decoder.expect_uint(8, "incident_key_8")?;
        let encoded_evidence_digest = QuarantineObservationDigestV1(decoder.digest()?);
        decoder.expect_uint(9, "incident_key_9")?;
        let state_root_uuid_commitment = StateRootUuidCommitmentV1(decoder.digest()?);
        decoder.finish()?;
        let evidence = QuarantineObservationEvidenceV1::new(
            raw_relative_path_components,
            observed_byte_size,
            raw_payload_digest,
            encoded_address_digest,
            reason,
            state_root_uuid_commitment,
        )?;
        if evidence.digest()? != encoded_evidence_digest {
            return Err(CanonicalRepoMapCodecErrorV1::EvidenceBindingMismatch);
        }
        let incident = Self::new(sequence, observed_at_unix_nanos, evidence)?;
        if incident.encode_canonical()?.as_slice() != bytes {
            return Err(CanonicalRepoMapCodecErrorV1::InvalidValue("incident_canonical_bytes"));
        }
        Ok(incident)
    }

    pub fn digest(&self) -> Result<QuarantineIncidentDigestV1, CanonicalRepoMapCodecErrorV1> {
        Ok(QuarantineIncidentDigestV1(domain_digest(
            QUARANTINE_INCIDENT_DOMAIN,
            self.encode_canonical()?.as_slice(),
        )))
    }
}

fn decode_raw_path_components(
    decoder: &mut Decoder<'_>,
) -> Result<Vec<Vec<u8>>, CanonicalRepoMapCodecErrorV1> {
    let component_count = usize::try_from(decoder.len(4)?)
        .map_err(|_error| CanonicalRepoMapCodecErrorV1::LengthOutOfRange)?;
    if component_count > decoder.bytes.len().saturating_sub(decoder.offset) {
        return Err(CanonicalRepoMapCodecErrorV1::UnexpectedEnd);
    }
    let mut components = Vec::with_capacity(component_count);
    for _index in 0..component_count {
        components.push(decoder.bytes()?.to_vec());
    }
    Ok(components)
}

fn encode_artifact(
    artifact: &ArtifactIdentityV1,
    bytes: &mut Vec<u8>,
) -> Result<(), CanonicalRepoMapCodecErrorV1> {
    push_map_len(bytes, 7);
    push_uint_pair(bytes, 0, 1);
    push_key(bytes, 1);
    push_text(
        bytes,
        artifact
            .logical_identity
            .repository_revision()
            .repo_id()
            .as_str(),
    )?;
    push_key(bytes, 2);
    push_text(
        bytes,
        artifact
            .logical_identity
            .repository_revision()
            .revision_id()
            .as_str(),
    )?;
    push_uint_pair(bytes, 3, artifact.logical_identity.generation());
    push_key(bytes, 4);
    push_text(bytes, REPOMAP_ARTIFACT_DOMAIN)?;
    push_key(bytes, 5);
    push_bytes(bytes, artifact.content_digest.as_bytes())?;
    push_uint_pair(bytes, 6, artifact.byte_size);
    Ok(())
}

fn decode_artifact(
    decoder: &mut Decoder<'_>,
) -> Result<ArtifactIdentityV1, CanonicalRepoMapCodecErrorV1> {
    decoder.expect_len(5, 7, "artifact_map")?;
    decoder.expect_uint(0, "artifact_key_0")?;
    decoder.expect_uint(1, "artifact_version")?;
    decoder.expect_uint(1, "artifact_key_1")?;
    let repo_id = RepoId::new(decoder.text()?.to_owned())?;
    decoder.expect_uint(2, "artifact_key_2")?;
    let revision_id = RevisionId::new(decoder.text()?.to_owned())?;
    decoder.expect_uint(3, "artifact_key_3")?;
    let generation = decoder.uint()?;
    decoder.expect_uint(4, "artifact_key_4")?;
    if decoder.text()? != REPOMAP_ARTIFACT_DOMAIN {
        return Err(CanonicalRepoMapCodecErrorV1::InvalidValue("artifact_domain"));
    }
    decoder.expect_uint(5, "artifact_key_5")?;
    let content_digest = ArtifactContentDigestV1(decoder.digest()?);
    decoder.expect_uint(6, "artifact_key_6")?;
    let byte_size = decoder.uint()?;
    Ok(ArtifactIdentityV1::new(
        LogicalGenerationIdentityV1::new(
            RepositoryRevisionIdentityV1::new(repo_id, revision_id),
            generation,
        ),
        content_digest,
        byte_size,
    ))
}
