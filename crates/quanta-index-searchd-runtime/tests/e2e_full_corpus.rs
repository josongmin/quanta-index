//! E2E-06 — machine-readable real-engine corpus rail.
//!
//! Runtime rows execute against the live daemon harness. This closeout corpus
//! is route-aware (`text` / `structural` / `history` / `runtime_metadata`) and
//! enforces exact ordered ids, duplicate-free success, typed runtime errors,
//! and row-declared provenance checks where the response surface exposes them.
//!
//! `runtime_rows.toml` is the executable-query inventory only. Non-executable
//! proof states (`active owner-local`, `parser_only`, `blocked`) stay in the
//! capability matrix and companion rails instead of being silently treated as
//! runtime green.

#![forbid(unsafe_code)]

use quanta_index_searchd_harness as e2e_harness;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Result as AnyResult;
use quanta_index_contract::lex::{
    LanguageCode, ParseNode, ParseRoleTag, ParseTreeRecord, compute_parse_tree_source_hash,
};
use quanta_index_contract::{
    FileContributorEntry, FileContributorIdentityEntry, FileContributorIngestBatch,
    FileOwnershipEntry, FileOwnershipIngestBatch, RepoCommitRecencyEntry,
    RepoCommitRecencyIngestBatch, RepoId, RepoMetaEntry, RepoMetaIngestBatch, RepoRelativePath,
    RepoTopicEntry, RepoTopicIngestBatch,
};
use quanta_index_contract::{LqVisibility, SearchExplanation, TextQuerySyntax};
use quanta_index_corpus_smoke::{
    CorpusRow, ExpectedShape, ExpectedStructuralBinding, Gate, RowClassification, RuntimeRoute,
    RuntimeSyntax, load_corpus,
};
use serde::ser::{Serialize, SerializeStruct, Serializer};
use toml::Value;

use crate::e2e_harness::{
    E2eHistoryFixtureSpec, E2eQueryResult, E2eRuntime, E2eRuntimeCatalogSpec,
    E2eRuntimeChangedSpec, E2eRuntimeEdgeSpec, E2eRuntimeFacetSpec, E2eRuntimeSnapshotSpec,
    E2eTextChunkSpec,
};

struct FixtureDoc {
    id: String,
    path: String,
    content: String,
    symbol_name: Option<String>,
    start_line: u32,
    end_line: u32,
    source_repo_id: Option<String>,
}

struct FixtureRepoMetadata {
    fork: bool,
    archived: bool,
    visibility: LqVisibility,
    contexts: Vec<String>,
}

enum FixtureStructuralSpec {
    Shorthand { path: String, identifier: String },
    Tree(FixtureStructuralTreeSpec),
}

struct FixtureStructuralTreeSpec {
    path: String,
    root: FixtureStructuralNodeSpec,
    role_tags: Vec<FixtureStructuralRoleTagSpec>,
}

struct FixtureStructuralNodeSpec {
    kind: String,
    start_byte: u32,
    end_byte: u32,
    start_line: u32,
    end_line: u32,
    children: Vec<FixtureStructuralNodeSpec>,
}

struct FixtureStructuralRoleTagSpec {
    role: String,
    start_byte: u32,
    end_byte: u32,
}

struct FixtureHistorySpec {
    commit_sha: String,
    file_path: String,
    author: String,
    committer: String,
    message: String,
    author_time_ms: u64,
    committer_time_ms: u64,
    applied_at_ms: u64,
    ref_name: String,
    tag_name: String,
    added_text: String,
    removed_text: String,
    touched_text: String,
}

struct FixtureRepoCommitRecencySpec {
    source_repo_id: String,
    latest_committer_time_ms: u64,
}

struct FixtureRepoMetaSpec {
    source_repo_id: String,
    key: String,
    value: String,
}

struct FixtureRepoTopicSpec {
    source_repo_id: String,
    topic: String,
}

struct FixtureFileOwnershipSpec {
    source_repo_id: String,
    repo_relative_path: String,
    owners: Vec<String>,
}

struct FixtureFileContributorSpec {
    source_repo_id: String,
    repo_relative_path: String,
    contributors: Vec<FixtureFileContributorIdentitySpec>,
}

#[derive(Clone)]
struct FixtureFileContributorIdentitySpec {
    canonical: String,
    name: Option<String>,
    email: Option<String>,
}

struct FixtureDirtySpec {
    path: String,
    applied_at_ms: u64,
}

struct LoadedFixture {
    docs: Vec<FixtureDoc>,
    repo_metadata: Option<FixtureRepoMetadata>,
    structural: Vec<FixtureStructuralSpec>,
    history: Vec<FixtureHistorySpec>,
    repo_commit_recency: Vec<FixtureRepoCommitRecencySpec>,
    repo_meta: Vec<FixtureRepoMetaSpec>,
    repo_topic: Vec<FixtureRepoTopicSpec>,
    file_ownership: Vec<FixtureFileOwnershipSpec>,
    file_contributor: Vec<FixtureFileContributorSpec>,
    dirty: Vec<FixtureDirtySpec>,
    runtime_catalog: Option<E2eRuntimeCatalogSpec>,
}

struct FixtureRuntimeState {
    candidate_id_to_id: BTreeMap<String, String>,
    structural_candidate_to_id: BTreeMap<String, String>,
}

struct RunSummary {
    runtime_passed: usize,
    typed_unavailable_rows: usize,
    parser_only_rows: usize,
    deferred_rows: usize,
}

struct RowReport {
    id: String,
    failure: Option<String>,
}

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lexical_corpus")
}

fn runtime_rows_path() -> PathBuf {
    fixtures_root().join("runtime_rows.toml")
}

fn language_from_path(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("rs") => "rust",
        Some("py") => "python",
        Some("ts") => "typescript",
        Some("js") => "javascript",
        Some("md") => "markdown",
        Some(_) | None => "text",
    }
}

struct RepoMetadataPayload<'a> {
    fork: bool,
    archived: bool,
    visibility: LqVisibility,
    contexts: &'a [String],
}

impl Serialize for RepoMetadataPayload<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMetadataPayload", 4)?;
        state.serialize_field("fork", &self.fork)?;
        state.serialize_field("archived", &self.archived)?;
        state.serialize_field("visibility", &self.visibility)?;
        state.serialize_field("contexts", &self.contexts)?;
        state.end()
    }
}

fn load_fixture(name: &str) -> AnyResult<LoadedFixture> {
    let path = fixtures_root().join(name);
    load_fixture_from_path(&path)
}

fn load_fixture_from_path(path: &Path) -> AnyResult<LoadedFixture> {
    let raw = std::fs::read_to_string(path)?;
    let root: Value = raw.parse::<Value>()?;
    parse_loaded_fixture(&root, path)
}

fn parse_loaded_fixture(root: &Value, path: &Path) -> AnyResult<LoadedFixture> {
    let table = root
        .as_table()
        .ok_or_else(|| anyhow::anyhow!("fixture {} root must be a table", path.display()))?;
    let docs = table
        .get("doc")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("fixture {} missing [[doc]] array", path.display()))?;

    let mut out = Vec::with_capacity(docs.len());
    for doc in docs {
        let table = doc
            .as_table()
            .ok_or_else(|| anyhow::anyhow!("fixture {} doc row must be a table", path.display()))?;
        let id = require_string(table, "id", path)?;
        let doc_path = require_string(table, "path", path)?;
        let content = require_string(table, "content", path)?;
        let symbol_name = optional_string(table, "symbol_name", path)?;
        out.push(FixtureDoc {
            id,
            path: doc_path,
            content,
            symbol_name,
            start_line: optional_u32(table, "start_line", path)?.unwrap_or(1),
            end_line: optional_u32(table, "end_line", path)?.unwrap_or(2),
            source_repo_id: optional_string(table, "source_repo_id", path)?,
        });
    }

    let repo_metadata = table
        .get("repo_metadata")
        .map(|value| parse_repo_metadata(value, path))
        .transpose()?;
    let structural = parse_structural_specs(
        table.get("structural"),
        table.get("structural_tree"),
        &out,
        path,
    )?;
    let history = parse_history_specs(table.get("history"), path)?;
    let repo_commit_recency =
        parse_repo_commit_recency_specs(table.get("repo_commit_recency"), path)?;
    let repo_meta = parse_repo_meta_specs(table.get("repo_meta"), path)?;
    let repo_topic = parse_repo_topic_specs(table.get("repo_topic"), path)?;
    let file_ownership = parse_file_ownership_specs(table.get("file_ownership"), path)?;
    let file_contributor = parse_file_contributor_specs(table.get("file_contributor"), path)?;
    let dirty = parse_dirty_specs(table.get("dirty"), path)?;
    let runtime_catalog = parse_runtime_catalog_spec(table.get("runtime_catalog"), path)?;

    Ok(LoadedFixture {
        docs: out,
        repo_metadata,
        structural,
        history,
        repo_commit_recency,
        repo_meta,
        repo_topic,
        file_ownership,
        file_contributor,
        dirty,
        runtime_catalog,
    })
}

fn require_string(
    table: &toml::map::Map<String, Value>,
    field: &str,
    path: &Path,
) -> AnyResult<String> {
    match table.get(field) {
        Some(Value::String(value)) => Ok(value.clone()),
        Some(other) => Err(anyhow::anyhow!(
            "fixture {} field `{field}` must be string, got {other:?}",
            path.display()
        )),
        None => Err(anyhow::anyhow!(
            "fixture {} missing `{field}`",
            path.display()
        )),
    }
}

fn optional_string(
    table: &toml::map::Map<String, Value>,
    field: &str,
    path: &Path,
) -> AnyResult<Option<String>> {
    match table.get(field) {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(other) => Err(anyhow::anyhow!(
            "fixture {} field `{field}` must be string, got {other:?}",
            path.display()
        )),
        None => Ok(None),
    }
}

