use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::bounded_cluster_members::{BoundedClusterMembersV1, BoundedVecV1};
use crate::canonical_order::{CanonicalOrderBreakV1, first_canonical_order_break_v1};
use crate::{GenerationPin, MAX_CLUSTER_MEMBERSHIP_BATCH_ITEMS_V1, SymbolId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClusterMembershipCompletenessV1 {
    Complete,
    Truncated,
}

impl ClusterMembershipCompletenessV1 {
    const VARIANTS: &'static [&'static str] = &["Complete", "Truncated"];

    const fn as_code_str(self) -> &'static str {
        match self {
            Self::Complete => "Complete",
            Self::Truncated => "Truncated",
        }
    }
}

impl Serialize for ClusterMembershipCompletenessV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

impl<'de> Deserialize<'de> for ClusterMembershipCompletenessV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct EnumVisitor;
        impl Visitor<'_> for EnumVisitor {
            type Value = ClusterMembershipCompletenessV1;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a ClusterMembershipCompletenessV1 string")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match value {
                    "Complete" => Ok(ClusterMembershipCompletenessV1::Complete),
                    "Truncated" => Ok(ClusterMembershipCompletenessV1::Truncated),
                    other => Err(de::Error::unknown_variant(
                        other,
                        ClusterMembershipCompletenessV1::VARIANTS,
                    )),
                }
            }
        }
        deserializer.deserialize_str(EnumVisitor)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClusterMembershipReadFailureV1 {
    CurrentGenerationMissing,
    CorruptSidecar,
    ClusterIdentityMismatch,
    GenerationMismatch,
    AuthorityDigestMismatch,
    EmptyMembership,
    EmptyMemberIdentity,
    DuplicateMemberIdentity,
    NonCanonicalMemberOrder,
    MemberLimitExceeded,
}

impl ClusterMembershipReadFailureV1 {
    const VARIANTS: &'static [&'static str] = &[
        "CurrentGenerationMissing",
        "CorruptSidecar",
        "ClusterIdentityMismatch",
        "GenerationMismatch",
        "AuthorityDigestMismatch",
        "EmptyMembership",
        "EmptyMemberIdentity",
        "DuplicateMemberIdentity",
        "NonCanonicalMemberOrder",
        "MemberLimitExceeded",
    ];

    const fn as_code_str(self) -> &'static str {
        match self {
            Self::CurrentGenerationMissing => "CurrentGenerationMissing",
            Self::CorruptSidecar => "CorruptSidecar",
            Self::ClusterIdentityMismatch => "ClusterIdentityMismatch",
            Self::GenerationMismatch => "GenerationMismatch",
            Self::AuthorityDigestMismatch => "AuthorityDigestMismatch",
            Self::EmptyMembership => "EmptyMembership",
            Self::EmptyMemberIdentity => "EmptyMemberIdentity",
            Self::DuplicateMemberIdentity => "DuplicateMemberIdentity",
            Self::NonCanonicalMemberOrder => "NonCanonicalMemberOrder",
            Self::MemberLimitExceeded => "MemberLimitExceeded",
        }
    }
}

impl fmt::Display for ClusterMembershipReadFailureV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_code_str())
    }
}

impl Serialize for ClusterMembershipReadFailureV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

impl<'de> Deserialize<'de> for ClusterMembershipReadFailureV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct EnumVisitor;
        impl Visitor<'_> for EnumVisitor {
            type Value = ClusterMembershipReadFailureV1;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a ClusterMembershipReadFailureV1 string")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match value {
                    "CurrentGenerationMissing" => {
                        Ok(ClusterMembershipReadFailureV1::CurrentGenerationMissing)
                    }
                    "CorruptSidecar" => Ok(ClusterMembershipReadFailureV1::CorruptSidecar),
                    "ClusterIdentityMismatch" => {
                        Ok(ClusterMembershipReadFailureV1::ClusterIdentityMismatch)
                    }
                    "GenerationMismatch" => Ok(ClusterMembershipReadFailureV1::GenerationMismatch),
                    "AuthorityDigestMismatch" => {
                        Ok(ClusterMembershipReadFailureV1::AuthorityDigestMismatch)
                    }
                    "EmptyMembership" => Ok(ClusterMembershipReadFailureV1::EmptyMembership),
                    "EmptyMemberIdentity" => {
                        Ok(ClusterMembershipReadFailureV1::EmptyMemberIdentity)
                    }
                    "DuplicateMemberIdentity" => {
                        Ok(ClusterMembershipReadFailureV1::DuplicateMemberIdentity)
                    }
                    "NonCanonicalMemberOrder" => {
                        Ok(ClusterMembershipReadFailureV1::NonCanonicalMemberOrder)
                    }
                    "MemberLimitExceeded" => {
                        Ok(ClusterMembershipReadFailureV1::MemberLimitExceeded)
                    }
                    other => Err(de::Error::unknown_variant(
                        other,
                        ClusterMembershipReadFailureV1::VARIANTS,
                    )),
                }
            }
        }
        deserializer.deserialize_str(EnumVisitor)
    }
}

