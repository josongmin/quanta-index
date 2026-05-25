use quanta_index_contract::lex::SymbolRecord;
use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationSelector, LexicalChunkDelete, LexicalChunkMutation,
    LexicalChunkUpsert, LexicalIngestBatch, LexicalSymbolDelete, LexicalSymbolMutation,
    LexicalSymbolUpsert, ManifestGeneration, RepoId, RevisionId, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcResponse, SymbolId, TextQueryRequest, TextQueryResponse, TextQuerySyntax,
};

use crate::{BatchMode, BatchReceipt, QuantaIndex, SdkError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChunkMutation {
    Upsert {
        chunk_id: ChunkId,
        record: ChunkRecord,
    },
    Delete {
        chunk_id: ChunkId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SymbolMutation {
    Upsert {
        symbol_id: SymbolId,
        record: SymbolRecord,
    },
    Delete {
        symbol_id: SymbolId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub mode: BatchMode,
    pub manifest_payload: Vec<u8>,
    pub chunks: Vec<ChunkMutation>,
    pub symbols: Vec<SymbolMutation>,
    pub seal: bool,
}

impl LexicalBatch {
    #[must_use]
    pub fn replace_generation(
        repo_id: RepoId,
        revision_id: RevisionId,
        generation: ManifestGeneration,
    ) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            mode: BatchMode::ReplaceGeneration,
            manifest_payload: Vec::new(),
            chunks: Vec::new(),
            symbols: Vec::new(),
            seal: true,
        }
    }

    #[must_use]
    pub fn delta(repo_id: RepoId, revision_id: RevisionId, generation: ManifestGeneration) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            mode: BatchMode::Delta,
            manifest_payload: Vec::new(),
            chunks: Vec::new(),
            symbols: Vec::new(),
            seal: true,
        }
    }

    #[must_use]
    pub fn manifest_payload(mut self, payload: Vec<u8>) -> Self {
        self.manifest_payload = payload;
        self
    }

    #[must_use]
    pub fn chunk_upsert(mut self, chunk_id: ChunkId, record: ChunkRecord) -> Self {
        self.chunks.push(ChunkMutation::Upsert { chunk_id, record });
        self
    }

    #[must_use]
    pub fn chunk_delete(mut self, chunk_id: ChunkId) -> Self {
        self.chunks.push(ChunkMutation::Delete { chunk_id });
        self
    }

    #[must_use]
    pub fn symbol_upsert(mut self, symbol_id: SymbolId, record: SymbolRecord) -> Self {
        self.symbols
            .push(SymbolMutation::Upsert { symbol_id, record });
        self
    }

    #[must_use]
    pub fn symbol_delete(mut self, symbol_id: SymbolId) -> Self {
        self.symbols.push(SymbolMutation::Delete { symbol_id });
        self
    }

    #[must_use]
    pub fn without_seal(mut self) -> Self {
        self.seal = false;
        self
    }
}

pub struct LexicalNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> LexicalNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    /// Sugar for `client.ns::<LexicalNs>().query()`. See QI-NS-01.
    #[must_use]
    pub fn query(&self) -> LexicalQueryBuilder<'a> {
        <LexicalNs as crate::NamespaceQuery>::query(self.client)
    }

    /// Sugar for `client.ns::<LexicalNs>().publish(batch)`. See QI-NS-01.
    pub fn publish(&self, batch: &LexicalBatch) -> Result<BatchReceipt, SdkError> {
        <LexicalNs as crate::NamespaceIngest>::publish(self.client, batch)
    }
}

/// QI-NS-01: marker type for the built-in lexical namespace. The
/// `client.lexical()` sugar delegates here through
/// [`crate::NamespaceIngest`] / [`crate::NamespaceQuery`]; downstream
/// callers that want explicit type-level routing can use
/// `client.ns::<LexicalNs>()` directly.
pub struct LexicalNs;

impl crate::NamespaceIngest for LexicalNs {
    type Batch = LexicalBatch;
    type Receipt = BatchReceipt;

