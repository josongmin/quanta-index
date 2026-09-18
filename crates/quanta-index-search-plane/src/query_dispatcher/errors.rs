//! Wire error codes, typed-error constructors, and the `CoreError` -> IPC error
//! envelope (including advisory repair metadata) for the query dispatcher.

use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_contract::{
    GenerationPin, HistoryOrderV1, ManifestGeneration, QueryErrorRepair, RepairClass,
    SearchPlaneIpcError,
};
use quanta_index_core::{
    AUX_EPOCH_EXPIRED_CODE, AUX_EPOCH_UNKNOWN_CODE, CoreError, HISTORY_TEXT_QUERY_UNSCORABLE_CODE,
};

pub(super) const ERR_INVALID: &str = "INVALID_REQUEST";
pub(super) const ERR_NOT_READY: &str = "NOT_READY";
pub(super) const ERR_NOT_FOUND: &str = "NOT_FOUND";
pub(super) const ERR_NOT_IMPLEMENTED: &str = "NOT_IMPLEMENTED";
pub(super) const ERR_INTERNAL: &str = "INTERNAL";
pub(super) const ERR_HISTORY_PRODUCER_UNAVAILABLE: &str = "HISTORY_PRODUCER_UNAVAILABLE";
pub(super) const ERR_HISTORY_GENERATION_NOT_READY: &str = "HISTORY_GENERATION_NOT_READY";
pub(super) const ERR_HISTORY_SHARD_UNAVAILABLE: &str = "HISTORY_SHARD_UNAVAILABLE";
pub(super) const ERR_HISTORY_INVALID_TIMEREF: &str = "HISTORY_INVALID_TIMEREF";
/// A cursor issued under one history order was sent to continue a walk of
/// the other (QI-BB-023 follow-up #1).
pub(super) const ERR_HISTORY_CURSOR_ORDER_MISMATCH: &str = "HISTORY_CURSOR_ORDER_MISMATCH";
/// The composition root wired no history text index, so the relevance
/// order has nothing to score with.
pub(super) const ERR_HISTORY_RELEVANCE_UNAVAILABLE: &str = "HISTORY_RELEVANCE_UNAVAILABLE";
pub(super) const ERR_RUNTIME_CATALOG_NOT_READY: &str = "RUNTIME_CATALOG_NOT_READY";
pub(super) const ERR_RUNTIME_CATALOG_HEAD_MISSING: &str = "RUNTIME_CATALOG_HEAD_MISSING";
pub(super) const ERR_RUNTIME_INVALID_SCOPE: &str = "RUNTIME_INVALID_SCOPE";
#[cfg(test)]
pub(super) const ERR_RUNTIME_DIRTY_ONLY_UNSUPPORTED: &str = "RUNTIME_DIRTY_ONLY_UNSUPPORTED";
pub(super) const ERR_SNAPSHOT_UNKNOWN: &str = "SNAPSHOT_UNKNOWN";

pub(super) fn core_error_to_ipc(err: CoreError) -> SearchPlaneIpcError {
    let (code, message) = match err {
        CoreError::InvalidContract(msg) => (ERR_INVALID.to_string(), msg),
        CoreError::Typed { code, message } => (code, message),
        CoreError::NotReady(msg) => (ERR_NOT_READY.to_string(), msg),
        CoreError::NotImplemented(msg) => (ERR_NOT_IMPLEMENTED.to_string(), msg),
        CoreError::NotFound(msg) => (ERR_NOT_FOUND.to_string(), msg),
        CoreError::Storage(msg) => (ERR_INTERNAL.to_string(), msg),
    };
    let repair = repair_for_code(&code);
    SearchPlaneIpcError {
        code,
        message,
        repair,
    }
}

