use quanta_index_core::index::{CatalogView, Digest, IndexError, SourceCut, ViewRef};
use sha2::{Digest as _, Sha256};

fn hash(domain: &[u8], parts: &[&[u8]]) -> Result<Digest, IndexError> {
    let mut hasher = Sha256::new();
    hasher.update(b"quanta-index-vnext\0");
    hasher.update(
        u64::try_from(domain.len())
            .map_err(|_conversion_error| IndexError::ResourceLimited)?
            .to_be_bytes(),
    );
    hasher.update(domain);
    for part in parts {
        hasher.update(
            u64::try_from(part.len())
                .map_err(|_conversion_error| IndexError::ResourceLimited)?
                .to_be_bytes(),
        );
        hasher.update(part);
    }
    Ok(Digest(hasher.finalize().into()))
}

pub(crate) fn source_context(source: SourceCut) -> Result<Digest, IndexError> {
    let bytes = serde_json::to_vec(&source)
        .map_err(|error| IndexError::InvalidInput(format!("source context encoding: {error}")))?;
    hash(b"source-context", &[&bytes])
}

pub(crate) fn view_identity(view: &CatalogView) -> Result<ViewRef, IndexError> {
    let mut identity = view.identity;
    identity.digest = view
        .identity_digest()
        .map_err(|error| IndexError::InvalidInput(format!("view encoding: {error}")))?;
    Ok(identity)
}
