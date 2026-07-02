//! Request-shaping for the `OpenAI` embeddings endpoint.
//!
//! Partitions a flat list of input texts into `[start, end)` request batches
//! bounded by both a max input count and an estimated per-request token budget,
//! so a real provider packs inputs into the fewest round-trips. Pure and
//! network-free — a single oversized text is emitted as its own singleton batch
//! rather than failing closed on an estimate.

/// A half-open `[start, end)` slice of the caller's input texts, plus the token
/// estimate the packing policy accumulated for it.
#[derive(Clone, Copy)]
pub(super) struct RequestBatch {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) estimated_tokens: usize,
}

pub(super) fn partition_request_batches(
    texts: &[&str],
    max_batch: usize,
    max_estimated_tokens_per_request: usize,
) -> Vec<RequestBatch> {
    let mut out = Vec::new();
    let mut start = 0_usize;
    let mut current_count = 0_usize;
    let mut current_estimated_tokens = 0_usize;
    for (index, text) in texts.iter().enumerate() {
        let estimated = estimate_text_tokens(text);
        let exceeds_count = current_count == max_batch;
        let exceeds_tokens = current_count > 0
            && current_estimated_tokens.saturating_add(estimated)
                > max_estimated_tokens_per_request;
        if exceeds_count || exceeds_tokens {
            out.push(RequestBatch {
                start,
                end: index,
                estimated_tokens: current_estimated_tokens,
            });
            start = index;
            current_count = 0;
            current_estimated_tokens = 0;
        }
        current_count = current_count.saturating_add(1);
        current_estimated_tokens = current_estimated_tokens.saturating_add(estimated);
    }
    if current_count > 0 {
        out.push(RequestBatch {
            start,
            end: texts.len(),
            estimated_tokens: current_estimated_tokens,
        });
    }
    out
}

fn estimate_text_tokens(text: &str) -> usize {
    let bytes = text.len();
    let byte_estimate = bytes
        .saturating_add(2)
        .checked_div(3)
        .unwrap_or(usize::MAX)
        .max(1);
    let word_estimate = text.split_whitespace().count().max(1);
    byte_estimate.max(word_estimate)
}
