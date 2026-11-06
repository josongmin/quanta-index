//! Connection-owned Catalog writer settings sampled at successful native open.
//! These instantaneous values do not measure PRAGMA execution time or admit memory.

use quanta_index_catalog::{CatalogWriterCacheObservation, CatalogWriterSettings};
use quanta_index_core::{
    DiagnosticContextV1, DiagnosticCounterV1, DiagnosticLaneV1, DiagnosticRootKindV1,
    DiagnosticStageV1, DiagnosticTerminalV1,
};

pub(crate) fn record_catalog_writer_cache(
    context: Option<&DiagnosticContextV1>,
    observation: &CatalogWriterCacheObservation,
) {
    let Some(context) = context else {
        return;
    };
    let Some(root) = crate::runtime_diagnostic_span(
        context,
        context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0)),
    ) else {
        return;
    };
    let child = context.child_context(&root);
    record_settings(&child, DiagnosticLaneV1(27), observation.before);
    record_settings(&child, DiagnosticLaneV1(28), observation.after);
    let _observed = root.finish(DiagnosticTerminalV1::Ok);
}

fn record_settings(
    context: &DiagnosticContextV1,
    lane: DiagnosticLaneV1,
    settings: CatalogWriterSettings,
) {
    let Some(span) =
        crate::runtime_diagnostic_span(context, context.enter(DiagnosticStageV1::SqliteWork, lane))
    else {
        return;
    };
    for (key, value) in [
        (
            DiagnosticCounterV1::SqlitePageSizeBytes,
            settings.page_size_bytes,
        ),
        (
            DiagnosticCounterV1::SqliteCacheSizeMagnitude,
            settings.cache_size.unsigned_abs(),
        ),
        (
            DiagnosticCounterV1::SqliteCacheSizeIsKib,
            u64::from(settings.cache_size < 0),
        ),
        (
            DiagnosticCounterV1::SqliteCacheSpillPages,
            settings.cache_spill_pages,
        ),
        (
            DiagnosticCounterV1::SqliteWalAutocheckpointPages,
            settings.wal_autocheckpoint_pages,
        ),
    ] {
        context.counter(&span, key, value);
    }
    let _observed = span.finish(DiagnosticTerminalV1::Ok);
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        io,
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, Ordering},
        },
    };

    use quanta_index_core::{
        DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticEmitV1, DiagnosticEventKindV1,
        DiagnosticEventSinkV1,
    };

    use super::*;

    #[derive(Debug, Default)]
    struct Clock(AtomicU64);

    impl DiagnosticClockV1 for Clock {
        fn now_ns(&self) -> u64 {
            self.0.fetch_add(1, Ordering::Relaxed)
        }
    }

    #[derive(Debug, Default)]
    struct Events(Mutex<Vec<DiagnosticEmitV1>>);

    impl DiagnosticEventSinkV1 for Events {
        fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
            let Ok(mut events) = self.0.lock() else {
                return false;
            };
            events.push(event);
            true
        }
    }

    fn require(condition: bool, detail: &str) -> Result<(), io::Error> {
        if condition {
            Ok(())
        } else {
            Err(io::Error::other(detail.to_owned()))
        }
    }

    fn require_eq<A: std::fmt::Debug + PartialEq<B>, B: std::fmt::Debug>(
        actual: &A,
        expected: B,
    ) -> Result<(), io::Error> {
        if actual == &expected {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "Catalog settings diagnostic gold differs: actual={actual:?}, expected={expected:?}"
            )))
        }
    }

    #[test]
    fn off_is_inert_and_record_emits_exact_before_after_settings() -> Result<(), io::Error> {
        let clock = Arc::new(Clock::default());
        let events = Arc::new(Events::default());
        let observation = CatalogWriterCacheObservation {
            before: CatalogWriterSettings {
                page_size_bytes: 4096,
                cache_size: -2000,
                cache_spill_pages: 2000,
                wal_autocheckpoint_pages: 1000,
            },
            after: CatalogWriterSettings {
                page_size_bytes: 4096,
                cache_size: -8192,
                cache_spill_pages: 8192,
                wal_autocheckpoint_pages: 1000,
            },
        };
        record_catalog_writer_cache(None, &observation);
        require_eq(&clock.0.load(Ordering::Relaxed), 0)?;
        require(
            events
                .0
                .lock()
                .map_err(|error| io::Error::other(format!("events lock poisoned: {error}")))?
                .is_empty(),
            "Off emitted Catalog settings diagnostics",
        )?;

        let context =
            DiagnosticContextV1::new(DiagnosticCaptureIdV1([7; 16]), clock, events.clone());
        record_catalog_writer_cache(Some(&context), &observation);
        let rows = {
            let mut events = events
                .0
                .lock()
                .map_err(|error| io::Error::other(format!("events lock poisoned: {error}")))?;
            std::mem::take(&mut *events)
        };
        let roots: Vec<_> = rows
            .iter()
            .filter_map(|row| {
                if let DiagnosticEventKindV1::SpanBegin {
                    span_id,
                    parent_span_id: None,
                    root_kind: Some(DiagnosticRootKindV1::Operation),
                    stage: DiagnosticStageV1::Operation,
                    lane: DiagnosticLaneV1(0),
                } = row.kind
                {
                    Some(span_id)
                } else {
                    None
                }
            })
            .collect();
        require_eq(&roots.len(), 1)?;
        let root = roots
            .into_iter()
            .next()
            .ok_or_else(|| io::Error::other("missing operation root"))?;
        let mut lanes = BTreeMap::new();
        for row in &rows {
            if let DiagnosticEventKindV1::SpanBegin {
                span_id,
                parent_span_id: Some(parent),
                stage: DiagnosticStageV1::SqliteWork,
                lane,
                ..
            } = row.kind
            {
                require_eq(&parent, root)?;
                require(
                    lanes.insert(lane.0, span_id).is_none(),
                    "duplicate Catalog settings diagnostic lane",
                )?;
            }
        }
        require_eq(&lanes.keys().copied().collect::<Vec<_>>(), [27, 28])?;
        for (lane, expected) in [
            (27, (4096, 2000, 1, 2000, 1000)),
            (28, (4096, 8192, 1, 8192, 1000)),
        ] {
            let span_id = *lanes
                .get(&lane)
                .ok_or_else(|| io::Error::other("missing settings lane"))?;
            let counters: Vec<_> = rows
                .iter()
                .filter_map(|row| {
                    if let DiagnosticEventKindV1::Counter {
                        span_id: owner,
                        key,
                        value,
                    } = row.kind
                    {
                        (owner == span_id).then_some((key, value))
                    } else {
                        None
                    }
                })
                .collect();
            require_eq(
                &counters,
                [
                    (DiagnosticCounterV1::SqlitePageSizeBytes, expected.0),
                    (DiagnosticCounterV1::SqliteCacheSizeMagnitude, expected.1),
                    (DiagnosticCounterV1::SqliteCacheSizeIsKib, expected.2),
                    (DiagnosticCounterV1::SqliteCacheSpillPages, expected.3),
                    (
                        DiagnosticCounterV1::SqliteWalAutocheckpointPages,
                        expected.4,
                    ),
                ],
            )?;
        }
        require_eq(
            &rows
                .iter()
                .filter(|row| {
                    matches!(
                        row.kind,
                        DiagnosticEventKindV1::SpanEnd {
                            outcome: DiagnosticTerminalV1::Ok,
                            ..
                        }
                    )
                })
                .count(),
            3,
        )?;
        Ok(())
    }
}
