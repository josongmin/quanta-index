//! Selected-view authority shared by semantic candidate traversal.
//!
//! Coverage is a search layout, not authority to replace the source or
//! producer maps in the selected ticket. Authenticate before accepting even
//! an empty, stale or editor-masked owner.

use std::ops::ControlFlow;

use super::{
    ReadContext, SearchQuery, SelectedArtifactInput, contribution_children, contribution_context,
    load_source_record_with_lifetime, source_file::FileRecord,
};
use quanta_index_core::index::{
    CoverageBinding, Digest, IndexError, MapPage, MapRoot, MapValue, MemoryLifetime,
    ObjectChildren, ObjectRef, OwnerCoverage, OwnerKey, OwnerMemberBinding, ProjectionBinding,
    ProjectionOwnerContribution, decode_owner_member_binding, decode_projection_owner,
    owner_map_key,
};

/// Validate the full page before a caller can stop at a prefix or owner cap.
/// A continuation follows `has_more` on the same immutable root. A fresh seek
/// may have a cursor and still return no entries.
pub(super) fn scan_ordered_page(
    root: MapRoot,
    after: Option<&[u8]>,
    limit: u32,
    continuation: bool,
    context: &mut ReadContext<'_>,
) -> Result<MapPage, IndexError> {
    context.budget.checkpoint()?;
    let page = context.maps.scan(root, after, limit, context.budget)?;
    if page.entries.len() > usize::try_from(limit).map_err(|_| IndexError::ResourceLimited)?
        || (page.entries.is_empty() && page.has_more)
    {
        return Err(IndexError::BackendViolation(
            "semantic map scan exceeded its page or made no progress".into(),
        ));
    }
    if (root.0.is_none() && !page.entries.is_empty())
        || (root.0.is_some() && after.is_none() && page.entries.is_empty())
    {
        return Err(IndexError::CorruptObject(
            "semantic map page contradicts its root".into(),
        ));
    }
    if continuation && page.entries.is_empty() {
        return Err(IndexError::CorruptObject(
            "semantic map scan lost its promised continuation".into(),
        ));
    }
    let mut previous = after;
    for (key, _) in &page.entries {
        context.budget.checkpoint()?;
        if previous.is_some_and(|prior| prior >= key.as_slice()) {
            return Err(IndexError::CorruptObject(
                "semantic map scan keys did not advance".into(),
            ));
        }
        previous = Some(key.as_slice());
    }
    Ok(page)
}

#[derive(Clone, Copy)]
pub(super) struct SelectedOwner {
    pub(super) coverage: OwnerCoverage,
    pub(super) contribution: Option<ProjectionOwnerContribution>,
    source_owners: MapRoot,
}

/// A source pair authenticated against the selected immutable map.
pub(super) struct AuthenticatedSource {
    pub(super) root: MapRoot,
    pub(super) owner: OwnerKey,
    pub(super) reference: ObjectRef,
}

/// Input authority for the selected view. A reverse traversal can reuse
/// the immutable source pair authenticated before opening its owner rows.
#[derive(Clone, Copy)]
pub(super) enum SourceScope<'a> {
    Lookup(MapRoot),
    VerifiedProjection(&'a AuthenticatedSource),
}

impl SourceScope<'_> {
    const fn root(self) -> MapRoot {
        match self {
            Self::Lookup(root) => root,
            Self::VerifiedProjection(source) => source.root,
        }
    }
}

impl SelectedOwner {
    /// Bind a matching path to its original source record for ranking and file output.
    pub(super) fn rank_input(
        &self,
        query: &SearchQuery<'_>,
        retained_file: Option<FileRecord>,
        context: &mut ReadContext<'_>,
    ) -> Result<ControlFlow<(), Option<SelectedArtifactInput>>, IndexError> {
        if self.has_no_candidate(query.source_files) {
            return Ok(ControlFlow::Break(()));
        }
        if let (Some(contribution), Some(record)) = (self.contribution, retained_file) {
            if query
                .exact_path
                .is_some_and(|path| path != record.path.as_str())
            {
                return Ok(ControlFlow::Break(()));
            }
            return Ok(ControlFlow::Continue(Some(
                SelectedArtifactInput::Projection {
                    contribution,
                    file_record: Some(record),
                },
            )));
        }
        let Some(path) = query.exact_path else {
            return Ok(ControlFlow::Continue(
                self.contribution.map(SelectedArtifactInput::projection),
            ));
        };
        let Some(reference) = self.coverage.desired.source else {
            if query.source_files {
                return Err(IndexError::CorruptObject(
                    "selected semantic file lacks source reference".into(),
                ));
            }
            // A source-free projection has no file-path membership.
            return Ok(ControlFlow::Break(()));
        };
        let record = load_source_record_with_lifetime(
            reference,
            context.objects,
            context.budget,
            if query.source_files {
                MemoryLifetime::Retained
            } else {
                MemoryLifetime::Temporary
            },
        )?;
        if record.path != path {
            return Ok(ControlFlow::Break(()));
        }
        let input = match self.contribution {
            Some(contribution) => SelectedArtifactInput::Projection {
                contribution,
                file_record: query.source_files.then_some(FileRecord::Owned(record)),
            },
            None => SelectedArtifactInput::Source { reference, record },
        };
        Ok(ControlFlow::Continue(Some(input)))
    }

