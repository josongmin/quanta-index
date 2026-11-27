//! Bounded retirement identity bytes. This is data until an installed BOOT
//! reader joins it to the held predecessor route and selected candidate.

use std::{
    fmt,
    io::{self, Write},
};

use sha2::{Digest as _, Sha256};

use super::{Digest, IndexInstanceId, ObjectRef, ProjectionId};
use crate::{RepoId, RevisionId};

pub const MAX_RETIRED_PROJECTION_IDENTITIES_V1: usize =
    super::MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1;
const MAGIC_V1: &[u8] = b"qidx.retired-projection-policy.v1\0";
const HEADER_BYTES_V1: usize = MAGIC_V1.len() + 4 + 32 * 6 + 4 + 4;
const IDENTITY_BYTES_V1: usize = 32;
pub const MAX_RETIRED_PROJECTION_POLICY_BYTES_V1: usize =
    HEADER_BYTES_V1 + IDENTITY_BYTES_V1 * MAX_RETIRED_PROJECTION_IDENTITIES_V1;

#[derive(Debug)]
pub enum RetiredProjectionPolicyErrorV1<E> {
    Format(&'static str),
    Admission(E),
    Io(io::Error),
}

impl<E: fmt::Display> fmt::Display for RetiredProjectionPolicyErrorV1<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format(reason) => formatter.write_str(reason),
            Self::Admission(error) => fmt::Display::fmt(error, formatter),
            Self::Io(error) => fmt::Display::fmt(error, formatter),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for RetiredProjectionPolicyErrorV1<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Format(_) => None,
            Self::Admission(error) => Some(error),
            Self::Io(error) => Some(error),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetiredProjectionIdentityV1 {
    pub projection: ProjectionId,
    pub instance: IndexInstanceId,
}

/// The predecessor selection digest must name the exact physically held BOOT.
/// The candidate fields are comparison axes, not self-issued authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetiredProjectionPolicyHeaderV1 {
    pub format_version: u32,
    pub old_selection_sha256: Digest,
    pub old_root_nonce: Digest,
    pub old_routes_ref: Option<ObjectRef>,
    /// Source head count only. Map-only routes appear in the retired identity
    /// count, which must match every authenticated old `repomap_heads` entry.
    pub old_source_head_count: u32,
    pub candidate_root_nonce: Digest,
    pub candidate_config_sha256: Digest,
    pub candidate_profile_ref: ObjectRef,
}

impl RetiredProjectionPolicyHeaderV1 {
    pub const FORMAT_VERSION_V1: u32 = 1;

    pub fn validate_v1(&self, identity_count: usize) -> Result<(), &'static str> {
        if self.format_version != Self::FORMAT_VERSION_V1
            || identity_count > MAX_RETIRED_PROJECTION_IDENTITIES_V1
            || self.old_selection_sha256.0 == [0; 32]
            || self.old_root_nonce.0 == [0; 32]
            || self.candidate_root_nonce.0 == [0; 32]
            || self.candidate_config_sha256.0 == [0; 32]
            || self.candidate_profile_ref.0.0 == [0; 32]
            || self.old_source_head_count as usize > super::MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1
            || self.old_routes_ref.is_none()
                && (self.old_source_head_count != 0 || identity_count != 0)
            || self
                .old_routes_ref
                .is_some_and(|reference| reference.0.0 == [0; 32])
        {
            return Err("retired projection policy header is invalid");
        }
        Ok(())
    }
}

/// Physical object reference and exact body digest. Neither field proves the
/// predecessor association without BOOT and selection verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetiredProjectionPolicyRefV1 {
    pub object: ObjectRef,
    pub sha256: Digest,
    pub identity_count: u32,
}

crate::native_serde_struct!([serde_identifier] RetiredProjectionPolicyRefV1 {
    object: ObjectRef,
    sha256: Digest,
    identity_count: u32,
});

