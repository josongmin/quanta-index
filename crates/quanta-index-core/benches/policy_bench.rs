//! Baseline policy validation benchmarks.
//!
//! These exist primarily as regression guards: validators sit on the hot path
//! of bundle ingestion and query serving, so we want CI signal if their cost
//! grows by an order of magnitude.

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use quanta_index_contract::{
    BundleArtifactRef, BundleEncoding, BundleMode, GenerationId, LqDirectiveSet, LqExpr,
    LqFilterSet, LqOptionSet, LqQuery, ManifestDigest, ManifestGeneration, PreparedBundleOutbox,
    PublishedGenerationSet, PublishedSearchGenerationActivateRequest, RepoId, RevisionId,
};
use quanta_index_core::{ActivationPolicy, BundlePolicy, QueryPolicy};

fn sample_outbox() -> PreparedBundleOutbox {
    PreparedBundleOutbox {
        outbox_id: "outbox-1".into(),
        repo_id: RepoId::new("repo"),
        revision_id: RevisionId::new("rev"),
        manifest_digest: ManifestDigest::new("digest"),
        bundle_schema_version: 1,
        prepared_at_ms: 1,
        mode: BundleMode::ServeOnly,
        manifest_ref: BundleArtifactRef {
            relative_path: "bundle/manifest.json".into(),
            encoding: BundleEncoding::Json,
            byte_length: 128,
            content_digest: ManifestDigest::new("digest"),
        },
        base_generation: None,
        changed_artifact_mask: 1,
    }
}

fn sample_request() -> PublishedSearchGenerationActivateRequest {
    PublishedSearchGenerationActivateRequest {
        generation: PublishedGenerationSet {
            repo_id: RepoId::new("repo"),
            revision_id: RevisionId::new("rev"),
            manifest_generation: ManifestGeneration::new(7),
            lexical_generation: GenerationId::new(10),
            symbol_generation: GenerationId::new(11),
            structural_generation: None,
            history_generation: None,
            semantic_generation: None,
            metadata_generation: None,
        },
        lexical_ready: true,
        semantic_ready: true,
        active_at_ms: 42,
    }
}

fn sample_query() -> LqQuery {
    LqQuery {
        expr: LqExpr::Raw("symbol:Foo AND path:src/lib.rs".into()),
        filters: LqFilterSet::default(),
        options: LqOptionSet::default(),
        directives: LqDirectiveSet::default(),
    }
}

#[expect(
    unused_results,
    reason = "criterion's bench_function returns &mut Criterion for fluent chaining"
)]
fn bench_bundle_policy(c: &mut Criterion) {
    let outbox = sample_outbox();
    c.bench_function("BundlePolicy::validate_outbox/ok", |b| {
        b.iter(|| BundlePolicy::validate_outbox(black_box(&outbox)));
    });
}

#[expect(
    unused_results,
    reason = "criterion's bench_function returns &mut Criterion for fluent chaining"
)]
fn bench_activation_policy(c: &mut Criterion) {
    let request = sample_request();
    c.bench_function("ActivationPolicy::validate_request/ok", |b| {
        b.iter(|| ActivationPolicy::validate_request(black_box(&request)));
    });
}

#[expect(
    unused_results,
    reason = "criterion's bench_function returns &mut Criterion for fluent chaining"
)]
fn bench_query_policy(c: &mut Criterion) {
    let query = sample_query();
    c.bench_function("QueryPolicy::validate_query/ok", |b| {
        b.iter(|| QueryPolicy::validate_query(black_box(&query)));
    });
}

criterion_group!(
    policy,
    bench_bundle_policy,
    bench_activation_policy,
    bench_query_policy
);
criterion_main!(policy);
