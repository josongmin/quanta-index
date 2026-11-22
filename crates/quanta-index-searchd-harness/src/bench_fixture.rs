//! Portable benchmark fixture records shared by socket and in-process runners.

use anyhow::Result as AnyResult;
use quanta_index_contract::lex::{
    LanguageCode, ParseNode, ParseRoleTag, ParseTreeRecord, compute_parse_tree_source_hash,
};
use quanta_index_contract::{
    ChunkId, GenerationPin, RuntimeCatalogIngestBatch, RuntimeChangedRecord, RuntimeDocFacetRecord,
    RuntimeEdgeAuthorityRecord, RuntimeSnapshotRecord,
};

pub const STRUCTURAL_TREE_PATH: &str = "src/tree.rs";
pub const STRUCTURAL_TREE_CONTENT: &str = "fn parity_needle_alpha() {}";
pub const STRUCTURAL_TREE_IDENTIFIER: &str = "parity_needle_alpha";
pub(crate) const STRUCTURAL_TREE_VIRTUAL_CONTENT: &str = "function_item parity_needle_alpha";
pub const RUNTIME_CATALOG_FILES: &[(&str, &str)] = &[
    ("src/clean.rs", "clean scope quartz"),
    ("src/dirty.rs", "dirty scope todo"),
    ("src/changed.rs", "fn catalog_changed_needle() {}"),
    ("src/stale.rs", "fn catalog_stale_needle() {}"),
    ("src/owner.rs", "fn catalog_owner_needle() {}"),
    ("src/service.rs", "fn catalog_service_needle() {}"),
    ("src/layer.rs", "fn catalog_layer_needle() {}"),
    ("src/surface.rs", "fn catalog_surface_needle() {}"),
    ("src/snap.rs", "fn catalog_snapshot_needle() {}"),
];
pub const RUNTIME_DIRTY_EPOCH_MS: u64 = 100;
pub(crate) const RUNTIME_CATALOG_VIRTUAL_CONTENT: &str = "dirty=src/dirty.rs@100 producer_head_applied_at_ms=100 generation_materialized_at_ms=20 changed=src/changed.rs@25,src/stale.rs@15 facets=src/owner.rs,src/service.rs,src/layer.rs,src/surface.rs:team-a/search/index/lexical snapshots=active:src/changed.rs,src/snap.rs affected=rebuild=lexical:src/changed.rs invalidated_by=rebuild=lexical:src/changed.rs";

#[derive(Clone, Debug)]
pub struct E2eRuntimeCatalogSpec {
    pub producer_head_applied_at_ms: u64,
    pub generation_materialized_at_ms: u64,
    pub changed: Vec<E2eRuntimeChangedSpec>,
    pub facets: Vec<E2eRuntimeFacetSpec>,
    pub snapshots: Vec<E2eRuntimeSnapshotSpec>,
    pub affected: Vec<E2eRuntimeEdgeSpec>,
    pub invalidated_by: Vec<E2eRuntimeEdgeSpec>,
}

#[derive(Clone, Debug)]
pub struct E2eRuntimeChangedSpec {
    pub path: String,
    pub applied_at_ms: u64,
}

#[derive(Clone, Debug)]
pub struct E2eRuntimeFacetSpec {
    pub path: String,
    pub owner: Option<String>,
    pub service: Option<String>,
    pub layer: Option<String>,
    pub surface: Option<String>,
}

