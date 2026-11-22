//! Disk oracle shared by the offline Legacy lexical pool tests.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn pooled_object_of_kind(root: &Path, kind: u8) -> Result<PathBuf, Box<dyn Error>> {
    let objects = root.join("indexes/lexical/lexical-object-pool/objects");
    for shard in fs::read_dir(objects)? {
        let shard = shard?;
        if !shard.file_type()?.is_dir() {
            continue;
        }
        for object in fs::read_dir(shard.path())? {
            let object = object?;
            if !object.file_type()?.is_file() {
                continue;
            }
            let bytes = fs::read(object.path())?;
            if bytes.get(..8) == Some(b"QLSTOR01".as_slice()) && bytes.get(10) == Some(&kind) {
                return Ok(object.path());
            }
        }
    }
    Err(format!("published lexical pool has no physical object of kind {kind}").into())
}
