use quanta_index_core::CoreError;

use crate::query_dispatcher::metrics::classify_error_metric_name;

#[test]
fn classify_error_metric_name_uses_closed_taxonomy() {
    let cases = [
        (
            CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                    quanta_index_contract::lex::LexicalErrorCode::ParseFail,
                ),
                message: "parse".to_string(),
            },
            "lq_typed_error_parse_total",
        ),
        (
            CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                    quanta_index_contract::lex::LexicalErrorCode::BridgeTranslateFail,
                ),
                message: "bridge".to_string(),
            },
            "lq_typed_error_parse_total",
        ),
        (
            CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::HistoryProducerUnavailable,
                message: "history".to_string(),
            },
            "lq_typed_error_unavailable_total",
        ),
        (
            CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                message: "plan".to_string(),
            },
            "lq_typed_error_plan_limit_total",
        ),
        (
            CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::RequestDeadlineExceeded,
                message: "deadline".to_string(),
            },
            "lq_typed_error_deadline_exceeded_total",
        ),
        (
            CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::RequestCancelled,
                message: "cancelled".to_string(),
            },
            "lq_typed_error_cancelled_total",
        ),
        (
            CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                    quanta_index_contract::lex::LexicalErrorCode::QueryTimeout,
                ),
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
                code: quanta_index_contract::SearchPlaneErrorCodeV2::StrGenerationNotReady,
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
                code: quanta_index_contract::SearchPlaneErrorCodeV2::RuntimeDirtyOnlyUnsupported,
                message: "dirty".to_string(),
            },
            "lq_typed_error_invalid_request_total",
        ),
        (
            CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::AnnIndexIncompatible,
                message: "other".to_string(),
            },
            "lq_typed_error_other_total",
        ),
    ];

    for (err, expected) in cases {
        assert_eq!(classify_error_metric_name(&err), expected);
    }
}