fn require_bool(
    table: &toml::map::Map<String, Value>,
    field: &str,
    path: &Path,
) -> AnyResult<bool> {
    match table.get(field) {
        Some(Value::Boolean(value)) => Ok(*value),
        Some(other) => Err(anyhow::anyhow!(
            "fixture {} field `{field}` must be boolean, got {other:?}",
            path.display()
        )),
        None => Err(anyhow::anyhow!(
            "fixture {} missing `{field}`",
            path.display()
        )),
    }
}

fn require_u64(table: &toml::map::Map<String, Value>, field: &str, path: &Path) -> AnyResult<u64> {
    match table.get(field) {
        Some(Value::Integer(value)) => u64::try_from(*value).map_err(|err| {
            anyhow::anyhow!(
                "fixture {} field `{field}` must be non-negative u64, got {} ({err})",
                path.display(),
                value
            )
        }),
        Some(other) => Err(anyhow::anyhow!(
            "fixture {} field `{field}` must be integer, got {other:?}",
            path.display()
        )),
        None => Err(anyhow::anyhow!(
            "fixture {} missing `{field}`",
            path.display()
        )),
    }
}

fn optional_u64(
    table: &toml::map::Map<String, Value>,
    field: &str,
    path: &Path,
) -> AnyResult<Option<u64>> {
    match table.get(field) {
        Some(Value::Integer(value)) => u64::try_from(*value).map(Some).map_err(|err| {
            anyhow::anyhow!(
                "fixture {} field `{field}` must be non-negative u64, got {} ({err})",
                path.display(),
                value
            )
        }),
        Some(other) => Err(anyhow::anyhow!(
            "fixture {} field `{field}` must be integer, got {other:?}",
            path.display()
        )),
        None => Ok(None),
    }
}

fn optional_u32(
    table: &toml::map::Map<String, Value>,
    field: &str,
    path: &Path,
) -> AnyResult<Option<u32>> {
    optional_u64(table, field, path)?
        .map(|value| {
            u32::try_from(value).map_err(|err| {
                anyhow::anyhow!(
                    "fixture {} field `{field}` must fit in u32, got {} ({err})",
                    path.display(),
                    value
                )
            })
        })
        .transpose()
}

fn require_u32(table: &toml::map::Map<String, Value>, field: &str, path: &Path) -> AnyResult<u32> {
    optional_u32(table, field, path)?
        .ok_or_else(|| anyhow::anyhow!("fixture {} missing `{field}`", path.display()))
}

fn require_string_array(
    table: &toml::map::Map<String, Value>,
    field: &str,
    path: &Path,
) -> AnyResult<Vec<String>> {
    match table.get(field) {
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| match value {
                Value::String(text) => Ok(text.clone()),
                other @ (Value::Integer(_)
                | Value::Float(_)
                | Value::Boolean(_)
                | Value::Datetime(_)
                | Value::Array(_)
                | Value::Table(_)) => Err(anyhow::anyhow!(
                    "fixture {} field `{field}` must be array<string>, got entry {other:?}",
                    path.display()
                )),
            })
            .collect(),
        Some(other) => Err(anyhow::anyhow!(
            "fixture {} field `{field}` must be array<string>, got {other:?}",
            path.display()
        )),
        None => Err(anyhow::anyhow!(
            "fixture {} missing `{field}`",
            path.display()
        )),
    }
}

fn parse_visibility(value: &str, path: &Path) -> AnyResult<LqVisibility> {
    match value {
        "public" => Ok(LqVisibility::Public),
        "private" => Ok(LqVisibility::Private),
        "any" => Ok(LqVisibility::Any),
        other => Err(anyhow::anyhow!(
            "fixture {} repo_metadata.visibility must be one of public|private|any, got `{other}`",
            path.display()
        )),
    }
}

fn parse_repo_metadata(value: &Value, path: &Path) -> AnyResult<FixtureRepoMetadata> {
    let table = value.as_table().ok_or_else(|| {
        anyhow::anyhow!("fixture {} [repo_metadata] must be a table", path.display())
    })?;
    let visibility = parse_visibility(&require_string(table, "visibility", path)?, path)?;
    Ok(FixtureRepoMetadata {
        fork: require_bool(table, "fork", path)?,
        archived: require_bool(table, "archived", path)?,
        visibility,
        contexts: require_string_array(table, "contexts", path)?,
    })
}

fn parse_structural_specs(
    shorthand_value: Option<&Value>,
    tree_value: Option<&Value>,
    docs: &[FixtureDoc],
    path: &Path,
) -> AnyResult<Vec<FixtureStructuralSpec>> {
    let mut out = Vec::new();
    if let Some(value) = shorthand_value {
        let rows = value.as_array().ok_or_else(|| {
            anyhow::anyhow!(
                "fixture {} [[structural]] must be an array of tables, got {value:?}",
                path.display()
            )
        })?;
        for row in rows {
            let table = row.as_table().ok_or_else(|| {
                anyhow::anyhow!(
                    "fixture {} [[structural]] row must be a table",
                    path.display()
                )
            })?;
            out.push(FixtureStructuralSpec::Shorthand {
                path: require_string(table, "path", path)?,
                identifier: require_string(table, "identifier", path)?,
            });
        }
    }
    if let Some(value) = tree_value {
        let rows = value.as_array().ok_or_else(|| {
            anyhow::anyhow!(
                "fixture {} [[structural_tree]] must be an array of tables, got {value:?}",
                path.display()
            )
        })?;
        for row in rows {
            let table = row.as_table().ok_or_else(|| {
                anyhow::anyhow!(
                    "fixture {} [[structural_tree]] row must be a table",
                    path.display()
                )
            })?;
            let structural_path = require_string(table, "path", path)?;
            let root_value = table.get("root").ok_or_else(|| {
                anyhow::anyhow!(
                    "fixture {} [[structural_tree]] row for path `{structural_path}` missing `root`",
                    path.display()
                )
            })?;
            let role_tags_value = table.get("role_tags").ok_or_else(|| {
                anyhow::anyhow!(
                    "fixture {} [[structural_tree]] row for path `{structural_path}` missing `role_tags`",
                    path.display()
                )
            })?;
            let root = parse_structural_node(root_value, path, "root")?;
            let role_tags = parse_structural_role_tags(role_tags_value, path)?;
            validate_structural_tree_spec(&structural_path, docs, &root, &role_tags, path)?;
            out.push(FixtureStructuralSpec::Tree(FixtureStructuralTreeSpec {
                path: structural_path,
                root,
                role_tags,
            }));
        }
    }
    Ok(out)
}

fn parse_structural_node(
    value: &Value,
    path: &Path,
    context: &str,
) -> AnyResult<FixtureStructuralNodeSpec> {
    let table = value.as_table().ok_or_else(|| {
        anyhow::anyhow!(
            "fixture {} structural node `{context}` must be a table",
            path.display()
        )
    })?;
    let kind = require_string(table, "kind", path)?;
    let start_byte = require_u32(table, "start_byte", path)?;
    let end_byte = require_u32(table, "end_byte", path)?;
    let start_line = require_u32(table, "start_line", path)?;
    let end_line = require_u32(table, "end_line", path)?;
    let children = match table.get("children") {
        Some(Value::Array(children)) => children
            .iter()
            .enumerate()
            .map(|(idx, child)| {
                parse_structural_node(child, path, &format!("{context}.children[{idx}]"))
            })
            .collect::<AnyResult<Vec<_>>>()?,
        Some(other) => {
            return Err(anyhow::anyhow!(
                "fixture {} structural node `{context}` field `children` must be array<table>, got {other:?}",
                path.display()
            ));
        }
        None => Vec::new(),
    };
    Ok(FixtureStructuralNodeSpec {
        kind,
        start_byte,
        end_byte,
        start_line,
        end_line,
        children,
    })
}

fn parse_structural_role_tags(
    value: &Value,
    path: &Path,
) -> AnyResult<Vec<FixtureStructuralRoleTagSpec>> {
    let rows = value.as_array().ok_or_else(|| {
        anyhow::anyhow!(
            "fixture {} structural `role_tags` must be an array of tables, got {value:?}",
            path.display()
        )
    })?;
    rows.iter()
        .enumerate()
        .map(|(idx, row)| {
            let table = row.as_table().ok_or_else(|| {
                anyhow::anyhow!(
                    "fixture {} structural role_tags[{idx}] must be a table",
                    path.display()
                )
            })?;
            Ok(FixtureStructuralRoleTagSpec {
                role: require_string(table, "role", path)?,
                start_byte: require_u32(table, "start_byte", path)?,
                end_byte: require_u32(table, "end_byte", path)?,
            })
        })
        .collect()
}

