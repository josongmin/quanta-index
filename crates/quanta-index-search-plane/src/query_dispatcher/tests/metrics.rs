use quanta_index_core::{CoreError, REQUEST_CANCELLED_CODE, REQUEST_DEADLINE_EXCEEDED_CODE};

use crate::query_dispatcher::errors::ERR_RUNTIME_DIRTY_ONLY_UNSUPPORTED;
use crate::query_dispatcher::metrics::classify_error_metric_name;

#[test]
fn classify_error_metric_name_uses_closed_taxonomy() {
    let cases = [
        (
            CoreError::Typed {
                code: "PARSE_FAIL".to_string(),
                message: "parse".to_string(),
            },
            "lq_typed_error_parse_total",
        ),
        (
            CoreError::Typed {
                code: "BRIDGE_TRANSLATE_FAIL".to_string(),
                message: "bridge".to_string(),
            },
            "lq_typed_error_parse_total",
        ),
        (
            CoreError::Typed {
                code: "HISTORY_PRODUCER_UNAVAILABLE".to_string(),
                message: "history".to_string(),
            },
            "lq_typed_error_unavailable_total",
        ),
        (
            CoreError::Typed {
                code: "LEX_TRIGRAM_PLAN_LIMIT_EXCEEDED".to_string(),
                message: "plan".to_string(),
            },
            "lq_typed_error_plan_limit_total",
        ),
        (
            CoreError::Typed {
                code: REQUEST_DEADLINE_EXCEEDED_CODE.to_string(),
                message: "deadline".to_string(),
            },
            "lq_typed_error_interrupted_total",
        ),
        (
            CoreError::Typed {
                code: REQUEST_CANCELLED_CODE.to_string(),
                message: "cancelled".to_string(),
            },
            "lq_typed_error_interrupted_total",
        ),
        (
            CoreError::Typed {
                code: "QUERY_TIMEOUT".to_string(),
                message: "timeout".to_string(),
            },
            "lq_typed_error_plan_limit_total",
        ),
        (
            CoreError::NotReady("replay".to_string()),
            "lq_typed_error_not_ready_total",
        ),
        (
            CoreError::Typed {
                code: "STR_GENERATION_NOT_READY".to_string(),
                message: "structural".to_string(),
            },
            "lq_typed_error_not_ready_total",
        ),
        (
            CoreError::Storage("disk".to_string()),
            "lq_typed_error_internal_total",
        ),
        (
            CoreError::InvalidContract("wire".to_string()),
            "lq_typed_error_invalid_request_total",
        ),
        (
            CoreError::Typed {
                code: ERR_RUNTIME_DIRTY_ONLY_UNSUPPORTED.to_string(),
                message: "dirty".to_string(),
            },
            "lq_typed_error_invalid_request_total",
        ),
        (
            CoreError::Typed {
                code: "SEM_EXECUTION_ODDITY".to_string(),
                message: "other".to_string(),
            },
            "lq_typed_error_other_total",
        ),
    ];

    for (err, expected) in cases {
        assert_eq!(classify_error_metric_name(&err), expected);
    }
}