impl RetiredProjectionPolicyRefV1 {
    pub fn validate_v1(&self) -> Result<(), &'static str> {
        if self.object.0.0 == [0; 32]
            || self.sha256.0 == [0; 32]
            || self.identity_count as usize > MAX_RETIRED_PROJECTION_IDENTITIES_V1
        {
            return Err("retired projection policy reference is invalid");
        }
        Ok(())
    }

    pub fn matches_body_v1<E>(
        &self,
        body: &[u8],
        scratch: &mut [IndexInstanceId],
        checkpoint: &mut impl FnMut() -> Result<(), E>,
    ) -> Result<bool, RetiredProjectionPolicyErrorV1<E>> {
        self.validate_v1()
            .map_err(RetiredProjectionPolicyErrorV1::Format)?;
        let view = decode_retired_projection_policy_v1(body, scratch, checkpoint)?;
        checkpoint().map_err(RetiredProjectionPolicyErrorV1::Admission)?;
        Ok(view.len_v1() == self.identity_count as usize
            && Digest(Sha256::digest(body).into()) == self.sha256)
    }
}

/// Zero-allocation view into already budgeted immutable object bytes.
pub struct RetiredProjectionPolicyViewV1<'a> {
    header: RetiredProjectionPolicyHeaderV1,
    identities: &'a [u8],
}

impl RetiredProjectionPolicyViewV1<'_> {
    pub fn header_v1(&self) -> RetiredProjectionPolicyHeaderV1 {
        self.header
    }

    pub fn len_v1(&self) -> usize {
        self.identities.len() / IDENTITY_BYTES_V1
    }

    pub fn identity_v1(&self, index: usize) -> Option<RetiredProjectionIdentityV1> {
        let start = index.checked_mul(IDENTITY_BYTES_V1)?;
        let end = start.checked_add(IDENTITY_BYTES_V1)?;
        let row = self.identities.get(start..end)?;
        Some(identity_from_row_v1(row))
    }

    pub fn rejects_v1<E>(
        &self,
        projection: ProjectionId,
        instance: IndexInstanceId,
        checkpoint: &mut impl FnMut() -> Result<(), E>,
    ) -> Result<bool, E> {
        for row in self.identities.chunks_exact(IDENTITY_BYTES_V1) {
            checkpoint()?;
            if &row[..16] == projection.0.as_slice()
                || &row[16..] == instance.0.as_slice()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn identity_from_row_v1(row: &[u8]) -> RetiredProjectionIdentityV1 {
    let mut projection = [0; 16];
    let mut instance = [0; 16];
    projection.copy_from_slice(&row[..16]);
    instance.copy_from_slice(&row[16..32]);
    RetiredProjectionIdentityV1 {
        projection: ProjectionId(projection),
        instance: IndexInstanceId(instance),
    }
}

fn validate_identities_v1<E>(
    identities: impl Iterator<Item = RetiredProjectionIdentityV1>,
    count: usize,
    scratch: &mut [IndexInstanceId],
    checkpoint: &mut impl FnMut() -> Result<(), E>,
) -> Result<(), RetiredProjectionPolicyErrorV1<E>> {
    if count > MAX_RETIRED_PROJECTION_IDENTITIES_V1 || scratch.len() < count {
        return Err(RetiredProjectionPolicyErrorV1::Format(
            "retired projection policy scratch or count exceeds bound",
        ));
    }
    let mut previous = None;
    let mut seen = 0;
    for (index, identity) in identities.enumerate() {
        checkpoint().map_err(RetiredProjectionPolicyErrorV1::Admission)?;
        if index >= count {
            return Err(RetiredProjectionPolicyErrorV1::Format(
                "retired projection identity count differs",
            ));
        }
        if identity.projection.0 == [0; 16]
            || identity.instance.0 == [0; 16]
            || previous.is_some_and(|prior| prior >= identity.projection.0)
        {
            return Err(RetiredProjectionPolicyErrorV1::Format(
                "retired projection identities are not sorted and unique",
            ));
        }
        scratch[index] = identity.instance;
        previous = Some(identity.projection.0);
        seen += 1;
    }
    if seen != count {
        return Err(RetiredProjectionPolicyErrorV1::Format(
            "retired projection identity count differs",
        ));
    }
    checkpoint().map_err(RetiredProjectionPolicyErrorV1::Admission)?;
    scratch[..count].sort_unstable_by_key(|instance| instance.0);
    checkpoint().map_err(RetiredProjectionPolicyErrorV1::Admission)?;
    if scratch[..count].windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(RetiredProjectionPolicyErrorV1::Format(
            "retired projection instance identity is duplicated",
        ));
    }
    Ok(())
}

/// Decode only after an immutable-object reader admitted the complete payload.
/// Count, exact length and order are checked before any heap allocation.
pub fn preflight_retired_projection_policy_v1(bytes: &[u8]) -> Result<usize, &'static str> {
    if bytes.len() < HEADER_BYTES_V1
        || bytes.len() > MAX_RETIRED_PROJECTION_POLICY_BYTES_V1
        || !bytes.starts_with(MAGIC_V1)
    {
        return Err("retired projection policy body length or magic is invalid");
    }
    let count = u32::from_le_bytes(
        bytes[HEADER_BYTES_V1 - 4..HEADER_BYTES_V1]
            .try_into()
            .expect("fixed count field"),
    ) as usize;
    if count > MAX_RETIRED_PROJECTION_IDENTITIES_V1
        || bytes.len() != HEADER_BYTES_V1 + count * IDENTITY_BYTES_V1
    {
        return Err("retired projection policy body length differs from count");
    }
    Ok(count)
}