/// Deterministic typed repair metadata for a wire error code (J7Q-06).
///
/// Advisory only — it never changes the fail-closed `code`/`message` outcome and
/// never rewrites the query. Each repairable code maps to its [`RepairClass`],
/// a set of *confirmed-supported* alternative filter shapes the caller can move
/// to, and a docs anchor pointing at the in-repo capability inventory. Internal
/// invariant breaks (e.g. `BRIDGE_TRANSLATE_FAIL`) and generic failures the
/// caller cannot act on return `None` rather than a misleading hint.
///
/// The alternative shapes are intentionally the small set verified to exist in
/// this plane (`repo:` / `file:` / `path:` / `lang:` / `rev:`); the anchor is the
/// authority for the full list. The auxiliary-epoch refusals (QI-BB-020
/// W2) name the one repair that exists for a stale continuation: start
/// the walk over without a cursor.
///
/// `pub` so the J7Q-06 ambiguity rail can snapshot the exact payloads the wire
/// boundary emits without re-deriving the policy.
#[must_use]
pub fn repair_for_code(code: &str) -> Option<QueryErrorRepair> {
    const DOCS_ANCHOR: &str = "docs/analysis/jun-4-dsl-capabilty.md";
    let (class, alternatives): (RepairClass, &[&str]) = match code {
        c if c == LexicalErrorCode::BridgeAmbiguousFilter.as_code_str() => (
            RepairClass::Ambiguous,
            &["repo:<value>", "file:<value>", "path:<value>"],
        ),
        c if c == LexicalErrorCode::BridgeUnsupportedFilter.as_code_str() => (
            RepairClass::Unsupported,
            &["repo:<name>", "file:<glob>", "path:<glob>", "lang:<name>"],
        ),
        c if c == LexicalErrorCode::BridgeUnsupportedDirective.as_code_str() => (
            RepairClass::Unsupported,
            &["remove the directive", "use an explicit filter shape"],
        ),
        c if c == LexicalErrorCode::BridgeVersionPin.as_code_str() => (
            RepairClass::Malformed,
            &["rev:<git-ref>", "remove the version pin"],
        ),
        // A continuation named an auxiliary epoch the plane no longer
        // retains (QI-BB-020 W2): resuming is impossible without mixing
        // epochs, so the caller starts the walk over.
        AUX_EPOCH_EXPIRED_CODE => (
            RepairClass::Expired,
            &["restart the page walk without a cursor"],
        ),
        // A cursor naming an epoch this state root never produced cannot
        // have come from a page it served.
        AUX_EPOCH_UNKNOWN_CODE => (
            RepairClass::Malformed,
            &["restart the page walk without a cursor"],
        ),
        // A cursor of one order cannot position a walk of the other.
        ERR_HISTORY_CURSOR_ORDER_MISMATCH => (
            RepairClass::Malformed,
            &[
                "continue with the order the cursor was issued under",
                "restart the page walk without a cursor",
            ],
        ),
        // Relevance scores keyword and phrase leaves; anything else has no
        // score and runs as a filter under recency.
        HISTORY_TEXT_QUERY_UNSCORABLE_CODE => (
            RepairClass::Unsupported,
            &["order: recency", "use keyword or phrase leaves"],
        ),
        _ => return None,
    };
    Some(QueryErrorRepair {
        class,
        supported_alternatives: alternatives.iter().map(|s| (*s).to_string()).collect(),
        docs_anchor: Some(DOCS_ANCHOR.to_string()),
    })
}

pub(super) fn history_absent_error(
    pin: &GenerationPin,
    lexical_materialized: Option<ManifestGeneration>,
) -> CoreError {
    match lexical_materialized {
        Some(materialized) if materialized.get() >= pin.manifest_generation.get() => {
            CoreError::Typed {
                code: ERR_HISTORY_PRODUCER_UNAVAILABLE.to_string(),
                message: format!(
                    "history: producer data is unavailable for generation {}",
                    pin.manifest_generation.get()
                ),
            }
        }
        _ => CoreError::Typed {
            code: ERR_HISTORY_GENERATION_NOT_READY.to_string(),
            message: format!(
                "history: generation {} is not yet materialized",
                pin.manifest_generation.get()
            ),
        },
    }
}

pub(super) fn history_shard_unavailable(message: impl Into<String>) -> CoreError {
    CoreError::Typed {
        code: ERR_HISTORY_SHARD_UNAVAILABLE.to_string(),
        message: message.into(),
    }
}

pub(super) fn history_invalid_request(message: impl Into<String>) -> CoreError {
    CoreError::InvalidContract(message.into())
}

pub(super) fn history_cursor_order_mismatch(
    cursor_order: HistoryOrderV1,
    requested: HistoryOrderV1,
) -> CoreError {
    CoreError::Typed {
        code: ERR_HISTORY_CURSOR_ORDER_MISMATCH.to_string(),
        message: format!(
            "history: a {cursor_order} cursor cannot continue a {requested} walk; continue with `order: {cursor_order}` or restart without a cursor"
        ),
    }
}

pub(super) fn history_relevance_unavailable() -> CoreError {
    CoreError::Typed {
        code: ERR_HISTORY_RELEVANCE_UNAVAILABLE.to_string(),
        message: "history: this search plane has no history text index wired; the relevance order cannot be served, use `order: recency`"
            .to_string(),
    }
}

