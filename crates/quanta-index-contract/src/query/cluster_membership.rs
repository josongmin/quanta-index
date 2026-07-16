use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use super::GenerationPin;
use crate::bounded_cluster_members::BoundedVecV1;

/// Hard upper bound for one structured ClusterCard membership read.
///
/// The limit is enforced by the contract decoder before any storage access.
pub const MAX_CLUSTER_MEMBERSHIP_READ_V1: u32 = 4_096;

/// Maximum number of ClusterCard records admitted by one transport read.
pub const MAX_CLUSTER_MEMBERSHIP_BATCH_ITEMS_V1: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClusterMembershipReadPolicyErrorV1 {
    EmptyClusterRecordId,
    EmptyExpectedAuthorityDigest,
    InvalidLimit { limit: u32, max: u32 },
    EmptyBatch,
    BatchTooLarge { items: usize, max: usize },
    DuplicateClusterRecordId,
    NonCanonicalClusterRecordOrder,
    TotalLimitExceeded { total: u32, max: u32 },
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
            Self::EmptyBatch => {
                formatter.write_str("cluster membership read batch must not be empty")
            }
            Self::BatchTooLarge { items, max } => write!(
                formatter,
                "cluster membership read batch has {items} items; maximum is {max}"
            ),
            Self::DuplicateClusterRecordId => {
                formatter.write_str("cluster membership read batch contains duplicate record ids")
            }
            Self::NonCanonicalClusterRecordOrder => formatter
                .write_str("cluster membership read batch record ids must be strictly increasing"),
            Self::TotalLimitExceeded { total, max } => write!(
                formatter,
                "cluster membership read batch total limit {total} exceeds maximum {max}"
            ),
        }
    }
}

/// Per-record authority inside one generation-pinned batch read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClusterMembershipBatchReadItemV1 {
    pub cluster_record_id: String,
    pub expected_authority_digest: String,
    pub limit: u32,
}

impl ClusterMembershipBatchReadItemV1 {
    pub fn as_single_request_v1(
        &self,
        generation: &GenerationPin,
    ) -> ClusterMembershipReadRequestV1 {
        ClusterMembershipReadRequestV1 {
            cluster_record_id: self.cluster_record_id.clone(),
            generation: generation.clone(),
            expected_authority_digest: self.expected_authority_digest.clone(),
            limit: self.limit,
        }
    }

    fn validate_v1(
        &self,
        generation: &GenerationPin,
    ) -> Result<(), ClusterMembershipReadPolicyErrorV1> {
        self.as_single_request_v1(generation).validate_v1()
    }
}

const CLUSTER_MEMBERSHIP_BATCH_READ_ITEM_V1_FIELDS: &[&str] =
    &["cluster_record_id", "expected_authority_digest", "limit"];

impl Serialize for ClusterMembershipBatchReadItemV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ClusterMembershipBatchReadItemV1", 3)?;
        state.serialize_field("cluster_record_id", &self.cluster_record_id)?;
        state.serialize_field("expected_authority_digest", &self.expected_authority_digest)?;
        state.serialize_field("limit", &self.limit)?;
        state.end()
    }
}

struct ClusterMembershipBatchReadItemV1Visitor;

impl<'de> Visitor<'de> for ClusterMembershipBatchReadItemV1Visitor {
    type Value = ClusterMembershipBatchReadItemV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a ClusterMembershipBatchReadItemV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut cluster_record_id = None;
        let mut expected_authority_digest = None;
        let mut limit = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "cluster_record_id" => {
                    if cluster_record_id.is_some() {
                        return Err(de::Error::duplicate_field("cluster_record_id"));
                    }
                    cluster_record_id = Some(map.next_value()?);
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
                        CLUSTER_MEMBERSHIP_BATCH_READ_ITEM_V1_FIELDS,
                    ));
                }
            }
        }
        Ok(ClusterMembershipBatchReadItemV1 {
            cluster_record_id: cluster_record_id
                .ok_or_else(|| de::Error::missing_field("cluster_record_id"))?,
            expected_authority_digest: expected_authority_digest
                .ok_or_else(|| de::Error::missing_field("expected_authority_digest"))?,
            limit: limit.ok_or_else(|| de::Error::missing_field("limit"))?,
        })
    }
}

