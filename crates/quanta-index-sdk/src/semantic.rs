use quanta_index_contract::{
    GenerationSelector, RepoId, RevisionId, SemanticQueryRequest, SemanticQueryResponse,
};

use crate::{QuantaIndex, SdkError, TextQuerySyntax, text_query_builder::VectorQueryBuilderState};

pub struct SemanticNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> SemanticNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    /// Public semantic surface is query-only. Corpus authority is derived
    /// inside `searchd` from search-corpus ingest; SDK callers do not publish
    /// semantic batches directly.
    #[must_use]
    pub fn query(&self) -> SemanticQueryBuilder<'a> {
        <SemanticNs as crate::NamespaceQuery>::query(self.client)
    }

    /// Contract-exact query replay surface. Accepts the shared wire DTO
    /// unchanged and routes it through the query transport.
    pub fn query_request(
        &self,
        request: SemanticQueryRequest,
    ) -> Result<SemanticQueryResponse, SdkError> {
        dispatch_semantic_query_request_v1(self.client, request)
    }
}

/// QI-NS-01: marker type for the built-in semantic namespace.
struct SemanticNs;

impl crate::NamespaceQuery for SemanticNs {
    type QueryBuilder<'a> = SemanticQueryBuilder<'a>;

    fn query(client: &QuantaIndex) -> SemanticQueryBuilder<'_> {
        SemanticQueryBuilder::new(client)
    }
}

pub struct SemanticQueryBuilder<
    'a,
    const HAS_TEXT: bool = false,
    const HAS_SELECTION: bool = false,
    const HAS_TOP_K: bool = false,
    const HAS_SCOPE: bool = false,
    const HAS_SCOPE_TOP_K: bool = false,
> {
    client: &'a QuantaIndex,
    state: VectorQueryBuilderState,
}

impl<'a> SemanticQueryBuilder<'a> {
    fn new(client: &'a QuantaIndex) -> Self {
        Self {
            client,
            state: VectorQueryBuilderState::new(),
        }
    }
}

impl<
    'a,
    const HAS_TEXT: bool,
    const HAS_SELECTION: bool,
    const HAS_TOP_K: bool,
    const HAS_SCOPE: bool,
    const HAS_SCOPE_TOP_K: bool,
