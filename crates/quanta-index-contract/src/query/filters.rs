pub use quanta_index_lq_norm::{
    LqFileScope, LqFilter, LqSelect, LqType, LqVisibility, LqYesNoOnly,
};

const REV_AT_TIME_PREFIX: &str = "at.time(";

/// Extract a nonempty time reference from the value of a revision filter.
#[must_use]
pub fn parse_rev_at_time_spec(value: &str) -> Option<&str> {
    let payload = value
        .trim()
        .strip_prefix(REV_AT_TIME_PREFIX)?
        .strip_suffix(')')?;
    let payload = payload.trim();
    (!payload.is_empty()).then_some(payload)
}

/// Whether the supported Sourcegraph query lowers to a time-based revision filter.
#[must_use]
pub fn sourcegraph_query_requests_rev_at_time(query_text: &str) -> bool {
    use quanta_index_lq_norm::LqPatternType;

    query_text.contains(REV_AT_TIME_PREFIX)
        && quanta_index_lq_bridge::lower_sourcegraph_query(query_text).is_ok_and(|query| {
            query.options.pattern_type != LqPatternType::Structural
                && query.filters.iter().any(|filter| {
                    matches!(filter, LqFilter::Rev { spec } if parse_rev_at_time_spec(spec).is_some())
                })
        })
}