pub(super) fn runtime_invalid_scope(message: impl Into<String>) -> CoreError {
    CoreError::Typed {
        code: ERR_RUNTIME_INVALID_SCOPE.to_string(),
        message: message.into(),
    }
}

pub(super) fn runtime_catalog_head_missing(field: &str) -> CoreError {
    CoreError::Typed {
        code: ERR_RUNTIME_CATALOG_HEAD_MISSING.to_string(),
        message: format!("runtime metadata: catalog field `{field}` is not materialized"),
    }
}

pub(super) fn runtime_snapshot_unknown(name: &str) -> CoreError {
    CoreError::Typed {
        code: ERR_SNAPSHOT_UNKNOWN.to_string(),
        message: format!("runtime metadata: snapshot `{name}` is unknown in the pinned catalog"),
    }
}

pub(super) fn structural_invalid_request(message: impl Into<String>) -> CoreError {
    CoreError::Typed {
        code: "STR_INVALID_REQUEST".to_string(),
        message: format!("structural: {}", message.into()),
    }
}

pub(super) fn history_invalid_timeref(message: impl Into<String>) -> CoreError {
    CoreError::Typed {
        code: ERR_HISTORY_INVALID_TIMEREF.to_string(),
        message: message.into(),
    }
}

#[cfg(test)]
mod repair_for_code_tests {
    use super::repair_for_code;
    use quanta_index_contract::RepairClass;
    use quanta_index_contract::lex::LexicalErrorCode;

    #[test]
    fn ambiguous_filter_maps_to_ambiguous_class() {
        let repair = repair_for_code(LexicalErrorCode::BridgeAmbiguousFilter.as_code_str())
            .expect("ambiguous filter is repairable");
        assert_eq!(repair.class, RepairClass::Ambiguous);
        assert!(!repair.supported_alternatives.is_empty());
        assert!(repair.docs_anchor.is_some());
    }

    #[test]
    fn unsupported_filter_and_directive_map_to_unsupported() {
        for code in [
            LexicalErrorCode::BridgeUnsupportedFilter,
            LexicalErrorCode::BridgeUnsupportedDirective,
        ] {
            let repair = repair_for_code(code.as_code_str())
                .unwrap_or_else(|| panic!("{} should be repairable", code.as_code_str()));
            assert_eq!(repair.class, RepairClass::Unsupported);
        }
    }

    #[test]
    fn version_pin_maps_to_malformed() {
        let repair = repair_for_code(LexicalErrorCode::BridgeVersionPin.as_code_str())
            .expect("version pin is repairable");
        assert_eq!(repair.class, RepairClass::Malformed);
    }

    /// QI-BB-020 W2: an expired continuation is repaired by restarting the
    /// walk; an unknown epoch is a malformed cursor, repaired the same way.
    #[test]
    fn aux_epoch_refusals_say_to_restart_the_page_walk() {
        let expired = repair_for_code(quanta_index_core::AUX_EPOCH_EXPIRED_CODE)
            .expect("an expired epoch is repairable");
        assert_eq!(expired.class, RepairClass::Expired);
        assert_eq!(
            expired.supported_alternatives,
            vec!["restart the page walk without a cursor".to_string()]
        );
        let unknown = repair_for_code(quanta_index_core::AUX_EPOCH_UNKNOWN_CODE)
            .expect("an unknown epoch is repairable");
        assert_eq!(unknown.class, RepairClass::Malformed);
        assert_eq!(
            unknown.supported_alternatives,
            expired.supported_alternatives
        );
    }

    #[test]
    fn internal_and_unknown_codes_have_no_repair() {
        // Translator invariant breaks are not user-repairable: no misleading hint.
        assert!(repair_for_code(LexicalErrorCode::BridgeTranslateFail.as_code_str()).is_none());
        assert!(repair_for_code("NOT_READY").is_none());
        assert!(repair_for_code("INTERNAL").is_none());
        assert!(repair_for_code("").is_none());
    }

    #[test]
    fn unsupported_filter_alternatives_are_confirmed_filter_families() {
        // Guard against drift into filter names this plane does not actually ship.
        let confirmed = ["repo:", "file:", "path:", "lang:", "rev:"];
        let repair = repair_for_code(LexicalErrorCode::BridgeUnsupportedFilter.as_code_str())
            .expect("repairable");
        for alt in &repair.supported_alternatives {
            assert!(
                confirmed.iter().any(|c| alt.starts_with(c)),
                "alternative `{alt}` is not a confirmed filter family"
            );
        }
    }
}