fn validate_structural_tree_spec(
    structural_path: &str,
    docs: &[FixtureDoc],
    root: &FixtureStructuralNodeSpec,
    role_tags: &[FixtureStructuralRoleTagSpec],
    fixture_path: &Path,
) -> AnyResult<()> {
    let matching_docs = docs
        .iter()
        .filter(|doc| doc.path == structural_path)
        .collect::<Vec<_>>();
    let [doc] = matching_docs.as_slice() else {
        return Err(anyhow::anyhow!(
            "fixture {} structural_tree path `{structural_path}` must map to exactly one [[doc]] row",
            fixture_path.display()
        ));
    };
    let byte_len = u32::try_from(doc.content.len()).map_err(|err| {
        anyhow::anyhow!(
            "fixture {} structural_tree path `{structural_path}` content length overflow: {err}",
            fixture_path.display()
        )
    })?;
    let line_count = u32::try_from(doc.content.lines().count().max(1)).map_err(|err| {
        anyhow::anyhow!(
            "fixture {} structural_tree path `{structural_path}` line count overflow: {err}",
            fixture_path.display()
        )
    })?;
    validate_structural_node(root, None, byte_len, line_count, fixture_path, "root")?;
    for (idx, role_tag) in role_tags.iter().enumerate() {
        if role_tag.start_byte > role_tag.end_byte {
            return Err(anyhow::anyhow!(
                "fixture {} structural role_tags[{idx}] has inverted byte span {}..{}",
                fixture_path.display(),
                role_tag.start_byte,
                role_tag.end_byte
            ));
        }
        if role_tag.end_byte > byte_len {
            return Err(anyhow::anyhow!(
                "fixture {} structural role_tags[{idx}] exceeds content length {} with span {}..{}",
                fixture_path.display(),
                byte_len,
                role_tag.start_byte,
                role_tag.end_byte
            ));
        }
        if role_tag.start_byte < root.start_byte || role_tag.end_byte > root.end_byte {
            return Err(anyhow::anyhow!(
                "fixture {} structural role_tags[{idx}] span {}..{} is outside root span {}..{}",
                fixture_path.display(),
                role_tag.start_byte,
                role_tag.end_byte,
                root.start_byte,
                root.end_byte
            ));
        }
    }
    Ok(())
}

fn validate_structural_node(
    node: &FixtureStructuralNodeSpec,
    parent: Option<&FixtureStructuralNodeSpec>,
    byte_len: u32,
    line_count: u32,
    fixture_path: &Path,
    context: &str,
) -> AnyResult<()> {
    if node.start_byte > node.end_byte {
        return Err(anyhow::anyhow!(
            "fixture {} structural node `{context}` has inverted byte span {}..{}",
            fixture_path.display(),
            node.start_byte,
            node.end_byte
        ));
    }
    if node.end_byte > byte_len {
        return Err(anyhow::anyhow!(
            "fixture {} structural node `{context}` exceeds content length {} with span {}..{}",
            fixture_path.display(),
            byte_len,
            node.start_byte,
            node.end_byte
        ));
    }
    if node.start_line == 0 || node.end_line == 0 || node.start_line > node.end_line {
        return Err(anyhow::anyhow!(
            "fixture {} structural node `{context}` has invalid line span {}..{}",
            fixture_path.display(),
            node.start_line,
            node.end_line
        ));
    }
    if node.end_line > line_count {
        return Err(anyhow::anyhow!(
            "fixture {} structural node `{context}` exceeds line count {} with span {}..{}",
            fixture_path.display(),
            line_count,
            node.start_line,
            node.end_line
        ));
    }
    if let Some(parent) = parent {
        if node.start_byte < parent.start_byte || node.end_byte > parent.end_byte {
            return Err(anyhow::anyhow!(
                "fixture {} structural node `{context}` span {}..{} is not nested within parent span {}..{}",
                fixture_path.display(),
                node.start_byte,
                node.end_byte,
                parent.start_byte,
                parent.end_byte
            ));
        }
        if node.start_line < parent.start_line || node.end_line > parent.end_line {
            return Err(anyhow::anyhow!(
                "fixture {} structural node `{context}` line span {}..{} is not nested within parent line span {}..{}",
                fixture_path.display(),
                node.start_line,
                node.end_line,
                parent.start_line,
                parent.end_line
            ));
        }
    }
    for (idx, child) in node.children.iter().enumerate() {
        validate_structural_node(
            child,
            Some(node),
            byte_len,
            line_count,
            fixture_path,
            &format!("{context}.children[{idx}]"),
        )?;
    }
    Ok(())
}

fn build_parse_tree_record(
    structural_path: &str,
    content: &str,
    tree: &FixtureStructuralTreeSpec,
) -> AnyResult<ParseTreeRecord> {
    Ok(ParseTreeRecord {
        wire_version: 1,
        lang: LanguageCode::new(language_from_path(structural_path)).map_err(|err| {
            anyhow::anyhow!("language_from_path must return canonical lowercase codes: {err}")
        })?,
        root: build_parse_node(&tree.root),
        source_hash: compute_parse_tree_source_hash(content),
        role_tag_schema_version: 1,
        role_tags: tree
            .role_tags
            .iter()
            .map(|role_tag| ParseRoleTag {
                role: role_tag.role.clone().into_boxed_str(),
                byte_start: role_tag.start_byte,
                byte_end: role_tag.end_byte,
            })
            .collect(),
    })
}

fn build_parse_node(node: &FixtureStructuralNodeSpec) -> ParseNode {
    ParseNode {
        kind: node.kind.clone().into_boxed_str(),
        byte_start: node.start_byte,
        byte_end: node.end_byte,
        children: node.children.iter().map(build_parse_node).collect(),
    }
}

fn parse_history_specs(value: Option<&Value>, path: &Path) -> AnyResult<Vec<FixtureHistorySpec>> {
    let Some(Value::Array(rows)) = value else {
        return value.map_or_else(
            || Ok(Vec::new()),
            |other| {
                Err(anyhow::anyhow!(
                    "fixture {} [[history]] must be an array of tables, got {other:?}",
                    path.display()
                ))
            },
        );
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let table = row.as_table().ok_or_else(|| {
            anyhow::anyhow!("fixture {} [[history]] row must be a table", path.display())
        })?;
        out.push(FixtureHistorySpec {
            commit_sha: optional_string(table, "commit_sha", path)?
                .unwrap_or_else(|| "0123456789abcdef0123456789abcdef01234567".to_string()),
            file_path: require_string(table, "file_path", path)?,
            author: optional_string(table, "author", path)?.unwrap_or_else(|| "alice".to_string()),
            committer: optional_string(table, "committer", path)?
                .unwrap_or_else(|| "alice".to_string()),
            message: optional_string(table, "message", path)?
                .unwrap_or_else(|| "fix: sample history alpha_content_needle".to_string()),
            author_time_ms: optional_u64(table, "author_time_ms", path)?.unwrap_or(11),
            committer_time_ms: optional_u64(table, "committer_time_ms", path)?.unwrap_or(12),
            applied_at_ms: optional_u64(table, "applied_at_ms", path)?.unwrap_or(13),
            ref_name: optional_string(table, "ref_name", path)?
                .unwrap_or_else(|| "refs/heads/main".to_string()),
            tag_name: optional_string(table, "tag_name", path)?
                .unwrap_or_else(|| "v1.0.0".to_string()),
            added_text: optional_string(table, "added_text", path)?
                .unwrap_or_else(|| "history added line".to_string()),
            removed_text: optional_string(table, "removed_text", path)?.unwrap_or_default(),
            touched_text: optional_string(table, "touched_text", path)?
                .unwrap_or_else(|| "history touched line".to_string()),
        });
    }
    Ok(out)
}

fn parse_repo_commit_recency_specs(
    value: Option<&Value>,
    path: &Path,
) -> AnyResult<Vec<FixtureRepoCommitRecencySpec>> {
    let Some(Value::Array(rows)) = value else {
        return value.map_or_else(
            || Ok(Vec::new()),
            |other| {
                Err(anyhow::anyhow!(
                    "fixture {} [[repo_commit_recency]] must be an array of tables, got {other:?}",
                    path.display()
                ))
            },
        );
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let table = row.as_table().ok_or_else(|| {
            anyhow::anyhow!(
                "fixture {} [[repo_commit_recency]] row must be a table",
                path.display()
            )
        })?;
        out.push(FixtureRepoCommitRecencySpec {
            source_repo_id: require_string(table, "source_repo_id", path)?,
            latest_committer_time_ms: require_u64(table, "latest_committer_time_ms", path)?,
        });
    }
    Ok(out)
}

fn parse_repo_meta_specs(
    value: Option<&Value>,
    path: &Path,
) -> AnyResult<Vec<FixtureRepoMetaSpec>> {
    let Some(Value::Array(rows)) = value else {
        return value.map_or_else(
            || Ok(Vec::new()),
            |other| {
                Err(anyhow::anyhow!(
                    "fixture {} [[repo_meta]] must be an array of tables, got {other:?}",
                    path.display()
                ))
            },
        );
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let table = row.as_table().ok_or_else(|| {
            anyhow::anyhow!(
                "fixture {} [[repo_meta]] row must be a table",
                path.display()
            )
        })?;
        out.push(FixtureRepoMetaSpec {
            source_repo_id: require_string(table, "source_repo_id", path)?,
            key: require_string(table, "key", path)?,
            value: require_string(table, "value", path)?,
        });
    }
    Ok(out)
}

fn parse_repo_topic_specs(
    value: Option<&Value>,
    path: &Path,
) -> AnyResult<Vec<FixtureRepoTopicSpec>> {
    let Some(Value::Array(rows)) = value else {
        return value.map_or_else(
            || Ok(Vec::new()),
            |other| {
                Err(anyhow::anyhow!(
                    "fixture {} [[repo_topic]] must be an array of tables, got {other:?}",
                    path.display()
                ))
            },
        );
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let table = row.as_table().ok_or_else(|| {
            anyhow::anyhow!(
                "fixture {} [[repo_topic]] row must be a table",
                path.display()
            )
        })?;
        out.push(FixtureRepoTopicSpec {
            source_repo_id: require_string(table, "source_repo_id", path)?,
            topic: require_string(table, "topic", path)?,
        });
    }
    Ok(out)
}

