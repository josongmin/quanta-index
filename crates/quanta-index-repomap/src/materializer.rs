//! Stored RepoMap projection metadata, compiled payload reopening, and query materialization.

use std::collections::BTreeMap;

use serde::{Deserialize as _, de::value::BorrowedStrDeserializer};

use quanta_index_contract::{
    ChunkId, CompiledRepoMapEdgeV1, CompiledRepoMapGraphV1, CompiledRepoMapNodeV1,
    CompiledRepoMapProjectionEntryV1, FileId, ManifestGeneration, RepoId, RepoMapDocType,
    RepoMapEdgeKind, RepoMapExactnessSummary, RepoMapGraphCoverageClass,
    RepoMapItemIndexAvailability, RepoMapModuleId, RepoMapNodeRef, RepoMapRedactionState,
    RepoMapSnapshotMeta, RepoMapSourceBundle, RepoRelativePath, RevisionId, SymbolId,
    repomap::compiler::{node_ref_parts, producer_digest},
};

pub use quanta_index_contract::{
    native_protocol::{
        NativeRepoMapFileOwnerProjectionV1, NativeRepoMapOwnerError, NativeRepoMapOwnerProjection,
    },
    repomap::compiler::RepoMapGraphCompiler,
};

use crate::model::{RepoMapEntry, RepoMapSnapshot};

#[cfg(test)]
use quanta_index_contract::repomap::compiler::encode_compiled_payload;

/// The bundle-declared projection metadata a sealed candidate's query
/// projection is rebuilt from.
///
/// The P03 store persists exactly this (as JSON in the catalog candidate
/// row) so the boot-rebuilt snapshot is byte-identical to the publish-time
/// one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateProjectionMeta {
    pub snapshot_id: String,
    pub projection_version: u32,
    pub authority_digest: String,
    /// Current persisted candidates require both custody digests.
    pub manifest_digest: String,
    pub source_bundle_digest: String,
    pub item_index_availability: RepoMapItemIndexAvailability,
    pub graph_coverage_class: RepoMapGraphCoverageClass,
    pub exactness_summary: RepoMapExactnessSummary,
    pub redaction_state: RepoMapRedactionState,
}

impl CandidateProjectionMeta {
    #[must_use]
    pub fn from_bundle_with_source_digest(
        bundle: &RepoMapSourceBundle,
        source_bundle_digest: String,
    ) -> Self {
        Self {
            snapshot_id: bundle.snapshot_id.clone(),
            projection_version: bundle.projection_version,
            authority_digest: bundle.authority_digest.clone(),
            manifest_digest: bundle.manifest_digest.clone(),
            source_bundle_digest,
            item_index_availability: bundle.graph_coverage.item_index_availability,
            graph_coverage_class: bundle.graph_coverage.graph_coverage_class,
            exactness_summary: bundle.exactness_summary,
            redaction_state: bundle.redaction_state,
        }
    }

    /// Canonical JSON column form. Enum fields use their wire strings.
    pub fn to_json(&self) -> Result<String, quanta_index_core::CoreError> {
        let value = serde_json::json!({
            "snapshot_id": self.snapshot_id,
            "projection_version": self.projection_version,
            "authority_digest": self.authority_digest,
            "manifest_digest": self.manifest_digest,
            "source_bundle_digest": self.source_bundle_digest,
            "item_index_availability": self.item_index_availability.as_code_str(),
            "graph_coverage_class": self.graph_coverage_class.as_code_str(),
            "exactness_summary": self.exactness_summary.as_code_str(),
            "redaction_state": self.redaction_state.as_code_str(),
        });
        serde_json::to_string(&value).map_err(|err| {
            quanta_index_core::CoreError::Storage(format!(
                "repomap projection meta encode failed: {err}"
            ))
        })
    }

