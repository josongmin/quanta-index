//! Durable installation of a private staging file under an owned mutation ticket.

use std::{fs::File, path::Path};

use quanta_index_core::CoreError;
use quanta_index_semantic::SemanticStagingTargetMode;

use crate::publication_file_capacity::{FileMutation, append_failure};

fn add_cleanup(primary: CoreError, context: &str, cleanup: Result<(), CoreError>) -> CoreError {
    match cleanup {
        Ok(()) => primary,
        Err(secondary) => append_failure(primary, context, &secondary),
    }
}

pub(super) fn payload_width(chunks: &[&[u8]]) -> Result<u64, CoreError> {
    chunks.iter().try_fold(0_u64, |total, chunk| {
        u64::try_from(chunk.len())
            .ok()
            .and_then(|length| total.checked_add(length))
            .ok_or_else(|| {
                CoreError::InvalidContract("semantic staging payload length overflow".into())
            })
    })
}

pub(super) fn open(mut mutation: FileMutation) -> Result<(FileMutation, File), CoreError> {
    let temp_path = mutation.allocate_private_temp_path()?;
    mutation.retain_control_create_for_recovery()?;
    mutation.creation_started()?;
    let writer = match mutation.open_declared_temp() {
        Ok(writer) => writer,
        Err(primary) => {
            return Err(add_cleanup(
                primary,
                "uncreated temporary retirement",
                mutation.confirmed_uncreated_absent_and_synced(),
            ));
        }
    };

    if let Err(primary) = mutation.created(&writer, &temp_path) {
        return Err(add_cleanup(
            primary,
            "rejected staging writer retirement",
            mutation.cancel_rejected_created_temp(writer),
        ));
    }
    Ok((mutation, writer))
}

pub(super) fn publish<T>(
    mutation: FileMutation,
    target: &Path,
    mode: SemanticStagingTargetMode,
    context: &str,
    write: impl FnOnce(&mut File) -> Result<T, CoreError>,
) -> Result<Option<T>, CoreError> {
    let (mut mutation, mut writer) = open(mutation)?;
    let output = match write(&mut writer) {
        Ok(output) => output,
        Err(primary) => {
            return Err(add_cleanup(
                primary,
                &format!("{context} cleanup"),
                mutation.cancel_declared_temp(writer),
            ));
        }
    };
    install(mutation, writer, target, mode, context).map(|installed| installed.then_some(output))
}

pub(super) fn install(
    mut mutation: FileMutation,
    writer: File,
    target: &Path,
    mode: SemanticStagingTargetMode,
    context: &str,
) -> Result<bool, CoreError> {
    let mut published = false;
    let operation = (|| {
        writer.sync_all().map_err(|error| {
            CoreError::storage_io(&format!("{context} sync staged bytes"), &error)
        })?;
        match mode {
            SemanticStagingTargetMode::Create => {
                if !mutation.publish_declared_temp_no_replace(&writer, target)? {
                    return Ok(false);
                }
            }
            SemanticStagingTargetMode::Overwrite => {
                mutation.publish_declared_temp_replace(&writer, target)?;
            }
        }
        published = true;
        mutation.sync_promoted_parents()?;
        Ok(true)
    })();
    match operation {
        Ok(true) => {}
        Ok(false) => {
            mutation.cancel_declared_temp(writer)?;
            return Ok(false);
        }
        Err(primary) => {
            let cleanup = if published {
                // Keep the installed inode under the original recovery ticket.
                mutation.confirmed_safe_writer_fd_retired(writer)
            } else {
                mutation.cancel_declared_temp(writer)
            };
            return Err(add_cleanup(primary, &format!("{context} cleanup"), cleanup));
        }
    }

    mutation.confirmed_safe_writer_fd_retired(writer)?;
    let _retained = mutation.settle_after_external_file_and_parent_sync()?;
    mutation.confirmed_safe_pin_retired()?;
    Ok(true)
}
