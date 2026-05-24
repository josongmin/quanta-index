//! Pipeline-stage benches: tokenize / parse / normalize / hash.
//!
//! Three deterministic input sizes (1 KiB / 4 KiB / 16 KiB) per stage.
//! The 16 KiB size sits at the parser's maximum input cap; the smaller
//! sizes track sub-millisecond regression budgets.

use criterion::{Criterion, criterion_group, criterion_main};

use quanta_index_lq_norm::hasher::canonical_hash;
use quanta_index_lq_norm::normalizer::normalize;
use quanta_index_lq_norm::parser::parse;
use quanta_index_lq_norm::tokenizer::tokenize;

/// Build a deterministic query of approximately `target_len` bytes.
///
/// Repeats a short keyword + filter pattern; the resulting input is still
/// valid LQ within the parser's caps. We pick a pattern whose AST fans
/// out within the per-node 64-child cap.
fn build_query(target_len: usize) -> String {
    let unit = "alpha OR beta lang:rust ";
    let mut out = String::with_capacity(target_len);
    loop {
        let Some(after) = out.len().checked_add(unit.len()) else {
            break;
        };
        if after > target_len {
            break;
        }
        out.push_str(unit);
    }
    // Trailing keyword so we never end on a dangling boolean operator.
    out.push_str("zeta");
    out
}

fn bench_tokenize(c: &mut Criterion) {
    let mut g = c.benchmark_group("tokenize");
    for &size in &[1_024_usize, 4_096, 16_384] {
        let input = build_query(size);
        let _registered: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime> = g
            .bench_with_input(
                criterion::BenchmarkId::from_parameter(size),
                &input,
                |b, s| {
                    b.iter(|| {
                        let res = tokenize(criterion::black_box(s.as_str()));
                        let _kept = criterion::black_box(&res);
                    });
                },
            );
    }
    g.finish();
}

fn bench_parse(c: &mut Criterion) {
    let mut g = c.benchmark_group("parse");
    for &size in &[1_024_usize, 4_096, 16_384] {
        let input = build_query(size);
        let tokens = match tokenize(&input) {
            Ok(t) => t,
            Err(_e) => continue,
        };
        let _registered: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime> = g
            .bench_with_input(
                criterion::BenchmarkId::from_parameter(size),
                &input,
                |b, s| {
                    b.iter(|| {
                        let res = parse(
                            criterion::black_box(&tokens),
                            criterion::black_box(s.as_str()),
                        );
                        let _kept = criterion::black_box(&res);
                    });
                },
            );
    }
    g.finish();
}

fn bench_normalize(c: &mut Criterion) {
    let mut g = c.benchmark_group("normalize");
    for &size in &[1_024_usize, 4_096, 16_384] {
        let input = build_query(size);
        let tokens = match tokenize(&input) {
            Ok(t) => t,
            Err(_e) => continue,
        };
        let parsed = match parse(&tokens, &input) {
            Ok(q) => q,
            Err(_e) => continue,
        };
        let _registered: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime> = g
            .bench_with_input(
                criterion::BenchmarkId::from_parameter(size),
                &parsed,
                |b, q| {
                    b.iter(|| {
                        let res = normalize(criterion::black_box(q.clone()));
                        let _kept = criterion::black_box(&res);
                    });
                },
            );
    }
    g.finish();
}

fn bench_hash(c: &mut Criterion) {
    let mut g = c.benchmark_group("hash");
    for &size in &[1_024_usize, 4_096, 16_384] {
        let input = build_query(size);
        let tokens = match tokenize(&input) {
            Ok(t) => t,
            Err(_e) => continue,
        };
        let parsed = match parse(&tokens, &input) {
            Ok(q) => q,
            Err(_e) => continue,
        };
        let normalized = match normalize(parsed) {
            Ok(q) => q,
            Err(_e) => continue,
        };
        let _registered: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime> = g
            .bench_with_input(
                criterion::BenchmarkId::from_parameter(size),
                &normalized,
                |b, q| {
                    b.iter(|| {
                        let res = canonical_hash(criterion::black_box(q));
                        let _kept = criterion::black_box(&res);
                    });
                },
            );
    }
    g.finish();
}

criterion_group!(
    benches,
    bench_tokenize,
    bench_parse,
    bench_normalize,
    bench_hash
);
criterion_main!(benches);
