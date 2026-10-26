//! Native text lowering before the general LQ parser's regex/structural guards.
//!
//! The lexer is the canonical LQ lexer. Inspecting its typed tokens lets this
//! route refuse other engines without parsing or compiling their patterns.

use quanta_index_contract::{
    LqQuery, MAX_NATIVE_CONTENT_INPUT_BYTES, TextQueryRequest, TextQuerySyntax,
    native_content_query_backing_bytes, tokenize_native_content_query,
};
use quanta_index_core::index::{IndexError, MemoryLease, MemoryLifetime};

/// The parser's owned query and the retained grant acquired before lexing.
/// Callers keep this value live until the plan has adopted its own grant.
pub(crate) struct LoweredNativeText {
    pub(crate) query: LqQuery,
    backing: [Option<MemoryLease>; 2],
}

impl LoweredNativeText {
    pub(crate) fn into_parts(self) -> (LqQuery, [Option<MemoryLease>; 2]) {
        (self.query, self.backing)
    }
}

/// Lower only the canonical Native text grammar, with a grant before parsing.
///
/// Raw-string, structural and predicate tokens are refused by the Contract
/// facade. Regex HIR has a separate before-parser retained reservation.
pub(crate) fn lower_native_text_input(
    request: &TextQueryRequest,
    reserve: impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<LoweredNativeText, IndexError> {
    if request.syntax != TextQuerySyntax::Native || request.cursor.is_some() {
        return Err(IndexError::InvalidInput(
            "native text lowering requires a first-page Native query".into(),
        ));
    }
    let input = request.query_text.as_str();
    if input.len() > MAX_NATIVE_CONTENT_INPUT_BYTES {
        return Err(IndexError::ResourceLimited);
    }
    let backing = reserve(
        native_content_query_backing_bytes(input.len()).ok_or(IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?;
    if backing.is_none() {
        return Err(IndexError::BackendViolation(
            "native text lowering requires allocation custody".into(),
        ));
    }
    let tokens = tokenize_native_content_query(input)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let preparation_bytes = tokens
        .preparation_backing_bytes()
        .ok_or(IndexError::ResourceLimited)?;
    let preparation = if preparation_bytes == 0 {
        None
    } else {
        let memory = reserve(preparation_bytes, MemoryLifetime::Retained)?;
        if memory.is_none() {
            return Err(IndexError::BackendViolation(
                "native regex preparation requires allocation custody".into(),
            ));
        }
        memory
    };
    let query = tokens
        .parse()
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    Ok(LoweredNativeText {
        query,
        backing: [backing, preparation],
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };

    use quanta_index_contract::{LqExpr, QueryConstraintSetV1};
    use quanta_index_core::index::MemoryLeaseOwner;

    use super::*;

    struct Releases(AtomicU64);

    impl MemoryLeaseOwner for Releases {
        fn release(&self, bytes: u64) {
            let _prior = self.0.fetch_add(bytes, Ordering::SeqCst);
        }
    }

    fn request(text: &str) -> TextQueryRequest {
        TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: text.into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            top_k: 1,
            cursor: None,
        }
    }

    #[test]
    fn regex_preparation_is_admitted_before_parser_and_released() {
        let owner = Arc::new(Releases(AtomicU64::new(0)));
        let admitted = AtomicU64::new(0);
        let result = lower_native_text_input(&request(r"/\p{L}/"), |bytes, lifetime| {
            assert_eq!(lifetime, MemoryLifetime::Retained);
            let _prior = admitted.fetch_add(bytes, Ordering::SeqCst);
            Ok(Some(MemoryLease::new(bytes, owner.clone())))
        });
        assert!(result.is_ok());
        drop(result);
        assert!(admitted.load(Ordering::SeqCst) > 0);
        assert_eq!(
            owner.0.load(Ordering::SeqCst),
            admitted.load(Ordering::SeqCst)
        );
    }

    #[test]
    fn quoted_regex_text_is_a_phrase_and_keeps_grant_until_drop() {
        let owner = Arc::new(Releases(AtomicU64::new(0)));
        let request = request(r#""/regex/""#);
        let lowered = lower_native_text_input(&request, |bytes, lifetime| {
            assert_eq!(lifetime, MemoryLifetime::Retained);
            Ok(Some(MemoryLease::new(bytes, owner.clone())))
        });
        let lowered = lowered.expect("quoted text should lower as phrase");
        assert!(matches!(
            &lowered.query.expr,
            LqExpr::Leaf(quanta_index_contract::LqLeaf::Phrase(_))
        ));
        assert_eq!(owner.0.load(Ordering::SeqCst), 0);
        let (_query, backing) = lowered.into_parts();
        let bytes: u64 = backing.iter().flatten().map(MemoryLease::bytes).sum();
        drop(backing);
        assert_eq!(owner.0.load(Ordering::SeqCst), bytes);
    }

    #[test]
    fn raw_structural_and_predicate_tokens_refuse_before_parser() {
        for source in ["'raw'", "match { foo }", "symbol.has.name(\"foo\")"] {
            let result = lower_native_text_input(&request(source), |bytes, _lifetime| {
                Ok(Some(MemoryLease::new(
                    bytes,
                    Arc::new(Releases(AtomicU64::new(0))),
                )))
            });
            assert!(matches!(
                result,
                Err(IndexError::InvalidInput(message)) if message.contains("native BM25")
            ));
        }
    }

    #[test]
    fn malformed_input_is_typed_and_oversize_refuses_before_reservation() {
        let malformed = lower_native_text_input(&request("\"unterminated"), |bytes, _lifetime| {
            Ok(Some(MemoryLease::new(
                bytes,
                Arc::new(Releases(AtomicU64::new(0))),
            )))
        });
        assert!(matches!(malformed, Err(IndexError::InvalidInput(_))));

        let oversized = request(&"a".repeat(MAX_NATIVE_CONTENT_INPUT_BYTES + 1));
        let reservation_calls = AtomicU64::new(0);
        let refused = lower_native_text_input(&oversized, |_bytes, _lifetime| {
            let _prior = reservation_calls.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        });
        assert!(matches!(refused, Err(IndexError::ResourceLimited)));
        assert_eq!(reservation_calls.load(Ordering::SeqCst), 0);
    }
}