#[derive(Clone, Debug)]
pub struct E2eRuntimeSnapshotSpec {
    pub name: String,
    pub paths: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct E2eRuntimeEdgeSpec {
    pub key: String,
    pub paths: Vec<String>,
}

pub fn runtime_catalog_fixture_spec() -> E2eRuntimeCatalogSpec {
    let facet = |path: &str| E2eRuntimeFacetSpec {
        path: path.to_string(),
        owner: Some("team-a".to_string()),
        service: Some("search".to_string()),
        layer: Some("index".to_string()),
        surface: Some("lexical".to_string()),
    };
    E2eRuntimeCatalogSpec {
        producer_head_applied_at_ms: 100,
        generation_materialized_at_ms: 20,
        changed: vec![
            E2eRuntimeChangedSpec {
                path: "src/changed.rs".to_string(),
                applied_at_ms: 25,
            },
            E2eRuntimeChangedSpec {
                path: "src/stale.rs".to_string(),
                applied_at_ms: 15,
            },
        ],
        facets: vec![
            facet("src/owner.rs"),
            facet("src/service.rs"),
            facet("src/layer.rs"),
            facet("src/surface.rs"),
        ],
        snapshots: vec![E2eRuntimeSnapshotSpec {
            name: "active".to_string(),
            paths: vec!["src/changed.rs".to_string(), "src/snap.rs".to_string()],
        }],
        affected: vec![E2eRuntimeEdgeSpec {
            key: "rebuild=lexical".to_string(),
            paths: vec!["src/changed.rs".to_string()],
        }],
        invalidated_by: vec![E2eRuntimeEdgeSpec {
            key: "rebuild=lexical".to_string(),
            paths: vec!["src/changed.rs".to_string()],
        }],
    }
}

/// Lower the fixture's path references without changing their order or values.
pub fn runtime_catalog_batch(
    catalog: &E2eRuntimeCatalogSpec,
    pin: &GenerationPin,
    chunk_at: impl Fn(&str) -> Option<ChunkId>,
) -> AnyResult<RuntimeCatalogIngestBatch> {
    let required = |kind: &str, path: &str| {
        chunk_at(path).ok_or_else(|| {
            anyhow::anyhow!(
                "e2e-harness: no lexical chunk recorded for runtime {kind} path `{path}`"
            )
        })
    };
    let changed_entries = catalog
        .changed
        .iter()
        .map(|changed| {
            Ok(RuntimeChangedRecord {
                doc_id: required("changed", &changed.path)?,
                applied_at_ms: changed.applied_at_ms,
                payload_hash: [0xaa; 32],
            })
        })
        .collect::<AnyResult<Vec<_>>>()?;
    let facet_entries = catalog
        .facets
        .iter()
        .map(|facet| {
            Ok(RuntimeDocFacetRecord {
                doc_id: required("facet", &facet.path)?,
                owner: facet.owner.clone(),
                service: facet.service.clone(),
                layer: facet.layer.clone(),
                surface: facet.surface.clone(),
            })
        })
        .collect::<AnyResult<Vec<_>>>()?;
    let snapshot_entries = catalog
        .snapshots
        .iter()
        .map(|snapshot| {
            Ok(RuntimeSnapshotRecord {
                name: snapshot.name.clone(),
                doc_ids: snapshot
                    .paths
                    .iter()
                    .map(|path| required("snapshot", path))
                    .collect::<AnyResult<Vec<_>>>()?,
            })
        })
        .collect::<AnyResult<Vec<_>>>()?;
    let affected_entries = catalog
        .affected
        .iter()
        .map(|edge| {
            Ok(RuntimeEdgeAuthorityRecord {
                key: edge.key.clone(),
                doc_ids: edge
                    .paths
                    .iter()
                    .map(|path| required("affected", path))
                    .collect::<AnyResult<Vec<_>>>()?,
            })
        })
        .collect::<AnyResult<Vec<_>>>()?;
    let invalidated_by_entries = catalog
        .invalidated_by
        .iter()
        .map(|edge| {
            Ok(RuntimeEdgeAuthorityRecord {
                key: edge.key.clone(),
                doc_ids: edge
                    .paths
                    .iter()
                    .map(|path| required("invalidated_by", path))
                    .collect::<AnyResult<Vec<_>>>()?,
            })
        })
        .collect::<AnyResult<Vec<_>>>()?;
    Ok(RuntimeCatalogIngestBatch {
        repo_id: pin.repo_id.clone(),
        revision_id: pin.revision_id.clone(),
        generation: pin.manifest_generation,
        overlay_epoch_ms: catalog.generation_materialized_at_ms,
        batch_digest: String::new(),
        producer_head_applied_at_ms: catalog.producer_head_applied_at_ms,
        generation_materialized_at_ms: catalog.generation_materialized_at_ms,
        changed_entries,
        facet_entries,
        snapshot_entries,
        affected_entries,
        invalidated_by_entries,
    })
}

pub(crate) fn language_from_path(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("rs") => "rust",
        Some("py") => "python",
        Some("ts") => "typescript",
        Some("js") => "javascript",
        Some("md") => "markdown",
        Some(_) | None => "text",
    }
}

fn structural_role_tags(
    root_end: u32,
    identifier_start: u32,
    identifier_end: u32,
    block_start: u32,
    block_end: u32,
) -> Vec<ParseRoleTag> {
    vec![
        ParseRoleTag {
            role: "item".to_string().into_boxed_str(),
            byte_start: 0,
            byte_end: root_end,
        },
        ParseRoleTag {
            role: "expr".to_string().into_boxed_str(),
            byte_start: identifier_start,
            byte_end: identifier_end,
        },
        ParseRoleTag {
            role: "stmt".to_string().into_boxed_str(),
            byte_start: block_start,
            byte_end: block_end,
        },
    ]
}

/// Build the fixed single-function tree used by Legacy compatibility fixtures.
pub fn structural_function_tree_record(
    path: &str,
    content: &str,
    identifier: &str,
) -> AnyResult<ParseTreeRecord> {
    let identifier_start = content.find(identifier).ok_or_else(|| {
        anyhow::anyhow!("e2e-harness: identifier `{identifier}` not present in structural content")
    })?;
    let identifier_end = identifier_start.saturating_add(identifier.len());
    let byte_end = u32::try_from(content.len())
        .map_err(|err| anyhow::anyhow!("e2e harness structural content overflow: {err}"))?;
    let identifier_start = u32::try_from(identifier_start)
        .map_err(|err| anyhow::anyhow!("e2e harness identifier start overflow: {err}"))?;
    let identifier_end = u32::try_from(identifier_end)
        .map_err(|err| anyhow::anyhow!("e2e harness identifier end overflow: {err}"))?;
    let block_start = byte_end.saturating_sub(2);
    Ok(ParseTreeRecord {
        wire_version: 1,
        lang: LanguageCode::new(language_from_path(path)).map_err(|err| {
            anyhow::anyhow!("language_from_path must return canonical lowercase codes: {err}")
        })?,
        root: ParseNode {
            kind: "function_item".to_string().into_boxed_str(),
            byte_start: 0,
            byte_end,
            children: vec![
                ParseNode {
                    kind: "identifier".to_string().into_boxed_str(),
                    byte_start: identifier_start,
                    byte_end: identifier_end,
                    children: Vec::new(),
                },
                ParseNode {
                    kind: "block".to_string().into_boxed_str(),
                    byte_start: block_start,
                    byte_end,
                    children: Vec::new(),
                },
            ],
        },
        source_hash: compute_parse_tree_source_hash(content),
        role_tag_schema_version: 1,
        role_tags: structural_role_tags(
            byte_end,
            identifier_start,
            identifier_end,
            block_start,
            byte_end,
        ),
    })
}
