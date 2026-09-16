//! Unit tests for semantic policy validation edges.

#![forbid(unsafe_code)]

use quanta_index_core::{CoreError, SemanticPolicy};

fn typed_code_or_debug(result: Result<(), CoreError>) -> String {
    match result {
        Err(CoreError::Typed { code, .. }) => code,
        other => format!("unexpected result: {other:?}"),
    }
}

// `top_k` is one contract for every route (QI-BB-025). The semantic route used
// to answer zero with `INVALID_FILTER_VALUE` and above-ceiling with
// `PLAN_LIMIT_EXCEEDED`, while hybrid answered both with its own code and the
// dispatcher's probe refused the public maximum outright. One code now.
#[test]
fn semantic_top_k_zero_uses_the_shared_out_of_range_code() {
    assert_eq!(
        typed_code_or_debug(SemanticPolicy::validate_top_k(0)),
        quanta_index_contract::TOP_K_OUT_OF_RANGE_CODE
    );
}

#[test]
fn semantic_top_k_above_ceiling_uses_the_shared_out_of_range_code() {
    assert_eq!(
        typed_code_or_debug(SemanticPolicy::validate_top_k(
            SemanticPolicy::max_top_k().saturating_add(1),
        )),
        quanta_index_contract::TOP_K_OUT_OF_RANGE_CODE
    );
}

#[test]
fn semantic_top_k_public_maximum_is_accepted() {
    assert!(SemanticPolicy::validate_top_k(SemanticPolicy::max_top_k()).is_ok());
    assert_eq!(
        SemanticPolicy::max_top_k(),
        quanta_index_contract::PUBLIC_TOP_K_MAX
    );
}

#[test]
fn semantic_query_vector_nan_is_invalid() {
    assert_eq!(
        typed_code_or_debug(SemanticPolicy::validate_query_vector(&[1.0, f32::NAN])),
        "SEM_INVALID_VECTOR"
    );
}

#[test]
fn semantic_query_vector_zero_norm_is_invalid() {
    assert_eq!(
        typed_code_or_debug(SemanticPolicy::validate_query_vector(&[0.0, 0.0, 0.0])),
        "SEM_INVALID_VECTOR"
    );
}

#[test]
fn semantic_query_vector_finite_non_zero_is_valid() {
    assert!(SemanticPolicy::validate_query_vector(&[1.0, 0.0, 2.0]).is_ok());
}
