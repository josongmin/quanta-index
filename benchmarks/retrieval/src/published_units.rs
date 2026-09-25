//! Typed published-unit registry (RBR-05).
//!
//! The single proof authority for SDK hits: every chunk and every symbol
//! published by the batch becomes one typed unit keyed by its exact wire
//! id, with the original source binding (path, source sha, span, producer
//! identity). A symbol id can never masquerade as a chunk id: kinds are
//! distinct, and duplicate ids across either namespace refuse the
//! registry instead of silently shadowing.

use std::collections::BTreeMap;

use quanta_index_contract::lex::SymbolRecord;

use crate::chunking::Chunk;
use crate::corpus::SourceFile;
use crate::symbols::SYMBOL_PRODUCER_IDENTITY;
use crate::{BenchError, BenchResult};

/// Which published namespace a unit belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishedUnitKind {
    Chunk,
    Symbol,
}

impl PublishedUnitKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chunk => "chunk",
            Self::Symbol => "symbol",
        }
    }
}

/// One provable published unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedUnit {
    pub kind: PublishedUnitKind,
    pub id: String,
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    /// Source byte span of the published unit (RBR-05 authority).
    pub byte_start: u32,
    pub byte_end: u32,
    /// SHA-256 of the source file at publish time: binds the unit to the
    /// exact source bytes, not just line numbers.
    pub source_sha256: String,
    /// Producer identity for provenance (chunker strategy or symbol
    /// producer identity).
    pub producer_identity: String,
    /// Chunk text, present only for chunk units: the sole authority that
    /// can prove an unanchored SDK hit.
    pub chunk_text: Option<String>,
}

/// The typed registry over every unit the batch published.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PublishedUnitRegistry {
    by_id: BTreeMap<String, PublishedUnit>,
    chunk_count: usize,
    symbol_count: usize,
}

impl PublishedUnitRegistry {
    /// Build the registry from the per-file chunk and symbol maps.
    ///
    /// # Errors
    ///
    /// Refuses when a chunk id and a symbol id collide, or when either
    /// namespace holds a duplicate id: proof authority must be unique.
    pub fn from_chunks_and_symbols(
        chunks: &BTreeMap<String, Vec<Chunk>>,
        symbols: &BTreeMap<String, Vec<SymbolRecord>>,
        sources: &BTreeMap<String, SourceFile>,
    ) -> BenchResult<Self> {
        let mut registry = Self::default();
        for (path, file_chunks) in chunks {
            let source = sources.get(path).ok_or_else(|| {
                BenchError::Protocol(format!("published chunk source is not admitted: {path}"))
            })?;
            for chunk in file_chunks {
                if chunk.path != *path {
                    return Err(BenchError::Protocol(format!(
                        "published chunk path differs from its source bucket: {} != {path}",
                        chunk.path
                    )));
                }
                registry.insert(PublishedUnit {
                    kind: PublishedUnitKind::Chunk,
                    id: chunk.chunk_id.clone(),
                    path: path.clone(),
                    start_line: chunk.start_line,
                    end_line: chunk.end_line,
                    byte_start: chunk.start_byte,
                    byte_end: chunk.end_byte,
                    source_sha256: source.sha256.clone(),
                    producer_identity: chunk.strategy.clone(),
                    chunk_text: Some(chunk.text.clone()),
                })?;
            }
        }
        for (path, file_symbols) in symbols {
            let source = sources.get(path).ok_or_else(|| {
                BenchError::Protocol(format!("published symbol source is not admitted: {path}"))
            })?;
            for symbol in file_symbols {
                if symbol.repo_relative_path.as_str() != path {
                    return Err(BenchError::Protocol(format!(
                        "published symbol path differs from its source bucket: {} != {path}",
                        symbol.repo_relative_path.as_str()
                    )));
                }
                registry.insert(PublishedUnit {
                    kind: PublishedUnitKind::Symbol,
                    id: symbol.symbol_id.as_str().to_string(),
                    path: path.clone(),
                    start_line: symbol.definition_span.line_start,
                    end_line: symbol.definition_span.line_end,
                    byte_start: symbol.definition_span.byte_start,
                    byte_end: symbol.definition_span.byte_end,
                    source_sha256: source.sha256.clone(),
                    producer_identity: SYMBOL_PRODUCER_IDENTITY.to_string(),
                    chunk_text: None,
                })?;
            }
        }
        Ok(registry)
    }

    fn insert(&mut self, unit: PublishedUnit) -> BenchResult<()> {
        if self.by_id.contains_key(&unit.id) {
            return Err(BenchError::Protocol(format!(
                "published unit id is registered twice: {}",
                unit.id
            )));
        }
        match unit.kind {
            PublishedUnitKind::Chunk => {
                self.chunk_count = self
                    .chunk_count
                    .checked_add(1)
                    .ok_or_else(|| BenchError::Protocol("chunk unit count overflow".to_string()))?;
            }
            PublishedUnitKind::Symbol => {
                self.symbol_count = self.symbol_count.checked_add(1).ok_or_else(|| {
                    BenchError::Protocol("symbol unit count overflow".to_string())
                })?;
            }
        }
        let _previous = self.by_id.insert(unit.id.clone(), unit);
        Ok(())
    }