    /// Strict parse of the JSON column form; unknown enum codes are
    /// refused typed, never defaulted.
    pub fn from_json(value: &str) -> Result<Self, quanta_index_core::CoreError> {
        fn enum_str<T: for<'de> serde::Deserialize<'de>>(
            value: &serde_json::Value,
            field: &str,
        ) -> Result<T, quanta_index_core::CoreError> {
            serde_json::from_value(value.get(field).cloned().ok_or_else(|| {
                quanta_index_core::CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                    message: format!("repomap projection meta is missing `{field}`"),
                }
            })?)
            .map_err(|err| quanta_index_core::CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                message: format!("repomap projection meta field `{field}` is invalid: {err}"),
            })
        }
        fn plain_str(
            value: &serde_json::Value,
            field: &str,
        ) -> Result<String, quanta_index_core::CoreError> {
            value
                .get(field)
                .and_then(|field| field.as_str())
                .map(str::to_owned)
                .ok_or_else(|| quanta_index_core::CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                    message: format!("repomap projection meta is missing string `{field}`"),
                })
        }
        let value: serde_json::Value =
            serde_json::from_str(value).map_err(|err| quanta_index_core::CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                message: format!("repomap projection meta is not valid JSON: {err}"),
            })?;
        let Some(Ok(projection_version)) = value
            .get("projection_version")
            .and_then(serde_json::Value::as_u64)
            .map(u32::try_from)
        else {
            return Err(quanta_index_core::CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                message: "repomap projection meta is missing `projection_version`".to_string(),
            });
        };
        let manifest_digest = plain_str(&value, "manifest_digest")?;
        let source_bundle_digest = plain_str(&value, "source_bundle_digest")?;
        let _validated_manifest_digest = producer_digest(&manifest_digest).map_err(|err| {
            quanta_index_core::CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                message: format!(
                    "repomap projection meta has an invalid V2 manifest custody digest: {err}"
                ),
            }
        })?;
        let source_hex = source_bundle_digest
            .strip_prefix("sha256:")
            .ok_or_else(|| quanta_index_core::CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                message: "repomap projection meta has a non-canonical V2 source-bundle digest"
                    .to_string(),
            })?;
        if source_hex.len() != 64
            || !source_hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(quanta_index_core::CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                message: "repomap projection meta has a non-canonical V2 source-bundle digest"
                    .to_string(),
            });
        }
        Ok(Self {
            snapshot_id: plain_str(&value, "snapshot_id")?,
            projection_version,
            authority_digest: plain_str(&value, "authority_digest")?,
            manifest_digest,
            source_bundle_digest,
            item_index_availability: enum_str(&value, "item_index_availability")?,
            graph_coverage_class: enum_str(&value, "graph_coverage_class")?,
            exactness_summary: enum_str(&value, "exactness_summary")?,
            redaction_state: enum_str(&value, "redaction_state")?,
        })
    }
}

/// Build the query snapshot from a candidate projection plus its persisted
/// meta. Deterministic: the same projection and meta always build the same
/// snapshot, at publish time and at boot alike.
#[must_use]
pub fn snapshot_from_projection(
    repo_id: &RepoId,
    revision_id: &RevisionId,
    manifest_generation: ManifestGeneration,
    meta: &CandidateProjectionMeta,
    projection: &[CompiledRepoMapProjectionEntryV1],
) -> RepoMapSnapshot {
    let snapshot_meta = RepoMapSnapshotMeta {
        snapshot_id: meta.snapshot_id.clone(),
        projection_version: meta.projection_version,
        authority_digest: meta.authority_digest.clone(),
        item_index_availability: meta.item_index_availability,
        graph_coverage_class: meta.graph_coverage_class,
        exactness_summary: meta.exactness_summary,
    };
    let entries = projection
        .iter()
        .map(|entry| projection_entry_to_model_entry(meta, entry))
        .collect();
    RepoMapSnapshot {
        repo_id: repo_id.clone(),
        revision_id: revision_id.clone(),
        manifest_generation,
        snapshot_meta,
        entries,
    }
}