    fn publish(client: &QuantaIndex, batch: &LexicalBatch) -> Result<BatchReceipt, SdkError> {
        let wire_batch = LexicalIngestBatch {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
            mode: batch.mode.to_wire(),
            manifest_payload: batch.manifest_payload.clone(),
            chunks: batch.chunks.iter().map(map_chunk).collect(),
            symbols: batch.symbols.iter().map(map_symbol).collect(),
            seal: batch.seal,
        };
        let response =
            client.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishLexicalBatch(wire_batch))?;
        match response {
            SearchPlaneIngestIpcResponse::LexicalReceipt(receipt) => Ok(receipt),
            other @ (SearchPlaneIngestIpcResponse::SemanticReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
            | quanta_index_contract::SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | quanta_index_contract::SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | quanta_index_contract::SearchPlaneIngestIpcResponse::StructuralReceipt(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::Protocol(format!(
                "expected lexical receipt, got {}",
                QuantaIndex::ingest_response_kind(&other)
            ))),
        }
    }
}

impl crate::NamespaceQuery for LexicalNs {
    type QueryBuilder<'a> = LexicalQueryBuilder<'a>;

    fn query<'a>(client: &'a QuantaIndex) -> LexicalQueryBuilder<'a> {
        LexicalQueryBuilder::new(client)
    }
}

fn map_chunk(mutation: &ChunkMutation) -> LexicalChunkMutation {
    match mutation {
        ChunkMutation::Upsert { chunk_id, record } => {
            LexicalChunkMutation::Upsert(LexicalChunkUpsert {
                chunk_id: chunk_id.clone(),
                record: record.clone(),
            })
        }
        ChunkMutation::Delete { chunk_id } => LexicalChunkMutation::Delete(LexicalChunkDelete {
            chunk_id: chunk_id.clone(),
        }),
    }
}

fn map_symbol(mutation: &SymbolMutation) -> LexicalSymbolMutation {
    match mutation {
        SymbolMutation::Upsert { symbol_id, record } => {
            LexicalSymbolMutation::Upsert(LexicalSymbolUpsert {
                symbol_id: symbol_id.clone(),
                record: record.clone(),
            })
        }
        SymbolMutation::Delete { symbol_id } => {
            LexicalSymbolMutation::Delete(LexicalSymbolDelete {
                symbol_id: symbol_id.clone(),
            })
        }
    }
}

pub struct LexicalQueryBuilder<'a> {
    client: &'a QuantaIndex,
    syntax: TextQuerySyntax,
    query_text: Option<String>,
    selection: Option<GenerationSelector>,
    top_k: Option<u32>,
}

impl<'a> LexicalQueryBuilder<'a> {
    const fn new(client: &'a QuantaIndex) -> Self {
        Self {
            client,
            syntax: TextQuerySyntax::Native,
            query_text: None,
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

    /// QI-QRY-01: required result cap. SDK enforces this is set before
    /// dispatch so the contract DTO carries an authoritative value.
    #[must_use]
    pub fn top_k(mut self, top_k: u32) -> Self {
        self.top_k = Some(top_k);
        self
    }

    pub fn execute(self) -> Result<TextQueryResponse, SdkError> {
        let query_text = self
            .query_text
            .ok_or_else(|| SdkError::Usage("lexical query text is required".to_string()))?;
        let selection = self.selection.ok_or_else(|| {
            SdkError::Usage("lexical generation selection is required".to_string())
        })?;
        let top_k = self
            .top_k
            .ok_or_else(|| SdkError::Usage("lexical top_k is required".to_string()))?;
        let (generation, generation_selector) = QuantaIndex::selection_to_fields(selection);
        let response =
            self.client
                .dispatch_query(quanta_index_contract::SearchPlaneQueryIpcRequest::Text(
                    TextQueryRequest {
                        syntax: self.syntax,
                        query_text,
                        generation,
                        generation_selector,
                        top_k,
                    },
                ))?;
        match response {
            quanta_index_contract::SearchPlaneQueryIpcResponse::Text(results) => Ok(results),
            other @ (quanta_index_contract::SearchPlaneQueryIpcResponse::Symbol(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Semantic(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Hybrid(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::History(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Structural(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Bridge(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Explain(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Error(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::Sourcegraph(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                Err(SdkError::Protocol(format!(
                    "expected text query response, got {}",
                    QuantaIndex::query_response_kind(&other)
                )))
            }
        }
    }
}
