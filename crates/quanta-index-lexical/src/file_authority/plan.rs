//! Changed-file-only plan bound to canonical operations and generation root.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::{SearchScopeSurface, SourceFileKey};
use quanta_index_core::CoreError;
use sha2::{Digest as _, Sha256};

use super::paged::{FileAuthorityMutation, FileSourceEdit};

#[derive(Eq, PartialEq)]
struct PlanBinding {
    root: Option<[u8; 32]>,
    operations: [u8; 32],
    policy: [u8; 32],
}

#[derive(Default)]
pub(crate) struct FileAuthorityPlanCache {
    entry: Option<(PlanBinding, FileAuthorityMutation)>,
    #[cfg(test)]
    preparations: u64,
}

impl FileAuthorityPlanCache {
    pub(crate) fn prepare(
        &mut self,
        directory: &Path,
        ops: &[LexicalChannelOp],
    ) -> Result<(), CoreError> {
        let root = super::read_root_bytes(directory)?;
        let binding = PlanBinding {
            root: root.as_deref().map(|bytes| Sha256::digest(bytes).into()),
            operations: operations_sha256(ops)?,
            policy: super::paged::policy_digest(),
        };
        if self
            .entry
            .as_ref()
            .is_some_and(|(prior, _)| *prior == binding)
        {
            return Ok(());
        }
        let mutation = derive_changed_ops(ops, binding.operations)?;
        self.entry = Some((binding, mutation));
        #[cfg(test)]
        {
            self.preparations = self.preparations.saturating_add(1);
        }
        Ok(())
    }

    pub(crate) fn take(
        &mut self,
        directory: &Path,
        ops: &[LexicalChannelOp],
    ) -> Result<FileAuthorityMutation, CoreError> {
        self.prepare(directory, ops)?;
        self.entry
            .take()
            .map(|(_, mutation)| mutation)
            .ok_or_else(|| super::invalid("admitted file mutation is unavailable"))
    }

    #[cfg(test)]
    pub(crate) fn preparations(&self) -> u64 {
        self.preparations
    }
}

pub(crate) fn plan_ops(
    directory: &Path,
    ops: &[LexicalChannelOp],
) -> Result<FileAuthorityMutation, CoreError> {
    FileAuthorityPlanCache::default().take(directory, ops)
}

pub(super) fn operations_sha256(ops: &[LexicalChannelOp]) -> Result<[u8; 32], CoreError> {
    struct HashWriter(Sha256);
    impl Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(Sha256::new());
    writer
        .0
        .update(b"quanta-index:file-authority-plan-operations:v2\0");
    for op in ops {
        let result = match op {
            LexicalChannelOp::ReplaceLexicalScope(value) => ciborium::into_writer(
                &(
                    "replace",
                    &value.repo_id,
                    &value.revision_id,
                    value.generation,
                    &value.payload,
                ),
                &mut writer,
            ),
            LexicalChannelOp::TombstoneLexicalScope(value) => ciborium::into_writer(
                &(
                    "tombstone",
                    &value.repo_id,
                    &value.revision_id,
                    value.generation,
                    &value.payload,
                ),
                &mut writer,
            ),
            LexicalChannelOp::ClearLexicalSurface(value) => ciborium::into_writer(
                &(
                    "clear",
                    &value.repo_id,
                    &value.revision_id,
                    value.generation,
                    value.base_generation,
                    value.surface,
                ),
                &mut writer,
            ),
            LexicalChannelOp::FullBundle(_)
            | LexicalChannelOp::UpsertChunk(_)
            | LexicalChannelOp::UpsertSymbol(_)
            | LexicalChannelOp::Seal(_)
            | LexicalChannelOp::UpsertCommit(_)
            | LexicalChannelOp::UpsertRef(_)
            | LexicalChannelOp::UpsertParseTree(_)
            | LexicalChannelOp::ReplaceStructuralScope(_) => continue,
        };
        result.map_err(|error| super::invalid(&format!("file plan operation digest: {error}")))?;
    }
    Ok(writer.0.finalize().into())
}