/// Strictly decode the compiled candidate payload back into its canonical
/// graph and projection tables.
///
/// Exactly one accepted form: the compiler's own canonical encoding; trailing
/// bytes, indefinite lengths, non-minimal integers and unknown variant tags
/// are refused.
pub fn decode_compiled_payload(
    bytes: &[u8],
) -> Result<
    (
        CompiledRepoMapGraphV1,
        Vec<CompiledRepoMapProjectionEntryV1>,
    ),
    quanta_index_core::CoreError,
> {
    let mut graph = CompiledRepoMapGraphV1 {
        nodes: Vec::new(),
        edges: Vec::new(),
    };
    let mut projection = Vec::new();
    visit_compiled_payload_with(
        bytes,
        |error| error,
        |entry| {
            match entry {
                CompiledPayloadEntry::Node(node) => graph.nodes.push(node),
                CompiledPayloadEntry::Edge(edge) => graph.edges.push(edge),
                CompiledPayloadEntry::Projection(entry) => projection.push(entry),
            }
            Ok(())
        },
    )?;
    Ok((graph, projection))
}

/// Use the serving decoder's validation without retaining decoded tables.
pub(crate) fn validate_compiled_payload(bytes: &[u8]) -> Result<(), quanta_index_core::CoreError> {
    visit_compiled_payload_with(bytes, |error| error, |_entry| Ok(()))
}

/// One canonical row from the old compiler payload. The visitor sees rows in
/// table order and can refuse before retaining any unbounded decoded table.
#[derive(Debug, Eq, PartialEq)]
pub enum CompiledPayloadEntry {
    Node(CompiledRepoMapNodeV1),
    Edge(CompiledRepoMapEdgeV1),
    Projection(CompiledRepoMapProjectionEntryV1),
}

pub fn visit_compiled_payload_with<E>(
    bytes: &[u8],
    map_error: impl Fn(quanta_index_core::CoreError) -> E,
    mut visit: impl FnMut(CompiledPayloadEntry) -> Result<(), E>,
) -> Result<(), E> {
    visit_compiled_payload_with_span(bytes, map_error, |row, _span| visit(row))
}