/// The caller reserves `count * size_of::<IndexInstanceId>()` with its Q
/// WorkBudget after preflight and lends that scratch for order validation.
pub fn decode_retired_projection_policy_v1<'a, E>(
    bytes: &'a [u8],
    scratch: &mut [IndexInstanceId],
    checkpoint: &mut impl FnMut() -> Result<(), E>,
) -> Result<RetiredProjectionPolicyViewV1<'a>, RetiredProjectionPolicyErrorV1<E>> {
    checkpoint().map_err(RetiredProjectionPolicyErrorV1::Admission)?;
    let preflight_count =
        preflight_retired_projection_policy_v1(bytes).map_err(RetiredProjectionPolicyErrorV1::Format)?;
    let mut offset = MAGIC_V1.len();
    let mut take = |length: usize| {
        let next = offset + length;
        let value = &bytes[offset..next];
        offset = next;
        value
    };
    let version = u32::from_le_bytes(take(4).try_into().expect("fixed version"));
    let digest = |field: &[u8]| {
        let mut value = [0; 32];
        value.copy_from_slice(field);
        Digest(value)
    };
    let old_selection_sha256 = digest(take(32));
    let old_root_nonce = digest(take(32));
    let old_routes_raw = digest(take(32));
    let old_source_head_count = u32::from_le_bytes(take(4).try_into().expect("fixed count"));
    let candidate_root_nonce = digest(take(32));
    let candidate_config_sha256 = digest(take(32));
    let candidate_profile_ref = ObjectRef(digest(take(32)));
    let count = u32::from_le_bytes(take(4).try_into().expect("fixed count")) as usize;
    debug_assert_eq!(count, preflight_count);
    let header = RetiredProjectionPolicyHeaderV1 {
        format_version: version,
        old_selection_sha256,
        old_root_nonce,
        old_routes_ref: (old_routes_raw.0 != [0; 32]).then_some(ObjectRef(old_routes_raw)),
        old_source_head_count,
        candidate_root_nonce,
        candidate_config_sha256,
        candidate_profile_ref,
    };
    header
        .validate_v1(count)
        .map_err(RetiredProjectionPolicyErrorV1::Format)?;
    let identities = &bytes[offset..];
    validate_identities_v1(
        identities
            .chunks_exact(IDENTITY_BYTES_V1)
            .map(identity_from_row_v1),
        count,
        scratch,
        checkpoint,
    )?;
    Ok(RetiredProjectionPolicyViewV1 { header, identities })
}

/// Write exact canonical bytes to a caller-budgeted sink. The caller supplies
/// the immutable object registration and verifies its readback separately.
pub fn encode_retired_projection_policy_v1<E>(
    header: RetiredProjectionPolicyHeaderV1,
    identities: &[RetiredProjectionIdentityV1],
    scratch: &mut [IndexInstanceId],
    checkpoint: &mut impl FnMut() -> Result<(), E>,
    mut output: impl Write,
) -> Result<Digest, RetiredProjectionPolicyErrorV1<E>> {
    header
        .validate_v1(identities.len())
        .map_err(RetiredProjectionPolicyErrorV1::Format)?;
    validate_identities_v1(
        identities.iter().copied(),
        identities.len(),
        scratch,
        checkpoint,
    )?;
    let mut hash = Sha256::new();
    let mut emit = |bytes: &[u8]| -> Result<(), RetiredProjectionPolicyErrorV1<E>> {
        output
            .write_all(bytes)
            .map_err(RetiredProjectionPolicyErrorV1::Io)?;
        hash.update(bytes);
        Ok(())
    };
    emit(MAGIC_V1)?;
    emit(&header.format_version.to_le_bytes())?;
    emit(&header.old_selection_sha256.0)?;
    emit(&header.old_root_nonce.0)?;
    emit(
        &header
            .old_routes_ref
            .map_or([0; 32], |reference| reference.0.0),
    )?;
    emit(&header.old_source_head_count.to_le_bytes())?;
    emit(&header.candidate_root_nonce.0)?;
    emit(&header.candidate_config_sha256.0)?;
    emit(&header.candidate_profile_ref.0.0)?;
    emit(&(identities.len() as u32).to_le_bytes())?;
    for identity in identities {
        checkpoint().map_err(RetiredProjectionPolicyErrorV1::Admission)?;
        emit(&identity.projection.0)?;
        emit(&identity.instance.0)?;
    }
    Ok(Digest(hash.finalize().into()))
}

