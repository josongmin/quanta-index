//! Bounded JSON preparation for fixed-field publication metadata.

use quanta_index_core::index::{
    CoverageBinding, IndexError, IndexProfile, MemberBinding, ObjectChildren, OwnerCoverage,
    OwnerMemberBinding, ProjectionBinding, ProjectionCertificate, ProjectionOwnerContribution,
    WorkBudget,
};

use crate::staging_payload::StagedPayload;

pub(crate) const MAX_BYTES: usize = quanta_index_core::index::MAX_BINDING_METADATA_BYTES;

macro_rules! fixed_binding {
    ($write:ident, $ty:ty) => {
        pub(crate) fn $write(
            binding: &$ty,
            budget: &WorkBudget,
        ) -> Result<StagedPayload, IndexError> {
            crate::staging_payload::encode(binding, MAX_BYTES, budget)
        }
    };
}

fixed_binding!(encode_coverage, CoverageBinding);
fixed_binding!(encode_owner_coverage, OwnerCoverage);
fixed_binding!(encode_member, MemberBinding);
fixed_binding!(encode_owner_member, OwnerMemberBinding);
fixed_binding!(encode_projection_owner, ProjectionOwnerContribution);
fixed_binding!(encode_projection_certificate, ProjectionCertificate);
fixed_binding!(encode_projection_binding, ProjectionBinding);

pub(crate) fn encode_profile(
    profile: &IndexProfile,
    budget: &WorkBudget,
) -> Result<StagedPayload, IndexError> {
    if profile.instances.len() > crate::profile::MAX_PROFILE_INSTANCES {
        return Err(IndexError::ResourceLimited);
    }
    // The instance count bounds this fixed-field configuration. Preserve its
    // existing wire domain; the original ledger admits the actual output size.
    crate::staging_payload::encode(profile, usize::MAX, budget)
}

pub(crate) fn coverage_children(binding: &CoverageBinding) -> ObjectChildren {
    let missing = match binding.status {
        quanta_index_core::index::CoverageStatus::Pending { previous_missing } => {
            previous_missing.0
        }
        quanta_index_core::index::CoverageStatus::Partial { missing } => missing.0,
        quanta_index_core::index::CoverageStatus::Complete => None,
    };
    // covered is a historical comparison stamp, not a physical dependency.
    ObjectChildren::from_references([
        binding.desired.root.0,
        binding.owners.0,
        missing,
        binding.searchable,
    ])
}

pub(crate) fn owner_member_children(binding: &OwnerMemberBinding) -> ObjectChildren {
    ObjectChildren::from_references([
        Some(binding.artifact),
        binding.input.source,
        binding.input.projection,
        None,
    ])
}

pub(crate) enum MemberChildren {
    Artifact([quanta_index_core::index::ObjectRef; 1]),
    ArtifactAndInput([quanta_index_core::index::ObjectRef; 2]),
}

impl MemberChildren {
    pub(crate) fn as_slice(&self) -> &[quanta_index_core::index::ObjectRef] {
        match self {
            Self::Artifact(values) => values,
            Self::ArtifactAndInput(values) => values,
        }
    }
}

pub(crate) fn member_children(binding: &MemberBinding) -> MemberChildren {
    // Member object edges are an ordered wire sequence: artifact, then input
    // root. Preserve both order and aliases; sorting/dedup changes its identity.
    binding
        .input
        .root
        .0
        .map_or(MemberChildren::Artifact([binding.artifact]), |root| {
            MemberChildren::ArtifactAndInput([binding.artifact, root])
        })
}