fn parse_file_ownership_specs(
    value: Option<&Value>,
    path: &Path,
) -> AnyResult<Vec<FixtureFileOwnershipSpec>> {
    let Some(Value::Array(rows)) = value else {
        return value.map_or_else(
            || Ok(Vec::new()),
            |other| {
                Err(anyhow::anyhow!(
                    "fixture {} [[file_ownership]] must be an array of tables, got {other:?}",
                    path.display()
                ))
            },
        );
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let table = row.as_table().ok_or_else(|| {
            anyhow::anyhow!(
                "fixture {} [[file_ownership]] row must be a table",
                path.display()
            )
        })?;
        let owners = match table.get("owners") {
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| match item {
                    Value::String(value) => Ok(value.clone()),
                    other @ (Value::Integer(_)
                    | Value::Float(_)
                    | Value::Boolean(_)
                    | Value::Datetime(_)
                    | Value::Array(_)
                    | Value::Table(_)) => Err(anyhow::anyhow!(
                        "fixture {} [[file_ownership]].owners entries must be strings, got {other:?}",
                        path.display()
                    )),
                })
                .collect::<AnyResult<Vec<_>>>()?,
            Some(other) => {
                return Err(anyhow::anyhow!(
                    "fixture {} [[file_ownership]].owners must be an array of strings, got {other:?}",
                    path.display()
                ));
            }
            None => Vec::new(),
        };
        out.push(FixtureFileOwnershipSpec {
            source_repo_id: require_string(table, "source_repo_id", path)?,
            repo_relative_path: require_string(table, "repo_relative_path", path)?,
            owners,
        });
    }
    Ok(out)
}

fn parse_file_contributor_specs(
    value: Option<&Value>,
    path: &Path,
) -> AnyResult<Vec<FixtureFileContributorSpec>> {
    let Some(Value::Array(rows)) = value else {
        return value.map_or_else(
            || Ok(Vec::new()),
            |other| {
                Err(anyhow::anyhow!(
                    "fixture {} [[file_contributor]] must be an array of tables, got {other:?}",
                    path.display()
                ))
            },
        );
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let table = row.as_table().ok_or_else(|| {
            anyhow::anyhow!(
                "fixture {} [[file_contributor]] row must be a table",
                path.display()
            )
        })?;
        let contributors = match table.get("contributors") {
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| match item {
                    Value::String(value) => Ok(FixtureFileContributorIdentitySpec {
                        canonical: value.clone(),
                        name: None,
                        email: None,
                    }),
                    Value::Table(fields) => Ok(FixtureFileContributorIdentitySpec {
                        canonical: fields
                            .get("canonical")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                anyhow::anyhow!(
                                    "fixture {} [[file_contributor]].contributors table must carry string `canonical`",
                                    path.display()
                                )
                            })?
                            .to_string(),
                        name: match fields.get("name") {
                            Some(Value::String(value)) => Some(value.clone()),
                            Some(Value::Integer(_)
                            | Value::Float(_)
                            | Value::Boolean(_)
                            | Value::Datetime(_)
                            | Value::Array(_)
                            | Value::Table(_)) => {
                                return Err(anyhow::anyhow!(
                                    "fixture {} [[file_contributor]].contributors.name must be a string when present",
                                    path.display()
                                ));
                            }
                            None => None,
                        },
                        email: match fields.get("email") {
                            Some(Value::String(value)) => Some(value.clone()),
                            Some(Value::Integer(_)
                            | Value::Float(_)
                            | Value::Boolean(_)
                            | Value::Datetime(_)
                            | Value::Array(_)
                            | Value::Table(_)) => {
                                return Err(anyhow::anyhow!(
                                    "fixture {} [[file_contributor]].contributors.email must be a string when present",
                                    path.display()
                                ));
                            }
                            None => None,
                        },
                    }),
                    other @ (Value::Integer(_)
                    | Value::Float(_)
                    | Value::Boolean(_)
                    | Value::Datetime(_)) => Err(anyhow::anyhow!(
                        "fixture {} [[file_contributor]].contributors entries must be strings or tables, got {other:?}",
                        path.display()
                    )),
                    Value::Array(_) => Err(anyhow::anyhow!(
                        "fixture {} [[file_contributor]].contributors entries must not be arrays",
                        path.display()
                    )),
                })
                .collect::<AnyResult<Vec<_>>>()?,
            Some(other) => {
                return Err(anyhow::anyhow!(
                    "fixture {} [[file_contributor]].contributors must be an array of strings, got {other:?}",
                    path.display()
                ));
            }
            None => Vec::new(),
        };
        out.push(FixtureFileContributorSpec {
            source_repo_id: require_string(table, "source_repo_id", path)?,
            repo_relative_path: require_string(table, "repo_relative_path", path)?,
            contributors,
        });
    }
    Ok(out)
}

fn parse_runtime_catalog_spec(
    value: Option<&Value>,
    path: &Path,
) -> AnyResult<Option<E2eRuntimeCatalogSpec>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let table = value.as_table().ok_or_else(|| {
        anyhow::anyhow!(
            "fixture {} [runtime_catalog] must be a table",
            path.display()
        )
    })?;
    let changed_key = table
        .get("changed")
        .map(|v| parse_runtime_changed_specs(v, path));
    let facet_key = table
        .get("facet")
        .map(|v| parse_runtime_facet_specs(v, path));
    let snapshot_key = table
        .get("snapshot")
        .map(|v| parse_runtime_snapshot_specs(v, path));
    let affected_key = table
        .get("affected")
        .map(|v| parse_runtime_edge_specs(v, path, "affected", "scope"));
    let invalidated_by_key = table
        .get("invalidated_by")
        .map(|v| parse_runtime_edge_specs(v, path, "invalidated_by", "source"));
    Ok(Some(E2eRuntimeCatalogSpec {
        producer_head_applied_at_ms: require_u64(table, "producer_head_applied_at_ms", path)?,
        generation_materialized_at_ms: require_u64(table, "generation_materialized_at_ms", path)?,
        changed: changed_key.transpose()?.unwrap_or_default(),
        facets: facet_key.transpose()?.unwrap_or_default(),
        snapshots: snapshot_key.transpose()?.unwrap_or_default(),
        affected: affected_key.transpose()?.unwrap_or_default(),
        invalidated_by: invalidated_by_key.transpose()?.unwrap_or_default(),
    }))
}

fn parse_runtime_changed_specs(
    value: &Value,
    path: &Path,
) -> AnyResult<Vec<E2eRuntimeChangedSpec>> {
    let Value::Array(rows) = value else {
        return Err(anyhow::anyhow!(
            "fixture {} runtime_catalog.changed must be an array of tables, got {value:?}",
            path.display()
        ));
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let table = row.as_table().ok_or_else(|| {
            anyhow::anyhow!(
                "fixture {} runtime_catalog.changed row must be a table",
                path.display()
            )
        })?;
        out.push(E2eRuntimeChangedSpec {
            path: require_string(table, "path", path)?,
            applied_at_ms: require_u64(table, "applied_at_ms", path)?,
        });
    }
    Ok(out)
}

fn parse_runtime_facet_specs(value: &Value, path: &Path) -> AnyResult<Vec<E2eRuntimeFacetSpec>> {
    let Value::Array(rows) = value else {
        return Err(anyhow::anyhow!(
            "fixture {} runtime_catalog.facet must be an array of tables, got {value:?}",
            path.display()
        ));
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let table = row.as_table().ok_or_else(|| {
            anyhow::anyhow!(
                "fixture {} runtime_catalog.facet row must be a table",
                path.display()
            )
        })?;
        out.push(E2eRuntimeFacetSpec {
            path: require_string(table, "path", path)?,
            owner: optional_string(table, "owner", path)?,
            service: optional_string(table, "service", path)?,
            layer: optional_string(table, "layer", path)?,
            surface: optional_string(table, "surface", path)?,
        });
    }
    Ok(out)
}

fn parse_runtime_snapshot_specs(
    value: &Value,
    path: &Path,
) -> AnyResult<Vec<E2eRuntimeSnapshotSpec>> {
    let Value::Array(rows) = value else {
        return Err(anyhow::anyhow!(
            "fixture {} runtime_catalog.snapshot must be an array of tables, got {value:?}",
            path.display()
        ));
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let table = row.as_table().ok_or_else(|| {
            anyhow::anyhow!(
                "fixture {} runtime_catalog.snapshot row must be a table",
                path.display()
            )
        })?;
        out.push(E2eRuntimeSnapshotSpec {
            name: require_string(table, "name", path)?,
            paths: require_string_array(table, "paths", path)?,
        });
    }
    Ok(out)
}

fn parse_runtime_edge_specs(
    value: &Value,
    path: &Path,
    section: &str,
    key_field: &str,
) -> AnyResult<Vec<E2eRuntimeEdgeSpec>> {
    let Value::Array(rows) = value else {
        return Err(anyhow::anyhow!(
            "fixture {} runtime_catalog.{section} must be an array of tables, got {value:?}",
            path.display()
        ));
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let table = row.as_table().ok_or_else(|| {
            anyhow::anyhow!(
                "fixture {} runtime_catalog.{section} row must be a table",
                path.display()
            )
        })?;
        out.push(E2eRuntimeEdgeSpec {
            key: require_string(table, key_field, path)?,
            paths: require_string_array(table, "paths", path)?,
        });
    }
    Ok(out)
}

fn parse_dirty_specs(value: Option<&Value>, path: &Path) -> AnyResult<Vec<FixtureDirtySpec>> {
    let Some(Value::Array(rows)) = value else {
        return value.map_or_else(
            || Ok(Vec::new()),
            |other| {
                Err(anyhow::anyhow!(
                    "fixture {} [[dirty]] must be an array of tables, got {other:?}",
                    path.display()
                ))
            },
        );
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let table = row.as_table().ok_or_else(|| {
            anyhow::anyhow!("fixture {} [[dirty]] row must be a table", path.display())
        })?;
        out.push(FixtureDirtySpec {
            path: require_string(table, "path", path)?,
            applied_at_ms: require_u64(table, "applied_at_ms", path)?,
        });
    }
    Ok(out)
}

