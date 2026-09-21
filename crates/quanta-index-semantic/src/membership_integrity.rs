//! Canonical integrity commitment for the structured `ClusterCard` sidecar.

use std::fmt::Write as _;

use quanta_index_contract::{
    MAX_CLUSTER_MEMBERSHIP_READ_V1, SymbolId, canonical_order::first_canonical_order_break_v1,
    cluster_membership_content_digest_v1,
};
use sha2::{Digest as _, Sha256};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClusterMembershipStoredRowV1 {
    pub(crate) cluster_record_id: String,
    pub(crate) authority_digest: String,
    pub(crate) owner_kind: String,
    pub(crate) owner_id: String,
    pub(crate) member_symbol_id: String,
    pub(crate) ordinal: u32,
    pub(crate) member_count: u32,
    pub(crate) membership_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClusterMembershipCommitmentV1 {
    pub(crate) root_digest: String,
    pub(crate) cluster_count: u64,
    pub(crate) member_row_count: u64,
}

fn hash_field_v1(hasher: &mut Sha256, value: &str) {
    hasher.update(value.len().to_string().as_bytes());
    hasher.update([0]);
    hasher.update(value.as_bytes());
}

fn encode_sha256_v1(digest: impl IntoIterator<Item = u8>) -> String {
    let mut encoded = String::with_capacity("sha256:".len().saturating_add(64));
    encoded.push_str("sha256:");
    for byte in digest {
        let _written = write!(&mut encoded, "{byte:02x}");
    }
    encoded
}

/// Computes one order-independent table commitment over the sidecar rows.
///
/// Rows are first reduced into canonical `(cluster_record_id, ordinal)` order.
/// Every durable column is committed, so replacing a sidecar with a
/// self-consistent table from another generation cannot pass unless its exact
/// authority content is identical.
pub(crate) fn cluster_membership_commitment_v1(
    mut rows: Vec<ClusterMembershipStoredRowV1>,
) -> Result<ClusterMembershipCommitmentV1, String> {
    let member_row_count = u64::try_from(rows.len())
        .map_err(|error| format!("cluster membership row count overflow: {error}"))?;
    rows.sort_unstable_by(|left, right| {
        left.cluster_record_id
            .cmp(&right.cluster_record_id)
            .then_with(|| left.ordinal.cmp(&right.ordinal))
    });

    let mut root = Sha256::new();
    root.update(b"quanta-index:cluster-membership-table:v1\0");
    let mut cluster_count = 0_u64;
    // Rows are sorted by `(cluster_record_id, ordinal)`, so each contiguous run
    // is exactly one cluster. Grouping with `chunk_by` keeps the run boundaries
    // out of index arithmetic that a malformed sidecar could walk off.
    for cluster in rows.chunk_by(|left, right| left.cluster_record_id == right.cluster_record_id) {
        let Some(first) = cluster.first() else {
            continue;
        };
        let cluster_record_id = first.cluster_record_id.as_str();
        let observed_count = u32::try_from(cluster.len()).map_err(|error| {
            format!("cluster membership cardinality overflow for {cluster_record_id:?}: {error}")
        })?;
        if observed_count != first.member_count {
            return Err(format!(
                "cluster membership count mismatch for {cluster_record_id:?}: committed={} observed={observed_count}",
                first.member_count
            ));
        }
        if observed_count > MAX_CLUSTER_MEMBERSHIP_READ_V1 {
            return Err(format!(
                "cluster membership cardinality exceeds {MAX_CLUSTER_MEMBERSHIP_READ_V1} for {cluster_record_id:?}"
            ));
        }

        let mut members = Vec::with_capacity(cluster.len());
        for (expected_ordinal, row) in (0_u32..).zip(cluster) {
            if row.cluster_record_id.is_empty()
                || row.authority_digest.is_empty()
                || row.owner_kind.is_empty()
                || row.owner_id.is_empty()
                || row.member_symbol_id.is_empty()
                || row.membership_digest.is_empty()
            {
                return Err(
                    "cluster membership sidecar contains an empty authority field".to_string()
                );
            }
            if row.authority_digest != first.authority_digest
                || row.owner_kind != first.owner_kind
                || row.owner_id != first.owner_id
                || row.member_count != first.member_count
                || row.membership_digest != first.membership_digest
            {
                return Err(format!(
                    "cluster membership metadata is inconsistent for {cluster_record_id:?}"
                ));
            }
            if row.ordinal != expected_ordinal {
                return Err(format!(
                    "cluster membership ordinals are not contiguous for {cluster_record_id:?}"
                ));
            }
            members.push(SymbolId::new(row.member_symbol_id.clone()));
        }
        let content_digest = cluster_membership_content_digest_v1(&members);
        if content_digest != first.membership_digest {
            return Err(format!(
                "cluster membership content digest mismatch for {cluster_record_id:?}"
            ));
        }
        if first_canonical_order_break_v1(&members, |member| member.as_str()).is_some() {
            return Err(format!(
                "cluster membership members are not unique canonical order for {cluster_record_id:?}"
            ));
        }

        hash_field_v1(&mut root, cluster_record_id);
        hash_field_v1(&mut root, &first.authority_digest);
        hash_field_v1(&mut root, &first.owner_kind);
        hash_field_v1(&mut root, &first.owner_id);
        hash_field_v1(&mut root, &first.member_count.to_string());
        hash_field_v1(&mut root, &first.membership_digest);
        for row in cluster {
            hash_field_v1(&mut root, &row.ordinal.to_string());
            hash_field_v1(&mut root, &row.member_symbol_id);
        }
        cluster_count = cluster_count
            .checked_add(1)
            .ok_or_else(|| "cluster membership cluster count overflow".to_string())?;
    }

    Ok(ClusterMembershipCommitmentV1 {
        root_digest: encode_sha256_v1(root.finalize()),
        cluster_count,
        member_row_count,
    })
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "fixtures in this module are built with known fixed lengths; an out-of-range index is a test authoring bug that should fail loudly"
)]
mod tests {
    use super::*;

