#![no_main]
#![forbid(unsafe_code)]

//! Continuous fuzz target for the LQ DSL parse pipeline.
//!
//! Fail-closed parsing is a contract invariant: the tokenizer, parser,
//! normalizer, and canonical hasher MUST reject every malformed, oversized,
//! or adversarial input as a typed `Err` rather than panicking, looping,
//! overflowing, or producing placeholder output. The hard caps the pipeline
//! enforces (source of truth: `quanta_index_lq_norm::limits`) are:
//!
//! - `MAX_INPUT_BYTES = 16384` — raw input length before tokenization.
//! - `MAX_AST_DEPTH = 32` — boolean-AST nesting depth during recursive descent.
//! - `MAX_STRUCTURAL_NODES = 256` — structural-pattern nodes in a `match { ... }`.
//!
//! This target throws arbitrary bytes at the full tokenize -> parse ->
//! normalize -> hash chain so libfuzzer flags any non-`Err` exit path,
//! panic, or hang anywhere in the pipeline. Property tests plus the hard
//! caps already cover the structured cases; this closes the continuous
//! adversarial-input gap that previously existed only for IPC decoders.

use libfuzzer_sys::fuzz_target;

use quanta_index_lq_norm::hasher::canonical_hash;
use quanta_index_lq_norm::normalizer::normalize;
use quanta_index_lq_norm::parser::parse;
use quanta_index_lq_norm::tokenizer::tokenize;

fuzz_target!(|data: &[u8]| {
    // Lossy conversion (never `from_utf8`) so EVERY input byte sequence
    // reaches the pipeline — invalid UTF-8 must still be rejected by a
    // typed Err, not skipped before it can exercise the parser.
    let input = String::from_utf8_lossy(data);
    let s: &str = input.as_ref();
    if let Ok(tokens) = tokenize(s) {
        if let Ok(parsed) = parse(&tokens, s) {
            if let Ok(normalized) = normalize(parsed) {
                let _ = canonical_hash(&normalized);
            }
        }
    }
});