/// Exact historical S route algorithm. A caller must authenticate the old
/// `(repo, revision)` pair and its Required profile before trusting the pair.
pub fn retired_repomap_route_identity_v1(
    repo: &RepoId,
    revision: &RevisionId,
) -> Result<RetiredProjectionIdentityV1, &'static str> {
    fn derive(role: &[u8], repo: &RepoId, revision: &RevisionId) -> Result<[u8; 16], &'static str> {
        let mut hash = Sha256::new();
        hash.update(b"semantica/native-repomap-route/v1\0");
        hash.update(role);
        hash.update(b"\0");
        for axis in [repo.as_str(), revision.as_str()] {
            hash.update(
                u64::try_from(axis.len())
                    .map_err(|_| "route axis length overflow")?
                    .to_be_bytes(),
            );
            hash.update(axis.as_bytes());
        }
        let value = hash.finalize();
        let mut id = [0; 16];
        id.copy_from_slice(&value[..16]);
        Ok(id)
    }
    Ok(RetiredProjectionIdentityV1 {
        projection: ProjectionId(derive(b"projection", repo, revision)?),
        instance: IndexInstanceId(derive(b"instance", repo, revision)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_policy_reference_wire_keeps_closed_named_fields() {
        let reference = RetiredProjectionPolicyRefV1 {
            object: ObjectRef(Digest([1; 32])),
            sha256: Digest([2; 32]),
            identity_count: 7,
        };
        let expected = format!(
            r#"{{"object":[{}],"sha256":[{}],"identity_count":7}}"#,
            ["1"; 32].join(","),
            ["2"; 32].join(","),
        );
        assert_eq!(
            serde_json::to_string(&reference).expect("reference"),
            expected
        );
        assert_eq!(
            serde_json::from_str::<RetiredProjectionPolicyRefV1>(&expected).expect("reference"),
            reference
        );
        for suffix in [r#", "identity_count": 7}"#, r#", "unknown": 1}"#] {
            let invalid = format!("{}{suffix}", expected.trim_end_matches('}'));
            assert!(serde_json::from_str::<RetiredProjectionPolicyRefV1>(&invalid).is_err());
        }
    }

    fn fixture() -> (
        RetiredProjectionPolicyHeaderV1,
        [RetiredProjectionIdentityV1; 2],
    ) {
        (
            RetiredProjectionPolicyHeaderV1 {
                format_version: 1,
                old_selection_sha256: Digest([1; 32]),
                old_root_nonce: Digest([2; 32]),
                old_routes_ref: Some(ObjectRef(Digest([3; 32]))),
                old_source_head_count: 0,
                candidate_root_nonce: Digest([4; 32]),
                candidate_config_sha256: Digest([5; 32]),
                candidate_profile_ref: ObjectRef(Digest([6; 32])),
            },
            [
                RetiredProjectionIdentityV1 {
                    projection: ProjectionId([1; 16]),
                    instance: IndexInstanceId([3; 16]),
                },
                RetiredProjectionIdentityV1 {
                    projection: ProjectionId([2; 16]),
                    instance: IndexInstanceId([4; 16]),
                },
            ],
        )
    }

    #[test]
    fn canonical_body_refuses_missing_duplicate_and_trailing_identity_v1() {
        let (header, mut identities) = fixture();
        let mut bytes = Vec::new();
        let mut scratch = [IndexInstanceId([0; 16]); 2];
        let mut checkpoint = || Ok::<(), ()>(());
        let digest = encode_retired_projection_policy_v1(
            header,
            &identities,
            &mut scratch,
            &mut checkpoint,
            &mut bytes,
        )
        .unwrap();
        let view =
            decode_retired_projection_policy_v1(&bytes, &mut scratch, &mut checkpoint).unwrap();
        assert_eq!(view.header_v1(), header);
        assert!(view.rejects_v1(ProjectionId([1; 16]), IndexInstanceId([0; 16]), &mut checkpoint).unwrap());
        assert!(view.rejects_v1(ProjectionId([0; 16]), IndexInstanceId([4; 16]), &mut checkpoint).unwrap());
        assert!(!view.rejects_v1(ProjectionId([9; 16]), IndexInstanceId([9; 16]), &mut checkpoint).unwrap());
        assert_eq!(view.identity_v1(usize::MAX / IDENTITY_BYTES_V1), None);
        assert_eq!(view.identity_v1(usize::MAX), None);
        assert_eq!(digest, Digest(Sha256::digest(&bytes).into()));
        bytes.push(0);
        assert!(
            decode_retired_projection_policy_v1(&bytes, &mut scratch, &mut checkpoint).is_err()
        );
        bytes[HEADER_BYTES_V1 - 4..HEADER_BYTES_V1]
            .copy_from_slice(&((MAX_RETIRED_PROJECTION_IDENTITIES_V1 as u32) + 1).to_le_bytes());
        assert!(preflight_retired_projection_policy_v1(&bytes).is_err());
        identities[1].instance = identities[0].instance;
        assert!(
            encode_retired_projection_policy_v1(
                header,
                &identities,
                &mut scratch,
                &mut checkpoint,
                io::sink()
            )
            .is_err()
        );
    }

    #[test]
    fn source_only_predecessor_seals_an_exact_empty_retired_census_v1() {
        let (mut header, _) = fixture();
        header.old_source_head_count = 1;
        let mut bytes = Vec::new();
        let mut scratch = [];
        let mut checkpoint = || Ok::<(), ()>(());
        let digest = encode_retired_projection_policy_v1(
            header,
            &[],
            &mut scratch,
            &mut checkpoint,
            &mut bytes,
        )
        .unwrap();
        assert_eq!(preflight_retired_projection_policy_v1(&bytes), Ok(0));
        let view = decode_retired_projection_policy_v1(&bytes, &mut scratch, &mut checkpoint)
            .unwrap();
        assert_eq!(view.header_v1(), header);
        assert_eq!(view.len_v1(), 0);
        assert_eq!(digest, Digest(Sha256::digest(&bytes).into()));
        assert!(!view.rejects_v1(ProjectionId([1; 16]), IndexInstanceId([2; 16]), &mut checkpoint).unwrap());
        let reference = RetiredProjectionPolicyRefV1 {
            object: ObjectRef(Digest([9; 32])),
            sha256: digest,
            identity_count: 0,
        };
        assert_eq!(reference.matches_body_v1(&bytes, &mut scratch, &mut checkpoint).unwrap(), true);
        let mut changed_candidate_config = bytes.clone();
        let candidate_config_at = MAGIC_V1.len() + 4 + 32 * 3 + 4 + 32;
        changed_candidate_config[candidate_config_at] ^= 1;
        assert_eq!(reference.matches_body_v1(&changed_candidate_config, &mut scratch, &mut checkpoint).unwrap(), false);
        let mut missing_old_route = header;
        missing_old_route.old_routes_ref = None;
        assert!(encode_retired_projection_policy_v1(
            missing_old_route,
            &[],
            &mut scratch,
            &mut checkpoint,
            io::sink(),
        ).is_err());
    }

    #[test]
    fn historical_semantica_route_pair_matches_fixed_golden_v1() {
        let repo = RepoId::new("repo:test").unwrap();
        let revision = RevisionId::new("revision:test").unwrap();
        let pair = retired_repomap_route_identity_v1(&repo, &revision).unwrap();
        assert_eq!(
            pair.projection.0,
            [
                0x31, 0xfa, 0x1a, 0x2b, 0xf2, 0x00, 0xe6, 0x8d, 0xe8, 0x45, 0xeb, 0x3d,
                0x94, 0xb9, 0x1e, 0x8f,
            ]
        );
        assert_eq!(
            pair.instance.0,
            [
                0x06, 0x99, 0xfb, 0x51, 0xf3, 0xda, 0xb0, 0xe2, 0x5f, 0x10, 0x63, 0xd2,
                0x85, 0xb7, 0x57, 0x09,
            ]
        );
    }
}
