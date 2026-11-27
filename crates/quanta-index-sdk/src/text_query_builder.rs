#![expect(
    clippy::redundant_pub_crate,
    reason = "crate-private query builders share this state across sibling modules"
)]

use quanta_index_contract::{
    GenerationPin, GenerationSelector, HybridQueryRequest, HybridSeedQueryRequest,
    QueryConstraintSetV1, SemanticQueryRequest, SemanticSeedCorpusBudgetV1, TextQueryRequest,
    TextQuerySyntax,
};

use crate::{LegacyQueryCapabilityV1, QuantaIndexClientPayloadV1, SdkError};

pub(crate) use crate::text_query_route_shared::{TextQueryBuilderState, accepted_top_k};

impl TextQueryBuilderState {
    pub(crate) fn build_request(
        self,
        client: &QuantaIndexClientPayloadV1,
        capability: LegacyQueryCapabilityV1,
        plane: &str,
    ) -> Result<TextQueryRequest, SdkError> {
        client.admit_legacy_query_v1(capability)?;
        let query_text = self
            .query_text
            .ok_or_else(|| SdkError::Usage(format!("{plane} query text is required")))?;
        let selection = self
            .selection
            .ok_or_else(|| SdkError::Usage(format!("{plane} generation selection is required")))?;
        let top_k = self
            .top_k
            .ok_or_else(|| SdkError::Usage(format!("{plane} top_k is required")))?;
        let top_k = accepted_top_k(plane, top_k)?;
        let (generation, generation_selector) =
            QuantaIndexClientPayloadV1::selection_to_fields(selection);
        Ok(TextQueryRequest {
            syntax: self.syntax,
            query_text,
            constraints: self.constraints,
            generation,
            generation_selector,
            top_k,
            cursor: self.after,
        })
    }
}

pub(crate) struct VectorQueryBuilderState {
    pub(crate) selection: Option<GenerationSelector>,
    pub(crate) top_k: Option<u32>,
    pub(crate) semantic_query_text: Option<String>,
    pub(crate) constraints: QueryConstraintSetV1,
    pub(crate) text_leg: Option<(TextQuerySyntax, String)>,
    pub(crate) scope_leg: Option<(TextQuerySyntax, String)>,
    pub(crate) scope_top_k: Option<u32>,
    pub(crate) dense_corpora: Vec<SemanticSeedCorpusBudgetV1>,
}

impl VectorQueryBuilderState {
    pub(crate) fn new() -> Self {
        Self {
            selection: None,
            top_k: None,
            semantic_query_text: None,
            constraints: QueryConstraintSetV1::unconstrained(),
            text_leg: None,
            scope_leg: None,
            scope_top_k: None,
            dense_corpora: Vec::new(),
        }
    }

    pub(crate) fn build_semantic_request(
        self,
        client: &QuantaIndexClientPayloadV1,
    ) -> Result<SemanticQueryRequest, SdkError> {
        client.admit_legacy_query_v1(LegacyQueryCapabilityV1::Semantic)?;
        let query_text = self
            .semantic_query_text
            .ok_or_else(|| SdkError::Usage("semantic query text is required".to_string()))?;
        let selection = self.selection.ok_or_else(|| {
            SdkError::Usage("semantic generation selection is required".to_string())
        })?;
        let top_k = self
            .top_k
            .ok_or_else(|| SdkError::Usage("semantic top_k is required".to_string()))?;
        let top_k = accepted_top_k("semantic", top_k)?;
        let (generation, generation_selector) = Self::selection_fields(selection);
        let constraints = self.constraints;
        let lexical_scope = match (self.scope_leg, self.scope_top_k) {
            (Some((syntax, query_text)), Some(scope_top_k)) => Some(TextQueryRequest {
                top_k: accepted_top_k("semantic scope", scope_top_k)?,
                syntax,
                query_text,
                constraints: constraints.clone(),
                generation: generation.clone(),
                generation_selector: generation_selector.clone(),
                cursor: None,
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
            constraints,
            generation,
            generation_selector,
            lexical_scope,
            top_k,
        })
    }

    /// The hybrid request (QI-BB-018): the text lane, the dense text, the
    /// selection and the fused `top_k`, gated once under the shared code.
    pub(crate) fn build_hybrid_request(
        self,
        client: &QuantaIndexClientPayloadV1,
        capability: LegacyQueryCapabilityV1,
    ) -> Result<HybridQueryRequest, SdkError> {
        client.admit_legacy_query_v1(capability)?;
        let (syntax, query_text) = self
            .text_leg
            .ok_or_else(|| SdkError::Usage("hybrid text query is required".to_string()))?;
        let semantic_query_text = self
            .semantic_query_text
            .ok_or_else(|| SdkError::Usage("hybrid semantic query text is required".to_string()))?;
        let selection = self.selection.ok_or_else(|| {
            SdkError::Usage("hybrid generation selection is required".to_string())
        })?;
        let top_k = self
            .top_k
            .ok_or_else(|| SdkError::Usage("hybrid top_k is required".to_string()))?;
        let top_k = accepted_top_k("hybrid", top_k)?;
        let (generation, generation_selector) = Self::selection_fields(selection);
        Ok(HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax,
                query_text,
                constraints: self.constraints,
                generation: generation.clone(),
                generation_selector: generation_selector.clone(),
                top_k,
                cursor: None,
            },
            semantic_query_text,
            generation,
            generation_selector,
            top_k,
        })
    }

    pub(crate) fn build_hybrid_seed_request(
        self,
        client: &QuantaIndexClientPayloadV1,
    ) -> Result<HybridSeedQueryRequest, SdkError> {
        client.admit_legacy_query_v1(LegacyQueryCapabilityV1::HybridSeed)?;
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
        let top_k = accepted_top_k("hybrid seed", top_k)?;
        let (generation, generation_selector) = Self::selection_fields(selection);
        Ok(HybridSeedQueryRequest {
            text_query: TextQueryRequest {
                syntax,
                query_text,
                constraints: self.constraints,
                generation: generation.clone(),
                generation_selector: generation_selector.clone(),
                top_k,
                cursor: None,
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
        QuantaIndexClientPayloadV1::selection_to_fields(selection)
    }
}
