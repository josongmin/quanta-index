//! Hydrate a private read-only Lance store from an authenticated Native fileset.

use super::{AnnMemoryStore, AnnStoreLimits, index_error};
use crate::native::ann_files::LoadedAnnFile;
use bytes::Bytes;
use object_store::{ObjectStore as _, PutOptions, PutPayload, path::Path};
use quanta_index_core::index::{IndexError, StoredObject, WorkBudget};
use std::sync::Arc;

struct StoredChunkBytes(StoredObject);

impl AsRef<[u8]> for StoredChunkBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0.payload
    }
}

impl AnnMemoryStore {
    /// The only mutations are one private PUT per verified file. No writable
    /// handle escapes this constructor, including on cancellation or error.
    pub(in crate::native) async fn from_verified_files(
        files: &[LoadedAnnFile],
        limits: AnnStoreLimits,
        budget: &mut WorkBudget,
    ) -> Result<Arc<Self>, IndexError> {
        let writes = u64::try_from(files.len()).map_err(|_| IndexError::ResourceLimited)?;
        if writes > u64::from(limits.max_files) {
            return Err(IndexError::ResourceLimited);
        }
        let reads = limits
            .max_ops
            .checked_sub(writes)
            .ok_or(IndexError::ResourceLimited)?;
        let store = Self::admit(limits, reads, writes, budget)?;
        for file in files {
            budget.checkpoint()?;
            let location = Path::parse(&file.path)
                .map_err(|error| IndexError::CorruptObject(format!("ANN file path: {error}")))?;
            if location.as_ref() != file.path.as_str() {
                return Err(IndexError::CorruptObject(
                    "ANN file path changed during hydration".into(),
                ));
            }
            // Pinned InMemory::put_opts shares a single Bytes. Only a file
            // spanning multiple chunks needs a contiguous copy here.
            let payload = file
                .chunks
                .iter()
                .map(|chunk| {
                    budget.checkpoint()?;
                    let length = u64::from(chunk.length);
                    if length > limits.max_part_bytes {
                        return Err(IndexError::ResourceLimited);
                    }
                    if length
                        != u64::try_from(chunk.object.payload.len())
                            .map_err(|_| IndexError::ResourceLimited)?
                    {
                        return Err(IndexError::CorruptObject(
                            "ANN file chunk length changed during hydration".into(),
                        ));
                    }
                    Ok(Bytes::from_owner(StoredChunkBytes(chunk.object.clone())))
                })
                .collect::<Result<PutPayload, IndexError>>()?;
            if u64::try_from(payload.content_length()).map_err(|_| IndexError::ResourceLimited)?
                != file.length
            {
                return Err(IndexError::CorruptObject(
                    "ANN file length changed during hydration".into(),
                ));
            }
            let _put = store
                .put_opts(&location, payload, PutOptions::default())
                .await
                .map_err(index_error)?;
        }
        store.freeze().await?;
        Ok(store)
    }
}
