//! Shared scrape oracles for the Legacy envelope tests and public old-Q SDK metrics.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::time::Duration;

use quanta_index_contract::MetricsSnapshotV1;

use crate::fail_closed_wait::{RealTicker, WaitError, wait_for};

#[derive(Debug)]
pub(super) struct Scrape {
    pub(super) counters: BTreeMap<String, u64>,
    pub(super) gauges: BTreeMap<String, f64>,
}

impl Scrape {
    pub(super) fn from_snapshot(snapshot: MetricsSnapshotV1) -> Self {
        Self {
            counters: snapshot
                .counters
                .into_iter()
                .map(|counter| (counter.name, counter.value))
                .collect(),
            gauges: snapshot
                .gauges
                .into_iter()
                .map(|gauge| (gauge.name, gauge.value))
                .collect(),
        }
    }

    pub(super) fn counter(&self, name: &str) -> Result<u64, Box<dyn Error>> {
        self.counters
            .get(name)
            .copied()
            .ok_or_else(|| format!("counter `{name}` is in the scrape").into())
    }

    pub(super) fn gauge(&self, name: &str) -> Result<f64, Box<dyn Error>> {
        self.gauges
            .get(name)
            .copied()
            .ok_or_else(|| format!("gauge `{name}` is in the scrape").into())
    }
}

/// Compare through `PartialEq` so a gauge (an `f64` carrying an exact
/// integer count) is checked for the value the daemon put on the wire.
pub(super) fn expect_eq<T: PartialEq + std::fmt::Debug>(
    what: &str,
    observed: &T,
    expected: &T,
) -> Result<(), Box<dyn Error>> {
    if observed == expected {
        Ok(())
    } else {
        Err(format!("{what}: observed {observed:?}, expected {expected:?}").into())
    }
}

pub(super) fn wait_for_scrape_with(
    read: impl FnMut() -> Result<Scrape, Box<dyn Error>>,
    bound: Duration,
    what: &str,
    condition: impl Fn(&Scrape) -> bool,
) -> Result<Scrape, Box<dyn Error>> {
    match wait_for(
        &RealTicker::new(),
        bound,
        Duration::from_millis(10),
        what,
        read,
        condition,
        |_| false,
    ) {
        Ok(scrape) => Ok(scrape),
        // A boxed scrape failure is already the terminal error: return
        // it unwrapped, never rendered-and-reboxed.
        Err(WaitError::Terminal(boxed)) => Err(boxed),
        Err(WaitError::Timeout(timeout)) => Err(Box::new(timeout)),
    }
}
