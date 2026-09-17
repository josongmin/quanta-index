//! The observability port (QI-BB-015).
//!
//! Adapters and registries that keep their own accounting — caches, writer
//! envelopes, snapshot registries, socket servers — expose it to the scrape
//! through one narrow port: a list of named points, read on demand. The
//! composition root collects the sources; the scrape merges their points
//! with the query plane's aggregated samples into one snapshot.

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
