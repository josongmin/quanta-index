//! The observability port (QI-BB-015).
//!
//! Adapters and registries that keep their own accounting — caches, writer
//! envelopes, snapshot registries, socket servers, the maintenance timer,
//! the embedding provider — expose it to the scrape through one narrow
//! port: a list of named points, read on demand. The composition root
//! collects the sources; the scrape merges their points with the query
//! plane's aggregated samples into one snapshot.
//!
//! # Operator scrape path
//!
//! The snapshot is served over the daemon's control socket (a private Unix
//! socket), never over an HTTP port, so a Prometheus server does not scrape
//! the daemon directly. The supported path is a scheduled
//! `quanta-index-searchctl metrics --output prometheus` run as the daemon's
//! user, writing (temporary file, then rename) into the directory
//! `node_exporter`'s textfile collector reads; `searchctl --help` shows the
//! exact invocation. Every point is process-wide and label-free: no
//! repository, generation or query text becomes a label (QI-BB-015 #4).

use crate::error::CoreError;

/// One scraped value.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricPointV1 {
    /// A registered, closed metric name (`[a-z][a-z0-9_]*`).
    pub name: String,
    pub value: MetricValueV1,
}

/// A counter never goes down between scrapes; a gauge may.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MetricValueV1 {
    Counter(u64),
    Gauge(f64),
}

impl MetricPointV1 {
    #[must_use]
    pub fn counter(name: impl Into<String>, value: u64) -> Self {
        Self {
            name: name.into(),
            value: MetricValueV1::Counter(value),
        }
    }

    #[must_use]
    pub fn gauge(name: impl Into<String>, value: f64) -> Self {
        Self {
            name: name.into(),
            value: MetricValueV1::Gauge(value),
        }
    }

    /// A gauge from an integer count, exact up to 2^53 and the nearest
    /// `f64` beyond, which no count in this daemon approaches.
    #[must_use]
    pub fn gauge_count(name: impl Into<String>, value: u64) -> Self {
        Self::gauge(name, count_as_f64(value))
    }
}

/// Something the scrape reads on demand.
pub trait MetricSourcePort: Send + Sync {
    /// Every point this source can name right now.
    ///
    /// Cheap and lock-light: the scrape calls it under no other lock. A
    /// source that cannot read its own accounting answers typed, and the
    /// whole scrape fails with it rather than serving a snapshot with a
    /// hole in it.
    fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError>;
}

/// A `usize` count as `u64`, saturating on a target wider than 64 bits.
#[must_use]
pub fn count_from_usize(value: usize) -> u64 {
    u64::try_from(value).map_or(u64::MAX, |count| count)
}

/// `u64` to `f64` without an `as` cast: the high and low halves are each
/// exact in `f64`, and their recombination rounds once.
#[must_use]
pub fn count_as_f64(value: u64) -> f64 {
    let high = u32::try_from(value >> 32).map_or(f64::MAX, f64::from);
    let low = u32::try_from(value & 0xFFFF_FFFF).map_or(0.0, f64::from);
    high.mul_add(4_294_967_296.0, low)
}

/// Wire code for a process memory envelope whose declared components sum
/// past its ceiling (QI-BB-016).
pub const PROCESS_MEMORY_ENVELOPE_EXCEEDED_CODE: &str = "PROCESS_MEMORY_ENVELOPE_EXCEEDED";
/// Wire code for a writer refused because the process is already above
/// its resident-memory ceiling (QI-BB-016).
pub const PROCESS_RSS_CEILING_EXCEEDED_CODE: &str = "PROCESS_RSS_CEILING_EXCEEDED";

/// Bytes the embedding cache ledger holds per resident entry: two
/// ordered-map nodes keyed by a 32-byte digest and a tick, their pointers,
/// and the slot's accounting.
pub const EMBEDDING_CACHE_LEDGER_BYTES_PER_ENTRY: u64 = 160;

/// The one process memory envelope every resident byte policy is declared
/// under (QI-BB-016).
///
/// Each component is the byte ceiling one subsystem's policy promises to
/// stay within; the envelope validates that they fit under one configured
/// ceiling at boot, so six independently sensible policies cannot add up
/// to a process the host cannot hold. The optional resident-memory ceiling
/// is the pressure signal the lexical writer gate refuses new writers
/// above; the process gauge `process_resident_bytes` reports what the
/// gate reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessMemoryEnvelopeV1 {
    /// The lexical writer envelope: every open generation writer's heap
    /// together.
    pub lexical_writer_bytes: u64,
    /// The snapshot registry's resident bound over opened generations.
    pub snapshot_registry_bytes: u64,
    /// The lexical regex match cache's resident bound.
    pub regex_match_cache_bytes: u64,
    /// The embedding cache's in-process ledger at its entry ceiling.
    pub embedding_cache_ledger_bytes: u64,
    /// The semantic stream window's vector residency for one batch.
    pub semantic_stream_window_bytes: u64,
    /// The ingest resource envelope's text and vector residency for one
    /// admitted batch.
    pub ingest_batch_bytes: u64,
    /// What the components must fit under together, in bytes.
    pub ceiling: u64,
    /// The resident-memory level, in bytes, above which no new lexical
    /// writer is opened; `None` disables the gate, which the boot log
    /// names.
    pub rss_ceiling: Option<u64>,
}