    pub(super) fn has_no_candidate(&self, source_files: bool) -> bool {
        self.contribution.is_some_and(|contribution| {
            (self.coverage.covered == Some(self.coverage.desired)
                && self.coverage.searchable.is_none()
                && contribution.output.is_none())
                || (source_files && contribution.source.is_none())
        })
    }

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
            SourceScope::Lookup(query.source_owners),
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
        source_scope: SourceScope<'_>,
        context: &mut ReadContext<'_>,
    ) -> Result<Self, IndexError> {
        let owner = quanta_index_core::index::read_owner_coverage(
            reference.into(),
            key,
            context.objects,
            context.budget,
        )?;
        Self::authenticate(key, owner, coverage, projection, source_scope, context)
    }

    /// Join a decoded coverage row whose owning map and key were already checked.
    pub(super) fn authenticate(
        key: &[u8],
        owner: OwnerCoverage,
        coverage: &CoverageBinding,
        projection: Option<&ProjectionBinding>,
        source_scope: SourceScope<'_>,
        context: &mut ReadContext<'_>,
    ) -> Result<Self, IndexError> {
        if owner.desired.derivation_policy != coverage.desired.policy {
            return Err(IndexError::CorruptObject(
                "selected semantic owner policy mismatch".into(),
            ));
        }
        let source_owners = source_scope.root();
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
            match source_scope {
                SourceScope::VerifiedProjection(source) => {
                    if contribution.source_owner != Some(source.owner)
                        || contribution.source != Some(source.reference)
                    {
                        return Err(IndexError::CorruptObject(
                            "projected reverse source stamp differs".into(),
                        ));
                    }
                }
                SourceScope::Lookup(source_owners) => {
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
                }
            }
            Some(contribution)
        } else {
            if matches!(source_scope, SourceScope::VerifiedProjection(_)) {
                return Err(IndexError::CorruptObject(
                    "projected reverse owner lacks contribution".into(),
                ));
            }
            Self::validate_source_local(&owner, key, source_owners, context)?;
            None
        };
        Ok(Self {
            coverage: owner,
            contribution,
            source_owners,
        })
    }

    pub(super) fn load_source_local(
        key: &[u8],
        reference: impl Into<MapValue>,
        coverage: &CoverageBinding,
        source_owners: MapRoot,
        context: &mut ReadContext<'_>,
    ) -> Result<Self, IndexError> {
        Self::load_selected(
            key,
            reference,
            coverage,
            None,
            SourceScope::Lookup(source_owners),
            context,
        )
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
        {
            return Err(IndexError::CorruptObject(
                "selected semantic member registration mismatch".into(),
            ));
        }
        self.validate_member_identity(&member, query)?;
        Ok(member)
    }

    pub(super) fn validate_member_identity(
        &self,
        member: &OwnerMemberBinding,
        query: &SearchQuery<'_>,
    ) -> Result<(), IndexError> {
        if member.instance != query.instance
            || member.format != query.format
            || member.owner != self.coverage.owner
            || member.input != self.coverage.desired
        {
            return Err(IndexError::CorruptObject(
                "selected semantic member binding mismatch".into(),
            ));
        }
        Ok(())
    }

    /// Reuse the immutable source-owner lookup from authentication only
    /// for the same selected root and the member it authorized.
    pub(super) fn validate_file_scope(
        &self,
        member: &OwnerMemberBinding,
        query: &SearchQuery<'_>,
    ) -> Result<(), IndexError> {
        self.validate_member_identity(member, query)?;
        if self.source_owners != query.source_owners {
            return Err(IndexError::CorruptObject(
                "file semantic source is outside the selected view".into(),
            ));
        }
        Ok(())
    }
}