/// Exact structured `ClusterCard` membership for one sealed generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClusterMembershipSnapshotV1 {
    pub cluster_record_id: String,
    pub generation: GenerationPin,
    pub authority_digest: String,
    pub members: Vec<SymbolId>,
    pub completeness: ClusterMembershipCompletenessV1,
}

impl ClusterMembershipSnapshotV1 {
    pub fn validate_v1(&self) -> Result<(), ClusterMembershipReadFailureV1> {
        if self.cluster_record_id.is_empty() {
            return Err(ClusterMembershipReadFailureV1::ClusterIdentityMismatch);
        }
        if self.authority_digest.is_empty() {
            return Err(ClusterMembershipReadFailureV1::AuthorityDigestMismatch);
        }
        if self.members.is_empty() {
            return Err(ClusterMembershipReadFailureV1::EmptyMembership);
        }
        let Ok(member_count) = u32::try_from(self.members.len()) else {
            return Err(ClusterMembershipReadFailureV1::MemberLimitExceeded);
        };
        if member_count > crate::MAX_CLUSTER_MEMBERSHIP_READ_V1 {
            return Err(ClusterMembershipReadFailureV1::MemberLimitExceeded);
        }
        for member in &self.members {
            if member.as_str().is_empty() {
                return Err(ClusterMembershipReadFailureV1::EmptyMemberIdentity);
            }
        }
        match first_canonical_order_break_v1(&self.members, |member| member.as_str()) {
            Some(CanonicalOrderBreakV1::Duplicate) => {
                Err(ClusterMembershipReadFailureV1::DuplicateMemberIdentity)
            }
            Some(CanonicalOrderBreakV1::OutOfOrder) => {
                Err(ClusterMembershipReadFailureV1::NonCanonicalMemberOrder)
            }
            None => Ok(()),
        }
    }

    /// Revalidates the response authority against the exact request before a
    /// caller admits any member into a downstream graph/source lookup.
    pub fn validate_against_v1(
        &self,
        request: &crate::ClusterMembershipReadRequestV1,
    ) -> Result<(), ClusterMembershipReadFailureV1> {
        self.validate_v1()?;
        if self.cluster_record_id != request.cluster_record_id {
            return Err(ClusterMembershipReadFailureV1::ClusterIdentityMismatch);
        }
        if self.generation != request.generation {
            return Err(ClusterMembershipReadFailureV1::GenerationMismatch);
        }
        if self.authority_digest != request.expected_authority_digest {
            return Err(ClusterMembershipReadFailureV1::AuthorityDigestMismatch);
        }
        let Ok(member_count) = u32::try_from(self.members.len()) else {
            return Err(ClusterMembershipReadFailureV1::MemberLimitExceeded);
        };
        if member_count > request.limit {
            return Err(ClusterMembershipReadFailureV1::MemberLimitExceeded);
        }
        if self.completeness == ClusterMembershipCompletenessV1::Truncated
            && member_count != request.limit
        {
            return Err(ClusterMembershipReadFailureV1::MemberLimitExceeded);
        }
        Ok(())
    }
}

const CLUSTER_MEMBERSHIP_SNAPSHOT_V1_FIELDS: &[&str] = &[
    "cluster_record_id",
    "generation",
    "authority_digest",
    "members",
    "completeness",
];

impl Serialize for ClusterMembershipSnapshotV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate_v1().map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("ClusterMembershipSnapshotV1", 5)?;
        state.serialize_field("cluster_record_id", &self.cluster_record_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("authority_digest", &self.authority_digest)?;
        state.serialize_field("members", &self.members)?;
        state.serialize_field("completeness", &self.completeness)?;
        state.end()
    }
}

struct ClusterMembershipSnapshotV1Visitor;

impl<'de> Visitor<'de> for ClusterMembershipSnapshotV1Visitor {
    type Value = ClusterMembershipSnapshotV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a ClusterMembershipSnapshotV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut cluster_record_id = None;
        let mut generation = None;
        let mut authority_digest = None;
        let mut members: Option<BoundedClusterMembersV1> = None;
        let mut completeness = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "cluster_record_id" => {
                    set_once_v1(&mut cluster_record_id, "cluster_record_id", &mut map)?;
                }
                "generation" => set_once_v1(&mut generation, "generation", &mut map)?,
                "authority_digest" => {
                    set_once_v1(&mut authority_digest, "authority_digest", &mut map)?;
                }
                "members" => set_once_v1(&mut members, "members", &mut map)?,
                "completeness" => set_once_v1(&mut completeness, "completeness", &mut map)?,
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        CLUSTER_MEMBERSHIP_SNAPSHOT_V1_FIELDS,
                    ));
                }
            }
        }
        let snapshot = ClusterMembershipSnapshotV1 {
            cluster_record_id: cluster_record_id
                .ok_or_else(|| de::Error::missing_field("cluster_record_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            authority_digest: authority_digest
                .ok_or_else(|| de::Error::missing_field("authority_digest"))?,
            members: members
                .ok_or_else(|| de::Error::missing_field("members"))?
                .into_inner(),
            completeness: completeness.ok_or_else(|| de::Error::missing_field("completeness"))?,
        };
        snapshot.validate_v1().map_err(de::Error::custom)?;
        Ok(snapshot)
    }
}

