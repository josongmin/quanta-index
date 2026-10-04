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
use crate::corpus::{SourceFile, split_line_starts};
use crate::symbols::{SYMBOL_PRODUCER_IDENTITY, SymbolNameSpan};
use crate::{BenchError, BenchResult, sha256_hex};

/// Which published namespace a unit belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishedUnitKind {
    Chunk,
    Symbol,
    /// A whole source-file candidate proved from file authority, not this
    /// published chunk/symbol registry.
    File,
}

impl PublishedUnitKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chunk => "chunk",
            Self::Symbol => "symbol",
            Self::File => "file",
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
    /// Present only when bound to the parser's exact local-name capture.
    pub name_span: Option<SymbolNameSpan>,
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
        for (path, source) in sources {
            validate_source(path, source)?;
        }
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
                validate_source_span(
                    source,
                    chunk.start_byte,
                    chunk.end_byte,
                    chunk.start_line,
                    chunk.end_line,
                    &chunk.chunk_id,
                )?;
                let start = usize::try_from(chunk.start_byte).map_err(|err| {
                    BenchError::Protocol(format!(
                        "published chunk start cannot fit usize for {}: {err}",
                        chunk.chunk_id
                    ))
                })?;
                let end = usize::try_from(chunk.end_byte).map_err(|err| {
                    BenchError::Protocol(format!(
                        "published chunk end cannot fit usize for {}: {err}",
                        chunk.chunk_id
                    ))
                })?;
                if source.bytes.get(start..end) != Some(chunk.text.as_bytes()) {
                    return Err(BenchError::Protocol(format!(
                        "published chunk text differs from admitted source bytes: {}",
                        chunk.chunk_id
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
                    name_span: None,
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
                validate_source_span(
                    source,
                    symbol.definition_span.byte_start,
                    symbol.definition_span.byte_end,
                    symbol.definition_span.line_start,
                    symbol.definition_span.line_end,
                    symbol.symbol_id.as_str(),
                )?;
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
                    name_span: None,
                })?;
            }
        }
        Ok(registry)
    }

    /// Bind all symbol names from the same preflight extraction, refusing
    /// partial inventories, foreign ids, and spans outside admitted definitions.
    pub fn with_symbol_names(
        mut self,
        names: &BTreeMap<String, BTreeMap<String, SymbolNameSpan>>,
        sources: &BTreeMap<String, SourceFile>,
    ) -> BenchResult<Self> {
        let supplied = names.values().map(BTreeMap::len).sum::<usize>();
        if supplied != self.symbol_count {
            return Err(BenchError::Protocol("symbol name inventory differs from published units".into()));
        }
        for (path, file_names) in names {
            let source = sources.get(path).ok_or_else(|| {
                BenchError::Protocol(format!("symbol name source is not admitted: {path}"))
            })?;
            validate_source(path, source)?;
            for (id, name) in file_names {
                let unit = self.by_id.get_mut(id).ok_or_else(|| {
                    BenchError::Protocol(format!("symbol name id is not published: {id}"))
                })?;
                if unit.kind != PublishedUnitKind::Symbol
                    || unit.path != *path
                    || unit.source_sha256 != source.sha256
                    || !(usize::try_from(unit.byte_start).is_ok_and(|start| start <= name.start_byte)
                        && name.start_byte < name.end_byte
                        && usize::try_from(unit.byte_end).is_ok_and(|end| name.end_byte <= end))
                    || source.text.get(name.start_byte..name.end_byte) != Some(name.name.as_str())
                {
                    return Err(BenchError::Protocol(format!("symbol name span differs from published source definition: {id}")));
                }
                unit.name_span = Some(name.clone());
            }
        }
        if self.by_id.values().any(|unit| unit.kind == PublishedUnitKind::Symbol && unit.name_span.is_none()) {
            return Err(BenchError::Protocol("published symbol lacks a parser name capture".into()));
        }
        Ok(self)
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
            PublishedUnitKind::File => {
                return Err(BenchError::Protocol(
                    "file identities are not published chunk/symbol units".to_string(),
                ));
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

fn validate_source(path: &str, source: &SourceFile) -> BenchResult<()> {
    let (expected_line_starts, exotic_boundary) = split_line_starts(&source.text);
    if source.path != path
        || source.text.as_bytes() != source.bytes
        || sha256_hex(&source.bytes) != source.sha256
        || exotic_boundary
        || source.line_starts != expected_line_starts
    {
        return Err(BenchError::Protocol(format!(
            "published-unit source authority is inconsistent: {path}"
        )));
    }
    Ok(())
}

fn validate_source_span(
    source: &SourceFile,
    byte_start: u32,
    byte_end: u32,
    line_start: u32,
    line_end: u32,
    unit_id: &str,
) -> BenchResult<()> {
    let start = usize::try_from(byte_start).map_err(|err| {
        BenchError::Protocol(format!(
            "published unit start cannot fit usize for {unit_id}: {err}"
        ))
    })?;
    let end = usize::try_from(byte_end).map_err(|err| {
        BenchError::Protocol(format!(
            "published unit end cannot fit usize for {unit_id}: {err}"
        ))
    })?;
    if start >= end || end > source.bytes.len() {
        return Err(BenchError::Protocol(format!(
            "published unit byte span is outside admitted source: {unit_id}"
        )));
    }
    let projected_start = source
        .line_starts
        .partition_point(|offset| *offset <= start);
    let projected_end = source
        .line_starts
        .partition_point(|offset| *offset <= end.saturating_sub(1));
    if usize::try_from(line_start) != Ok(projected_start)
        || usize::try_from(line_end) != Ok(projected_end)
    {
        return Err(BenchError::Protocol(format!(
            "published unit line projection differs from its byte span: {unit_id}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbols::extract_symbols;

    fn chunk(path: &str, text: &str) -> Chunk {
        let (line_starts, _) = split_line_starts(text);
        Chunk {
            chunk_id: format!("chunk-{path}-0"),
            path: path.to_string(),
            start_byte: 0,
            end_byte: u32::try_from(text.len()).expect("fixture length"),
            start_line: 1,
            end_line: u32::try_from(line_starts.len()).expect("fixture line count"),
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

    #[test]
    fn registry_refuses_units_that_do_not_match_source_bytes_or_line_projection() {
        let source_map = sources("src/lib.rs", RUST_SOURCE);
        let mut wrong_text = chunk("src/lib.rs", RUST_SOURCE);
        wrong_text.text = "forged".to_string();
        assert!(
            PublishedUnitRegistry::from_chunks_and_symbols(
                &BTreeMap::from([("src/lib.rs".to_string(), vec![wrong_text])]),
                &BTreeMap::new(),
                &source_map,
            )
            .is_err()
        );

        let mut wrong_line = chunk("src/lib.rs", RUST_SOURCE);
        wrong_line.end_line = 1;
        assert!(
            PublishedUnitRegistry::from_chunks_and_symbols(
                &BTreeMap::from([("src/lib.rs".to_string(), vec![wrong_line])]),
                &BTreeMap::new(),
                &source_map,
            )
            .is_err()
        );

        let mut forged_source = sources("src/lib.rs", RUST_SOURCE);
        forged_source.get_mut("src/lib.rs").expect("source").sha256 = "0".repeat(64);
        assert!(
            PublishedUnitRegistry::from_chunks_and_symbols(
                &BTreeMap::from([(
                    "src/lib.rs".to_string(),
                    vec![chunk("src/lib.rs", RUST_SOURCE)],
                )]),
                &BTreeMap::new(),
                &forged_source,
            )
            .is_err()
        );

        let mut forged_lines = sources("src/lib.rs", RUST_SOURCE);
        forged_lines
            .get_mut("src/lib.rs")
            .expect("source")
            .line_starts = vec![0, 1];
        assert!(
            PublishedUnitRegistry::from_chunks_and_symbols(
                &BTreeMap::new(),
                &BTreeMap::new(),
                &forged_lines,
            )
            .is_err()
        );

        let empty_source = SourceFile {
            path: "empty.rs".to_string(),
            bytes: Vec::new(),
            text: String::new(),
            line_starts: Vec::new(),
            sha256: sha256_hex(b""),
        };
        assert!(
            PublishedUnitRegistry::from_chunks_and_symbols(
                &BTreeMap::new(),
                &BTreeMap::new(),
                &BTreeMap::from([("empty.rs".to_string(), empty_source)]),
            )
            .is_ok()
        );
    }
}
