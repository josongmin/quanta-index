//! Pure physical-address and state-root security primitives for layout V3.
//!
//! This module performs no filesystem mutation or open. P03 supplies actual
//! lstat/no-follow-open/fstat observations and consumes these fail-closed
//! verdicts before reading or publishing bytes.

use std::path::PathBuf;

use quanta_index_contract::{
    CandidateObjectDigestV1, QuarantineIncidentDigestV1, QuarantinePayloadDigestV1,
    QuarantineReasonCodeV1, StateRootUuidCommitmentV1,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutV3AddressError {
    WrongComponentCount,
    WrongStaticComponent,
    WrongFanoutWidth,
    WrongLeafGrammar,
    NonLowercaseHex,
}

impl core::fmt::Display for LayoutV3AddressError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let code = match self {
            Self::WrongComponentCount => "LAYOUT_V3_WRONG_COMPONENT_COUNT",
            Self::WrongStaticComponent => "LAYOUT_V3_WRONG_STATIC_COMPONENT",
            Self::WrongFanoutWidth => "LAYOUT_V3_WRONG_FANOUT_WIDTH",
            Self::WrongLeafGrammar => "LAYOUT_V3_WRONG_LEAF_GRAMMAR",
            Self::NonLowercaseHex => "LAYOUT_V3_NON_LOWERCASE_HEX",
        };
        formatter.write_str(code)
    }
}

impl std::error::Error for LayoutV3AddressError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateObjectAddressV1 {
    digest: CandidateObjectDigestV1,
}

impl CandidateObjectAddressV1 {
    #[must_use]
    pub const fn new(digest: CandidateObjectDigestV1) -> Self {
        Self { digest }
    }

    #[must_use]
    pub const fn digest(&self) -> CandidateObjectDigestV1 {
        self.digest
    }

    #[must_use]
    pub fn relative_path(&self) -> PathBuf {
        content_address_path("objects", "", self.digest.as_bytes(), "cbor")
    }

