use quanta_index_contract::{GenerationSelector, RepoId, RevisionId, SymbolQueryResponse, TextQuerySyntax};

use crate::{QuantaIndex, SdkError, lexical::execute_symbol_query};

pub struct SymbolNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> SymbolNamespace<'a> {
    pub(crate) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    #[must_use]
    pub fn query(&self) -> SymbolQueryBuilder<'a> {
        SymbolQueryBuilder::new(self.client)
    }
}

pub struct SymbolQueryBuilder<'a> {
    client: &'a QuantaIndex,
    syntax: TextQuerySyntax,
    query_text: Option<String>,
    selection: Option<GenerationSelector>,
}

impl<'a> SymbolQueryBuilder<'a> {
    const fn new(client: &'a QuantaIndex) -> Self {
        Self {
            client,
            syntax: TextQuerySyntax::Native,
            query_text: None,
            selection: None,
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
    pub fn pinned(mut self, pin: quanta_index_contract::GenerationPin) -> Self {
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

    pub fn execute(self) -> Result<SymbolQueryResponse, SdkError> {
        let query_text = self
            .query_text
            .ok_or_else(|| SdkError::Usage("symbol query text is required".to_string()))?;
        let selection = self
            .selection
            .ok_or_else(|| SdkError::Usage("symbol generation selection is required".to_string()))?;
        execute_symbol_query(self.client, self.syntax, query_text, selection)
    }
}