/// The span points at the exact canonical row in the original immutable CBOR
/// payload, including its three/five-field tuple header. It can be persisted
/// as an inline Native map value without duplicating the old graph's rows.
pub fn visit_compiled_payload_with_span<E>(
    bytes: &[u8],
    map_error: impl Fn(quanta_index_core::CoreError) -> E,
    mut visit: impl FnMut(CompiledPayloadEntry, std::ops::Range<usize>) -> Result<(), E>,
) -> Result<(), E> {
    let refuse = |detail: String| -> E {
        map_error(quanta_index_core::CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
            message: format!("repomap compiled payload decode refused: {detail}"),
        })
    };
    let mut reader = CompileCborReader::new(bytes);
    // The compiler's payload frame is: a top-level array header whose
    // length is the schema version (2), then the three canonical tables
    // (nodes, edges, projection) each under its own definite array header.
    let version = reader.array_header("payload").map_err(refuse)?;
    if version != 2 {
        return Err(refuse(format!(
            "payload schema version {version}, expected 2"
        )));
    }
    let nodes_len = reader.array_header("nodes").map_err(refuse)?;
    if !matches!(nodes_len, 0..=2_000_000) {
        return Err(refuse(format!("node table holds {nodes_len} entries")));
    }
    for _ in 0..nodes_len {
        let start = reader.position;
        let entry_len = reader.array_header("node").map_err(refuse)?;
        if entry_len != 3 {
            return Err(refuse(format!(
                "node array holds {entry_len} items, expected 3"
            )));
        }
        let identity = reader.node_ref("node.identity").map_err(refuse)?;
        let degree_in = reader.u32_value("node.degree_in").map_err(refuse)?;
        let degree_out = reader.u32_value("node.degree_out").map_err(refuse)?;
        visit(
            CompiledPayloadEntry::Node(CompiledRepoMapNodeV1 {
                identity,
                degree_in,
                degree_out,
            }),
            start..reader.position,
        )?;
    }
    let edges_len = reader.array_header("edges").map_err(refuse)?;
    if !matches!(edges_len, 0..=20_000_000) {
        return Err(refuse(format!("edge table holds {edges_len} entries")));
    }
    for _ in 0..edges_len {
        let start = reader.position;
        let entry_len = reader.array_header("edge").map_err(refuse)?;
        if entry_len != 3 {
            return Err(refuse(format!(
                "edge array holds {entry_len} items, expected 3"
            )));
        }
        let kind = reader.edge_kind("edge.kind").map_err(refuse)?;
        let source = reader.node_ref("edge.source").map_err(refuse)?;
        let target = reader.node_ref("edge.target").map_err(refuse)?;
        visit(
            CompiledPayloadEntry::Edge(CompiledRepoMapEdgeV1 {
                kind,
                source,
                target,
            }),
            start..reader.position,
        )?;
    }
    let projection_len = reader.array_header("projection").map_err(refuse)?;
    if !matches!(projection_len, 0..=2_000_000) {
        return Err(refuse(format!(
            "projection table holds {projection_len} entries"
        )));
    }
    for _ in 0..projection_len {
        let start = reader.position;
        let entry_len = reader.array_header("projection-entry").map_err(refuse)?;
        if entry_len != 5 {
            return Err(refuse(format!(
                "projection entry holds {entry_len} items, expected 5"
            )));
        }
        let subject = reader.node_ref("projection.subject").map_err(refuse)?;
        let doc_type = reader.doc_type("projection.doc_type").map_err(refuse)?;
        let owner_path = reader.path_id("projection.owner_path").map_err(refuse)?;
        let search_text = reader.text("projection.search_text").map_err(refuse)?;
        let final_score_millis = reader
            .i64_value("projection.final_score_millis")
            .map_err(refuse)?;
        visit(
            CompiledPayloadEntry::Projection(CompiledRepoMapProjectionEntryV1 {
                subject,
                doc_type,
                owner_path,
                search_text: search_text.to_owned(),
                final_score_millis,
            }),
            start..reader.position,
        )?;
    }
    reader.expect_end().map_err(refuse)
}

/// Decode one authenticated index span without allocating the complete old
/// graph. The caller must first join its span to a verified import header and
/// compare the returned full identity with the requested digest-keyed row.
pub fn decode_compiled_payload_span_v1(
    bytes: &[u8],
    column: usize,
    start: usize,
    end: usize,
) -> Result<CompiledPayloadEntry, quanta_index_core::CoreError> {
    let refuse = |detail: String| quanta_index_core::CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
        message: format!("repomap compiled span decode refused: {detail}"),
    };
    let span = bytes
        .get(start..end)
        .ok_or_else(|| refuse("row bounds differ".into()))?;
    let mut reader = CompileCborReader::new(span);
    let row = match column {
        0 => {
            if reader.array_header("node").map_err(refuse)? != 3 {
                return Err(refuse("node tuple length differs".into()));
            }
            CompiledPayloadEntry::Node(CompiledRepoMapNodeV1 {
                identity: reader.node_ref("node.identity").map_err(refuse)?,
                degree_in: reader.u32_value("node.degree_in").map_err(refuse)?,
                degree_out: reader.u32_value("node.degree_out").map_err(refuse)?,
            })
        }
        1 => {
            if reader.array_header("edge").map_err(refuse)? != 3 {
                return Err(refuse("edge tuple length differs".into()));
            }
            CompiledPayloadEntry::Edge(CompiledRepoMapEdgeV1 {
                kind: reader.edge_kind("edge.kind").map_err(refuse)?,
                source: reader.node_ref("edge.source").map_err(refuse)?,
                target: reader.node_ref("edge.target").map_err(refuse)?,
            })
        }
        2 => {
            if reader.array_header("projection-entry").map_err(refuse)? != 5 {
                return Err(refuse("projection tuple length differs".into()));
            }
            CompiledPayloadEntry::Projection(CompiledRepoMapProjectionEntryV1 {
                subject: reader.node_ref("projection.subject").map_err(refuse)?,
                doc_type: reader.doc_type("projection.doc_type").map_err(refuse)?,
                owner_path: reader.path_id("projection.owner_path").map_err(refuse)?,
                search_text: reader
                    .text("projection.search_text")
                    .map_err(refuse)?
                    .to_owned(),
                final_score_millis: reader
                    .i64_value("projection.final_score_millis")
                    .map_err(refuse)?,
            })
        }
        _ => return Err(refuse("row column differs".into())),
    };
    reader.expect_end().map_err(refuse)?;
    Ok(row)
}