    /// Resolve one unit by its exact wire id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&PublishedUnit> {
        self.by_id.get(id)
    }

    /// The published chunk text for a chunk unit, if the id is one.
    #[must_use]
    pub fn chunk_text(&self, id: &str) -> Option<&str> {
        self.by_id
            .get(id)
            .and_then(|unit| unit.chunk_text.as_deref())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    #[must_use]
    pub const fn chunk_count(&self) -> usize {
        self.chunk_count
    }

    #[must_use]
    pub const fn symbol_count(&self) -> usize {
        self.symbol_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::split_line_starts;
    use crate::sha256_hex;
    use crate::symbols::extract_symbols;

    fn chunk(path: &str, text: &str) -> Chunk {
        Chunk {
            chunk_id: format!("chunk-{path}-0"),
            path: path.to_string(),
            start_byte: 0,
            end_byte: u32::try_from(text.len()).expect("fixture length"),
            start_line: 1,
            end_line: 1,
            text: text.to_string(),
            strategy: "whole_file".to_string(),
            version: "v1".to_string(),
            config: "{}".to_string(),
            fallback: false,
        }
    }

    const RUST_SOURCE: &str = "pub fn alpha() {}\nstruct Beta;\n";

    fn sources(path: &str, text: &str) -> BTreeMap<String, SourceFile> {
        let (line_starts, _) = split_line_starts(text);
        BTreeMap::from([(
            path.to_string(),
            SourceFile {
                path: path.to_string(),
                bytes: text.as_bytes().to_vec(),
                text: text.to_string(),
                line_starts,
                sha256: sha256_hex(text.as_bytes()),
            },
        )])
    }

    #[test]
    fn registry_keeps_kinds_apart_and_counts_them() {
        let chunks = BTreeMap::from([(
            "src/lib.rs".to_string(),
            vec![chunk("src/lib.rs", RUST_SOURCE)],
        )]);
        let extracted = extract_symbols("src/lib.rs", RUST_SOURCE).expect("symbols");
        let symbol_id = extracted
            .first()
            .expect("at least one extracted symbol")
            .symbol_id
            .as_str()
            .to_string();
        let symbols = BTreeMap::from([("src/lib.rs".to_string(), extracted)]);
        let registry = PublishedUnitRegistry::from_chunks_and_symbols(
            &chunks,
            &symbols,
            &sources("src/lib.rs", RUST_SOURCE),
        )
        .expect("registry");
        assert_eq!(registry.chunk_count(), 1);
        assert_eq!(registry.symbol_count(), 2);
        assert_eq!(registry.len(), 3);
        let unit = registry
            .get("chunk-src/lib.rs-0")
            .expect("chunk unit present");
        assert_eq!(unit.kind, PublishedUnitKind::Chunk);
        assert_eq!(unit.byte_start, 0);
        assert_eq!(
            unit.byte_end,
            u32::try_from(RUST_SOURCE.len()).expect("fixture length")
        );
        assert_eq!(unit.source_sha256, sha256_hex(RUST_SOURCE.as_bytes()));
        let symbol_unit = registry.get(&symbol_id).expect("symbol unit present");
        assert_eq!(symbol_unit.kind, PublishedUnitKind::Symbol);
        assert!(symbol_unit.byte_end > symbol_unit.byte_start);
        assert_eq!(
            symbol_unit.source_sha256,
            sha256_hex(RUST_SOURCE.as_bytes())
        );
        assert_eq!(symbol_unit.producer_identity, SYMBOL_PRODUCER_IDENTITY);

        assert!(
            PublishedUnitRegistry::from_chunks_and_symbols(&chunks, &symbols, &BTreeMap::new())
                .is_err()
        );
    }

    #[test]
    fn duplicate_and_cross_namespace_ids_refuse() {
        let chunks = BTreeMap::from([(
            "src/lib.rs".to_string(),
            vec![
                chunk("src/lib.rs", RUST_SOURCE),
                chunk("src/lib.rs", RUST_SOURCE),
            ],
        )]);
        assert!(
            PublishedUnitRegistry::from_chunks_and_symbols(
                &chunks,
                &BTreeMap::new(),
                &sources("src/lib.rs", RUST_SOURCE),
            )
            .is_err()
        );

        // A symbol id forged onto a chunk id collides across namespaces.
        let mut symbols = extract_symbols("src/lib.rs", RUST_SOURCE).expect("symbols");
        symbols
            .first_mut()
            .expect("at least one extracted symbol")
            .symbol_id = quanta_index_contract::SymbolId::new("chunk-src/lib.rs-0");
        let single = BTreeMap::from([(
            "src/lib.rs".to_string(),
            vec![chunk("src/lib.rs", RUST_SOURCE)],
        )]);
        let symbol_map = BTreeMap::from([("src/lib.rs".to_string(), symbols)]);
        assert!(
            PublishedUnitRegistry::from_chunks_and_symbols(
                &single,
                &symbol_map,
                &sources("src/lib.rs", RUST_SOURCE),
            )
            .is_err()
        );
    }
}
