#![expect(
    clippy::redundant_pub_crate,
    reason = "crate-private query builders share this state across sibling modules"
)]

use quanta_index_contract::{
    GenerationPin, GenerationSelector, HybridSeedQueryRequest, SemanticQueryRequest,
    SemanticSeedCorpusBudgetV1, TextQueryRequest, TextQuerySyntax,
};

use crate::{QuantaIndex, SdkError};

pub(crate) struct TextQueryBuilderState {
    pub(crate) syntax: TextQuerySyntax,
    pub(crate) query_text: Option<String>,
    pub(crate) selection: Option<GenerationSelector>,
    pub(crate) top_k: Option<u32>,
}

impl TextQueryBuilderState {
    pub(crate) const fn new() -> Self {
        Self {
            syntax: TextQuerySyntax::Native,
            query_text: None,
            selection: None,
            top_k: None,
        }
    }

    pub(crate) fn build_request(self, plane: &str) -> Result<TextQueryRequest, SdkError> {
        let query_text = self
            .query_text
            .ok_or_else(|| SdkError::Usage(format!("{plane} query text is required")))?;
        let selection = self
            .selection
            .ok_or_else(|| SdkError::Usage(format!("{plane} generation selection is required")))?;
        let top_k = self
            .top_k
            .ok_or_else(|| SdkError::Usage(format!("{plane} top_k is required")))?;
        let (generation, generation_selector) = QuantaIndex::selection_to_fields(selection);
        Ok(TextQueryRequest {
            syntax: self.syntax,
            query_text,
            generation,
            generation_selector,
            top_k,
        })
    }
}

pub(crate) struct VectorQueryBuilderState {
    pub(crate) selection: Option<GenerationSelector>,
    pub(crate) top_k: Option<u32>,
    pub(crate) semantic_query_text: Option<String>,
    pub(crate) text_leg: Option<(TextQuerySyntax, String)>,
    pub(crate) scope_leg: Option<(TextQuerySyntax, String)>,
    pub(crate) scope_top_k: Option<u32>,
    pub(crate) dense_corpora: Vec<SemanticSeedCorpusBudgetV1>,
}

impl VectorQueryBuilderState {
    pub(crate) const fn new() -> Self {
        Self {
            selection: None,
            top_k: None,
            semantic_query_text: None,
            text_leg: None,
            scope_leg: None,
            scope_top_k: None,
            dense_corpora: Vec::new(),
        }
    }

    pub(crate) fn build_semantic_request(self) -> Result<SemanticQueryRequest, SdkError> {
        let query_text = self
            .semantic_query_text
            .ok_or_else(|| SdkError::Usage("semantic query text is required".to_string()))?;
        let selection = self.selection.ok_or_else(|| {
            SdkError::Usage("semantic generation selection is required".to_string())
        })?;
        let top_k = self
            .top_k
            .ok_or_else(|| SdkError::Usage("semantic top_k is required".to_string()))?;
        let (generation, generation_selector) = Self::selection_fields(selection);
        let lexical_scope = match (self.scope_leg, self.scope_top_k) {
            (Some((syntax, query_text)), Some(scope_top_k)) => Some(TextQueryRequest {
                syntax,
                query_text,
                generation: generation.clone(),
                generation_selector: generation_selector.clone(),
                top_k: scope_top_k,
            }),
            (Some(_), None) => {
                return Err(SdkError::Usage(
                    "semantic lexical scope is set but scope_top_k is missing; \
                     scope_top_k is the lexical candidate cap and must be supplied \
                     explicitly when scope_native / scope_sourcegraph is used"
                        .to_string(),
                ));
            }
            (None, Some(_)) => {
                return Err(SdkError::Usage(
                    "semantic scope_top_k is set but no lexical scope was \
                     configured; call scope_native(...) or scope_sourcegraph(...) \
                     to enable the lexical scope stage"
                        .to_string(),
                ));
            }
            (None, None) => None,
        };
        Ok(SemanticQueryRequest {
            query_text,
            generation,
            generation_selector,
            lexical_scope,
            top_k,
        })
    }

    pub(crate) fn build_hybrid_seed_request(self) -> Result<HybridSeedQueryRequest, SdkError> {
        let (syntax, query_text) = self
            .text_leg
            .ok_or_else(|| SdkError::Usage("hybrid seed text query is required".to_string()))?;
        let semantic_query_text = self.semantic_query_text.ok_or_else(|| {
            SdkError::Usage("hybrid seed semantic query text is required".to_string())
        })?;
        let selection = self.selection.ok_or_else(|| {
            SdkError::Usage("hybrid seed generation selection is required".to_string())
        })?;
        let top_k = self
            .top_k
            .ok_or_else(|| SdkError::Usage("hybrid seed top_k is required".to_string()))?;
        let (generation, generation_selector) = Self::selection_fields(selection);
        Ok(HybridSeedQueryRequest {
            text_query: TextQueryRequest {
                syntax,
                query_text,
                generation: generation.clone(),
                generation_selector: generation_selector.clone(),
                top_k,
            },
            semantic_query_text,
            generation,
            generation_selector,
            dense_corpora: self.dense_corpora,
            top_k,
        })
    }

    fn selection_fields(
        selection: GenerationSelector,
    ) -> (Option<GenerationPin>, Option<GenerationSelector>) {
        QuantaIndex::selection_to_fields(selection)
    }
}