#[cfg(test)]
mod native_span_tests {
    use super::*;

    #[test]
    fn canonical_compiler_rows_reopen_from_exact_spans() {
        let first = RepoMapNodeRef::File(FileId::new("src/a.rs"));
        let second = RepoMapNodeRef::File(FileId::new("src/b.rs"));
        let graph = CompiledRepoMapGraphV1 {
            nodes: vec![
                CompiledRepoMapNodeV1 {
                    identity: first.clone(),
                    degree_in: 0,
                    degree_out: 1,
                },
                CompiledRepoMapNodeV1 {
                    identity: second.clone(),
                    degree_in: 1,
                    degree_out: 0,
                },
            ],
            edges: vec![CompiledRepoMapEdgeV1 {
                kind: RepoMapEdgeKind::Contains,
                source: first.clone(),
                target: second,
            }],
        };
        let projection = vec![CompiledRepoMapProjectionEntryV1 {
            subject: first,
            doc_type: RepoMapDocType::File,
            owner_path: RepoRelativePath::new("src/a.rs"),
            search_text: "alpha".into(),
            final_score_millis: 500,
        }];
        let bytes = encode_compiled_payload(&graph, &projection);
        let mut seen = 0;
        let mut last_end = 0;
        visit_compiled_payload_with_span(
            &bytes,
            |error| error,
            |row, span| {
                assert!(span.start >= last_end && span.start < span.end);
                let column = match row {
                    CompiledPayloadEntry::Node(_) => 0,
                    CompiledPayloadEntry::Edge(_) => 1,
                    CompiledPayloadEntry::Projection(_) => 2,
                };
                assert_eq!(
                    decode_compiled_payload_span_v1(&bytes, column, span.start, span.end)?,
                    row
                );
                assert!(
                    decode_compiled_payload_span_v1(&bytes, column, span.start, span.end - 1)
                        .is_err()
                );
                last_end = span.end;
                seen += 1;
                Ok::<_, quanta_index_core::CoreError>(())
            },
        )
        .expect("canonical spans");
        assert_eq!(seen, 4);
    }
}