fn encode_repo_metadata_payload(metadata: &FixtureRepoMetadata) -> AnyResult<Vec<u8>> {
    let payload = RepoMetadataPayload {
        fork: metadata.fork,
        archived: metadata.archived,
        visibility: metadata.visibility,
        contexts: &metadata.contexts,
    };
    let mut buf = Vec::new();
    ciborium::into_writer(&payload, &mut buf)?;
    Ok(buf)
}

fn ingest_fixture(rt: &mut E2eRuntime, fixture: &LoadedFixture) -> AnyResult<FixtureRuntimeState> {
    let mut docs_by_path: BTreeMap<String, Vec<&FixtureDoc>> = BTreeMap::new();
    for doc in &fixture.docs {
        docs_by_path.entry(doc.path.clone()).or_default().push(doc);
    }
    let mut candidate_id_to_id = BTreeMap::new();
    for (path, docs) in &docs_by_path {
        if docs.len() > 1 && docs.iter().any(|doc| doc.symbol_name.is_some()) {
            return Err(anyhow::anyhow!(
                "fixture path `{path}` mixes duplicate text chunks with symbol rows; current harness requires unique path for symbols"
            ));
        }
        let chunk_specs = docs
            .iter()
            .map(|doc| E2eTextChunkSpec {
                content: &doc.content,
                start_line: doc.start_line,
                end_line: doc.end_line,
                source_repo_id: doc.source_repo_id.as_deref(),
            })
            .collect::<Vec<_>>();
        let candidate_ids = rt.ingest_text_chunks("repo-e2e", path, &chunk_specs)?;
        if candidate_ids.len() != docs.len() {
            return Err(anyhow::anyhow!(
                "fixture path `{path}` ingested {} docs but harness returned {} candidate ids",
                docs.len(),
                candidate_ids.len()
            ));
        }
        for (doc, candidate_id) in docs.iter().zip(candidate_ids.into_iter()) {
            let _old = candidate_id_to_id.insert(candidate_id, doc.id.clone());
        }
        if let [doc] = docs.as_slice()
            && let Some(symbol_name) = doc.symbol_name.as_deref()
        {
            rt.ingest_symbol("repo-e2e", path, &doc.id, symbol_name)?;
            let _old = candidate_id_to_id.insert(doc.id.clone(), doc.id.clone());
        }
    }
    if let Some(repo_metadata) = fixture.repo_metadata.as_ref() {
        rt.publish_repo_metadata_bundle(encode_repo_metadata_payload(repo_metadata)?)?;
    }

    let mut structural_candidate_to_id = BTreeMap::new();
    for structural in &fixture.structural {
        let structural_path = match structural {
            FixtureStructuralSpec::Shorthand { path, .. } => path,
            FixtureStructuralSpec::Tree(tree) => &tree.path,
        };
        let docs = docs_by_path.get(structural_path).ok_or_else(|| {
            anyhow::anyhow!(
                "fixture structural path `{structural_path}` missing matching [[doc]] row"
            )
        })?;
        let [doc] = docs.as_slice() else {
            return Err(anyhow::anyhow!(
                "fixture structural path `{structural_path}` must map to exactly one [[doc]] row",
            ));
        };
        match structural {
            FixtureStructuralSpec::Shorthand { path, identifier } => {
                rt.ingest_structural_function_tree(path, &doc.content, identifier)?;
            }
            FixtureStructuralSpec::Tree(tree) => {
                rt.ingest_structural_tree(
                    &tree.path,
                    build_parse_tree_record(&tree.path, &doc.content, tree)?,
                )?;
            }
        }
        let candidate_id = rt.candidate_id_for_path(structural_path)?;
        let _old = structural_candidate_to_id.insert(candidate_id, doc.id.clone());
    }

    for history in &fixture.history {
        rt.ingest_history_fixture_spec(&E2eHistoryFixtureSpec {
            commit_sha: &history.commit_sha,
            file_path: &history.file_path,
            author: &history.author,
            committer: &history.committer,
            message: &history.message,
            author_time_ms: history.author_time_ms,
            committer_time_ms: history.committer_time_ms,
            applied_at_ms: history.applied_at_ms,
            ref_name: &history.ref_name,
            tag_name: &history.tag_name,
            added_text: &history.added_text,
            removed_text: &history.removed_text,
            touched_text: &history.touched_text,
        })?;
    }

    if !fixture.repo_commit_recency.is_empty() {
        rt.publish_repo_commit_recency_batch(RepoCommitRecencyIngestBatch {
            repo_id: rt.repo(),
            revision_id: rt.revision(),
            generation: rt.current_generation(),
            batch_digest: "e2e-full-corpus:repo-commit-recency".to_string(),
            entries: fixture
                .repo_commit_recency
                .iter()
                .map(|entry| RepoCommitRecencyEntry {
                    source_repo_id: RepoId::new(&entry.source_repo_id),
                    latest_committer_time_ms: entry.latest_committer_time_ms,
                })
                .collect(),
        })?;
    }

    if !fixture.repo_meta.is_empty() {
        rt.publish_repo_meta_batch(RepoMetaIngestBatch {
            repo_id: rt.repo(),
            revision_id: rt.revision(),
            generation: rt.current_generation(),
            batch_digest: "e2e-full-corpus:repo-meta".to_string(),
            entries: fixture
                .repo_meta
                .iter()
                .map(|entry| RepoMetaEntry {
                    source_repo_id: RepoId::new(&entry.source_repo_id),
                    key: entry.key.clone(),
                    value: entry.value.clone(),
                })
                .collect(),
        })?;
    }

    if !fixture.repo_topic.is_empty() {
        rt.publish_repo_topic_batch(RepoTopicIngestBatch {
            repo_id: rt.repo(),
            revision_id: rt.revision(),
            generation: rt.current_generation(),
            batch_digest: "e2e-full-corpus:repo-topic".to_string(),
            entries: fixture
                .repo_topic
                .iter()
                .map(|entry| RepoTopicEntry {
                    source_repo_id: RepoId::new(&entry.source_repo_id),
                    topic: entry.topic.clone(),
                })
                .collect(),
        })?;
    }

    if !fixture.file_ownership.is_empty() {
        rt.publish_file_ownership_batch(FileOwnershipIngestBatch {
            repo_id: rt.repo(),
            revision_id: rt.revision(),
            generation: rt.current_generation(),
            batch_digest: "e2e-full-corpus:file-ownership".to_string(),
            entries: fixture
                .file_ownership
                .iter()
                .map(|entry| FileOwnershipEntry {
                    source_repo_id: RepoId::new(&entry.source_repo_id),
                    repo_relative_path: RepoRelativePath::new(&entry.repo_relative_path),
                    owners: entry.owners.clone(),
                })
                .collect(),
        })?;
    }

    if !fixture.file_contributor.is_empty() {
        rt.publish_file_contributor_batch(FileContributorIngestBatch {
            repo_id: rt.repo(),
            revision_id: rt.revision(),
            generation: rt.current_generation(),
            batch_digest: "e2e-full-corpus:file-contributor".to_string(),
            entries: fixture
                .file_contributor
                .iter()
                .map(|entry| FileContributorEntry {
                    source_repo_id: RepoId::new(&entry.source_repo_id),
                    repo_relative_path: RepoRelativePath::new(&entry.repo_relative_path),
                    contributors: entry
                        .contributors
                        .iter()
                        .cloned()
                        .map(|identity| FileContributorIdentityEntry {
                            canonical: identity.canonical,
                            name: identity.name,
                            email: identity.email,
                        })
                        .collect(),
                })
                .collect(),
        })?;
    }

    for dirty in &fixture.dirty {
        rt.ingest_dirty_for_path(&dirty.path, dirty.applied_at_ms)?;
    }

    if let Some(catalog) = fixture.runtime_catalog.as_ref() {
        rt.ingest_runtime_catalog(catalog)?;
    }

    Ok(FixtureRuntimeState {
        candidate_id_to_id,
        structural_candidate_to_id,
    })
}

fn runtime_syntax(row: &CorpusRow) -> Result<TextQuerySyntax, String> {
    match row.syntax {
        Some(RuntimeSyntax::Native) => Ok(TextQuerySyntax::Native),
        Some(RuntimeSyntax::Sourcegraph) => Ok(TextQuerySyntax::Sourcegraph),
        None => Err(format!("row {} missing runtime syntax", row.id)),
    }
}

fn runtime_route(row: &CorpusRow) -> Result<RuntimeRoute, String> {
    row.runtime_route
        .ok_or_else(|| format!("row {} missing runtime_route", row.id))
}

