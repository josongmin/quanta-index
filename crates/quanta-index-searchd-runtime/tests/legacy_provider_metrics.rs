//! OpenAI provider and cache boot metrics from a pinned Legacy process.
//! This is a registration-only profile probe; it sends no provider request.

#![forbid(unsafe_code)]

use std::error::Error;

use quanta_index_searchd_harness::legacy_reference::{LegacySearchdBinary, SearchdBinaryProcess};
use quanta_index_searchd_harness::private_tempdir;

use crate::process_envelope_scrape::{Scrape, expect_eq};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn openai_provider_and_cache_metrics_are_registered_before_any_embedding_request() -> TestResult {
    let parent = private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let mut command = binary.command()?;
    let _configured = command
        .arg("serve")
        .arg("--state-root")
        .arg(&state_root)
        .apply_searchd_env(8)
        .env("QUANTA_INDEX_CACHE_ROOT", parent.path().join("cache"))
        .envs([
            ("QUANTA_INDEX_EMBEDDER", "openai"),
            ("OPENAI_API_KEY", "sk-scrape-test"),
            ("QUANTA_INDEX_EMBED_MODEL", "text-embedding-3-small"),
            ("QUANTA_INDEX_EMBED_MODEL_REVISION", "scrape-test"),
            ("QUANTA_INDEX_EMBED_DIM", "1536"),
            ("QUANTA_INDEX_EMBED_CACHE", "on"),
            ("QUANTA_INDEX_PROVIDER_TENANT", "metrics-test-tenant"),
            (
                "QUANTA_INDEX_PROVIDER_ENDPOINT",
                "https://api.openai.com/v1/embeddings",
            ),
            ("QUANTA_INDEX_PROVIDER_OPENAI_MAX_REQUEST_COST_MICROS", "1"),
            ("QUANTA_INDEX_PROVIDER_REGION", "metrics-test-region"),
            ("QUANTA_INDEX_PROVIDER_RETENTION", "metrics-test-retention"),
            ("QUANTA_INDEX_PROVIDER_PROFILE", "metrics-test-profile"),
            ("QUANTA_INDEX_PROVIDER_SOURCE_CONTENT_CONSENT", "false"),
        ]);
    for key in [
        "QUANTA_INDEX_EMBED_BATCH",
        "QUANTA_INDEX_EMBED_MAX_EST_TOKENS",
        "QUANTA_INDEX_EMBED_MAX_RETRIES",
        "QUANTA_INDEX_EMBED_TIMEOUT_SECS",
        "QUANTA_INDEX_EMBED_CONCURRENCY",
        "QUANTA_INDEX_EMBED_CACHE_MAX_ENTRIES",
        "QUANTA_INDEX_EMBED_CACHE_MAX_BYTES",
        "QUANTA_INDEX_EMBED_CACHE_MAX_NAMESPACES",
        "QUANTA_INDEX_EMBED_CACHE_MAX_AGE_SECS",
        "QUANTA_INDEX_EMBED_CACHE_MAX_TOTAL_BYTES",
    ] {
        let _configured = command.env_remove(key);
    }

    let process = SearchdBinaryProcess::start_with_pinned_command(&state_root, command)?;
    let checked = (|| -> TestResult {
        let snapshot =
            Scrape::from_snapshot(process.connect()?.observability().metrics_snapshot()?);
        for name in [
            "embed_provider_retries_total",
            "embed_provider_http_failures_total",
            "embed_provider_transport_failures_total",
            "embedding_cache_hits_total",
            "embedding_cache_misses_total",
            "embedding_cache_expirations_total",
            "embedding_cache_manifest_flushes_total",
        ] {
            let _registered = snapshot.counter(name)?;
        }
        for name in [
            "embed_provider_http_requests_total",
            "semantic_embedding_raw_vectors_normalized_total",
            "semantic_embedding_raw_vectors_off_unit_total",
        ] {
            expect_eq(name, &snapshot.counter(name)?, &0)?;
        }
        expect_eq(
            "raw norm deviation",
            &snapshot.gauge("semantic_embedding_raw_norm_deviation_max")?,
            &0.0,
        )?;
        for name in [
            "embedding_cache_entries",
            "embedding_cache_retained_foreign_bytes",
        ] {
            let _registered = snapshot.gauge(name)?;
        }
        expect_eq(
            "cache open stat calls",
            &snapshot.gauge("embedding_cache_open_stat_calls")?,
            &0.0,
        )?;
        expect_eq(
            "cache manifest flushes",
            &snapshot.counter("embedding_cache_manifest_flushes_total")?,
            &1,
        )?;
        expect_eq(
            "development profile",
            &snapshot.gauge("boot_semantic_profile_is_dev")?,
            &0.0,
        )?;
        Ok(())
    })();
    let stopped = process.stop();
    checked.and(stopped)?;
    binary.verify()
}
