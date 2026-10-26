//! Selected-view authority shared by semantic candidate traversal.
//!
//! Coverage is a search layout, not authority to replace the source or
//! producer maps in the selected ticket. Authenticate before accepting even
//! an empty, stale or editor-masked owner.

use super::{ReadContext, SearchQuery, contribution_children, contribution_context};
use quanta_index_core::index::{
    CoverageBinding, Digest, IndexError, MapRoot, MapValue, ObjectChildren, OwnerCoverage,
    OwnerMemberBinding, ProjectionBinding, ProjectionOwnerContribution,
    decode_owner_member_binding, decode_projection_owner, owner_map_key,
};

pub(super) struct SelectedOwner {
    pub(super) coverage: OwnerCoverage,
    pub(super) contribution: Option<ProjectionOwnerContribution>,
}

impl SelectedOwner {
    pub(super) fn load(
        key: &[u8],
        reference: impl Into<MapValue>,
        query: &SearchQuery<'_>,
        context: &mut ReadContext<'_>,
    ) -> Result<Self, IndexError> {
        Self::load_selected(
            key,
            reference,
            query.coverage,
            query.projection.as_ref(),
            query.source_owners,
            context,
        )
    }

    /// Authenticate one owner against the selected coverage and producer cut
    /// before a caller can use its current member or lexical eligibility.
    pub(super) fn load_selected(
        key: &[u8],
        reference: impl Into<MapValue>,
        coverage: &CoverageBinding,
        projection: Option<&ProjectionBinding>,
        source_owners: MapRoot,
        context: &mut ReadContext<'_>,
    ) -> Result<Self, IndexError> {
        let owner = quanta_index_core::index::read_owner_coverage(
            reference.into(),
            key,
            context.objects,
            context.budget,
        )?;
        if owner.desired.derivation_policy != coverage.desired.policy {
            return Err(IndexError::CorruptObject(
                "selected semantic owner policy mismatch".into(),
            ));
        }
        let contribution = if let Some(binding) = projection {
            let actual = context
                .maps
                .lookup_object(binding.owners, key, context.budget)?;
            if actual != owner.desired.projection {
                return Err(IndexError::CorruptObject(
                    "projection owner root differs from coverage".into(),
                ));
            }
            let reference = actual
                .ok_or_else(|| IndexError::CorruptObject("projection owner is absent".into()))?;
            let object = context.objects.get(reference, context.budget)?;
            let contribution = decode_projection_owner(&object.payload, context.budget)?;
            if object.registration.reference != reference
                || object.registration.children.as_slice()
                    != contribution_children(contribution).as_ref()
                || contribution.owner != owner.owner
                || contribution.projection != binding.projection
                || contribution.producer != binding.producer
                || contribution.source != owner.desired.source
                || contribution.source_owner.is_some() != contribution.source.is_some()
                || contribution_context(contribution) != owner.desired.producer_context
            {
                return Err(IndexError::CorruptObject(
                    "projection owner stamp mismatch".into(),
                ));
            }
            if let (Some(source_owner), Some(source)) =
                (contribution.source_owner, contribution.source)
                && context.maps.lookup_object(
                    source_owners,
                    &owner_map_key(source_owner),
                    context.budget,
                )? != Some(source)
            {
                return Err(IndexError::CorruptObject(
                    "projection source owner stamp mismatch".into(),
                ));
            }
            Some(contribution)
        } else {
            Self::validate_source_local(&owner, key, source_owners, context)?;
            None
        };
        Ok(Self {
            coverage: owner,
            contribution,
        })
    }

    pub(super) fn load_source_local(
        key: &[u8],
        reference: impl Into<MapValue>,
        coverage: &CoverageBinding,
        source_owners: MapRoot,
        context: &mut ReadContext<'_>,
    ) -> Result<Self, IndexError> {
        Self::load_selected(key, reference, coverage, None, source_owners, context)
    }

    fn validate_source_local(
        owner: &OwnerCoverage,
        key: &[u8],
        source_owners: MapRoot,
        context: &mut ReadContext<'_>,
    ) -> Result<(), IndexError> {
        if owner.desired.projection.is_some()
            || owner.desired.source.is_none()
            || owner.desired.producer_context != Digest([0; 32])
            || context
                .maps
                .lookup_object(source_owners, key, context.budget)?
                != owner.desired.source
        {
            return Err(IndexError::CorruptObject(
                "selected source owner differs from semantic coverage".into(),
            ));
        }
        Ok(())
    }

    /// Apply the public predicate to the source certified by this selected owner.
    /// Projection owners may share one source; their own owner IDs are not paths.
    pub(super) fn matches_exact_path(
        &self,
        query: &SearchQuery<'_>,
        context: &mut ReadContext<'_>,
    ) -> Result<bool, IndexError> {
        let Some(path) = query.exact_path else {
            return Ok(true);
        };
        context.budget.checkpoint()?;
        let source = self
            .contribution
            .map_or(self.coverage.desired.source, |value| value.source);
        let Some(source) = source else {
            // A certified source-free projection has no file-path membership.
            return Ok(false);
        };
        let record =
            super::load_source_record_from_objects(source, context.objects, context.budget)?;
        Ok(record.path == path)
    }

    pub(super) fn current_member(
        &self,
        query: &SearchQuery<'_>,
        context: &mut ReadContext<'_>,
    ) -> Result<OwnerMemberBinding, IndexError> {
        let owner = self.coverage;
        if owner.covered != Some(owner.desired) {
            return Err(IndexError::CorruptObject(
                "stale semantic owner cannot authorize candidates".into(),
            ));
        }
        let reference = owner.searchable.ok_or_else(|| {
            IndexError::CorruptObject("current owner has no searchable artifact".into())
        })?;
        let object = context.objects.get(reference, context.budget)?;
        let member = decode_owner_member_binding(&object.payload, context.budget)?;
        let children = ObjectChildren::from_references([
            Some(member.artifact),
            member.input.source,
            member.input.projection,
            None,
        ]);
        if object.registration.reference != reference
            || object.registration.children.as_slice() != children.as_slice()
            || member.instance != query.instance
            || member.format != query.format
            || member.owner != owner.owner
            || member.input != owner.desired
        {
            return Err(IndexError::CorruptObject(
                "selected semantic member binding mismatch".into(),
            ));
        }
        Ok(member)
    }
}