/// Minimal strict CBOR reader for the compiler's canonical payload schema:
/// definite lengths, minimal integers, UTF-8 text only.
struct CompileCborReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> CompileCborReader<'a> {
    // The persisted node, edge and document tags are all short ASCII tokens.
    // Bound diagnostics before passing untrusted text to a variant decoder.
    const MAX_TAG_BYTES: usize = 16;

    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn peek(&self) -> Result<u8, String> {
        self.bytes
            .get(self.position)
            .copied()
            .ok_or_else(|| "unexpected end".to_string())
    }

    fn take(&mut self) -> Result<u8, String> {
        let byte = self.peek()?;
        self.position = self.position.saturating_add(1);
        Ok(byte)
    }

    fn uint_value(&mut self, label: &str) -> Result<u64, String> {
        let major = self.take()?;
        if major & 0xe0 != 0 {
            return Err(format!("{label}: expected unsigned integer"));
        }
        self.uint_body(major & 0x1f, label)
    }

    /// The compiler's framing writes table and tuple lengths as bare
    /// canonical unsigned integers (not CBOR array heads); read exactly
    /// that, with the minimality check `uint_value` carries.
    fn array_header(&mut self, label: &str) -> Result<u64, String> {
        let value = self.uint_value(label)?;
        Ok(value)
    }

    fn uint_body(&mut self, info: u8, label: &str) -> Result<u64, String> {
        let (width, minimum) = match info {
            0..=23 => return Ok(u64::from(info)),
            24 => (1, 24),
            25 => (2, 1 << 8),
            26 => (4, 1 << 16),
            27 => (8, 1 << 32),
            _ => return Err(format!("{label}: unsupported integer width")),
        };
        let mut raw = [0_u8; 8];
        let end = self.position.saturating_add(width);
        let slice = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| format!("{label}: unexpected end"))?;
        let offset = 8_usize
            .checked_sub(width)
            .ok_or_else(|| format!("{label}: width out of range"))?;
        raw.get_mut(offset..)
            .ok_or_else(|| format!("{label}: width out of range"))?
            .copy_from_slice(slice);
        self.position = end;
        let value = u64::from_be_bytes(raw);
        if value < minimum {
            return Err(format!("{label}: non-minimal integer"));
        }
        Ok(value)
    }

    fn text(&mut self, label: &str) -> Result<&'a str, String> {
        let major = self.take()?;
        if (major >> 5) != 3 {
            return Err(format!("{label}: expected text"));
        }
        let len = self.uint_body(major & 0x1f, label)?;
        let len = usize::try_from(len).map_err(|_error| format!("{label}: length out of range"))?;
        let end = self.position.saturating_add(len);
        let slice = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| format!("{label}: unexpected end"))?;
        self.position = end;
        std::str::from_utf8(slice).map_err(|_error| format!("{label}: invalid UTF-8"))
    }

    fn tag(&mut self, label: &str) -> Result<&'a str, String> {
        let tag = self.text(label)?;
        if tag.len() > Self::MAX_TAG_BYTES {
            return Err(format!("{label}: tag is too long"));
        }
        Ok(tag)
    }

    fn u32_value(&mut self, label: &str) -> Result<u32, String> {
        let value = self.uint_value(label)?;
        u32::try_from(value).map_err(|_error| format!("{label}: does not fit u32"))
    }

    fn i64_value(&mut self, label: &str) -> Result<i64, String> {
        let major = self.peek()?;
        if major & 0xe0 == 0x20 {
            // The existing V2 payload writer uses absolute magnitude in
            // the negative integer argument. Keep its persisted encoding.
            let tag = self.take()?;
            let magnitude = self.uint_body(tag & 0x1f, label)?;
            match magnitude {
                0 => Err(format!("{label}: negative zero")),
                0x8000_0000_0000_0000 => Ok(i64::MIN),
                _ => i64::try_from(magnitude)
                    .map(|value| value.saturating_neg())
                    .map_err(|_error| format!("{label}: overflow")),
            }
        } else {
            let value = self.uint_value(label)?;
            i64::try_from(value).map_err(|_error| format!("{label}: does not fit i64"))
        }
    }

    fn node_ref(&mut self, label: &str) -> Result<RepoMapNodeRef, String> {
        let entry_len = self.array_header(label)?;
        if entry_len != 2 {
            return Err(format!(
                "{label}: array holds {entry_len} items, expected 2"
            ));
        }
        let variant = self.tag(label)?;
        let id = self.text(label)?;
        match variant {
            "File" => Ok(RepoMapNodeRef::File(FileId::new(id))),
            "Module" => Ok(RepoMapNodeRef::Module(RepoMapModuleId::new(id))),
            "Symbol" => Ok(RepoMapNodeRef::Symbol(SymbolId::new(id))),
            "Chunk" => Ok(RepoMapNodeRef::Chunk(ChunkId::new(id))),
            other => Err(format!("{label}: unknown node variant `{other}`")),
        }
    }

    fn edge_kind(&mut self, label: &str) -> Result<RepoMapEdgeKind, String> {
        let code = BorrowedStrDeserializer::<serde::de::value::Error>::new(self.tag(label)?);
        RepoMapEdgeKind::deserialize(code).map_err(|error| format!("{label}: {error}"))
    }

    fn doc_type(&mut self, label: &str) -> Result<RepoMapDocType, String> {
        let code = BorrowedStrDeserializer::<serde::de::value::Error>::new(self.tag(label)?);
        RepoMapDocType::deserialize(code).map_err(|error| format!("{label}: {error}"))
    }

    fn path_id(&mut self, label: &str) -> Result<RepoRelativePath, String> {
        Ok(RepoRelativePath::new(self.text(label)?))
    }

    fn expect_end(&self) -> Result<(), String> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err("trailing bytes".to_string())
        }
    }
}