pub(crate) fn owner_coverage_children(binding: &OwnerCoverage) -> ObjectChildren {
    ObjectChildren::from_references([
        binding.desired.source,
        binding.desired.projection,
        binding.searchable,
        None,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::staging_payload::tests::{require, scoped_budget};
    use quanta_index_core::index::{
        CoverageStatus, Digest, IndexInstanceId, InputBinding, MapRoot, ObjectRef,
        OwnerInputBinding, OwnerKey, ProducerId, SchemaId, StableOwnerId, StorageFormatId,
        decode_coverage_binding, decode_member_binding, decode_owner_coverage,
        decode_owner_member_binding, decode_projection_binding, decode_projection_certificate,
        decode_projection_owner,
    };

    fn reference(byte: u8) -> ObjectRef {
        ObjectRef(Digest([byte; 32]))
    }

    fn input(root: u8) -> InputBinding {
        InputBinding {
            root: MapRoot(Some(reference(root))),
            schema: SchemaId([255; 16]),
            policy: Digest([255; 32]),
            context: Digest([255; 32]),
        }
    }

    fn coverage_gold() -> CoverageBinding {
        CoverageBinding {
            instance: IndexInstanceId([255; 16]),
            desired: input(1),
            covered: Some(input(99)),
            searchable: Some(reference(4)),
            owners: MapRoot(Some(reference(2))),
            status: CoverageStatus::Partial {
                missing: MapRoot(Some(reference(3))),
            },
        }
    }

    fn owner_input(source: u8) -> OwnerInputBinding {
        OwnerInputBinding {
            source: Some(reference(source)),
            projection: Some(reference(2)),
            producer_context: Digest([255; 32]),
            derivation_policy: Digest([255; 32]),
        }
    }

    fn owner() -> OwnerKey {
        OwnerKey {
            producer: ProducerId([255; 16]),
            stable_id: StableOwnerId([255; 16]),
        }
    }

    fn check_preparation<T: serde::Serialize>(
        value: &T,
        prepare: impl Fn(&T, &WorkBudget) -> Result<StagedPayload, IndexError>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let gold = serde_json::to_vec(value)?;
        let floor = u64::try_from(gold.len())?;
        let (denied, denied_ledger) = scoped_budget(floor.checked_sub(1).ok_or("empty wire")?)?;
        require(matches!(
            prepare(value, &denied),
            Err(IndexError::ResourceLimited)
        ))?;
        require(denied_ledger.live() == 0)?;
        let (budget, ledger) = scoped_budget(floor)?;
        let prepared = prepare(value, &budget)?;
        require(prepared.bytes == gold && ledger.live() == floor)?;
        drop(prepared);
        require(ledger.live() == 0)?;
        budget.request().cancel_handle().cancel();
        require(matches!(
            prepare(value, &budget),
            Err(IndexError::Cancelled)
        ))?;
        require(ledger.live() == 0)?;
        Ok(())
    }

    #[test]
    fn projection_stage_wires_hold_output_credit_until_drop()
    -> Result<(), Box<dyn std::error::Error>> {
        use quanta_index_core::index::{
            OwnerMutationSet, ProjectionBinding, ProjectionId, SourceCut,
        };
        let source = SourceCut {
            owners: MapRoot(Some(reference(1))),
            paths: MapRoot(Some(reference(2))),
            frontiers: MapRoot(Some(reference(3))),
        };
        let binding = ProjectionBinding {
            projection: ProjectionId([255; 16]),
            producer: ProducerId([255; 16]),
            owners: MapRoot(Some(reference(4))),
            source,
            schema: SchemaId([255; 16]),
            producer_policy: Digest([255; 32]),
            dependency_context: Digest([255; 32]),
            certificate: reference(5),
        };
        let certificate = ProjectionCertificate {
            projection: binding.projection,
            producer: binding.producer,
            prior: Some(binding),
            target_source: source,
            schema: binding.schema,
            producer_policy: binding.producer_policy,
            dependency_context: binding.dependency_context,
            affected: OwnerMutationSet {
                root: MapRoot(Some(reference(6))),
                count: u64::MAX,
                digest: Digest([255; 32]),
            },
        };
        let contribution = ProjectionOwnerContribution {
            projection: binding.projection,
            producer: binding.producer,
            owner: owner(),
            source_owner: Some(owner()),
            source: Some(reference(1)),
            dependency_stamp: Digest([255; 32]),
            output: Some(reference(2)),
        };
        check_preparation(&certificate, encode_projection_certificate)?;
        check_preparation(&contribution, encode_projection_owner)?;
        check_preparation(&binding, encode_projection_binding)?;
        check_projection_read(&certificate, decode_projection_certificate)?;
        check_projection_read(&contribution, decode_projection_owner)?;
        check_projection_read(&binding, decode_projection_binding)
    }

    fn check_projection_read<T: serde::Serialize + PartialEq>(
        value: &T,
        read: impl Fn(&[u8], &WorkBudget) -> Result<T, IndexError>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let gold = serde_json::to_vec(value)?;
        let floor = u64::try_from(gold.len())?
            .checked_mul(8)
            .and_then(|bytes| bytes.checked_add(512))
            .ok_or("parser floor")?;
        let (denied, denied_ledger) = scoped_budget(floor.checked_sub(1).ok_or("parser floor")?)?;
        require(matches!(
            read(&gold, &denied),
            Err(IndexError::ResourceLimited)
        ))?;
        require(denied_ledger.live() == 0)?;
        let (budget, ledger) = scoped_budget(floor)?;
        require(read(&gold, &budget)? == *value && ledger.live() == 0)?;
        require(matches!(
            read(b"{", &budget),
            Err(IndexError::CorruptObject(_))
        ))?;
        require(ledger.live() == 0)?;
        let (zero, zero_ledger) = scoped_budget(0)?;
        let overbound = vec![b' '; MAX_BYTES.checked_add(1).ok_or("metadata bound")?];
        require(matches!(
            read(&overbound, &zero),
            Err(IndexError::CorruptObject(_))
        ))?;
        require(zero_ledger.live() == 0)?;
        zero.request().cancel_handle().cancel();
        require(matches!(
            read(&overbound, &zero),
            Err(IndexError::Cancelled)
        ))?;
        require(zero_ledger.live() == 0)?;
        Ok(())
    }

    #[test]
    fn profile_stage_preserves_wire_and_refuses_count_before_output_allocation()
    -> Result<(), Box<dyn std::error::Error>> {
        use quanta_index_core::index::{
            GroupActivation, IndexKind, InputDomain, InstanceSpec, PublicationGroupId,
        };
        let mut profile = IndexProfile {
            instances: Vec::new(),
        };
        check_preparation(&profile, encode_profile)?;
        for index in 0..crate::profile::MAX_PROFILE_INSTANCES {
            profile.instances.push(InstanceSpec {
                instance: IndexInstanceId(u128::try_from(index)?.to_be_bytes()),
                kind: IndexKind([255; 16]),
                group: PublicationGroupId([255; 16]),
                activation: GroupActivation::CoreRequired,
                input_domain: InputDomain::SourceLocal,
                schema: SchemaId([255; 16]),
                policy: Digest([255; 32]),
                format: StorageFormatId([255; 16]),
            });
        }
        check_preparation(&profile, encode_profile)?;
        profile
            .instances
            .push(profile.instances.first().ok_or("profile fixture")?.clone());
        let (budget, ledger) = scoped_budget(0)?;
        require(matches!(
            encode_profile(&profile, &budget),
            Err(IndexError::ResourceLimited)
        ))?;
        require(ledger.live() == 0)?;
        Ok(())
    }

    #[test]
    fn fixed_wires_decode_floor_and_refund_match_old_serde()
    -> Result<(), Box<dyn std::error::Error>> {
        let (budget, ledger) = scoped_budget(u64::MAX)?;
        let (denied, denied_ledger) = scoped_budget(0)?;
        macro_rules! wire {
            ($value:expr, $read:ident, $write:ident) => {{
                let value = $value;
                let old_wire = serde_json::to_vec(&value)?;
                let prepared = $write(&value, &budget)?;
                require(prepared.bytes == old_wire && old_wire.len() <= MAX_BYTES)?;
                require($read(&old_wire, &budget)? == value)?;
                drop(prepared);
                require(ledger.live() == 0)?;
                require(matches!(
                    $write(&value, &denied),
                    Err(IndexError::ResourceLimited)
                ))?;
                require(matches!(
                    $read(&old_wire, &denied),
                    Err(IndexError::ResourceLimited)
                ))?;
                require(matches!(
                    $read(b"{", &budget),
                    Err(IndexError::CorruptObject(_))
                ))?;
                require(ledger.live() == 0 && denied_ledger.live() == 0)?;
            }};
        }
        wire!(coverage_gold(), decode_coverage_binding, encode_coverage);
        wire!(
            OwnerCoverage {
                owner: owner(),
                desired: owner_input(1),
                covered: Some(owner_input(99)),
                searchable: Some(reference(4)),
            },
            decode_owner_coverage,
            encode_owner_coverage
        );
        wire!(
            MemberBinding {
                instance: IndexInstanceId([255; 16]),
                format: StorageFormatId([255; 16]),
                artifact: reference(4),
                input: input(1),
                logical_commitment: Digest([255; 32]),
            },
            decode_member_binding,
            encode_member
        );
        wire!(
            OwnerMemberBinding {
                instance: IndexInstanceId([255; 16]),
                owner: owner(),
                input: owner_input(1),
                format: StorageFormatId([255; 16]),
                artifact: reference(4),
                logical_commitment: Digest([255; 32]),
            },
            decode_owner_member_binding,
            encode_owner_member
        );
        Ok(())
    }

    #[test]
    fn physical_edge_gold_has_four_children_and_no_historical_dependency() -> Result<(), IndexError>
    {
        let mut binding = coverage_gold();
        require(
            coverage_children(&binding).as_slice()
                == [reference(1), reference(2), reference(3), reference(4)],
        )?;
        binding.status = CoverageStatus::Pending {
            previous_missing: MapRoot(Some(reference(3))),
        };
        require(
            coverage_children(&binding).as_slice()
                == [reference(1), reference(2), reference(3), reference(4)],
        )?;
        binding.status = CoverageStatus::Complete;
        require(
            coverage_children(&binding).as_slice() == [reference(1), reference(2), reference(4)],
        )?;
        binding.owners = binding.desired.root;
        binding.searchable = binding.desired.root.0;
        require(coverage_children(&binding).as_slice() == [reference(1)])?;
        binding.desired.root = MapRoot(None);
        binding.owners = MapRoot(None);
        binding.searchable = None;
        require(coverage_children(&binding).as_slice().is_empty())?;
        let owner_binding = OwnerCoverage {
            owner: owner(),
            desired: owner_input(1),
            covered: Some(owner_input(99)),
            searchable: Some(reference(4)),
        };
        require(
            owner_coverage_children(&owner_binding).as_slice()
                == [reference(1), reference(2), reference(4)],
        )?;
        let member_binding = MemberBinding {
            instance: IndexInstanceId([255; 16]),
            format: StorageFormatId([255; 16]),
            artifact: reference(4),
            input: input(1),
            logical_commitment: Digest([255; 32]),
        };
        require(member_children(&member_binding).as_slice() == [reference(4), reference(1)])?;
        let mut alias_member = member_binding;
        alias_member.input.root = MapRoot(Some(alias_member.artifact));
        require(member_children(&alias_member).as_slice() == [reference(4), reference(4)])?;
        alias_member.input.root = MapRoot(None);
        require(member_children(&alias_member).as_slice() == [reference(4)])?;
        let owner_member_binding = OwnerMemberBinding {
            instance: IndexInstanceId([255; 16]),
            owner: owner(),
            input: owner_input(1),
            format: StorageFormatId([255; 16]),
            artifact: reference(4),
            logical_commitment: Digest([255; 32]),
        };
        require(
            owner_member_children(&owner_member_binding).as_slice()
                == [reference(1), reference(2), reference(4)],
        )
    }

    #[test]
    fn oversize_and_original_cancel_refuse_without_live_parser_or_output() -> Result<(), IndexError>
    {
        let (denied, ledger) = scoped_budget(0)?;
        let bytes = vec![
            b' ';
            MAX_BYTES
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?
        ];
        require(matches!(
            decode_coverage_binding(&bytes, &denied),
            Err(IndexError::CorruptObject(_))
        ))?;
        require(matches!(
            decode_owner_coverage(&bytes, &denied),
            Err(IndexError::CorruptObject(_))
        ))?;
        require(matches!(
            decode_member_binding(&bytes, &denied),
            Err(IndexError::CorruptObject(_))
        ))?;
        require(matches!(
            decode_owner_member_binding(&bytes, &denied),
            Err(IndexError::CorruptObject(_))
        ))?;
        denied.request().cancel_handle().cancel();
        require(matches!(
            decode_coverage_binding(b"{}", &denied),
            Err(IndexError::Cancelled)
        ))?;
        require(matches!(
            encode_coverage(&coverage_gold(), &denied),
            Err(IndexError::Cancelled)
        ))?;
        require(ledger.live() == 0)
    }
}