impl ProcessMemoryEnvelopeV1 {
    /// The ceiling the shipped policy defaults are validated against: two
    /// gibibytes, which the default components fit under with headroom for
    /// mapped segments and query allocations the envelope does not
    /// declare.
    pub const DEFAULT_CEILING_BYTES: u64 = 2 * 1024 * 1024 * 1024;

    /// The components' sum, saturating.
    #[must_use]
    pub const fn declared_bytes(&self) -> u64 {
        self.lexical_writer_bytes
            .saturating_add(self.snapshot_registry_bytes)
            .saturating_add(self.regex_match_cache_bytes)
            .saturating_add(self.embedding_cache_ledger_bytes)
            .saturating_add(self.semantic_stream_window_bytes)
            .saturating_add(self.ingest_batch_bytes)
    }

    /// Refuse, typed, an envelope whose components do not fit under its
    /// ceiling, or whose resident-memory ceiling is below what the
    /// components already declare.
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.ceiling == 0 {
            return Err(CoreError::InvalidContract(
                "process memory envelope: the ceiling must be at least one byte".to_string(),
            ));
        }
        let declared = self.declared_bytes();
        if declared > self.ceiling {
            return Err(CoreError::Typed {
                code: PROCESS_MEMORY_ENVELOPE_EXCEEDED_CODE.to_string(),
                message: format!(
                    "process memory envelope: declared policies sum to {declared} bytes (lexical writers {}, snapshot registry {}, regex cache {}, embedding cache ledger {}, semantic stream window {}, ingest batch {}) over the {} byte ceiling; lower a policy or raise the ceiling",
                    self.lexical_writer_bytes,
                    self.snapshot_registry_bytes,
                    self.regex_match_cache_bytes,
                    self.embedding_cache_ledger_bytes,
                    self.semantic_stream_window_bytes,
                    self.ingest_batch_bytes,
                    self.ceiling
                ),
            });
        }
        if let Some(rss_ceiling) = self.rss_ceiling
            && rss_ceiling < self.ceiling
        {
            return Err(CoreError::InvalidContract(format!(
                "process memory envelope: the resident-memory ceiling {rss_ceiling} is below the envelope ceiling {}; the gate would refuse every writer the envelope allows",
                self.ceiling
            )));
        }
        Ok(())
    }
}

/// Where the process learns its own resident memory.
///
/// A port so the writer gate and the scrape are provable with a scripted
/// reading; the daemon reads the kernel's own accounting.
pub trait ProcessMemoryProbePort: Send + Sync {
    /// Bytes of this process resident in memory now; on a platform that
    /// only reports a high-water mark, that mark, which the implementer
    /// documents.
    fn resident_bytes(&self) -> Result<u64, CoreError>;
}

/// Admits or refuses opening one more lexical generation writer
/// (QI-BB-016).
///
/// The lexical adapter asks before it opens a writer it does not already
/// hold; a refusal is the typed error the build surfaces, never a writer
/// that is opened anyway.
pub trait WriterAdmissionPort: Send + Sync {
    fn admit_writer_open(&self) -> Result<(), CoreError>;
}

/// The gate with no resident-memory ceiling: every open is admitted. The
/// composition root installs it when the envelope names no ceiling and
/// says so in the boot log.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UnboundedWriterAdmission;

