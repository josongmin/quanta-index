//! Project the held work directory into ObjectStore list responses.

use std::io;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use futures::{StreamExt as _, TryStreamExt as _, stream, stream::BoxStream};
use object_store::path::Path;
use object_store::{ListResult, ObjectMeta, Result as StoreResult};
use url::Url;

use crate::{SemanticGenerationFileIo, SemanticWorkListEntry};

use super::{
    blocking_file_error, core_file_error, file_error, local_file_etag, validate_local_path,
};

fn filesystem_prefix(prefix: &Path) -> StoreResult<PathBuf> {
    // File names ending in #digits are hidden from object listing, but a
    // directory with such a name can still be a legitimate list prefix.
    let mut url = Url::parse("file:///")
        .map_err(|error| file_error(FsPath::new("/"), io::Error::other(error)))?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|()| file_error(FsPath::new("/"), io::Error::other("invalid file URL")))?;
        let _updated = segments.pop_if_empty().extend(prefix.parts());
    }
    let path = url
        .to_file_path()
        .map_err(|()| file_error(FsPath::new("/"), io::Error::other("invalid list prefix")))?;
    validate_local_path(prefix, path)
}

fn is_object_filename(path: &Path) -> bool {
    path.filename().is_some_and(|filename| {
        filename.split_once('#').is_none_or(|(_, suffix)| {
            suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit())
        })
    })
}

async fn scan(
    generation: Arc<dyn SemanticGenerationFileIo>,
    prefix: Option<Path>,
    recursive: bool,
) -> StoreResult<ListResult> {
    let prefix = prefix.ok_or_else(|| {
        file_error(
            FsPath::new("/"),
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "work listing requires a dataset prefix",
            ),
        )
    })?;
    let physical = filesystem_prefix(&prefix)?;
    tokio::task::spawn_blocking(move || {
        let entries = generation
            .list_work_dataset(&physical, recursive)
            .map_err(|error| core_file_error(&physical, error))?;
        let mut common_prefixes = Vec::new();
        let mut objects = Vec::new();
        for entry in entries {
            match entry {
                SemanticWorkListEntry::File {
                    path,
                    inode,
                    bytes,
                    modified,
                } => {
                    let location = Path::from_absolute_path(path)?;
                    if is_object_filename(&location) {
                        objects.push(ObjectMeta {
                            location,
                            last_modified: modified.into(),
                            size: bytes,
                            e_tag: Some(local_file_etag(inode, bytes, Some(modified))),
                            version: None,
                        });
                    }
                }
                SemanticWorkListEntry::Directory(path) if !recursive => {
                    common_prefixes.push(Path::from_absolute_path(path)?);
                }
                SemanticWorkListEntry::Directory(_) => {}
            }
        }
        common_prefixes.sort_unstable();
        objects.sort_unstable_by(|left, right| left.location.cmp(&right.location));
        Ok(ListResult {
            common_prefixes,
            objects,
        })
    })
    .await
    .map_err(blocking_file_error)?
}

pub(super) fn list(
    generation: Arc<dyn SemanticGenerationFileIo>,
    prefix: Option<&Path>,
    offset: Option<&Path>,
) -> BoxStream<'static, StoreResult<ObjectMeta>> {
    let prefix = prefix.cloned();
    let offset = offset.cloned();
    stream::once(async move {
        let result = scan(generation, prefix, true).await?;
        Ok::<_, object_store::Error>(stream::iter(
            result
                .objects
                .into_iter()
                .filter(move |object| {
                    offset
                        .as_ref()
                        .is_none_or(|offset| &object.location > offset)
                })
                .map(Ok::<_, object_store::Error>),
        ))
    })
    .try_flatten()
    .boxed()
}

pub(super) async fn list_with_delimiter(
    generation: Arc<dyn SemanticGenerationFileIo>,
    prefix: Option<&Path>,
) -> StoreResult<ListResult> {
    scan(generation, prefix.cloned(), false).await
}
