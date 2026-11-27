#![expect(
    clippy::redundant_pub_crate,
    reason = "crate-private query route state is shared across sibling modules"
)]

//! Client-independent query route inputs and bounded top-k validation.

use quanta_index_contract::{
    ContinuationTokenV2, GenerationSelector, QueryConstraintSetV1, TextQuerySyntax,
};

use crate::SdkError;

pub(crate) struct TextQueryBuilderState {
    pub(crate) syntax: TextQuerySyntax,
    pub(crate) query_text: Option<String>,
    pub(crate) constraints: QueryConstraintSetV1,
    pub(crate) selection: Option<GenerationSelector>,
    pub(crate) top_k: Option<u32>,
    /// Opaque server-minted continuation for a text or symbol page;
    /// routes that do not page refuse one typed.
    pub(crate) after: Option<ContinuationTokenV2>,
}

impl TextQueryBuilderState {
    pub(crate) fn new() -> Self {
        Self {
            // No request can execute until a text setter supplies query_text.
            // The lexical `.text(...)` setter uses this product default;
            // other namespaces select Native or Sourcegraph explicitly.
            syntax: TextQuerySyntax::CodeSearch,
            query_text: None,
            constraints: QueryConstraintSetV1::unconstrained(),
            selection: None,
            top_k: None,
            after: None,
        }
    }
}

/// Refuse an out-of-range `top_k` at build time with the same code the daemon
/// would answer over the wire.
///
/// The SDK's type-state builders guarantee `top_k` is *present*; the value
/// itself is a contract obligation shared with every query route, so a caller
/// learns about a bad value before a round trip, and learns it under the same
/// `QUERY_TOP_K_OUT_OF_RANGE` code it would otherwise get back.
pub(crate) fn accepted_top_k(plane: &str, top_k: u32) -> Result<u32, SdkError> {
    quanta_index_contract::validate_public_top_k(top_k).map_err(|refused| SdkError::Remote {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
            quanta_index_contract::lex::LexicalErrorCode::QueryTopKOutOfRange,
        ),
        message: format!("{plane}: {refused}"),
        repair: None,
    })
}
