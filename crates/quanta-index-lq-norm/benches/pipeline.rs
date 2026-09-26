//! All four successful pipeline stages at exactly 1/4/16 KiB.
//! Invalid fixtures fail before Criterion registers or measures any case.

use criterion::{BenchmarkId, Criterion, black_box};
use quanta_index_lq_norm::errors::LqParseError;
use quanta_index_lq_norm::hasher::canonical_hash;
use quanta_index_lq_norm::normalizer::normalize;
use quanta_index_lq_norm::parser::parse;
use quanta_index_lq_norm::tokenizer::tokenize;

/// Sixteen distinct terms keep AST fan-out bounded independently of byte size.
fn build_query(size: usize) -> String {
    let bytes = size.saturating_sub(15_usize.saturating_mul(" OR ".len()));
    let mut terms = Vec::with_capacity(16);
    for index in 0..16 {
        let width = bytes
            .div_euclid(16)
            .saturating_add(usize::from(index < bytes.rem_euclid(16)));
        let mut term = format!("term{index:02}");
        term.extend(core::iter::repeat_n('x', width.saturating_sub(term.len())));
        terms.push(term);
    }
    terms.join(" OR ")
}

fn main() -> Result<(), LqParseError> {
    let mut fixtures = Vec::new();
    for size in [1_024_usize, 4_096, 16_384] {
        let input = build_query(size);
        let tokens = tokenize(&input)?;
        let parsed = parse(&tokens, &input)?;
        let normalized = normalize(parsed.clone())?;
        fixtures.push((size, input, tokens, parsed, normalized));
    }
    let mut criterion = Criterion::default().configure_from_args();
    for stage in ["tokenize", "parse", "normalize", "hash"] {
        let mut group = criterion.benchmark_group(stage);
        for (size, input, tokens, parsed, normalized) in &fixtures {
            let _registered =
                group.bench_function(BenchmarkId::from_parameter(size), |b| match stage {
                    "tokenize" => b.iter(|| black_box(tokenize(black_box(input.as_str())))),
                    "parse" => b.iter(|| black_box(parse(black_box(tokens), black_box(input)))),
                    "normalize" => b.iter(|| black_box(normalize(black_box(parsed.clone())))),
                    _ => b.iter(|| black_box(canonical_hash(black_box(normalized)))),
                });
        }
        group.finish();
    }
    criterion.final_summary();
    drop(criterion);
    Ok(())
}