fn derive_changed_ops(
    ops: &[LexicalChannelOp],
    operations_sha256: [u8; 32],
) -> Result<FileAuthorityMutation, CoreError> {
    let mut edits = BTreeMap::<SourceFileKey, FileSourceEdit>::new();
    let mut clear_all = false;
    for op in ops {
        match op {
            LexicalChannelOp::ReplaceLexicalScope(payload) => {
                let (_, _, scope) =
                    crate::channel_payloads::decode_replace_scope_payload(&payload.payload)?;
                scope
                    .coverage
                    .validate()
                    .map_err(|error| super::invalid(&error.to_string()))?;
                if scope.source_bytes.len() > super::MAX_FILE_BYTES {
                    return Err(CoreError::Typed {
                        code: quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                        message: "file authority: changed file exceeds 8 MiB admission".into(),
                    });
                }
                if <[u8; 32]>::from(Sha256::digest(&scope.source_bytes))
                    != scope.coverage.source.source_sha256
                {
                    return Err(super::invalid("changed file differs from source revision"));
                }
                let key = scope.coverage.source.file.clone();
                let _previous = edits.insert(
                    key,
                    FileSourceEdit::Upsert {
                        coverage: scope.coverage,
                        bytes: scope.source_bytes,
                    },
                );
            }
            LexicalChannelOp::TombstoneLexicalScope(payload) => {
                let (_, _, scope) =
                    crate::channel_payloads::decode_tombstone_scope_payload(&payload.payload)?;
                scope.file.validate().map_err(super::invalid)?;
                let key = scope.file;
                let _previous = edits.insert(key.clone(), FileSourceEdit::Delete { key });
            }
            LexicalChannelOp::ClearLexicalSurface(payload)
                if payload.surface == SearchScopeSurface::Chunk =>
            {
                edits.clear();
                clear_all = true;
            }
            LexicalChannelOp::ClearLexicalSurface(_)
            | LexicalChannelOp::FullBundle(_)
            | LexicalChannelOp::UpsertChunk(_)
            | LexicalChannelOp::UpsertSymbol(_)
            | LexicalChannelOp::Seal(_)
            | LexicalChannelOp::UpsertCommit(_)
            | LexicalChannelOp::UpsertRef(_)
            | LexicalChannelOp::UpsertParseTree(_)
            | LexicalChannelOp::ReplaceStructuralScope(_) => {}
        }
    }
    Ok(FileAuthorityMutation {
        edits: edits.into_values().collect(),
        clear_all,
        operations_sha256,
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::FileAuthorityPlanCache;

    #[test]
    fn plan_rebinds_late_root_and_rejects_non_directory_family() {
        let generation = tempfile::tempdir().expect("generation");
        let generation_path =
            std::fs::canonicalize(generation.path()).expect("canonical generation");
        let mut cache = FileAuthorityPlanCache::default();
        cache.prepare(&generation_path, &[]).expect("absent root");
        assert_eq!(cache.preparations(), 1);

        let family = generation_path.join("file-authority");
        symlink(&generation_path, &family).expect("symlink family");
        assert!(cache.prepare(&generation_path, &[]).is_err());
        std::fs::remove_file(&family).expect("remove symlink");

        std::fs::write(&family, b"not a directory").expect("regular family");
        assert!(cache.prepare(&generation_path, &[]).is_err());
        std::fs::remove_file(&family).expect("remove regular family");

        std::fs::create_dir(&family).expect("directory family");
        std::fs::write(family.join("root.cbor"), b"late root").expect("root");
        cache
            .prepare(&generation_path, &[])
            .expect("late root binding");
        assert_eq!(cache.preparations(), 2);
    }

    #[test]
    fn missing_generation_is_absent_but_dangling_family_is_refused() {
        let family = tempfile::tempdir().expect("generation family");
        let family_path = std::fs::canonicalize(family.path()).expect("canonical family");
        let missing = family_path.join("pending-generation");
        let mut cache = FileAuthorityPlanCache::default();
        cache.prepare(&missing, &[]).expect("uncreated generation");
        assert_eq!(cache.preparations(), 1);

        let redirected = family_path.join("redirected-family");
        symlink(family_path.join("absent-target"), &redirected).expect("dangling family");
        assert!(cache.prepare(&redirected.join("generation"), &[]).is_err());
    }
}
