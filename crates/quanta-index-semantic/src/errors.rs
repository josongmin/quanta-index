//! Shared `CoreError` constructors for the semantic crate's storage paths.
//!
//! `build.rs` and `search.rs` both wrap lancedb/arrow/fs failures into
//! `CoreError::Storage` with an identical `semantic: …` prefix; keeping the
//! constructors here is the single source of truth for those message shapes.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use std::error::Error;
use std::path::Path;

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::CoreError;

pub(crate) fn fs_err(action: &str, path: &Path, err: &std::io::Error) -> CoreError {
    if let Some(typed) = admitted_cause(err) {
        return typed;
    }
    CoreError::Storage(format!("semantic: {action} {}: {err}", path.display()))
}

/// Preserve typed capacity and filesystem causes in the real source chain.
/// OS ENOSPC and string lookalikes never become capacity refusals.
fn admitted_cause(err: &(dyn Error + 'static)) -> Option<CoreError> {
    let mut source: Option<&(dyn Error + 'static)> = Some(err);
    while let Some(error) = source {
        if let Some(typed) = error.downcast_ref::<CoreError>()
            && (matches!(typed, CoreError::Typed { code, .. } if *code == SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded)
                || matches!(typed, CoreError::StorageIo { .. }))
        {
            return Some(typed.clone());
        }
        if let Some(io) = error.downcast_ref::<std::io::Error>()
            && io.raw_os_error().is_some()
        {
            return Some(CoreError::storage_io("semantic filesystem", io));
        }
        // `io::Error::source` can skip the custom inner error itself. Inspect
        // its owned payload before following the ordinary source chain.
        source = error
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
            .map(|inner| -> &(dyn Error + 'static) { inner })
            .or_else(|| error.source());
    }
    None
}

pub(crate) fn lancedb_err(action: &str, err: impl Error + 'static) -> CoreError {
    if let Some(typed) = admitted_cause(&err) {
        return typed;
    }
    CoreError::Storage(format!("semantic: lancedb {action}: {err}"))
}

pub(crate) fn admitted_fs_err(action: &str, err: impl Error + 'static) -> CoreError {
    if let Some(typed) = admitted_cause(&err) {
        return typed;
    }
    CoreError::Storage(format!("semantic: admitted fs {action}: {err}"))
}

pub(crate) fn arrow_err(action: &str, err: impl core::fmt::Display) -> CoreError {
    CoreError::Storage(format!("semantic: arrow {action}: {err}"))
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn real_nested_budget_cause_keeps_its_public_code() {
        let expected = CoreError::Typed {
            code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            message: "physical writer budget".into(),
        };
        let observed = lancedb_err("table.add", io::Error::other(expected));
        assert!(matches!(
            observed,
            CoreError::Typed {
                code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                ..
            }
        ));
    }

    #[test]
    fn actual_enospc_and_text_lookalike_remain_storage() {
        let enospc = lancedb_err("table.add", io::Error::from_raw_os_error(28));
        let lookalike = lancedb_err(
            "table.add",
            io::Error::other("INGEST_RESOURCE_BUDGET_EXCEEDED"),
        );
        assert!(matches!(
            enospc,
            CoreError::StorageIo {
                kind: io::ErrorKind::StorageFull,
                raw_os_error: Some(28),
                ..
            }
        ));
        assert!(matches!(lookalike, CoreError::Storage(_)));
    }

    #[test]
    fn nested_filesystem_cause_keeps_kind_and_raw_errno() {
        let original = io::Error::from_raw_os_error(rustix::io::Errno::EXIST.raw_os_error());
        let raw = original.raw_os_error();
        let cause = CoreError::storage_io("renameat", &original);
        let observed = admitted_fs_err("move", io::Error::new(original.kind(), cause));
        assert!(matches!(observed, CoreError::StorageIo {
            kind: io::ErrorKind::AlreadyExists, raw_os_error, ..
        } if raw_os_error == raw));
    }
}