impl<'de> Deserialize<'de> for ClusterMembershipBatchReadItemV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ClusterMembershipBatchReadItemV1",
            CLUSTER_MEMBERSHIP_BATCH_READ_ITEM_V1_FIELDS,
            ClusterMembershipBatchReadItemV1Visitor,
        )
    }
}

/// One bounded transport request for multiple ClusterCard records in one
/// sealed generation. Items are canonicalized by record id before transport.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClusterMembershipBatchReadRequestV1 {
    pub generation: GenerationPin,
    pub items: Vec<ClusterMembershipBatchReadItemV1>,
}

impl ClusterMembershipBatchReadRequestV1 {
    pub fn validate_v1(&self) -> Result<(), ClusterMembershipReadPolicyErrorV1> {
        if self.items.is_empty() {
            return Err(ClusterMembershipReadPolicyErrorV1::EmptyBatch);
        }
        if self.items.len() > MAX_CLUSTER_MEMBERSHIP_BATCH_ITEMS_V1 {
            return Err(ClusterMembershipReadPolicyErrorV1::BatchTooLarge {
                items: self.items.len(),
                max: MAX_CLUSTER_MEMBERSHIP_BATCH_ITEMS_V1,
            });
        }
        let mut total = 0u32;
        for item in &self.items {
            item.validate_v1(&self.generation)?;
            total = total.checked_add(item.limit).ok_or(
                ClusterMembershipReadPolicyErrorV1::TotalLimitExceeded {
                    total: u32::MAX,
                    max: MAX_CLUSTER_MEMBERSHIP_READ_V1,
                },
            )?;
        }
        if total > MAX_CLUSTER_MEMBERSHIP_READ_V1 {
            return Err(ClusterMembershipReadPolicyErrorV1::TotalLimitExceeded {
                total,
                max: MAX_CLUSTER_MEMBERSHIP_READ_V1,
            });
        }
        for pair in self.items.windows(2) {
            match pair[0].cluster_record_id.cmp(&pair[1].cluster_record_id) {
                core::cmp::Ordering::Equal => {
                    return Err(ClusterMembershipReadPolicyErrorV1::DuplicateClusterRecordId);
                }
                core::cmp::Ordering::Greater => {
                    return Err(ClusterMembershipReadPolicyErrorV1::NonCanonicalClusterRecordOrder);
                }
                core::cmp::Ordering::Less => {}
            }
        }
        Ok(())
    }

    pub fn single_v1(request: ClusterMembershipReadRequestV1) -> Self {
        Self {
            generation: request.generation,
            items: vec![ClusterMembershipBatchReadItemV1 {
                cluster_record_id: request.cluster_record_id,
                expected_authority_digest: request.expected_authority_digest,
                limit: request.limit,
            }],
        }
    }
}

const CLUSTER_MEMBERSHIP_BATCH_READ_REQUEST_V1_FIELDS: &[&str] = &["generation", "items"];

impl Serialize for ClusterMembershipBatchReadRequestV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate_v1().map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("ClusterMembershipBatchReadRequestV1", 2)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("items", &self.items)?;
        state.end()
    }
}

struct ClusterMembershipBatchReadRequestV1Visitor;

impl<'de> Visitor<'de> for ClusterMembershipBatchReadRequestV1Visitor {
    type Value = ClusterMembershipBatchReadRequestV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a ClusterMembershipBatchReadRequestV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut generation = None;
        let mut items: Option<
            BoundedVecV1<ClusterMembershipBatchReadItemV1, MAX_CLUSTER_MEMBERSHIP_BATCH_ITEMS_V1>,
        > = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "items" => {
                    if items.is_some() {
                        return Err(de::Error::duplicate_field("items"));
                    }
                    items = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        CLUSTER_MEMBERSHIP_BATCH_READ_REQUEST_V1_FIELDS,
                    ));
                }
            }
        }
        let request = ClusterMembershipBatchReadRequestV1 {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            items: items
                .ok_or_else(|| de::Error::missing_field("items"))?
                .into_inner(),
        };
        request.validate_v1().map_err(de::Error::custom)?;
        Ok(request)
    }
}