impl<'de> Deserialize<'de> for ClusterMembershipSnapshotV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ClusterMembershipSnapshotV1",
            CLUSTER_MEMBERSHIP_SNAPSHOT_V1_FIELDS,
            ClusterMembershipSnapshotV1Visitor,
        )
    }
}

fn set_once_v1<'de, A, T>(
    slot: &mut Option<T>,
    field: &'static str,
    map: &mut A,
) -> Result<(), A::Error>
where
    A: MapAccess<'de>,
    T: Deserialize<'de>,
{
    if slot.is_some() {
        return Err(de::Error::duplicate_field(field));
    }
    *slot = Some(map.next_value()?);
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClusterMembershipReadRejectionV1 {
    pub cluster_record_id: String,
    pub generation: GenerationPin,
    pub expected_authority_digest: String,
    pub failure: ClusterMembershipReadFailureV1,
}

impl ClusterMembershipReadRejectionV1 {
    pub fn validate_v1(&self) -> Result<(), ClusterMembershipReadFailureV1> {
        validate_cluster_membership_terminal_authority_v1(
            self.cluster_record_id.as_str(),
            self.expected_authority_digest.as_str(),
        )
    }
}

fn validate_cluster_membership_terminal_authority_v1(
    cluster_record_id: &str,
    authority_digest: &str,
) -> Result<(), ClusterMembershipReadFailureV1> {
    if cluster_record_id.is_empty() {
        return Err(ClusterMembershipReadFailureV1::ClusterIdentityMismatch);
    }
    if authority_digest.is_empty() {
        return Err(ClusterMembershipReadFailureV1::AuthorityDigestMismatch);
    }
    Ok(())
}

fn validate_cluster_membership_authority_fields_v1(
    cluster_record_id: &str,
    authority_digest: &str,
) -> Result<(), &'static str> {
    if cluster_record_id.is_empty() {
        return Err("cluster membership cluster_record_id must not be empty");
    }
    if authority_digest.is_empty() {
        return Err("cluster membership authority digest must not be empty");
    }
    Ok(())
}

macro_rules! impl_cluster_membership_authority_payload_serde {
    ($ty:ident, $visitor:ident, $fields:ident, $failure:ident) => {
        impl Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                validate_cluster_membership_authority_fields_v1(
                    self.cluster_record_id.as_str(),
                    self.expected_authority_digest.as_str(),
                )
                .map_err(serde::ser::Error::custom)?;
                let mut state = serializer.serialize_struct(stringify!($ty), 4)?;
                state.serialize_field("cluster_record_id", &self.cluster_record_id)?;
                state.serialize_field("generation", &self.generation)?;
                state.serialize_field(
                    "expected_authority_digest",
                    &self.expected_authority_digest,
                )?;
                state.serialize_field("failure", &self.$failure)?;
                state.end()
            }
        }

        struct $visitor;
        impl<'de> Visitor<'de> for $visitor {
            type Value = $ty;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!("a ", stringify!($ty), " map"))
            }
            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut cluster_record_id = None;
                let mut generation = None;
                let mut expected_authority_digest = None;
                let mut $failure = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "cluster_record_id" => {
                            set_once_v1(&mut cluster_record_id, "cluster_record_id", &mut map)?
                        }
                        "generation" => set_once_v1(&mut generation, "generation", &mut map)?,
                        "expected_authority_digest" => set_once_v1(
                            &mut expected_authority_digest,
                            "expected_authority_digest",
                            &mut map,
                        )?,
                        "failure" => set_once_v1(&mut $failure, "failure", &mut map)?,
                        other => return Err(de::Error::unknown_field(other, $fields)),
                    }
                }
                let cluster_record_id: String = cluster_record_id
                    .ok_or_else(|| de::Error::missing_field("cluster_record_id"))?;
                let expected_authority_digest: String = expected_authority_digest
                    .ok_or_else(|| de::Error::missing_field("expected_authority_digest"))?;
                validate_cluster_membership_authority_fields_v1(
                    cluster_record_id.as_str(),
                    expected_authority_digest.as_str(),
                )
                .map_err(de::Error::custom)?;
                Ok($ty {
                    cluster_record_id,
                    generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
                    expected_authority_digest,
                    $failure: $failure.ok_or_else(|| de::Error::missing_field("failure"))?,
                })
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_struct(stringify!($ty), $fields, $visitor)
            }
        }
    };
}

const CLUSTER_MEMBERSHIP_READ_REJECTION_V1_FIELDS: &[&str] = &[
    "cluster_record_id",
    "generation",
    "expected_authority_digest",
    "failure",
];
impl_cluster_membership_authority_payload_serde!(
    ClusterMembershipReadRejectionV1,
    ClusterMembershipReadRejectionV1Visitor,
    CLUSTER_MEMBERSHIP_READ_REJECTION_V1_FIELDS,
    failure
);

