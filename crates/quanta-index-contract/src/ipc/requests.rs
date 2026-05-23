use crate::{LexicalCandidate, LqFilterSet, LqQuery, PublishedGenerationSet};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneLexicalQueryRequest {
    pub query: LqQuery,
    pub generation: Option<PublishedGenerationSet>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneSemanticQueryRequest {
    pub query_text: String,
    pub generation: Option<PublishedGenerationSet>,
    pub lexical_filters: LqFilterSet,
    pub top_k: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneHybridQueryRequest {
    pub lexical_query: LqQuery,
    pub semantic_query_text: String,
    pub generation: Option<PublishedGenerationSet>,
    pub top_k: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneExplainQueryRequest {
    pub generation: PublishedGenerationSet,
    pub candidate: LexicalCandidate,
}
