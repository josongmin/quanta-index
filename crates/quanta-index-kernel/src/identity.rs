use quanta_index_core::index::{
    CatalogView, Digest, IndexError, MapRoot, ObjectRef, SourceCut, ViewRef, WorkspaceKey,
};

pub(crate) fn genesis_view(
    workspace: WorkspaceKey,
    epoch: u64,
    profile: ObjectRef,
) -> Result<CatalogView, IndexError> {
    let mut view = CatalogView {
        identity: ViewRef {
            workspace,
            epoch,
            sequence: 0,
            digest: Digest([0; 32]),
        },
        profile,
        source: SourceCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        },
        projections: MapRoot(None),
        members: MapRoot(None),
        coverage: MapRoot(None),
    };
    view.identity = view_identity(&view)?;
    Ok(view)
}

pub(crate) fn source_context(source: SourceCut) -> Result<Digest, IndexError> {
    source
        .input_context_digest()
        .map_err(|error| IndexError::InvalidInput(format!("source context encoding: {error}")))
}

pub(crate) fn view_identity(view: &CatalogView) -> Result<ViewRef, IndexError> {
    let mut identity = view.identity;
    identity.digest = view
        .identity_digest()
        .map_err(|error| IndexError::InvalidInput(format!("view encoding: {error}")))?;
    Ok(identity)
}