/// Typed result of a membership read.
///
/// Current-format missing, stale, or invalid data is always `Rejected`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClusterMembershipReadOutcomeV1 {
    Available(ClusterMembershipSnapshotV1),
    Rejected(ClusterMembershipReadRejectionV1),
}

impl ClusterMembershipReadOutcomeV1 {
    pub fn validate_v1(&self) -> Result<(), ClusterMembershipReadFailureV1> {
        match self {
            Self::Available(snapshot) => snapshot.validate_v1(),
            Self::Rejected(rejection) => rejection.validate_v1(),
        }
    }

    pub fn validate_against_v1(
        &self,
        request: &crate::ClusterMembershipReadRequestV1,
    ) -> Result<(), ClusterMembershipReadFailureV1> {
        match self {
            Self::Available(snapshot) => snapshot.validate_against_v1(request),
            Self::Rejected(rejection) => {
                rejection.validate_v1()?;
                validate_outcome_authority_v1(
                    rejection.cluster_record_id.as_str(),
                    &rejection.generation,
                    rejection.expected_authority_digest.as_str(),
                    request,
                )
            }
        }
    }
}

/// Ordered response for one bounded membership batch.
///
/// The vector position is authoritative: decoders and SDK callers revalidate
/// every entry against the request at the same position before returning any
/// outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClusterMembershipBatchReadResponseV1 {
    pub outcomes: Vec<ClusterMembershipReadOutcomeV1>,
}

impl ClusterMembershipBatchReadResponseV1 {
    fn validate_wire_shape_v1(&self) -> Result<(), &'static str> {
        if self.outcomes.is_empty() {
            return Err("cluster membership batch response must not be empty");
        }
        if self.outcomes.len() > MAX_CLUSTER_MEMBERSHIP_BATCH_ITEMS_V1 {
            return Err("cluster membership batch response exceeds the bounded item count");
        }
        Ok(())
    }

    pub fn validate_against_v1(
        &self,
        request: &crate::ClusterMembershipBatchReadRequestV1,
    ) -> Result<(), ClusterMembershipReadFailureV1> {
        if self.outcomes.len() != request.items.len() {
            return Err(ClusterMembershipReadFailureV1::ClusterIdentityMismatch);
        }
        for (outcome, item) in self.outcomes.iter().zip(&request.items) {
            outcome.validate_against_v1(&item.as_single_request_v1(&request.generation))?;
        }
        Ok(())
    }
}

const CLUSTER_MEMBERSHIP_BATCH_READ_RESPONSE_V1_FIELDS: &[&str] = &["outcomes"];

impl Serialize for ClusterMembershipBatchReadResponseV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate_wire_shape_v1()
            .map_err(serde::ser::Error::custom)?;
        for outcome in &self.outcomes {
            outcome.validate_v1().map_err(serde::ser::Error::custom)?;
        }
        let mut state = serializer.serialize_struct("ClusterMembershipBatchReadResponseV1", 1)?;
        state.serialize_field("outcomes", &self.outcomes)?;
        state.end()
    }
}

struct ClusterMembershipBatchReadResponseV1Visitor;

impl<'de> Visitor<'de> for ClusterMembershipBatchReadResponseV1Visitor {
    type Value = ClusterMembershipBatchReadResponseV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a ClusterMembershipBatchReadResponseV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut outcomes: Option<
            BoundedVecV1<ClusterMembershipReadOutcomeV1, MAX_CLUSTER_MEMBERSHIP_BATCH_ITEMS_V1>,
        > = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "outcomes" => set_once_v1(&mut outcomes, "outcomes", &mut map)?,
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        CLUSTER_MEMBERSHIP_BATCH_READ_RESPONSE_V1_FIELDS,
                    ));
                }
            }
        }
        let response = ClusterMembershipBatchReadResponseV1 {
            outcomes: outcomes
                .ok_or_else(|| de::Error::missing_field("outcomes"))?
                .into_inner(),
        };
        response
            .validate_wire_shape_v1()
            .map_err(de::Error::custom)?;
        for outcome in &response.outcomes {
            outcome.validate_v1().map_err(de::Error::custom)?;
        }
        Ok(response)
    }
}

impl<'de> Deserialize<'de> for ClusterMembershipBatchReadResponseV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ClusterMembershipBatchReadResponseV1",
            CLUSTER_MEMBERSHIP_BATCH_READ_RESPONSE_V1_FIELDS,
            ClusterMembershipBatchReadResponseV1Visitor,
        )
    }
}