fn ensure_runtime_row_config(row: &CorpusRow) -> Result<(), String> {
    match row.classification {
        Some(RowClassification::Runtime) => {
            if !matches!(&row.gate, Gate::Active) {
                return Err(format!(
                    "row {} runtime classification must use gate=active",
                    row.id
                ));
            }
            if row.fixture.is_none()
                || row.top_k.is_none()
                || row.syntax.is_none()
                || row.runtime_route.is_none()
            {
                return Err(format!(
                    "row {} runtime classification requires fixture, top_k, syntax, and runtime_route",
                    row.id
                ));
            }
            let has_expected_ids = row.expected_ids.is_some();
            let has_runtime_error = row.runtime_error_code.is_some();
            if has_expected_ids == has_runtime_error {
                return Err(format!(
                    "row {} runtime classification must carry exactly one of expected_ids or runtime_error_code",
                    row.id
                ));
            }
            if row.runtime_error_message_contains.is_some() && row.runtime_error_code.is_none() {
                return Err(format!(
                    "row {} carries runtime_error_message_contains without runtime_error_code",
                    row.id
                ));
            }
            if matches!(
                row.runtime_route,
                Some(RuntimeRoute::History | RuntimeRoute::Structural)
            ) && (!row.expected_engines_touched.is_empty()
                || !row.expected_summary_substrings.is_empty())
            {
                return Err(format!(
                    "row {} uses history/structural route but also carries explanation provenance fields",
                    row.id
                ));
            }
            Ok(())
        }
        Some(RowClassification::TypedUnavailable) => {
            if !matches!(&row.gate, Gate::Active) {
                return Err(format!(
                    "row {} typed_unavailable classification must use gate=active",
                    row.id
                ));
            }
            Ok(())
        }
        Some(RowClassification::ParserOnly) => {
            if !matches!(&row.gate, Gate::Pending { .. }) {
                return Err(format!(
                    "row {} parser_only classification must use gate=pending",
                    row.id
                ));
            }
            Ok(())
        }
        Some(RowClassification::DeferredExternalProducer) => {
            if !matches!(&row.gate, Gate::Blocked { .. }) {
                return Err(format!(
                    "row {} deferred_external_producer classification must use gate=blocked",
                    row.id
                ));
            }
            Ok(())
        }
        None => Err(format!("row {} missing classification", row.id)),
    }
}

fn observed_text_fixture_ids(
    result: &E2eQueryResult,
    candidate_id_to_id: &BTreeMap<String, String>,
) -> Result<Vec<String>, String> {
    let observed: Vec<String> = result
        .candidates
        .iter()
        .map(|candidate| {
            candidate_id_to_id
                .get(candidate.candidate_id.as_str())
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "unmapped candidate id `{}` / path `{}` in fixture",
                        candidate.candidate_id,
                        candidate.repo_relative_path.as_str()
                    )
                })
        })
        .collect::<Result<_, _>>()?;
    ensure_no_duplicate_ids("text/runtime_metadata", &observed)?;
    Ok(observed)
}

fn observed_structural_fixture_ids(
    candidate_ids: &[String],
    structural_candidate_to_id: &BTreeMap<String, String>,
) -> Result<Vec<String>, String> {
    let observed: Vec<String> = candidate_ids
        .iter()
        .map(|candidate_id| {
            structural_candidate_to_id
                .get(candidate_id)
                .cloned()
                .ok_or_else(|| {
                    format!("unmapped structural candidate id `{candidate_id}` in fixture")
                })
        })
        .collect::<Result<_, _>>()?;
    ensure_no_duplicate_ids("structural", &observed)?;
    Ok(observed)
}

fn ensure_no_duplicate_ids(label: &str, observed: &[String]) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for id in observed {
        if !seen.insert(id.clone()) {
            return Err(format!(
                "{label} observed duplicate fixture id `{id}` in ordered result set {observed:?}"
            ));
        }
    }
    Ok(())
}

fn expected_shape_matches_ids(row: &CorpusRow, observed: &[String]) -> Result<(), String> {
    match &row.expected {
        ExpectedShape::Empty => {
            if observed.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "row {} expected empty result set, got ids={observed:?}",
                    row.id
                ))
            }
        }
        ExpectedShape::Single => {
            if observed.len() == 1 {
                Ok(())
            } else {
                Err(format!(
                    "row {} expected single result, got ids={observed:?}",
                    row.id
                ))
            }
        }
        ExpectedShape::Multi { min, max } => {
            let len = u32::try_from(observed.len())
                .map_err(|err| format!("row {} observed ids length overflow: {err}", row.id))?;
            let upper_ok = max.is_none_or(|upper| len <= upper);
            if len >= *min && upper_ok {
                Ok(())
            } else {
                Err(format!(
                    "row {} expected multi bounds min={} max={:?}, got ids={observed:?}",
                    row.id, min, max
                ))
            }
        }
        ExpectedShape::Paginated { page_size } => Err(format!(
            "row {} uses paginated expected shape unsupported by runtime rail (page_size={page_size})",
            row.id
        )),
        ExpectedShape::Error { code } => Err(format!(
            "row {} uses parser/conformance error {:?}; runtime rail expects runtime_error_code instead",
            row.id, code
        )),
    }
}

fn required_expected_ids(row: &CorpusRow) -> Result<&[String], String> {
    row.expected_ids
        .as_deref()
        .ok_or_else(|| format!("row {} missing expected_ids for success assertion", row.id))
}

fn assert_expected_paths(row: &CorpusRow, result: &E2eQueryResult) -> Result<(), String> {
    let Some(expected_paths) = row.expected_paths.as_ref() else {
        return Ok(());
    };
    let observed_paths = result
        .candidates
        .iter()
        .map(|candidate| candidate.repo_relative_path.as_str().to_string())
        .collect::<Vec<_>>();
    if &observed_paths != expected_paths {
        return Err(format!(
            "row {} expected paths={expected_paths:?} observed paths={observed_paths:?}",
            row.id
        ));
    }
    Ok(())
}

fn assert_expected_snippets(row: &CorpusRow, result: &E2eQueryResult) -> Result<(), String> {
    let Some(expected_snippets) = row.expected_snippets.as_ref() else {
        return Ok(());
    };
    let observed_snippets = result
        .candidates
        .iter()
        .map(|candidate| candidate.snippet.clone())
        .collect::<Vec<_>>();
    if &observed_snippets != expected_snippets {
        return Err(format!(
            "row {} expected snippets={expected_snippets:?} observed snippets={observed_snippets:?}",
            row.id
        ));
    }
    Ok(())
}

