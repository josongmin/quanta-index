use quanta_index_contract::{
    GenerationPin, GenerationSelector, HybridQueryRequest, HybridQueryResponse, LexicalCandidate,
    RepoId, RevisionId, SearchPlaneExplainQueryRequest, SearchPlaneExplainQueryResponse,
    SemanticVectorRef, TextQueryRequest, TextQuerySyntax,
};

use crate::{QuantaIndex, SdkError, SemanticVector};

pub struct SearchNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> SearchNamespace<'a> {
    pub(crate) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    #[must_use]
    pub fn hybrid(&self) -> HybridQueryBuilder<'a> {
        HybridQueryBuilder::new(self.client)
    }

    pub fn explain(
        &self,
        generation: GenerationPin,
        candidate: LexicalCandidate,
    ) -> Result<SearchPlaneExplainQueryResponse, SdkError> {
        let response = self.client.dispatch_query(
            quanta_index_contract::SearchPlaneQueryIpcRequest::Explain(
                SearchPlaneExplainQueryRequest {
                    generation,
                    candidate,
                },
            ),
        )?;
        match response {
            quanta_index_contract::SearchPlaneQueryIpcResponse::Explain(results) => Ok(results),
            other => Err(SdkError::Protocol(format!(
                "expected explain response, got {other:?}"
            ))),
        }
    }
}

pub struct HybridQueryBuilder<'a> {
    client: &'a QuantaIndex,
    syntax: TextQuerySyntax,
    query_text: Option<String>,
    vector: Option<SemanticVector>,
    selection: Option<GenerationSelector>,
    top_k: Option<u32>,
}

impl<'a> HybridQueryBuilder<'a> {
    const fn new(client: &'a QuantaIndex) -> Self {
        Self {
            client,
            syntax: TextQuerySyntax::Native,
            query_text: None,
            vector: None,
            selection: None,
            top_k: None,
        }
    }

    #[must_use]
    pub fn native(mut self, query_text: impl Into<String>) -> Self {
        self.syntax = TextQuerySyntax::Native;
        self.query_text = Some(query_text.into());
        self
    }

    #[must_use]
    pub fn sourcegraph(mut self, query_text: impl Into<String>) -> Self {
        self.syntax = TextQuerySyntax::Sourcegraph;
        self.query_text = Some(query_text.into());
        self
    }

    #[must_use]
    pub fn vector(mut self, vector: Vec<f32>) -> Self {
        self.vector = Some(SemanticVector::Inline(vector));
        self
    }

    #[must_use]
    pub fn vector_handle(mut self, handle: impl Into<String>) -> Self {
        self.vector = Some(SemanticVector::Handle(handle.into()));
        self
    }

    #[must_use]
    pub fn pinned(mut self, pin: GenerationPin) -> Self {
        self.selection = Some(GenerationSelector::Pinned(pin));
        self
    }

    #[must_use]
    pub fn active(mut self, repo_id: RepoId, revision_id: RevisionId) -> Self {
        self.selection = Some(GenerationSelector::Active {
            repo_id,
            revision_id,
        });
        self
    }

    #[must_use]
    pub fn top_k(mut self, top_k: u32) -> Self {
        self.top_k = Some(top_k);
        self
    }

    pub fn execute(self) -> Result<HybridQueryResponse, SdkError> {
        let query_text = self
            .query_text
            .ok_or_else(|| SdkError::Usage("hybrid text query is required".to_string()))?;
        let vector_ref = match self
            .vector
            .ok_or_else(|| SdkError::Usage("hybrid semantic vector is required".to_string()))?
        {
            SemanticVector::Inline(vector) => {
                if vector.is_empty() {
                    return Err(SdkError::Usage(
                        "hybrid semantic vector must not be empty".to_string(),
                    ));
                }
                SemanticVectorRef::Inline(vector)
            }
            SemanticVector::Handle(handle) => {
                if handle.is_empty() {
                    return Err(SdkError::Usage(
                        "hybrid semantic vector handle must not be empty".to_string(),
                    ));
                }
                SemanticVectorRef::Handle(handle)
            }
        };
        let selection = self
            .selection
            .ok_or_else(|| SdkError::Usage("hybrid generation selection is required".to_string()))?;
        let top_k = self
            .top_k
            .ok_or_else(|| SdkError::Usage("hybrid top_k is required".to_string()))?;
        let (generation, generation_selector) = QuantaIndex::selection_to_fields(selection.clone());
        let (text_generation, text_generation_selector) =
            QuantaIndex::selection_to_fields(selection);
        let response = self.client.dispatch_query(
            quanta_index_contract::SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax: self.syntax,
                    query_text,
                    generation: text_generation,
                    generation_selector: text_generation_selector,
                },
                semantic_query_text: String::new(),
                semantic_vector: None,
                semantic_vector_ref: Some(vector_ref),
                generation,
                generation_selector,
                top_k,
            }),
        )?;
        match response {
            quanta_index_contract::SearchPlaneQueryIpcResponse::Hybrid(results) => Ok(results),
            other => Err(SdkError::Protocol(format!(
                "expected hybrid query response, got {other:?}"
            ))),
        }
    }
}