> SemanticQueryBuilder<'a, HAS_TEXT, HAS_SELECTION, HAS_TOP_K, HAS_SCOPE, HAS_SCOPE_TOP_K>
{
    fn transition<
        const NEXT_TEXT: bool,
        const NEXT_SELECTION: bool,
        const NEXT_TOP_K: bool,
        const NEXT_SCOPE: bool,
        const NEXT_SCOPE_TOP_K: bool,
    >(
        mut self,
        update: impl FnOnce(&mut VectorQueryBuilderState),
    ) -> SemanticQueryBuilder<'a, NEXT_TEXT, NEXT_SELECTION, NEXT_TOP_K, NEXT_SCOPE, NEXT_SCOPE_TOP_K>
    {
        update(&mut self.state);
        SemanticQueryBuilder {
            client: self.client,
            state: self.state,
        }
    }

    #[must_use]
    pub fn text(
        self,
        query_text: impl Into<String>,
    ) -> SemanticQueryBuilder<'a, true, HAS_SELECTION, HAS_TOP_K, HAS_SCOPE, HAS_SCOPE_TOP_K> {
        self.transition(|state| {
            state.semantic_query_text = Some(query_text.into());
        })
    }

    #[must_use]
    pub fn scope_native(
        self,
        query_text: impl Into<String>,
    ) -> SemanticQueryBuilder<'a, HAS_TEXT, HAS_SELECTION, HAS_TOP_K, true, HAS_SCOPE_TOP_K> {
        self.transition(|state| {
            state.scope_leg = Some((TextQuerySyntax::Native, query_text.into()));
        })
    }

    #[must_use]
    pub fn scope_sourcegraph(
        self,
        query_text: impl Into<String>,
    ) -> SemanticQueryBuilder<'a, HAS_TEXT, HAS_SELECTION, HAS_TOP_K, true, HAS_SCOPE_TOP_K> {
        self.transition(|state| {
            state.scope_leg = Some((TextQuerySyntax::Sourcegraph, query_text.into()));
        })
    }

    /// Replace the canonical OR-set applied to both semantic recall and the
    /// optional lexical scope leg.
    #[must_use]
    pub fn language_any_of(
        self,
        languages: impl IntoIterator<Item = quanta_index_contract::lex::LanguageCode>,
    ) -> Self {
        self.transition(|state| {
            state.constraints = std::mem::take(&mut state.constraints).with_languages(languages);
        })
    }

    /// Restrict both dense recall and the optional lexical scope leg to one
    /// validated repository-relative path.
    #[must_use]
    pub fn exact_repo_relative_path(
        self,
        path: quanta_index_contract::ExactRepoRelativePathV1,
    ) -> Self {
        self.transition(|state| {
            state.constraints =
                std::mem::take(&mut state.constraints).with_exact_repo_relative_path(path);
        })
    }

    #[must_use]
    pub fn pinned(
        self,
        pin: quanta_index_contract::GenerationPin,
    ) -> SemanticQueryBuilder<'a, HAS_TEXT, true, HAS_TOP_K, HAS_SCOPE, HAS_SCOPE_TOP_K> {
        self.transition(|state| {
            state.selection = Some(GenerationSelector::Pinned(pin));
        })
    }

    #[must_use]
    pub fn active(
        self,
        repo_id: RepoId,
        revision_id: RevisionId,
    ) -> SemanticQueryBuilder<'a, HAS_TEXT, true, HAS_TOP_K, HAS_SCOPE, HAS_SCOPE_TOP_K> {
        self.transition(|state| {
            state.selection = Some(GenerationSelector::Active {
                repo_id,
                revision_id,
            });
        })
    }

    #[must_use]
    pub fn top_k(
        self,
        top_k: u32,
    ) -> SemanticQueryBuilder<'a, HAS_TEXT, HAS_SELECTION, true, HAS_SCOPE, HAS_SCOPE_TOP_K> {
        self.transition(|state| {
            state.top_k = Some(top_k);
        })
    }

    /// QI-QRY-01: explicit lexical-scope candidate cap. When the
    /// builder's lexical scope (`scope_native` / `scope_sourcegraph`) is
    /// set, this MUST also be set before the builder reaches an
    /// executable typestate. The two values are semantically
    /// distinct: outer `top_k` is the final semantic recall cap, while
    /// `scope_top_k` is the lexical candidate cap fed into the hybrid
    /// scope stage.
    #[must_use]
    pub fn scope_top_k(
        self,
        scope_top_k: u32,
    ) -> SemanticQueryBuilder<'a, HAS_TEXT, HAS_SELECTION, HAS_TOP_K, HAS_SCOPE, true> {
        self.transition(|state| {
            state.scope_top_k = Some(scope_top_k);
        })
    }
}

impl SemanticQueryBuilder<'_, true, true, true, false, false> {
    pub fn execute(self) -> Result<SemanticQueryResponse, SdkError> {
        dispatch_semantic_query_request_v1(self.client, self.state.build_semantic_request()?)
    }
}

impl SemanticQueryBuilder<'_, true, true, true, true, true> {
    pub fn execute(self) -> Result<SemanticQueryResponse, SdkError> {
        dispatch_semantic_query_request_v1(self.client, self.state.build_semantic_request()?)
    }
}

fn dispatch_semantic_query_request_v1(
    client: &QuantaIndex,
    request: SemanticQueryRequest,
) -> Result<SemanticQueryResponse, SdkError> {
    let response = client.dispatch_query(
        quanta_index_contract::SearchPlaneQueryIpcRequest::Semantic(request),
    )?;
    match response {
        quanta_index_contract::SearchPlaneQueryIpcResponse::Semantic(results) => Ok(results),
        other @ (quanta_index_contract::SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(
            _,
        )
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Text(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Symbol(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Hybrid(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::HybridSeed(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::History(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Structural(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Explain(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            Err(SdkError::Protocol(format!(
                "expected semantic response, got {}",
                QuantaIndex::query_response_kind(&other)
            )))
        }
    }
}
