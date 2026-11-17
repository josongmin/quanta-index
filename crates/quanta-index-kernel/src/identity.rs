use quanta_index_core::index::{CatalogView, Digest, IndexError, SourceCut, ViewRef};

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