fn projection_entry_to_model_entry(
    meta: &CandidateProjectionMeta,
    entry: &CompiledRepoMapProjectionEntryV1,
) -> RepoMapEntry {
    let final_score_millis = u32::try_from(entry.final_score_millis.clamp(0, 1_000))
        .map_or(0, |final_score_millis| final_score_millis);
    let (raw_identity, discriminant) = node_ref_parts(&entry.subject);
    let projection_authority_artifact_id = format!(
        "repo-map:{}:{discriminant}:{raw_identity}",
        meta.snapshot_id
    );
    RepoMapEntry {
        subject_identity: raw_identity,
        subject_doc_type: entry.doc_type,
        subject_kind: if entry.doc_type == RepoMapDocType::File {
            "file".to_string()
        } else {
            "symbol".to_string()
        },
        owner_path: entry.owner_path.as_str().to_string(),
        score: f32::from(
            u16::try_from(final_score_millis.min(u32::from(u16::MAX))).map_or(0, u16::from),
        ) / 1_000.0,
        final_score_millis,
        importance_score_millis: final_score_millis,
        utility_score_millis: final_score_millis.div_euclid(2),
        freshness_score_millis: freshness_score(meta),
        evidence_priority_millis: final_score_millis.div_euclid(2),
        token_budget_hint: 64,
        contributing_signals: BTreeMap::new(),
        projection_evidence_kind: "CompiledRepoMapCandidateV1".to_string(),
        projection_authority_artifact_id,
        projection_authority_digest: meta.authority_digest.clone(),
        projection_status: projection_status(meta),
        redaction_state: meta.redaction_state,
        search_text: entry.search_text.clone(),
        source_symbol_count: 0,
        source_chunk_token_total: 0,
        source_call_incoming_edges: 0,
        source_call_outgoing_edges: 0,
        source_import_incoming_edges: 0,
        source_import_outgoing_edges: 0,
    }
}

fn freshness_score(meta: &CandidateProjectionMeta) -> u32 {
    if matches!(
        meta.item_index_availability,
        RepoMapItemIndexAvailability::Available | RepoMapItemIndexAvailability::Full
    ) && matches!(
        meta.graph_coverage_class,
        RepoMapGraphCoverageClass::Complete | RepoMapGraphCoverageClass::Full
    ) {
        return 860;
    }
    500
}

fn projection_status(meta: &CandidateProjectionMeta) -> String {
    if matches!(
        meta.item_index_availability,
        RepoMapItemIndexAvailability::Available | RepoMapItemIndexAvailability::Full
    ) && matches!(
        meta.graph_coverage_class,
        RepoMapGraphCoverageClass::Complete | RepoMapGraphCoverageClass::Full
    ) {
        return "Complete".to_string();
    }
    "Partial".to_string()
}
