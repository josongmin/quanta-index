use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use super::GenerationPin;

/// Hard upper bound for one structured ClusterCard membership read.
///
/// The limit is enforced by the contract decoder before any storage access.
pub const MAX_CLUSTER_MEMBERSHIP_READ_V1: u32 = 4_096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClusterMembershipReadPolicyErrorV1 {
    EmptyClusterRecordId,
    EmptyExpectedAuthorityDigest,
    InvalidLimit { limit: u32, max: u32 },
}

impl fmt::Display for ClusterMembershipReadPolicyErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyClusterRecordId => {
                formatter.write_str("cluster_record_id must not be empty")
            }
            Self::EmptyExpectedAuthorityDigest => {
                formatter.write_str("expected_authority_digest must not be empty")
            }
            Self::InvalidLimit { limit, max } => {
                write!(
                    formatter,
                    "cluster membership read limit must be in 1..={max}; got {limit}"
                )
            }
        }
    }
}

impl std::error::Error for ClusterMembershipReadPolicyErrorV1 {}

/// Generation-pinned structured membership read for one ClusterCard record.
///
/// `cluster_record_id` identifies the ClusterCard semantic-source record, not
/// rendered card text. `expected_authority_digest` binds the read to the exact
/// structured facts that produced that record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClusterMembershipReadRequestV1 {
    pub cluster_record_id: String,
    pub generation: GenerationPin,
    pub expected_authority_digest: String,
    pub limit: u32,
}

impl ClusterMembershipReadRequestV1 {
    pub fn validate_v1(&self) -> Result<(), ClusterMembershipReadPolicyErrorV1> {
        if self.cluster_record_id.is_empty() {
            return Err(ClusterMembershipReadPolicyErrorV1::EmptyClusterRecordId);
        }
        if self.expected_authority_digest.is_empty() {
            return Err(ClusterMembershipReadPolicyErrorV1::EmptyExpectedAuthorityDigest);
        }
        if self.limit == 0 || self.limit > MAX_CLUSTER_MEMBERSHIP_READ_V1 {
            return Err(ClusterMembershipReadPolicyErrorV1::InvalidLimit {
                limit: self.limit,
                max: MAX_CLUSTER_MEMBERSHIP_READ_V1,
            });
        }
        Ok(())
    }
}

const CLUSTER_MEMBERSHIP_READ_REQUEST_V1_FIELDS: &[&str] = &[
    "cluster_record_id",
    "generation",
    "expected_authority_digest",
    "limit",
];

impl Serialize for ClusterMembershipReadRequestV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate_v1().map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("ClusterMembershipReadRequestV1", 4)?;
        state.serialize_field("cluster_record_id", &self.cluster_record_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("expected_authority_digest", &self.expected_authority_digest)?;
        state.serialize_field("limit", &self.limit)?;
        state.end()
    }
}

struct ClusterMembershipReadRequestV1Visitor;

impl<'de> Visitor<'de> for ClusterMembershipReadRequestV1Visitor {
    type Value = ClusterMembershipReadRequestV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a ClusterMembershipReadRequestV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut cluster_record_id: Option<String> = None;
        let mut generation: Option<GenerationPin> = None;
        let mut expected_authority_digest: Option<String> = None;
        let mut limit: Option<u32> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "cluster_record_id" => {
                    if cluster_record_id.is_some() {
                        return Err(de::Error::duplicate_field("cluster_record_id"));
                    }
                    cluster_record_id = Some(map.next_value()?);
                }
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "expected_authority_digest" => {
                    if expected_authority_digest.is_some() {
                        return Err(de::Error::duplicate_field("expected_authority_digest"));
                    }
                    expected_authority_digest = Some(map.next_value()?);
                }
                "limit" => {
                    if limit.is_some() {
                        return Err(de::Error::duplicate_field("limit"));
                    }
                    limit = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        CLUSTER_MEMBERSHIP_READ_REQUEST_V1_FIELDS,
                    ));
                }
            }
        }
        let request = ClusterMembershipReadRequestV1 {
            cluster_record_id: cluster_record_id
                .ok_or_else(|| de::Error::missing_field("cluster_record_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            expected_authority_digest: expected_authority_digest
                .ok_or_else(|| de::Error::missing_field("expected_authority_digest"))?,
            limit: limit.ok_or_else(|| de::Error::missing_field("limit"))?,
        };
        request.validate_v1().map_err(de::Error::custom)?;
        Ok(request)
    }
}

impl<'de> Deserialize<'de> for ClusterMembershipReadRequestV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ClusterMembershipReadRequestV1",
            CLUSTER_MEMBERSHIP_READ_REQUEST_V1_FIELDS,
            ClusterMembershipReadRequestV1Visitor,
        )
    }
}

#[cfg(test)]
mod cluster_membership_request_tests {
    use super::*;
    use crate::{ManifestGeneration, RepoId, RevisionId};

    fn request(limit: u32) -> ClusterMembershipReadRequestV1 {
        ClusterMembershipReadRequestV1 {
            cluster_record_id: "cluster-card:auth-service".to_string(),
            generation: GenerationPin::new(
                RepoId::new("repo"),
                RevisionId::new("rev"),
                ManifestGeneration::new(17),
            ),
            expected_authority_digest: "authority-digest".to_string(),
            limit,
        }
    }

    #[test]
    fn cluster_membership_read_policy_rejects_zero_and_over_limit_bounds_v1() {
        assert!(matches!(
            request(0).validate_v1(),
            Err(ClusterMembershipReadPolicyErrorV1::InvalidLimit { limit: 0, .. })
        ));
        assert!(request(1).validate_v1().is_ok());
        assert!(
            request(MAX_CLUSTER_MEMBERSHIP_READ_V1)
                .validate_v1()
                .is_ok()
        );
        assert!(matches!(
            request(MAX_CLUSTER_MEMBERSHIP_READ_V1.saturating_add(1)).validate_v1(),
            Err(ClusterMembershipReadPolicyErrorV1::InvalidLimit { .. })
        ));
    }

    #[test]
    fn cluster_membership_read_wire_rejects_empty_authority_and_unknown_fields_v1() {
        let empty_authority = serde_json::json!({
            "cluster_record_id": "cluster-card:auth-service",
            "generation": {
                "repo_id": "repo",
                "revision_id": "rev",
                "manifest_generation": 17
            },
            "expected_authority_digest": "",
            "limit": 1
        });
        assert!(serde_json::from_value::<ClusterMembershipReadRequestV1>(empty_authority).is_err());

        let mut unknown = serde_json::to_value(request(1)).expect("valid request");
        assert!(
            unknown
                .as_object_mut()
                .expect("request object")
                .insert("fallback".to_string(), serde_json::Value::Bool(true))
                .is_none(),
            "negative fixture must add rather than replace the unknown field"
        );
        assert!(serde_json::from_value::<ClusterMembershipReadRequestV1>(unknown).is_err());
    }
}
