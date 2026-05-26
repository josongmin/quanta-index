use quanta_index_contract::{GenerationSelector, TextQueryRequest, TextQuerySyntax};

use crate::{QuantaIndex, SdkError};

pub(super) struct TextQueryBuilderState {
    pub(super) syntax: TextQuerySyntax,
    pub(super) query_text: Option<String>,
    pub(super) selection: Option<GenerationSelector>,
    pub(super) top_k: Option<u32>,
}

impl TextQueryBuilderState {
    pub(super) const fn new() -> Self {
        Self {
            syntax: TextQuerySyntax::Native,
            query_text: None,
            selection: None,
            top_k: None,
        }
    }

    pub(super) fn build_request(self, plane: &str) -> Result<TextQueryRequest, SdkError> {
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