fn assert_expected_bindings(row: &CorpusRow, result: &E2eQueryResult) -> Result<(), String> {
    let Some(expected_binding_sets) = row.expected_bindings.as_ref() else {
        return Ok(());
    };
    let observed_binding_sets = result
        .structural_results
        .iter()
        .map(|candidate| {
            candidate
                .bindings
                .iter()
                .map(|binding| ExpectedStructuralBinding {
                    metavariable: binding.metavariable.clone(),
                    start_byte: binding.start_byte,
                    end_byte: binding.end_byte,
                    start_line: binding.start_line,
                    end_line: binding.end_line,
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    if &observed_binding_sets != expected_binding_sets {
        return Err(format!(
            "row {} expected structural bindings={expected_binding_sets:?} observed bindings={observed_binding_sets:?}",
            row.id
        ));
    }
    Ok(())
}

fn explain_candidates(
    rt: &mut E2eRuntime,
    result: &E2eQueryResult,
) -> Result<Vec<(String, SearchExplanation)>, String> {
    let mut out = Vec::with_capacity(result.candidates.len());
    for candidate in &result.candidates {
        let explain = rt.explain_candidate(candidate.clone());
        if let Some(err) = explain.typed_error {
            return Err(format!(
                "candidate={} typed_error(code={}, message={})",
                candidate.candidate_id, err.code, err.message
            ));
        }
        let explanation = explain.explanation.ok_or_else(|| {
            format!(
                "candidate={} missing explanation on explain surface",
                candidate.candidate_id
            )
        })?;
        out.push((candidate.candidate_id.clone(), explanation));
    }
    Ok(out)
}

fn explanation_artifact(rt: &mut E2eRuntime, result: &E2eQueryResult) -> String {
    match explain_candidates(rt, result) {
        Ok(explanations) if explanations.is_empty() => "none".to_string(),
        Ok(explanations) => format!("{explanations:?}"),
        Err(err) => err,
    }
}

fn assert_runtime_success_provenance(
    rt: &mut E2eRuntime,
    row: &CorpusRow,
    result: &E2eQueryResult,
) -> Result<(), String> {
    if row.expected_engines_touched.is_empty() && row.expected_summary_substrings.is_empty() {
        return Ok(());
    }
    let explanations = explain_candidates(rt, result)?;
    if explanations.is_empty() {
        return Err(format!(
            "row {} expected explanation provenance but got no candidates",
            row.id
        ));
    }
    for (candidate_id, explanation) in explanations {
        let observed_engines = explanation
            .engines_touched
            .iter()
            .map(|engine| engine.as_str().to_string())
            .collect::<Vec<_>>();
        for expected_engine in &row.expected_engines_touched {
            if !observed_engines
                .iter()
                .any(|observed| observed == expected_engine)
            {
                return Err(format!(
                    "row {} candidate {} expected explanation engine `{expected_engine}`, observed engines={observed_engines:?}",
                    row.id, candidate_id
                ));
            }
        }
        for needle in &row.expected_summary_substrings {
            if !explanation.summary.contains(needle) {
                return Err(format!(
                    "row {} candidate {} expected explanation summary to contain `{needle}`, got summary=`{}`",
                    row.id, candidate_id, explanation.summary
                ));
            }
        }
    }
    Ok(())
}

fn assess_runtime_row(
    rt: &mut E2eRuntime,
    row: &CorpusRow,
    fixture_state: &FixtureRuntimeState,
) -> RowReport {
    let syntax = match runtime_syntax(row) {
        Ok(syntax) => syntax,
        Err(err) => {
            return RowReport {
                id: row.id.clone(),
                failure: Some(err),
            };
        }
    };
    let Some(top_k) = row.top_k else {
        return RowReport {
            id: row.id.clone(),
            failure: Some(format!("row {} missing top_k", row.id)),
        };
    };
    let route = match runtime_route(row) {
        Ok(route) => route,
        Err(err) => {
            return RowReport {
                id: row.id.clone(),
                failure: Some(err),
            };
        }
    };
    match route {
        RuntimeRoute::Text => {
            let result = rt.query_text(syntax, &row.query, top_k);
            if let Some(expected_code) = row.runtime_error_code.as_deref() {
                let Some(err) = result.typed_error.as_ref() else {
                    let observed_ids =
                        match observed_text_fixture_ids(&result, &fixture_state.candidate_id_to_id)
                        {
                            Ok(ids) => format!("{ids:?}"),
                            Err(mapping_err) => format!("<unmapped: {mapping_err}>"),
                        };
                    return RowReport {
                        id: row.id.clone(),
                        failure: Some(format!(
                            "row {} expected typed_error(code={expected_code}) but observed ids={observed_ids}; query=`{}` syntax={:?} fixture={} explanation={}",
                            row.id,
                            row.query,
                            row.syntax,
                            row.fixture.as_deref().unwrap_or("<missing>"),
                            explanation_artifact(rt, &result)
                        )),
                    };
                };
                if err.code != expected_code {
                    return RowReport {
                        id: row.id.clone(),
                        failure: Some(format!(
                            "row {} expected typed_error(code={expected_code}) got code={} message={}; query=`{}` syntax={:?} fixture={}",
                            row.id,
                            err.code,
                            err.message,
                            row.query,
                            row.syntax,
                            row.fixture.as_deref().unwrap_or("<missing>"),
                        )),
                    };
                }
                if let Some(needle) = row.runtime_error_message_contains.as_deref()
                    && !err.message.contains(needle)
                {
                    return RowReport {
                        id: row.id.clone(),
                        failure: Some(format!(
                            "row {} expected typed_error message to contain `{needle}`, got `{}`",
                            row.id, err.message
                        )),
                    };
                }
                return RowReport {
                    id: row.id.clone(),
                    failure: None,
                };
            }
            if let Some(err) = result.typed_error.as_ref() {
                return RowReport {
                    id: row.id.clone(),
                    failure: Some(format!(
                        "row {} expected ids={:?} but got typed_error(code={}, message={}); query=`{}` syntax={:?} fixture={} explanation={}",
                        row.id,
                        row.expected_ids,
                        err.code,
                        err.message,
                        row.query,
                        row.syntax,
                        row.fixture.as_deref().unwrap_or("<missing>"),
                        explanation_artifact(rt, &result)
                    )),
                };
            }
            let observed =
                match observed_text_fixture_ids(&result, &fixture_state.candidate_id_to_id) {
                    Ok(observed) => observed,
                    Err(err) => {
                        return RowReport {
                            id: row.id.clone(),
                            failure: Some(err),
                        };
                    }
                };
            if let Err(err) = expected_shape_matches_ids(row, &observed) {
                return RowReport {
                    id: row.id.clone(),
                    failure: Some(err),
                };
            }
            let expected_ids = match required_expected_ids(row) {
                Ok(expected_ids) => expected_ids,
                Err(err) => {
                    return RowReport {
                        id: row.id.clone(),
                        failure: Some(err),
                    };
                }
            };
            if observed != expected_ids {
                return RowReport {
                    id: row.id.clone(),
                    failure: Some(format!(
                        "row {} expected ids={:?} observed ids={observed:?}; query=`{}` syntax={:?} fixture={} explanation={}",
                        row.id,
                        expected_ids,
                        row.query,
                        row.syntax,
                        row.fixture.as_deref().unwrap_or("<missing>"),
                        explanation_artifact(rt, &result)
                    )),
                };
            }
            if let Err(err) = assert_expected_paths(row, &result) {
                return RowReport {
                    id: row.id.clone(),
                    failure: Some(err),
                };
            }
            if let Err(err) = assert_expected_snippets(row, &result) {
                return RowReport {
                    id: row.id.clone(),
                    failure: Some(err),
                };
            }
            if let Err(err) = assert_runtime_success_provenance(rt, row, &result) {
                return RowReport {
                    id: row.id.clone(),
                    failure: Some(err),
                };
            }
        }
        RuntimeRoute::Structural => {
            let result = rt.query_structural(syntax, &row.query, top_k);
            if let Some(expected_code) = row.runtime_error_code.as_deref() {
                let Some(err) = result.typed_error.as_ref() else {
                    return RowReport {
                        id: row.id.clone(),
                        failure: Some(format!(
                            "row {} expected structural typed_error(code={expected_code}) but query succeeded",
                            row.id
                        )),
                    };
                };
                if err.code != expected_code {
                    return RowReport {
                        id: row.id.clone(),
                        failure: Some(format!(
                            "row {} expected structural typed_error(code={expected_code}) got code={} message={}",
                            row.id, err.code, err.message
                        )),
                    };
                }
                if let Some(needle) = row.runtime_error_message_contains.as_deref()
                    && !err.message.contains(needle)
                {
                    return RowReport {
                        id: row.id.clone(),
                        failure: Some(format!(
                            "row {} expected structural typed_error message to contain `{needle}`, got `{}`",
                            row.id, err.message
                        )),
                    };
                }
                return RowReport {
                    id: row.id.clone(),
                    failure: None,
                };
            }
            if let Some(err) = result.typed_error.as_ref() {
                return RowReport {
                    id: row.id.clone(),
                    failure: Some(format!(
                        "row {} expected ids={:?} but got structural typed_error(code={}, message={}); query=`{}` syntax={:?} fixture={}",
                        row.id,
                        row.expected_ids,
                        err.code,
                        err.message,
                        row.query,
                        row.syntax,
                        row.fixture.as_deref().unwrap_or("<missing>"),
                    )),
                };
            }
            let observed = match observed_structural_fixture_ids(
                &result.candidate_ids,
                &fixture_state.structural_candidate_to_id,
            ) {
                Ok(observed) => observed,
                Err(err) => {
                    return RowReport {
                        id: row.id.clone(),
                        failure: Some(err),
                    };
                }
            };
            if let Err(err) = expected_shape_matches_ids(row, &observed) {
                return RowReport {
                    id: row.id.clone(),
                    failure: Some(err),
                };
            }
            let expected_ids = match required_expected_ids(row) {
                Ok(expected_ids) => expected_ids,
                Err(err) => {
                    return RowReport {
                        id: row.id.clone(),
                        failure: Some(err),
                    };
                }
            };
            if observed != expected_ids {
                return RowReport {
                    id: row.id.clone(),
                    failure: Some(format!(
                        "row {} expected ids={:?} observed ids={observed:?}; query=`{}` syntax={:?} fixture={}",
                        row.id,
                        expected_ids,
                        row.query,
                        row.syntax,
                        row.fixture.as_deref().unwrap_or("<missing>"),
                    )),
                };
            }
            if let Err(err) = assert_expected_bindings(row, &result) {
                return RowReport {
                    id: row.id.clone(),
                    failure: Some(err),
                };
            }
        }
        RuntimeRoute::History => {
            let result = rt.query_history(syntax, &row.query, top_k);
            if let Some(expected_code) = row.runtime_error_code.as_deref() {
                let Some(err) = result.typed_error.as_ref() else {
                    return RowReport {
                        id: row.id.clone(),
                        failure: Some(format!(
                            "row {} expected history typed_error(code={expected_code}) but query succeeded",
                            row.id
                        )),
                    };
                };
                if err.code != expected_code {
                    return RowReport {
                        id: row.id.clone(),
                        failure: Some(format!(
                            "row {} expected history typed_error(code={expected_code}) got code={} message={}",
                            row.id, err.code, err.message
                        )),
                    };
                }
                if let Some(needle) = row.runtime_error_message_contains.as_deref()
                    && !err.message.contains(needle)
                {
                    return RowReport {
                        id: row.id.clone(),
                        failure: Some(format!(
                            "row {} expected history typed_error message to contain `{needle}`, got `{}`",
                            row.id, err.message
                        )),
                    };
                }
                return RowReport {
                    id: row.id.clone(),
                    failure: None,
                };
            }
            if let Some(err) = result.typed_error.as_ref() {
                return RowReport {
                    id: row.id.clone(),
                    failure: Some(format!(
                        "row {} expected ids={:?} but got history typed_error(code={}, message={}); query=`{}` syntax={:?} fixture={}",
                        row.id,
                        row.expected_ids,
                        err.code,
                        err.message,
                        row.query,
                        row.syntax,
                        row.fixture.as_deref().unwrap_or("<missing>"),
                    )),
                };
            }
            let observed = if result.commit_ids.is_empty() {
                result.diff_paths
            } else {
                result.commit_ids
            };
            if let Err(err) = ensure_no_duplicate_ids("history", &observed) {
                return RowReport {
                    id: row.id.clone(),
                    failure: Some(err),
                };
            }
            if let Err(err) = expected_shape_matches_ids(row, &observed) {
                return RowReport {
                    id: row.id.clone(),
                    failure: Some(err),
                };
            }
            let expected_ids = match required_expected_ids(row) {
                Ok(expected_ids) => expected_ids,
                Err(err) => {
                    return RowReport {
                        id: row.id.clone(),
                        failure: Some(err),
                    };
                }
            };
            if observed != expected_ids {
                return RowReport {
                    id: row.id.clone(),
                    failure: Some(format!(
                        "row {} expected ids={:?} observed ids={observed:?}; query=`{}` syntax={:?} fixture={}",
                        row.id,
                        expected_ids,
                        row.query,
                        row.syntax,
                        row.fixture.as_deref().unwrap_or("<missing>"),
                    )),
                };
            }
        }
        RuntimeRoute::RuntimeMetadata => {
            let result = rt.query_runtime_metadata(syntax, &row.query, top_k);
            if let Some(expected_code) = row.runtime_error_code.as_deref() {
                let Some(err) = result.typed_error.as_ref() else {
                    return RowReport {
                        id: row.id.clone(),
                        failure: Some(format!(
                            "row {} expected runtime-metadata typed_error(code={expected_code}) but query succeeded",
                            row.id
                        )),
                    };
                };
                if err.code != expected_code {
                    return RowReport {
                        id: row.id.clone(),
                        failure: Some(format!(
                            "row {} expected runtime-metadata typed_error(code={expected_code}) got code={} message={}",
                            row.id, err.code, err.message
                        )),
                    };
                }
                if let Some(needle) = row.runtime_error_message_contains.as_deref()
                    && !err.message.contains(needle)
                {
                    return RowReport {
                        id: row.id.clone(),
                        failure: Some(format!(
                            "row {} expected runtime-metadata typed_error message to contain `{needle}`, got `{}`",
                            row.id, err.message
                        )),
                    };
                }
                return RowReport {
                    id: row.id.clone(),
                    failure: None,
                };
            }
            if let Some(err) = result.typed_error.as_ref() {
                return RowReport {
                    id: row.id.clone(),
                    failure: Some(format!(
                        "row {} expected runtime-metadata ids={:?} but got typed_error(code={}, message={}); query=`{}` syntax={:?} fixture={} explanation={}",
                        row.id,
                        row.expected_ids,
                        err.code,
                        err.message,
                        row.query,
                        row.syntax,
                        row.fixture.as_deref().unwrap_or("<missing>"),
                        explanation_artifact(rt, &result)
                    )),
                };
            }
            let observed =
                match observed_text_fixture_ids(&result, &fixture_state.candidate_id_to_id) {
                    Ok(observed) => observed,
                    Err(err) => {
                        return RowReport {
                            id: row.id.clone(),
                            failure: Some(err),
                        };
                    }
                };
            if let Err(err) = expected_shape_matches_ids(row, &observed) {
                return RowReport {
                    id: row.id.clone(),
                    failure: Some(err),
                };
            }
            let expected_ids = match required_expected_ids(row) {
                Ok(expected_ids) => expected_ids,
                Err(err) => {
                    return RowReport {
                        id: row.id.clone(),
                        failure: Some(err),
                    };
                }
            };
            if observed != expected_ids {
                return RowReport {
                    id: row.id.clone(),
                    failure: Some(format!(
                        "row {} expected runtime-metadata ids={:?} observed ids={observed:?}; query=`{}` syntax={:?} fixture={} explanation={}",
                        row.id,
                        expected_ids,
                        row.query,
                        row.syntax,
                        row.fixture.as_deref().unwrap_or("<missing>"),
                        explanation_artifact(rt, &result)
                    )),
                };
            }
            if let Err(err) = assert_expected_paths(row, &result) {
                return RowReport {
                    id: row.id.clone(),
                    failure: Some(err),
                };
            }
            if let Err(err) = assert_expected_snippets(row, &result) {
                return RowReport {
                    id: row.id.clone(),
                    failure: Some(err),
                };
            }
            if let Err(err) = assert_runtime_success_provenance(rt, row, &result) {
                return RowReport {
                    id: row.id.clone(),
                    failure: Some(err),
                };
            }
        }
    }
    RowReport {
        id: row.id.clone(),
        failure: None,
    }
}

#[test]
fn full_corpus_runtime_fixture_executes_real_rows_only() -> AnyResult<()> {
    let corpus = load_corpus(&runtime_rows_path())?;
    let mut fixtures = BTreeMap::new();
    let mut current_fixture_name: Option<String> = None;
    let mut current_fixture_state: Option<FixtureRuntimeState> = None;
    let mut runtime: Option<E2eRuntime> = None;

    let mut summary = RunSummary {
        runtime_passed: 0,
        typed_unavailable_rows: 0,
        parser_only_rows: 0,
        deferred_rows: 0,
    };
    let mut failures = Vec::new();

    for row in &corpus.rows {
        if let Err(err) = ensure_runtime_row_config(row) {
            failures.push(RowReport {
                id: row.id.clone(),
                failure: Some(err),
            });
            continue;
        }
        match row.classification {
            Some(RowClassification::Runtime) => {
                let fixture_name = row.fixture.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("row {} missing fixture after validation", row.id)
                })?;
                if current_fixture_name.as_deref() != Some(fixture_name.as_str()) {
                    let fixture = if let Some(existing) = fixtures.get(fixture_name) {
                        existing
                    } else {
                        let loaded = load_fixture(fixture_name)?;
                        let _old = fixtures.insert(fixture_name.clone(), loaded);
                        fixtures
                            .get(fixture_name)
                            .ok_or_else(|| anyhow::anyhow!("fixture cache insert lost row"))?
                    };
                    let mut rt = E2eRuntime::boot()?;
                    let fixture_state = ingest_fixture(&mut rt, fixture)?;
                    if fixture.structural.is_empty() {
                        _ = rt.seal()?;
                    } else {
                        _ = rt.seal_lexical_generation_for_tracks(&[
                            quanta_index_contract::SearchPlaneTrackKind::Lexical,
                            quanta_index_contract::SearchPlaneTrackKind::Structural,
                        ])?;
                    }
                    runtime = Some(rt.reopen());
                    current_fixture_name = Some(fixture_name.clone());
                    current_fixture_state = Some(fixture_state);
                }
                let rt = runtime
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("runtime missing after fixture boot"))?;
                let fixture_state = current_fixture_state.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("fixture runtime state missing after fixture boot")
                })?;
                let report = assess_runtime_row(rt, row, fixture_state);
                if report.failure.is_some() {
                    failures.push(report);
                } else {
                    summary.runtime_passed = summary.runtime_passed.saturating_add(1);
                }
            }
            Some(RowClassification::TypedUnavailable) => {
                summary.typed_unavailable_rows = summary.typed_unavailable_rows.saturating_add(1);
            }
            Some(RowClassification::ParserOnly) => {
                summary.parser_only_rows = summary.parser_only_rows.saturating_add(1);
            }
            Some(RowClassification::DeferredExternalProducer) => {
                summary.deferred_rows = summary.deferred_rows.saturating_add(1);
            }
            None => {
                failures.push(RowReport {
                    id: row.id.clone(),
                    failure: Some("row missing classification".to_string()),
                });
            }
        }
    }

    if failures.is_empty() {
        return Ok(());
    }

    let mut buf = String::new();
    writeln!(
        buf,
        "E2E-06 full corpus runtime rail: {} failures; runtime_passed={} typed_unavailable_rows={} parser_only_rows={} deferred_external_producer_rows={}",
        failures.len(),
        summary.runtime_passed,
        summary.typed_unavailable_rows,
        summary.parser_only_rows,
        summary.deferred_rows
    )?;
    for failure in &failures {
        if let Some(message) = &failure.failure {
            writeln!(buf, "  - [{}] {}", failure.id, message)?;
        }
    }
    Err(anyhow::anyhow!("{buf}"))
}

