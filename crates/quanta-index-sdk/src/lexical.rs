use quanta_index_channel::{BundleChannelPublisher, open_lexical_publisher};
use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationSelector, LexicalChannelOp, LexicalFullBundle,
    ManifestGeneration, RepoId, RevisionId, SymbolId, SymbolQueryResponse,
    TextQueryRequest, TextQueryResponse, TextQuerySyntax, UpsertChunk, UpsertSymbol,
};
use quanta_index_contract::lex::SymbolRecord;

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
        self.symbols.push(SymbolMutation::Upsert { symbol_id, record });
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
    pub(crate) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    #[must_use]
    pub fn query(&self) -> LexicalQueryBuilder<'a> {
        LexicalQueryBuilder::new(self.client)
    }

    pub fn publish(&self, batch: &LexicalBatch) -> Result<BatchReceipt, SdkError> {
        let publisher = open_lexical_publisher(self.client.state_root()?)?;
        let mut receipt = BatchReceipt::default();
        if matches!(batch.mode, BatchMode::ReplaceGeneration) {
            let seq = publisher.publish(LexicalChannelOp::FullBundle(LexicalFullBundle {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                generation: batch.generation,
                payload: batch.manifest_payload.clone(),
            }))?;
            receipt.record(seq);
        }
        for chunk in &batch.chunks {
            let op = match chunk {
                ChunkMutation::Upsert { chunk_id, record } => {
                    let payload = QuantaIndex::encode_cbor(record)?;
                    LexicalChannelOp::UpsertChunk(UpsertChunk {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        chunk_id: chunk_id.clone(),
                        payload,
                    })
                }
                ChunkMutation::Delete { chunk_id } => {
                    LexicalChannelOp::DeleteChunk(quanta_index_contract::DeleteChunk {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        chunk_id: chunk_id.clone(),
                    })
                }
            };
            let seq = publisher.publish(op)?;
            receipt.record(seq);
        }
        for symbol in &batch.symbols {
            let op = match symbol {
                SymbolMutation::Upsert { symbol_id, record } => {
                    let payload = QuantaIndex::encode_cbor(record)?;
                    LexicalChannelOp::UpsertSymbol(UpsertSymbol {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        symbol_id: symbol_id.clone(),
                        payload,
                    })
                }
                SymbolMutation::Delete { symbol_id } => {
                    LexicalChannelOp::DeleteSymbol(quanta_index_contract::DeleteSymbol {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        symbol_id: symbol_id.clone(),
                    })
                }
            };
            let seq = publisher.publish(op)?;
            receipt.record(seq);
        }
        if batch.seal {
            let seq =
                publisher.seal(batch.repo_id.clone(), batch.revision_id.clone(), batch.generation)?;
            receipt.record(seq);
            receipt.mark_sealed();
        }
        publisher.flush()?;
        Ok(receipt)
    }
}

pub struct LexicalQueryBuilder<'a> {
    client: &'a QuantaIndex,
    syntax: TextQuerySyntax,
    query_text: Option<String>,
    selection: Option<GenerationSelector>,
}

impl<'a> LexicalQueryBuilder<'a> {
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

    pub fn execute(self) -> Result<TextQueryResponse, SdkError> {
        let query_text = self
            .query_text
            .ok_or_else(|| SdkError::Usage("lexical query text is required".to_string()))?;
        let selection = self
            .selection
            .ok_or_else(|| SdkError::Usage("lexical generation selection is required".to_string()))?;
        let (generation, generation_selector) = QuantaIndex::selection_to_fields(selection);
        let response = self
            .client
            .dispatch_query(quanta_index_contract::SearchPlaneQueryIpcRequest::Text(
                TextQueryRequest {
                    syntax: self.syntax,
                    query_text,
                    generation,
                    generation_selector,
                },
            ))?;
        match response {
            quanta_index_contract::SearchPlaneQueryIpcResponse::Text(results) => Ok(results),
            other => Err(SdkError::Protocol(format!(
                "expected text query response, got {other:?}"
            ))),
        }
    }
}

pub(crate) fn execute_symbol_query(
    client: &QuantaIndex,
    syntax: TextQuerySyntax,
    query_text: String,
    selection: GenerationSelector,
) -> Result<SymbolQueryResponse, SdkError> {
    let (generation, generation_selector) = QuantaIndex::selection_to_fields(selection);
    let response = client.dispatch_query(quanta_index_contract::SearchPlaneQueryIpcRequest::Symbol(
        quanta_index_contract::SymbolQueryRequest {
            syntax,
            query_text,
            generation,
            generation_selector,
        },
    ))?;
    match response {
        quanta_index_contract::SearchPlaneQueryIpcResponse::Symbol(results) => Ok(results),
        other => Err(SdkError::Protocol(format!(
            "expected symbol query response, got {other:?}"
        ))),
    }
}