fn validate_outcome_authority_v1(
    cluster_record_id: &str,
    generation: &GenerationPin,
    authority_digest: &str,
    request: &crate::ClusterMembershipReadRequestV1,
) -> Result<(), ClusterMembershipReadFailureV1> {
    if cluster_record_id != request.cluster_record_id {
        return Err(ClusterMembershipReadFailureV1::ClusterIdentityMismatch);
    }
    if generation != &request.generation {
        return Err(ClusterMembershipReadFailureV1::GenerationMismatch);
    }
    if authority_digest != request.expected_authority_digest {
        return Err(ClusterMembershipReadFailureV1::AuthorityDigestMismatch);
    }
    Ok(())
}

const CLUSTER_MEMBERSHIP_READ_OUTCOME_V1_FIELDS: &[&str] = &["kind", "payload"];
const CLUSTER_MEMBERSHIP_READ_OUTCOME_V1_VARIANTS: &[&str] = &["Available", "Rejected"];

impl Serialize for ClusterMembershipReadOutcomeV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ClusterMembershipReadOutcomeV1", 2)?;
        match self {
            Self::Available(payload) => {
                state.serialize_field("kind", "Available")?;
                state.serialize_field("payload", payload)?;
            }
            Self::Rejected(payload) => {
                state.serialize_field("kind", "Rejected")?;
                state.serialize_field("payload", payload)?;
            }
        }
        state.end()
    }
}

struct ClusterMembershipReadOutcomeV1Visitor;

struct ClusterMembershipOutcomePayloadBufferV1 {
    cluster_record_id: Option<String>,
    generation: Option<GenerationPin>,
    authority_digest: Option<String>,
    expected_authority_digest: Option<String>,
    members: Option<BoundedClusterMembersV1>,
    completeness: Option<ClusterMembershipCompletenessV1>,
    failure: Option<ClusterMembershipReadFailureV1>,
}

const CLUSTER_MEMBERSHIP_OUTCOME_PAYLOAD_FIELDS: &[&str] = &[
    "cluster_record_id",
    "generation",
    "authority_digest",
    "expected_authority_digest",
    "members",
    "completeness",
    "failure",
];

struct ClusterMembershipOutcomePayloadBufferV1Visitor;
impl<'de> Visitor<'de> for ClusterMembershipOutcomePayloadBufferV1Visitor {
    type Value = ClusterMembershipOutcomePayloadBufferV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a buffered ClusterMembershipReadOutcomeV1 payload map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut payload = ClusterMembershipOutcomePayloadBufferV1 {
            cluster_record_id: None,
            generation: None,
            authority_digest: None,
            expected_authority_digest: None,
            members: None,
            completeness: None,
            failure: None,
        };
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "cluster_record_id" => set_once_v1(
                    &mut payload.cluster_record_id,
                    "cluster_record_id",
                    &mut map,
                )?,
                "generation" => set_once_v1(&mut payload.generation, "generation", &mut map)?,
                "authority_digest" => {
                    set_once_v1(&mut payload.authority_digest, "authority_digest", &mut map)?;
                }
                "expected_authority_digest" => set_once_v1(
                    &mut payload.expected_authority_digest,
                    "expected_authority_digest",
                    &mut map,
                )?,
                "members" => set_once_v1(&mut payload.members, "members", &mut map)?,
                "completeness" => set_once_v1(&mut payload.completeness, "completeness", &mut map)?,
                "failure" => set_once_v1(&mut payload.failure, "failure", &mut map)?,
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        CLUSTER_MEMBERSHIP_OUTCOME_PAYLOAD_FIELDS,
                    ));
                }
            }
        }
        Ok(payload)
    }
}

impl<'de> Deserialize<'de> for ClusterMembershipOutcomePayloadBufferV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ClusterMembershipReadOutcomeV1Payload",
            CLUSTER_MEMBERSHIP_OUTCOME_PAYLOAD_FIELDS,
            ClusterMembershipOutcomePayloadBufferV1Visitor,
        )
    }
}

