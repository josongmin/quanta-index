use crate::{LexicalCandidate, PublishedGenerationSet, SearchExplanation};

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneLexicalQueryResponse {
    pub generation: PublishedGenerationSet,
    pub results: Vec<LexicalCandidate>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneSemanticQueryResponse {
    pub generation: PublishedGenerationSet,
    pub results: Vec<LexicalCandidate>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneHybridQueryResponse {
    pub generation: PublishedGenerationSet,
    pub results: Vec<LexicalCandidate>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneExplainQueryResponse {
    pub generation: PublishedGenerationSet,
    pub explanation: SearchExplanation,
}