#[test]
fn generic_structural_tree_fixture_loads() -> AnyResult<()> {
    let raw = r#"
[[doc]]
id = "alpha"
path = "src/main.rs"
content = "fn main {}"

[[structural_tree]]
path = "src/main.rs"
role_tags = [
  { role = "item", start_byte = 0, end_byte = 10 },
  { role = "expr", start_byte = 3, end_byte = 7 },
  { role = "stmt", start_byte = 8, end_byte = 10 },
]
root = { kind = "function_item", start_byte = 0, end_byte = 10, start_line = 1, end_line = 1, children = [
  { kind = "identifier", start_byte = 3, end_byte = 7, start_line = 1, end_line = 1 },
  { kind = "block", start_byte = 8, end_byte = 10, start_line = 1, end_line = 1 }
] }
"#;
    let root = raw.parse::<Value>()?;
    let fixture = parse_loaded_fixture(&root, Path::new("inline-structural.toml"))?;
    if !matches!(
        fixture.structural.as_slice(),
        [FixtureStructuralSpec::Tree(_)]
    ) {
        return Err(anyhow::anyhow!(
            "expected a single structural Tree spec, got {} spec(s)",
            fixture.structural.len()
        ));
    }
    Ok(())
}

#[test]
fn generic_structural_tree_rejects_missing_child_field() -> AnyResult<()> {
    let raw = r#"
[[doc]]
id = "alpha"
path = "src/main.rs"
content = "fn main {}"

[[structural_tree]]
path = "src/main.rs"
role_tags = [{ role = "item", start_byte = 0, end_byte = 10 }]
root = { kind = "function_item", start_byte = 0, end_byte = 10, start_line = 1, end_line = 1, children = [
  { start_byte = 3, end_byte = 7, start_line = 1, end_line = 1 }
] }
"#;
    let root = raw.parse::<Value>()?;
    let Err(err) = parse_loaded_fixture(&root, Path::new("inline-structural-missing.toml")) else {
        return Err(anyhow::anyhow!("missing child kind must fail closed"));
    };
    if !err.to_string().contains("missing `kind`") {
        return Err(anyhow::anyhow!(
            "expected `missing `kind`` error, got: {err}"
        ));
    }
    Ok(())
}

#[test]
fn generic_structural_tree_rejects_invalid_nesting() -> AnyResult<()> {
    let raw = r#"
[[doc]]
id = "alpha"
path = "src/main.rs"
content = "fn main {}"

[[structural_tree]]
path = "src/main.rs"
role_tags = [{ role = "item", start_byte = 0, end_byte = 10 }]
root = { kind = "function_item", start_byte = 0, end_byte = 9, start_line = 1, end_line = 1, children = [
  { kind = "identifier", start_byte = 3, end_byte = 10, start_line = 1, end_line = 1 }
] }
"#;
    let root = raw.parse::<Value>()?;
    let Err(err) = parse_loaded_fixture(&root, Path::new("inline-structural-nesting.toml")) else {
        return Err(anyhow::anyhow!(
            "child outside parent span must fail closed"
        ));
    };
    if !err.to_string().contains("not nested within parent span") {
        return Err(anyhow::anyhow!(
            "expected `not nested within parent span` error, got: {err}"
        ));
    }
    Ok(())
}
