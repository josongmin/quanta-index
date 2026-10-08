//! One admitted metadata plan per publication input. Reuse authenticates the
//! current root/manifest bytes and repeats filesystem admission on every call.

use std::io::Write;
use std::path::Path;

use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_core::CoreError;
use sha2::{Digest as _, Sha256};

use super::FileAuthorityDelta;

#[derive(Eq, PartialEq)]
struct PlanBinding {
    root: Option<[u8; 32]>,
    manifest: Option<[u8; 32]>,
    operations: [u8; 32],
    policy: [u8; 32],
}

/// Bounded by the existing manifest and source admission. Only source metadata,
/// changed-source lengths and encoded manifest bytes survive a preflight.
#[derive(Default)]
pub(crate) struct FileAuthorityPlanCache {
    entry: Option<(PlanBinding, FileAuthorityDelta)>,
    #[cfg(test)]
    preparations: u64,
}

impl FileAuthorityPlanCache {
    pub(crate) fn prepare(
        &mut self,
        directory: &Path,
        ops: &[LexicalChannelOp],
    ) -> Result<(), CoreError> {
        // Read bounded, nofollow inputs before consulting custody. Metadata
        // equality, including inode and restored mtime, grants no reuse.
        let root_bytes = super::read_root_bytes(directory)?;
        let manifest_bytes = super::read_manifest_bytes(directory)?;
        let binding = PlanBinding {
            root: root_bytes
                .as_deref()
                .map(|bytes| Sha256::digest(bytes).into()),
            manifest: manifest_bytes
                .as_deref()
                .map(|bytes| Sha256::digest(bytes).into()),
            operations: operations_sha256(ops)?,
            policy: super::policy().digest(),
        };
        if let Some((prior, plan)) = self.entry.as_ref()
            && *prior == binding
        {
            return crate::causal_profile::timed_work(
                "lexical_file_authority_plan_revalidate",
                || super::validate_plan_storage(directory, plan),
            );
        }
        let plan =
            crate::causal_profile::timed_work("lexical_file_authority_plan_prepare", || {
                let root = root_bytes
                    .as_deref()
                    .map(|bytes| super::decode_root_bytes(directory, bytes))
                    .transpose()?;
                let rows = match manifest_bytes.as_deref() {
                    Some(bytes) => super::decode_verified_manifest(bytes, directory)?,
                    None => root
                        .as_ref()
                        .into_iter()
                        .flat_map(|root| {
                            root.sources
                                .iter()
                                .map(|row| (row.source.clone(), row.posting_memberships))
                        })
                        .collect(),
                };
                let plan = super::derive_plan_ops(ops, root.as_ref(), rows, binding.operations)?;
                super::validate_plan_storage(directory, &plan)?;
                Ok::<_, CoreError>(plan)
            })?;
        self.entry = Some((binding, plan));
        #[cfg(test)]
        {
            self.preparations = self.preparations.saturating_add(1);
        }
        Ok(())
    }

    /// Authenticate the writer's actual target, then transfer the plan exactly
    /// once. A partial retry with different target inputs derives a fresh plan.
    pub(crate) fn take(
        &mut self,
        directory: &Path,
        ops: &[LexicalChannelOp],
    ) -> Result<FileAuthorityDelta, CoreError> {
        self.prepare(directory, ops)?;
        self.entry
            .take()
            .map(|(_, plan)| plan)
            .ok_or_else(|| super::invalid("admitted file authority plan is unavailable"))
    }

    #[cfg(test)]
    pub(crate) fn preparations(&self) -> u64 {
        self.preparations
    }
}

/// Raw callers use the same planner with fresh custody.
pub(crate) fn plan_ops(
    directory: &Path,
    ops: &[LexicalChannelOp],
) -> Result<FileAuthorityDelta, CoreError> {
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
        .update(b"quanta-index:file-authority-plan-operations:v1\0");
    for op in ops {
        // Bind exactly the operations consumed by derive_plan_ops. Seal and
        // other authorities have no file mutation semantics. The containing
        // publication owner separately binds the complete batch body.
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