impl<'de> Visitor<'de> for ClusterMembershipReadOutcomeV1Visitor {
    type Value = ClusterMembershipReadOutcomeV1;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a ClusterMembershipReadOutcomeV1 adjacent-tagged map")
    }
    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind: Option<String> = None;
        let mut payload: Option<ClusterMembershipOutcomePayloadBufferV1> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "kind" => set_once_v1(&mut kind, "kind", &mut map)?,
                "payload" => set_once_v1(&mut payload, "payload", &mut map)?,
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        CLUSTER_MEMBERSHIP_READ_OUTCOME_V1_FIELDS,
                    ));
                }
            }
        }
        let kind = kind.ok_or_else(|| de::Error::missing_field("kind"))?;
        if !CLUSTER_MEMBERSHIP_READ_OUTCOME_V1_VARIANTS.contains(&kind.as_str()) {
            return Err(de::Error::unknown_variant(
                kind.as_str(),
                CLUSTER_MEMBERSHIP_READ_OUTCOME_V1_VARIANTS,
            ));
        }
        let payload = payload.ok_or_else(|| de::Error::missing_field("payload"))?;
        let cluster_record_id = payload
            .cluster_record_id
            .ok_or_else(|| de::Error::missing_field("cluster_record_id"))?;
        let generation = payload
            .generation
            .ok_or_else(|| de::Error::missing_field("generation"))?;
        match kind.as_str() {
            "Available" => {
                if payload.expected_authority_digest.is_some() || payload.failure.is_some() {
                    return Err(de::Error::custom(
                        "Available cluster membership payload contains rejection-only fields",
                    ));
                }
                let snapshot = ClusterMembershipSnapshotV1 {
                    cluster_record_id,
                    generation,
                    authority_digest: payload
                        .authority_digest
                        .ok_or_else(|| de::Error::missing_field("authority_digest"))?,
                    members: payload
                        .members
                        .ok_or_else(|| de::Error::missing_field("members"))?
                        .into_inner(),
                    completeness: payload
                        .completeness
                        .ok_or_else(|| de::Error::missing_field("completeness"))?,
                };
                snapshot.validate_v1().map_err(de::Error::custom)?;
                Ok(ClusterMembershipReadOutcomeV1::Available(snapshot))
            }
            "Rejected" => {
                if payload.authority_digest.is_some()
                    || payload.members.is_some()
                    || payload.completeness.is_some()
                {
                    return Err(de::Error::custom(
                        "Rejected cluster membership payload contains available-only fields",
                    ));
                }
                let expected_authority_digest = payload
                    .expected_authority_digest
                    .ok_or_else(|| de::Error::missing_field("expected_authority_digest"))?;
                validate_cluster_membership_authority_fields_v1(
                    cluster_record_id.as_str(),
                    expected_authority_digest.as_str(),
                )
                .map_err(de::Error::custom)?;
                Ok(ClusterMembershipReadOutcomeV1::Rejected(
                    ClusterMembershipReadRejectionV1 {
                        cluster_record_id,
                        generation,
                        expected_authority_digest,
                        failure: payload
                            .failure
                            .ok_or_else(|| de::Error::missing_field("failure"))?,
                    },
                ))
            }
            other => Err(de::Error::unknown_variant(
                other,
                CLUSTER_MEMBERSHIP_READ_OUTCOME_V1_VARIANTS,
            )),
        }
    }
}

impl<'de> Deserialize<'de> for ClusterMembershipReadOutcomeV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ClusterMembershipReadOutcomeV1",
            CLUSTER_MEMBERSHIP_READ_OUTCOME_V1_FIELDS,
            ClusterMembershipReadOutcomeV1Visitor,
        )
    }
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "fixture rows are built in this module with known fixed lengths; an out-of-range index here is a test authoring bug that should fail loudly"
)]
mod tests {
    use super::*;
    use crate::{ManifestGeneration, RepoId, RevisionId};

    fn sample_generation_pin() -> GenerationPin {
        GenerationPin::new(
            RepoId::new("repo-seed").expect("static fixture ID satisfies canonical policy"),
            RevisionId::new("rev-seed").expect("static fixture ID satisfies canonical policy"),
            ManifestGeneration::new(7),
        )
    }

    fn sample_cluster_membership_snapshot() -> ClusterMembershipSnapshotV1 {
        ClusterMembershipSnapshotV1 {
            cluster_record_id: "cluster-card:auth-service".to_string(),
            generation: sample_generation_pin(),
            authority_digest: "cluster-authority-digest".to_string(),
            members: vec![
                SymbolId::new("symbol:auth::authenticate"),
                SymbolId::new("symbol:auth::authorize"),
            ],
            completeness: ClusterMembershipCompletenessV1::Complete,
        }
    }

    #[test]
    fn cluster_membership_outcome_roundtrip_preserves_generation_authority_and_completeness_v1() {
        let available =
            ClusterMembershipReadOutcomeV1::Available(sample_cluster_membership_snapshot());
        let rejected = ClusterMembershipReadOutcomeV1::Rejected(ClusterMembershipReadRejectionV1 {
            cluster_record_id: "cluster-card:missing".to_string(),
            generation: sample_generation_pin(),
            expected_authority_digest: "missing-authority-digest".to_string(),
            failure: ClusterMembershipReadFailureV1::CurrentGenerationMissing,
        });
        for outcome in [available, rejected] {
            let json = serde_json::to_value(&outcome).expect("outcome JSON encode");
            let decoded_json: ClusterMembershipReadOutcomeV1 =
                serde_json::from_value(json).expect("outcome JSON decode");
            assert_eq!(decoded_json, outcome);

            let mut cbor = Vec::new();
            ciborium::ser::into_writer(&outcome, &mut cbor).expect("outcome CBOR encode");
            let decoded_cbor: ClusterMembershipReadOutcomeV1 =
                ciborium::de::from_reader(cbor.as_slice()).expect("outcome CBOR decode");
            assert_eq!(decoded_cbor, outcome);
        }
    }

