//! Independent disk and boot expectations for the seal and GC crash points.

use std::collections::BTreeSet;
use std::error::Error;

use quanta_index_search_plane::crash_point::{
    self, AFTER_CATALOG_TRANSACTION, AFTER_FENCE, AFTER_LEDGER_RECONCILE, AFTER_RETENTION_RECEIPT,
    AFTER_SEMANTIC_SEAL, BEFORE_AUTHORITY_RECORD, BEFORE_RECORD_FORGET, BETWEEN_TRACK_RECLAIMS,
};

pub(super) struct SealCase {
    pub(super) point: &'static str,
    /// Remove the lexical identity to reproduce a crash inside that seal.
    pub(super) unseal_lexical: bool,
    /// Whether generation 2 has a directory for each of lexical and semantic.
    pub(super) dirs: [bool; 2],
    /// The retry must keep existing files for each sealed track.
    pub(super) sealed: [bool; 2],
    pub(super) half_sealed_pairs: u32,
}

pub(super) const SEAL_CASES: [SealCase; 3] = [
    SealCase {
        point: AFTER_SEMANTIC_SEAL,
        unseal_lexical: false,
        dirs: [false, true],
        sealed: [false, true],
        half_sealed_pairs: 1,
    },
    SealCase {
        point: BEFORE_AUTHORITY_RECORD,
        unseal_lexical: false,
        dirs: [true, true],
        sealed: [true, true],
        half_sealed_pairs: 0,
    },
    SealCase {
        point: BEFORE_AUTHORITY_RECORD,
        unseal_lexical: true,
        dirs: [true, true],
        sealed: [false, true],
        half_sealed_pairs: 1,
    },
];

pub(super) struct GcCase {
    pub(super) point: &'static str,
    /// Whether lexical and semantic still hold the retired generation.
    pub(super) retired_left: [bool; 2],
}

pub(super) const GC_CASES: [GcCase; 6] = [
    GcCase {
        point: AFTER_RETENTION_RECEIPT,
        retired_left: [true, true],
    },
    GcCase {
        point: AFTER_CATALOG_TRANSACTION,
        retired_left: [true, true],
    },
    GcCase {
        point: AFTER_LEDGER_RECONCILE,
        retired_left: [true, true],
    },
    GcCase {
        point: AFTER_FENCE,
        retired_left: [true, true],
    },
    GcCase {
        point: BETWEEN_TRACK_RECLAIMS,
        retired_left: [false, true],
    },
    GcCase {
        point: BEFORE_RECORD_FORGET,
        retired_left: [false, false],
    },
];

pub(super) fn require_declared_crash_points() -> Result<(), Box<dyn Error>> {
    let cased: BTreeSet<&str> = SEAL_CASES
        .iter()
        .map(|case| case.point)
        .chain(GC_CASES.iter().map(|case| case.point))
        .collect();
    let declared: BTreeSet<&str> = crash_point::ALL.into_iter().collect();
    if cased != declared {
        return Err(format!(
            "points without a case: {:?}; cases without a declared point: {:?}",
            declared.difference(&cased).collect::<Vec<_>>(),
            cased.difference(&declared).collect::<Vec<_>>(),
        )
        .into());
    }
    Ok(())
}