    fn rows_v1() -> Vec<ClusterMembershipStoredRowV1> {
        let members = vec![SymbolId::new("symbol:a"), SymbolId::new("symbol:b")];
        let digest = cluster_membership_content_digest_v1(&members);
        members
            .into_iter()
            .enumerate()
            .map(|(ordinal, member)| ClusterMembershipStoredRowV1 {
                cluster_record_id: "cluster:a".to_string(),
                authority_digest: "authority:a".to_string(),
                owner_kind: "Module".to_string(),
                owner_id: "module:a".to_string(),
                member_symbol_id: member.as_str().to_string(),
                ordinal: u32::try_from(ordinal).expect("small fixture ordinal"),
                member_count: 2,
                membership_digest: digest.clone(),
            })
            .collect()
    }

    #[test]
    fn commitment_is_order_independent_but_rejects_row_mutation_v1() {
        let rows = rows_v1();
        let expected = cluster_membership_commitment_v1(rows.clone()).expect("valid commitment");
        let mut reordered = rows.clone();
        reordered.reverse();
        assert_eq!(
            cluster_membership_commitment_v1(reordered).expect("canonical reorder"),
            expected
        );

        for mutation in 0..4 {
            let mut corrupt = rows.clone();
            match mutation {
                0 => {
                    let _deleted = corrupt.pop();
                }
                1 => corrupt[1].member_symbol_id = "symbol:c".to_string(),
                2 => corrupt[1].ordinal = 0,
                3 => corrupt[1].authority_digest = "authority:other".to_string(),
                other => panic!("mutation index {other} is outside the fixture's range"),
            }
            assert!(cluster_membership_commitment_v1(corrupt).is_err());
        }

        let mut self_consistent_substitution = rows;
        self_consistent_substitution[1].member_symbol_id = "symbol:c".to_string();
        let replacement_members = vec![SymbolId::new("symbol:a"), SymbolId::new("symbol:c")];
        let replacement_digest = cluster_membership_content_digest_v1(&replacement_members);
        for row in &mut self_consistent_substitution {
            row.membership_digest.clone_from(&replacement_digest);
        }
        let replacement = cluster_membership_commitment_v1(self_consistent_substitution)
            .expect("self-consistent replacement");
        assert_ne!(replacement.root_digest, expected.root_digest);

        let mut self_consistent_addition = rows_v1();
        let added_members = vec![
            SymbolId::new("symbol:a"),
            SymbolId::new("symbol:b"),
            SymbolId::new("symbol:c"),
        ];
        let added_digest = cluster_membership_content_digest_v1(&added_members);
        for row in &mut self_consistent_addition {
            row.member_count = 3;
            row.membership_digest.clone_from(&added_digest);
        }
        let mut added_row = self_consistent_addition[1].clone();
        added_row.ordinal = 2;
        added_row.member_symbol_id = "symbol:c".to_string();
        self_consistent_addition.push(added_row);
        let addition = cluster_membership_commitment_v1(self_consistent_addition)
            .expect("self-consistent addition");
        assert_ne!(addition.root_digest, expected.root_digest);

        let mut self_consistent_reorder = rows_v1();
        self_consistent_reorder[0].member_symbol_id = "symbol:b".to_string();
        self_consistent_reorder[1].member_symbol_id = "symbol:a".to_string();
        let reordered_digest = cluster_membership_content_digest_v1(&[
            SymbolId::new("symbol:b"),
            SymbolId::new("symbol:a"),
        ]);
        for row in &mut self_consistent_reorder {
            row.membership_digest.clone_from(&reordered_digest);
        }
        assert!(cluster_membership_commitment_v1(self_consistent_reorder).is_err());
    }
}