    #[test]
    fn cluster_membership_legacy_unavailable_outcome_is_refused() {
        let old = serde_json::json!({
            "kind": "Unavailable",
            "payload": {
                "cluster_record_id": "cluster-card:legacy",
                "generation": sample_generation_pin(),
                "expected_authority_digest": "legacy-authority-digest"
            }
        });
        assert!(serde_json::from_value::<ClusterMembershipReadOutcomeV1>(old.clone()).is_err());
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&old, &mut cbor).expect("old CBOR fixture encodes");
        assert!(
            ciborium::de::from_reader::<ClusterMembershipReadOutcomeV1, _>(cbor.as_slice())
                .is_err()
        );
    }

    #[test]
    fn cluster_membership_batch_response_decoder_bounds_outcomes_before_allocation_v1() {
        let outcome = serde_json::to_value(ClusterMembershipReadOutcomeV1::Available(
            sample_cluster_membership_snapshot(),
        ))
        .expect("valid outcome wire");
        let oversized = serde_json::json!({
            "outcomes": vec![outcome; MAX_CLUSTER_MEMBERSHIP_BATCH_ITEMS_V1 + 1]
        });
        assert!(serde_json::from_value::<ClusterMembershipBatchReadResponseV1>(oversized).is_err());
        assert!(
            serde_json::from_value::<ClusterMembershipBatchReadResponseV1>(
                serde_json::json!({"outcomes": []})
            )
            .is_err()
        );
    }

    #[test]
    fn cluster_membership_outcome_decoder_accepts_payload_before_kind_for_json_and_cbor_v1() {
        let outcome =
            ClusterMembershipReadOutcomeV1::Available(sample_cluster_membership_snapshot());
        let payload_json = serde_json::to_string(&sample_cluster_membership_snapshot())
            .expect("membership payload JSON");
        let reversed_json = format!(r#"{{"payload":{payload_json},"kind":"Available"}}"#);
        assert_eq!(
            serde_json::from_str::<ClusterMembershipReadOutcomeV1>(&reversed_json)
                .expect("payload-first JSON decode"),
            outcome
        );

        let mut encoded_value =
            ciborium::value::Value::serialized(&outcome).expect("outcome value encode");
        let ciborium::value::Value::Map(entries) = &mut encoded_value else {
            panic!("outcome must encode as a CBOR map");
        };
        entries.sort_by_key(|(key, _value)| {
            let payload_first = matches!(
                key,
                ciborium::value::Value::Text(name) if name == "payload"
            );
            u8::from(!payload_first)
        });
        let mut reversed_cbor = Vec::new();
        ciborium::ser::into_writer(&encoded_value, &mut reversed_cbor)
            .expect("payload-first CBOR encode");
        assert_eq!(
            ciborium::de::from_reader::<ClusterMembershipReadOutcomeV1, _>(
                reversed_cbor.as_slice()
            )
            .expect("payload-first CBOR decode"),
            outcome
        );
    }

    #[test]
    fn cluster_membership_outcome_common_authority_validation_rejects_forged_terminal_states_v1() {
        let request = crate::ClusterMembershipReadRequestV1 {
            cluster_record_id: "cluster-card:auth-service".to_string(),
            generation: sample_generation_pin(),
            expected_authority_digest: "cluster-authority-digest".to_string(),
            limit: 2,
        };
        let forged_identity =
            ClusterMembershipReadOutcomeV1::Rejected(ClusterMembershipReadRejectionV1 {
                cluster_record_id: "cluster-card:forged".to_string(),
                generation: request.generation.clone(),
                expected_authority_digest: request.expected_authority_digest.clone(),
                failure: ClusterMembershipReadFailureV1::ClusterIdentityMismatch,
            });
        assert_eq!(
            forged_identity.validate_against_v1(&request),
            Err(ClusterMembershipReadFailureV1::ClusterIdentityMismatch)
        );
        let forged_rejection =
            ClusterMembershipReadOutcomeV1::Rejected(ClusterMembershipReadRejectionV1 {
                cluster_record_id: request.cluster_record_id.clone(),
                generation: GenerationPin::new(
                    RepoId::new("repo-seed").expect("static fixture ID satisfies canonical policy"),
                    RevisionId::new("rev-seed")
                        .expect("static fixture ID satisfies canonical policy"),
                    ManifestGeneration::new(8),
                ),
                expected_authority_digest: request.expected_authority_digest.clone(),
                failure: ClusterMembershipReadFailureV1::CurrentGenerationMissing,
            });
        assert_eq!(
            forged_rejection.validate_against_v1(&request),
            Err(ClusterMembershipReadFailureV1::GenerationMismatch)
        );

        let malformed_identity =
            ClusterMembershipReadOutcomeV1::Rejected(ClusterMembershipReadRejectionV1 {
                cluster_record_id: String::new(),
                generation: request.generation.clone(),
                expected_authority_digest: request.expected_authority_digest.clone(),
                failure: ClusterMembershipReadFailureV1::ClusterIdentityMismatch,
            });
        assert_eq!(
            malformed_identity.validate_against_v1(&request),
            Err(ClusterMembershipReadFailureV1::ClusterIdentityMismatch)
        );

        let malformed_rejection =
            ClusterMembershipReadOutcomeV1::Rejected(ClusterMembershipReadRejectionV1 {
                cluster_record_id: request.cluster_record_id.clone(),
                generation: request.generation.clone(),
                expected_authority_digest: String::new(),
                failure: ClusterMembershipReadFailureV1::CorruptSidecar,
            });
        assert_eq!(
            malformed_rejection.validate_against_v1(&request),
            Err(ClusterMembershipReadFailureV1::AuthorityDigestMismatch)
        );
    }

    #[test]
    fn current_generation_missing_or_mismatched_membership_fails_closed_v1() {
        let missing = ClusterMembershipReadOutcomeV1::Rejected(ClusterMembershipReadRejectionV1 {
            cluster_record_id: "cluster-card:auth-service".to_string(),
            generation: sample_generation_pin(),
            expected_authority_digest: "cluster-authority-digest".to_string(),
            failure: ClusterMembershipReadFailureV1::CurrentGenerationMissing,
        });
        let value = serde_json::to_value(&missing).expect("typed missing outcome");
        assert_eq!(
            value.pointer("/kind").and_then(serde_json::Value::as_str),
            Some("Rejected")
        );
        assert_eq!(
            value
                .pointer("/payload/failure")
                .and_then(serde_json::Value::as_str),
            Some("CurrentGenerationMissing")
        );

        let mut duplicate = sample_cluster_membership_snapshot();
        duplicate.members[1] = duplicate.members[0].clone();
        assert_eq!(
            duplicate.validate_v1(),
            Err(ClusterMembershipReadFailureV1::DuplicateMemberIdentity)
        );
        assert!(serde_json::to_value(&duplicate).is_err());

        let mut unordered = sample_cluster_membership_snapshot();
        unordered.members.reverse();
        assert_eq!(
            unordered.validate_v1(),
            Err(ClusterMembershipReadFailureV1::NonCanonicalMemberOrder)
        );
        assert!(serde_json::to_value(&unordered).is_err());

        let request = crate::ClusterMembershipReadRequestV1 {
            cluster_record_id: "cluster-card:auth-service".to_string(),
            generation: sample_generation_pin(),
            expected_authority_digest: "cluster-authority-digest".to_string(),
            limit: 2,
        };
        let snapshot = sample_cluster_membership_snapshot();
        assert!(snapshot.validate_against_v1(&request).is_ok());

        let mut stale = snapshot.clone();
        stale.authority_digest = "stale-authority-digest".to_string();
        assert_eq!(
            stale.validate_against_v1(&request),
            Err(ClusterMembershipReadFailureV1::AuthorityDigestMismatch)
        );

        let mut wrong_generation = snapshot;
        wrong_generation.generation = GenerationPin::new(
            RepoId::new("repo-seed").expect("static fixture ID satisfies canonical policy"),
            RevisionId::new("rev-seed").expect("static fixture ID satisfies canonical policy"),
            ManifestGeneration::new(8),
        );
        assert_eq!(
            wrong_generation.validate_against_v1(&request),
            Err(ClusterMembershipReadFailureV1::GenerationMismatch)
        );

        let duplicate_wire = serde_json::json!({
            "cluster_record_id": "cluster-card:auth-service",
            "generation": {
                "repo_id": "repo-seed",
                "revision_id": "rev-seed",
                "manifest_generation": 7
            },
            "authority_digest": "cluster-authority-digest",
            "members": ["symbol:auth::authenticate", "symbol:auth::authenticate"],
            "completeness": "Complete"
        });
        assert!(
            serde_json::from_value::<ClusterMembershipSnapshotV1>(duplicate_wire).is_err(),
            "duplicate membership wire must fail before reaching a consumer"
        );

        let mut unknown_wire =
            serde_json::to_value(sample_cluster_membership_snapshot()).expect("snapshot wire");
        assert!(
            unknown_wire
                .as_object_mut()
                .expect("snapshot object")
                .insert("fallback_members".to_string(), serde_json::json!([]))
                .is_none(),
            "negative fixture must add rather than replace the unknown field"
        );
        assert!(
            serde_json::from_value::<ClusterMembershipSnapshotV1>(unknown_wire).is_err(),
            "unknown membership fields must fail closed"
        );

        let oversized_members = (0..=crate::MAX_CLUSTER_MEMBERSHIP_READ_V1)
            .map(|index| serde_json::Value::String(format!("symbol:{index:04}")))
            .collect::<Vec<_>>();
        let oversized_outcome = serde_json::json!({
            "kind": "Available",
            "payload": {
                "cluster_record_id": "cluster-card:auth-service",
                "generation": {
                    "repo_id": "repo-seed",
                    "revision_id": "rev-seed",
                    "manifest_generation": 7
                },
                "authority_digest": "cluster-authority-digest",
                "members": oversized_members,
                "completeness": "Complete"
            }
        });
        assert!(
            serde_json::from_value::<ClusterMembershipReadOutcomeV1>(oversized_outcome).is_err(),
            "oversized outcome must fail in the bounded member visitor"
        );
    }
}