impl<'de> Deserialize<'de> for ClusterMembershipBatchReadRequestV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ClusterMembershipBatchReadRequestV1",
            CLUSTER_MEMBERSHIP_BATCH_READ_REQUEST_V1_FIELDS,
            ClusterMembershipBatchReadRequestV1Visitor,
        )
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

        let over_limit = serde_json::json!({
            "cluster_record_id": "cluster-card:auth-service",
            "generation": {
                "repo_id": "repo",
                "revision_id": "rev",
                "manifest_generation": 17
            },
            "expected_authority_digest": "authority-digest",
            "limit": MAX_CLUSTER_MEMBERSHIP_READ_V1 + 1
        });
        assert!(
            serde_json::from_value::<ClusterMembershipReadRequestV1>(over_limit).is_err(),
            "oversized request must fail during contract decode"
        );
    }

    fn batch(count: usize, limit: u32) -> ClusterMembershipBatchReadRequestV1 {
        ClusterMembershipBatchReadRequestV1 {
            generation: request(1).generation,
            items: (0..count)
                .map(|index| ClusterMembershipBatchReadItemV1 {
                    cluster_record_id: format!("cluster-card:{index:02}"),
                    expected_authority_digest: format!("authority:{index:02}"),
                    limit,
                })
                .collect(),
        }
    }

    #[test]
    fn cluster_membership_batch_rejects_zero_seventeen_duplicate_and_total_limit_v1() {
        assert!(matches!(
            batch(0, 1).validate_v1(),
            Err(ClusterMembershipReadPolicyErrorV1::EmptyBatch)
        ));
        assert!(matches!(
            batch(MAX_CLUSTER_MEMBERSHIP_BATCH_ITEMS_V1 + 1, 1).validate_v1(),
            Err(ClusterMembershipReadPolicyErrorV1::BatchTooLarge { .. })
        ));

        let mut duplicate = batch(2, 1);
        duplicate.items[1].cluster_record_id = duplicate.items[0].cluster_record_id.clone();
        assert!(matches!(
            duplicate.validate_v1(),
            Err(ClusterMembershipReadPolicyErrorV1::DuplicateClusterRecordId)
        ));

        assert!(matches!(
            batch(2, MAX_CLUSTER_MEMBERSHIP_READ_V1 / 2 + 1).validate_v1(),
            Err(ClusterMembershipReadPolicyErrorV1::TotalLimitExceeded { .. })
        ));

        let items = (0..=MAX_CLUSTER_MEMBERSHIP_BATCH_ITEMS_V1)
            .map(|index| {
                serde_json::json!({
                    "cluster_record_id": format!("cluster-card:{index:02}"),
                    "expected_authority_digest": format!("authority:{index:02}"),
                    "limit": 1
                })
            })
            .collect::<Vec<_>>();
        let oversized_wire = serde_json::json!({
            "generation": {
                "repo_id": "repo",
                "revision_id": "rev",
                "manifest_generation": 17
            },
            "items": items
        });
        assert!(
            serde_json::from_value::<ClusterMembershipBatchReadRequestV1>(oversized_wire).is_err(),
            "batch request must reject the first over-limit item in its sequence visitor"
        );
    }

    #[test]
    fn cluster_membership_batch_accepts_sixteen_canonical_items_and_round_trips_v1() {
        let request = batch(MAX_CLUSTER_MEMBERSHIP_BATCH_ITEMS_V1, 1);
        assert!(request.validate_v1().is_ok());
        let value = serde_json::to_value(&request).expect("encode batch");
        assert_eq!(
            serde_json::from_value::<ClusterMembershipBatchReadRequestV1>(value)
                .expect("decode batch"),
            request
        );
    }
}
