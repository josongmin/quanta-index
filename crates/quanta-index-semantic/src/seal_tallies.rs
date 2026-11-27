//! Seal measurement counters shared by the builder and adapter.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use std::sync::atomic::{AtomicU64, Ordering};

use quanta_index_core::MetricPointV1;

/// What the adapter's seals measured, for the metrics scrape
/// (QI-BB-006 #4): the bytes a seal hashed itself against the bytes and
/// files whose digest it inherited from its base.
#[derive(Debug, Default)]
pub(crate) struct SealTalliesV1 {
    hashed_bytes: AtomicU64,
    inherited_bytes: AtomicU64,
    inherited_files: AtomicU64,
}

impl SealTalliesV1 {
    pub(crate) fn record(&self, measurement: crate::sealed_manifest::SealMeasurementV1) {
        let _previous = self
            .hashed_bytes
            .fetch_add(measurement.hashed_bytes, Ordering::Relaxed);
        let _previous = self
            .inherited_bytes
            .fetch_add(measurement.inherited_bytes, Ordering::Relaxed);
        let _previous = self
            .inherited_files
            .fetch_add(measurement.inherited_files, Ordering::Relaxed);
    }

    /// `semantic_seal_hashed_bytes_total`, `semantic_seal_inherited_bytes_total`
    /// and `semantic_seal_inherited_files_total`.
    pub(crate) fn scrape(&self) -> Vec<MetricPointV1> {
        vec![
            MetricPointV1::counter(
                "semantic_seal_hashed_bytes_total",
                self.hashed_bytes.load(Ordering::Relaxed),
            ),
            MetricPointV1::counter(
                "semantic_seal_inherited_bytes_total",
                self.inherited_bytes.load(Ordering::Relaxed),
            ),
            MetricPointV1::counter(
                "semantic_seal_inherited_files_total",
                self.inherited_files.load(Ordering::Relaxed),
            ),
        ]
    }
}
