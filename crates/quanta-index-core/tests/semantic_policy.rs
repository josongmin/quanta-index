//! Unit tests for semantic policy validation edges.

#![forbid(unsafe_code)]

use quanta_index_core::{CoreError, SemanticPolicy};

fn typed_code_or_debug(result: Result<(), CoreError>) -> String {
    match result {
        Err(CoreError::Typed { code, .. }) => code,
        other => format!("unexpected result: {other:?}"),
    }
}

#[test]
fn semantic_top_k_zero_uses_invalid_filter_value_code() {
    assert_eq!(
        typed_code_or_debug(SemanticPolicy::validate_top_k(0)),
        "INVALID_FILTER_VALUE"
    );
}

#[test]
fn semantic_top_k_above_ceiling_uses_plan_limit_exceeded_code() {
    assert_eq!(
        typed_code_or_debug(SemanticPolicy::validate_top_k(
            SemanticPolicy::max_top_k().saturating_add(1),
        )),
        "PLAN_LIMIT_EXCEEDED"
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