    pub fn parse_components(components: &[&[u8]]) -> Result<Self, LayoutV3AddressError> {
        let digest = parse_content_address(components, &[b"objects"], "cbor")?;
        Ok(Self::new(CandidateObjectDigestV1::from_bytes(digest)))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuarantineIncidentAddressV1 {
    digest: QuarantineIncidentDigestV1,
}

impl QuarantineIncidentAddressV1 {
    #[must_use]
    pub const fn new(digest: QuarantineIncidentDigestV1) -> Self {
        Self { digest }
    }

    #[must_use]
    pub const fn digest(&self) -> QuarantineIncidentDigestV1 {
        self.digest
    }

    #[must_use]
    pub fn relative_path(&self) -> PathBuf {
        content_address_path("quarantine", "incidents", self.digest.as_bytes(), "cbor")
    }

    pub fn parse_components(components: &[&[u8]]) -> Result<Self, LayoutV3AddressError> {
        let digest = parse_content_address(components, &[b"quarantine", b"incidents"], "cbor")?;
        Ok(Self::new(QuarantineIncidentDigestV1::from_bytes(digest)))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuarantinePayloadAddressV1 {
    digest: QuarantinePayloadDigestV1,
}

impl QuarantinePayloadAddressV1 {
    #[must_use]
    pub const fn new(digest: QuarantinePayloadDigestV1) -> Self {
        Self { digest }
    }

    #[must_use]
    pub const fn digest(&self) -> QuarantinePayloadDigestV1 {
        self.digest
    }

    #[must_use]
    pub fn relative_path(&self) -> PathBuf {
        content_address_path("quarantine", "payloads", self.digest.as_bytes(), "bin")
    }

    pub fn parse_components(components: &[&[u8]]) -> Result<Self, LayoutV3AddressError> {
        let digest = parse_content_address(components, &[b"quarantine", b"payloads"], "bin")?;
        Ok(Self::new(QuarantinePayloadDigestV1::from_bytes(digest)))
    }
}

fn content_address_path(root: &str, family: &str, digest: &[u8; 32], extension: &str) -> PathBuf {
    let hex = lowercase_hex(digest);
    let mut path = PathBuf::from(root);
    if !family.is_empty() {
        path.push(family);
    }
    path.push("sha256");
    let (first_fanout, rest) = hex.split_at(2);
    let (second_fanout, leaf_hex) = rest.split_at(2);
    path.push(first_fanout);
    path.push(second_fanout);
    path.push(format!("{leaf_hex}.{extension}"));
    path
}

fn parse_content_address(
    components: &[&[u8]],
    prefix: &[&[u8]],
    extension: &str,
) -> Result<[u8; 32], LayoutV3AddressError> {
    let expected_count = prefix.len().saturating_add(4);
    if components.len() != expected_count {
        return Err(LayoutV3AddressError::WrongComponentCount);
    }
    for (actual, expected) in components.iter().zip(prefix.iter()) {
        if actual != expected {
            return Err(LayoutV3AddressError::WrongStaticComponent);
        }
    }
    let algorithm = components
        .get(prefix.len())
        .ok_or(LayoutV3AddressError::WrongComponentCount)?;
    if algorithm != b"sha256" {
        return Err(LayoutV3AddressError::WrongStaticComponent);
    }
    let first = components
        .get(prefix.len().saturating_add(1))
        .ok_or(LayoutV3AddressError::WrongComponentCount)?;
    let second = components
        .get(prefix.len().saturating_add(2))
        .ok_or(LayoutV3AddressError::WrongComponentCount)?;
    if first.len() != 2 || second.len() != 2 {
        return Err(LayoutV3AddressError::WrongFanoutWidth);
    }
    let leaf = components
        .get(prefix.len().saturating_add(3))
        .ok_or(LayoutV3AddressError::WrongComponentCount)?;
    let expected_suffix = format!(".{extension}");
    if leaf.len() != 60_usize.saturating_add(expected_suffix.len())
        || !leaf.ends_with(expected_suffix.as_bytes())
    {
        return Err(LayoutV3AddressError::WrongLeafGrammar);
    }
    let leaf_hex = leaf
        .get(..60)
        .ok_or(LayoutV3AddressError::WrongLeafGrammar)?;
    let mut encoded = Vec::with_capacity(64);
    encoded.extend_from_slice(first);
    encoded.extend_from_slice(second);
    encoded.extend_from_slice(leaf_hex);
    decode_lowercase_hex(&encoded)
}

fn lowercase_hex(digest: &[u8; 32]) -> String {
    fn hex_digit(nibble: u8) -> char {
        match nibble {
            0..=9 => char::from(b'0'.saturating_add(nibble)),
            _ => char::from(b'a'.saturating_add(nibble.saturating_sub(10))),
        }
    }
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        encoded.push(hex_digit(byte >> 4));
        encoded.push(hex_digit(byte & 0x0f));
    }
    encoded
}

fn decode_lowercase_hex(encoded: &[u8]) -> Result<[u8; 32], LayoutV3AddressError> {
    if encoded.len() != 64 {
        return Err(LayoutV3AddressError::WrongLeafGrammar);
    }
    let mut digest = [0_u8; 32];
    for (index, pair) in encoded.chunks_exact(2).enumerate() {
        let high = hex_nibble(
            pair.first()
                .copied()
                .ok_or(LayoutV3AddressError::WrongLeafGrammar)?,
        )?;
        let low = hex_nibble(
            pair.get(1)
                .copied()
                .ok_or(LayoutV3AddressError::WrongLeafGrammar)?,
        )?;
        let slot = digest
            .get_mut(index)
            .ok_or(LayoutV3AddressError::WrongLeafGrammar)?;
        *slot = (high << 4) | low;
    }
    Ok(digest)
}

fn hex_nibble(value: u8) -> Result<u8, LayoutV3AddressError> {
    match value {
        b'0'..=b'9' => Ok(value.saturating_sub(b'0')),
        b'a'..=b'f' => Ok(value.saturating_sub(b'a').saturating_add(10)),
        _ => Err(LayoutV3AddressError::NonLowercaseHex),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservedFileKindV1 {
    RegularFile,
    Directory,
    Symlink,
    Other,
}

/// Platform metadata reduced to fields relevant to the immutable state-root policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObservedFileMetadataV1 {
    pub device: u64,
    pub inode: u64,
    pub uid: u32,
    /// Permission bits only, not the platform file-type bits.
    pub mode: u32,
    pub link_count: u64,
    pub kind: ObservedFileKindV1,
}

/// The immutable lstat/fstat observations around one no-follow open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SecureMetadataPairV1 {
    pub lstat: ObservedFileMetadataV1,
    /// `None` means a no-follow open or fstat result was unavailable.
    pub fstat: Option<ObservedFileMetadataV1>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateRootSecurityVerificationErrorV1 {
    InvalidRelativePath,
    SymlinkEncountered,
    SecureOpenUnavailable,
    HardlinkEncountered,
    MetadataMismatch,
}

impl StateRootSecurityVerificationErrorV1 {
    #[must_use]
    pub const fn quarantine_reason_code(self) -> QuarantineReasonCodeV1 {
        match self {
            Self::InvalidRelativePath => QuarantineReasonCodeV1::NonCanonicalSourceAddress,
            Self::SymlinkEncountered => QuarantineReasonCodeV1::SymlinkEncountered,
            Self::SecureOpenUnavailable => QuarantineReasonCodeV1::SecureIoUnavailable,
            Self::HardlinkEncountered => QuarantineReasonCodeV1::HardlinkEncountered,
            Self::MetadataMismatch => QuarantineReasonCodeV1::UnsafeFilesystemMetadata,
        }
    }
}

impl core::fmt::Display for StateRootSecurityVerificationErrorV1 {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let code = match self {
            Self::InvalidRelativePath => "STATE_ROOT_RELATIVE_PATH_INVALID",
            Self::SymlinkEncountered => "STATE_ROOT_SYMLINK_ENCOUNTERED",
            Self::SecureOpenUnavailable => "STATE_ROOT_SECURE_OPEN_UNAVAILABLE",
            Self::HardlinkEncountered => "STATE_ROOT_HARDLINK_ENCOUNTERED",
            Self::MetadataMismatch => "STATE_ROOT_METADATA_MISMATCH",
        };
        formatter.write_str(code)
    }
}

impl std::error::Error for StateRootSecurityVerificationErrorV1 {}

/// State-root-bound immutable policy. The P03 open boundary captures the
/// effective UID and root UUID exactly once, then supplies actual metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StateRootSecurityContextV1 {
    expected_uid: u32,
    state_root_uuid_commitment: StateRootUuidCommitmentV1,
}

impl StateRootSecurityContextV1 {
    pub const DIRECTORY_MODE: u32 = 0o700;
    pub const FILE_MODE: u32 = 0o600;

    #[must_use]
    pub const fn new(
        expected_uid: u32,
        state_root_uuid_commitment: StateRootUuidCommitmentV1,
    ) -> Self {
        Self {
            expected_uid,
            state_root_uuid_commitment,
        }
    }

    #[must_use]
    pub const fn expected_uid(self) -> u32 {
        self.expected_uid
    }

    #[must_use]
    pub const fn state_root_uuid_commitment(self) -> StateRootUuidCommitmentV1 {
        self.state_root_uuid_commitment
    }

    /// Validate bytes that are safe to pass to an openat-style traversal.
    /// Raw quarantine evidence is deliberately not accepted by this function.
    pub fn validate_relative_components(
        components: &[&[u8]],
    ) -> Result<(), StateRootSecurityVerificationErrorV1> {
        if components.is_empty()
            || components.iter().any(|component| {
                component.is_empty()
                    || component.contains(&0)
                    || component.contains(&b'/')
                    || *component == b"."
                    || *component == b".."
            })
        {
            return Err(StateRootSecurityVerificationErrorV1::InvalidRelativePath);
        }
        Ok(())
    }

    /// Verify all directory observations and the final regular-file observation.
    /// Error order matches SEP-21-001: symlink, open I/O, hardlink, metadata.
    pub fn verify_opened_file(
        self,
        traversed_directories: &[SecureMetadataPairV1],
        leaf: SecureMetadataPairV1,
    ) -> Result<(), StateRootSecurityVerificationErrorV1> {
        let observations = || traversed_directories.iter().chain(std::iter::once(&leaf));
        if observations().any(|pair| {
            pair.lstat.kind == ObservedFileKindV1::Symlink
                || pair
                    .fstat
                    .is_some_and(|metadata| metadata.kind == ObservedFileKindV1::Symlink)
        }) {
            return Err(StateRootSecurityVerificationErrorV1::SymlinkEncountered);
        }
        if observations().any(|pair| pair.fstat.is_none()) {
            return Err(StateRootSecurityVerificationErrorV1::SecureOpenUnavailable);
        }
        if leaf.lstat.link_count != 1 || leaf.fstat.is_some_and(|metadata| metadata.link_count != 1)
        {
            return Err(StateRootSecurityVerificationErrorV1::HardlinkEncountered);
        }
        for directory in traversed_directories {
            self.verify_metadata_pair(
                *directory,
                ObservedFileKindV1::Directory,
                Self::DIRECTORY_MODE,
            )?;
        }
        self.verify_metadata_pair(leaf, ObservedFileKindV1::RegularFile, Self::FILE_MODE)
    }

    fn verify_metadata_pair(
        self,
        pair: SecureMetadataPairV1,
        expected_kind: ObservedFileKindV1,
        expected_mode: u32,
    ) -> Result<(), StateRootSecurityVerificationErrorV1> {
        let Some(fstat) = pair.fstat else {
            return Err(StateRootSecurityVerificationErrorV1::SecureOpenUnavailable);
        };
        if pair.lstat.device != fstat.device
            || pair.lstat.inode != fstat.inode
            || pair.lstat.kind != expected_kind
            || fstat.kind != expected_kind
            || pair.lstat.uid != self.expected_uid
            || fstat.uid != self.expected_uid
            || pair.lstat.mode != expected_mode
            || fstat.mode != expected_mode
        {
            return Err(StateRootSecurityVerificationErrorV1::MetadataMismatch);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_roundtrip_and_case_refusal() {
        let address =
            CandidateObjectAddressV1::new(CandidateObjectDigestV1::from_bytes([0xab; 32]));
        assert_eq!(
            address.relative_path(),
            PathBuf::from(format!("objects/sha256/ab/ab/{}.cbor", "ab".repeat(30)))
        );
        let leaf = format!("{}.cbor", "ab".repeat(30));
        match CandidateObjectAddressV1::parse_components(&[
            b"objects",
            b"sha256",
            b"ab",
            b"ab",
            leaf.as_bytes(),
        ]) {
            Ok(parsed) => assert_eq!(parsed, address),
            Err(error) => panic!("expected roundtrip parse, got {error}"),
        }
        let upper_leaf = format!("{}.cbor", "AB".repeat(30));
        assert_eq!(
            CandidateObjectAddressV1::parse_components(&[
                b"objects",
                b"sha256",
                b"AB",
                b"AB",
                upper_leaf.as_bytes()
            ]),
            Err(LayoutV3AddressError::NonLowercaseHex)
        );
    }

    #[test]
    fn security_error_precedence_is_fixed() {
        let context = StateRootSecurityContextV1::new(
            501,
            StateRootUuidCommitmentV1::for_uuid_bytes([1; 16]),
        );
        let symlink = ObservedFileMetadataV1 {
            device: 1,
            inode: 2,
            uid: 999,
            mode: 0,
            link_count: 9,
            kind: ObservedFileKindV1::Symlink,
        };
        assert_eq!(
            context.verify_opened_file(
                &[],
                SecureMetadataPairV1 {
                    lstat: symlink,
                    fstat: None
                }
            ),
            Err(StateRootSecurityVerificationErrorV1::SymlinkEncountered)
        );
    }
}