impl WriterAdmissionPort for UnboundedWriterAdmission {
    fn admit_writer_open(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

/// The gate that refuses a new writer while the process is above its
/// resident-memory ceiling; every refusal is counted for the scrape.
pub struct ResidentMemoryWriterAdmission {
    probe: std::sync::Arc<dyn ProcessMemoryProbePort>,
    ceiling_bytes: u64,
    refusals: std::sync::atomic::AtomicU64,
}

impl core::fmt::Debug for ResidentMemoryWriterAdmission {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ResidentMemoryWriterAdmission")
            .field("ceiling_bytes", &self.ceiling_bytes)
            .field("refusals", &self.refusals())
            .finish_non_exhaustive()
    }
}

impl ResidentMemoryWriterAdmission {
    #[must_use]
    pub fn new(probe: std::sync::Arc<dyn ProcessMemoryProbePort>, ceiling_bytes: u64) -> Self {
        Self {
            probe,
            ceiling_bytes,
            refusals: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Writers refused so far because the process was above the ceiling.
    #[must_use]
    pub fn refusals(&self) -> u64 {
        self.refusals.load(std::sync::atomic::Ordering::Acquire)
    }
}

/// The gate's ceiling and refusals as scrape points; the unbounded gate
/// reports the gate as disabled.
impl MetricSourcePort for ResidentMemoryWriterAdmission {
    fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
        Ok(vec![
            MetricPointV1::gauge_count("lexical_writer_rss_gate_enabled", 1),
            MetricPointV1::gauge_count("lexical_writer_rss_ceiling_bytes", self.ceiling_bytes),
            MetricPointV1::counter("lexical_writer_rss_refusals_total", self.refusals()),
        ])
    }
}

impl MetricSourcePort for UnboundedWriterAdmission {
    fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
        Ok(vec![
            MetricPointV1::gauge_count("lexical_writer_rss_gate_enabled", 0),
            MetricPointV1::counter("lexical_writer_rss_refusals_total", 0),
        ])
    }
}

impl WriterAdmissionPort for ResidentMemoryWriterAdmission {
    fn admit_writer_open(&self) -> Result<(), CoreError> {
        let resident = self.probe.resident_bytes()?;
        if resident > self.ceiling_bytes {
            let _prior = self
                .refusals
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            return Err(CoreError::Typed {
                code: PROCESS_RSS_CEILING_EXCEEDED_CODE.to_string(),
                message: format!(
                    "lexical: refusing to open another generation writer: the process holds {resident} resident bytes, above the {} byte ceiling",
                    self.ceiling_bytes
                ),
            });
        }
        Ok(())
    }
}

/// Commits and releases every generation writer nothing has touched for
/// the policy's idle interval (QI-BB-016).
///
/// The composition root's maintenance timer calls it on its own schedule,
/// so an idle producer's heap goes back to the envelope whether or not
/// another batch ever arrives.
pub trait WriterIdleSweepPort: Send + Sync {
    /// Sweep once; the count is how many writers this sweep released.
    fn sweep_idle_writers(&self) -> Result<u64, CoreError>;
}

/// Measures what one search-corpus track holds on disk (QI-BB-015).
///
/// Each adapter answers with its own byte walker over every generation
/// directory it owns; the composition root refreshes the per-track gauge
/// from it on the maintenance timer, never inside a scrape.
pub trait TrackDiskUsagePort: Send + Sync {
    /// Regular-file bytes under every generation directory of the track.
    fn track_disk_bytes(&self) -> Result<u64, CoreError>;
}

#[cfg(test)]
mod tests {
    use super::{
        PROCESS_MEMORY_ENVELOPE_EXCEEDED_CODE, PROCESS_RSS_CEILING_EXCEEDED_CODE,
        ProcessMemoryEnvelopeV1, ProcessMemoryProbePort, ResidentMemoryWriterAdmission,
        WriterAdmissionPort,
    };
    use crate::error::CoreError;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn envelope(ceiling: u64) -> ProcessMemoryEnvelopeV1 {
        ProcessMemoryEnvelopeV1 {
            lexical_writer_bytes: 100,
            snapshot_registry_bytes: 200,
            regex_match_cache_bytes: 300,
            embedding_cache_ledger_bytes: 400,
            semantic_stream_window_bytes: 500,
            ingest_batch_bytes: 600,
            ceiling,
            rss_ceiling: None,
        }
    }

    #[test]
    fn the_envelope_sums_every_component_and_refuses_a_sum_over_the_ceiling() {
        assert_eq!(envelope(2_100).declared_bytes(), 2_100);
        assert!(envelope(2_100).validate().is_ok());
        let over = envelope(2_099).validate();
        assert!(
            matches!(&over, Err(CoreError::Typed { code, message })
                if code == PROCESS_MEMORY_ENVELOPE_EXCEEDED_CODE && message.contains("2100 bytes")),
            "{over:?}"
        );
        assert!(envelope(0).validate().is_err());
        let mut narrow_rss = envelope(2_100);
        narrow_rss.rss_ceiling = Some(2_099);
        assert!(narrow_rss.validate().is_err());
        narrow_rss.rss_ceiling = Some(2_100);
        assert!(narrow_rss.validate().is_ok());
    }

    struct ScriptedProbe(AtomicU64);

    impl ProcessMemoryProbePort for ScriptedProbe {
        fn resident_bytes(&self) -> Result<u64, CoreError> {
            Ok(self.0.load(Ordering::Acquire))
        }
    }

    #[test]
    fn the_resident_memory_gate_refuses_typed_above_the_ceiling_and_counts_it() {
        let probe = Arc::new(ScriptedProbe(AtomicU64::new(1_000)));
        let probe_port: Arc<dyn ProcessMemoryProbePort> = probe.clone();
        let gate = ResidentMemoryWriterAdmission::new(probe_port, 1_000);
        assert!(gate.admit_writer_open().is_ok());
        probe.0.store(1_001, Ordering::Release);
        let refused = gate.admit_writer_open();
        assert!(
            matches!(&refused, Err(CoreError::Typed { code, .. })
                if code == PROCESS_RSS_CEILING_EXCEEDED_CODE),
            "{refused:?}"
        );
        assert_eq!(gate.refusals(), 1);
        probe.0.store(999, Ordering::Release);
        assert!(gate.admit_writer_open().is_ok());
        assert_eq!(gate.refusals(), 1);
    }
}
