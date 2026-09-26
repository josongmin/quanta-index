//! Source-bound multilingual symbol producer (RBR-04).
//!
//! Extracts a definition inventory from admitted source files with pinned
//! tree-sitter grammars and emits canonical [`SymbolRecord`]s: every record
//! binds the repo-relative path, language, kind, local/qualified/container
//! names, the definition byte/line span, and a deterministic symbol id.
//!
//! Extraction is source-driven only: symbols are never derived from query
//! text, gold labels, or name regexes, and unsupported files or parse
//! failures are typed coverage failures — never silently skipped.
//! Anonymous definitions never receive an invented public name; they are
//! simply not emitted as named symbols.

use std::collections::BTreeSet;

use quanta_index_contract::lex::{
    LanguageCode, SymbolKindCode, SymbolRecord, SymbolRelationship, SymbolSpan,
};
use quanta_index_contract::{RepoRelativePath, SymbolId};
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, StreamingIterator};

use crate::sha256_hex;

mod preflight;
use preflight::ExtractionControl;
pub use preflight::{
    SymbolCoveragePolicy, SymbolFileDiagnostic, SymbolFileReport, SymbolPreflight,
    SymbolPreflightOptions, SymbolPreflightPolicy, SymbolPreflightReport, preflight_corpus_symbols,
};

/// Pinned grammar identity bound into every producer policy. The
/// versions mirror the workspace lockfile; changing a grammar changes the
/// digest and invalidates frozen evidence.
pub const SYMBOL_PRODUCER_GRAMMARS: &str = concat!(
    "tree-sitter@0.25.10;",
    "rust@0.24.2;",
    "go@0.25.0;",
    "javascript@0.25.0;",
    "python@0.25.0;",
    "typescript@0.23.2+quanta-typescript-compatibility-1;vendored-source-sha256=",
    env!("QI_TYPESCRIPT_GRAMMAR_SHA256"),
);

// Refuse compilation against an unpatched registry dependency. Hashing a local
// vendor directory alone must not claim that those bytes were linked.
const _: &str = tree_sitter_typescript::QUANTA_COMPATIBILITY_PATCH_ID;

/// Producer identity for batch digests.
pub const SYMBOL_PRODUCER_IDENTITY: &str = "source-bound-symbols-v2";

/// One supported extraction language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolLanguage {
    Rust,
    Go,
    Python,
    JavaScript,
    /// `is_tsx` selects the TSX grammar variant (same language code).
    TypeScript {
        is_tsx: bool,
    },
}

/// Typed extraction failures. Every variant is a coverage failure that the
/// caller must surface, never a skip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SymbolExtractError {
    /// The file extension maps to no supported grammar.
    Unsupported {
        path: String,
    },
    /// tree-sitter reported parse errors in the file.
    ParseFailure {
        path: String,
    },
    /// Two definitions collapsed onto one deterministic id.
    IdCollision {
        path: String,
        symbol_id: String,
    },
    /// The producer itself is inconsistent with the pinned grammar (query
    /// construction failed or a kind code is unregistered). A producer
    /// defect aborts the whole corpus run; it is never a per-file parse
    /// failure.
    ProducerDefect {
        detail: String,
    },
    Cancelled {
        path: String,
    },
    TimedOut {
        path: String,
    },
    ResourceLimit {
        path: String,
    },
}

impl std::fmt::Display for SymbolExtractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported { path } => write!(formatter, "unsupported symbol language: {path}"),
            Self::ParseFailure { path } => write!(formatter, "parse failure: {path}"),
            Self::IdCollision { path, symbol_id } => {
                write!(formatter, "symbol id collision in {path}: {symbol_id}")
            }
            Self::ProducerDefect { detail } => {
                write!(formatter, "symbol producer defect: {detail}")
            }
            Self::Cancelled { path } => write!(formatter, "symbol extraction cancelled: {path}"),
            Self::TimedOut { path } => write!(formatter, "symbol extraction timed out: {path}"),
            Self::ResourceLimit { path } => {
                write!(formatter, "symbol extraction resource limit: {path}")
            }
        }
    }
}

impl std::error::Error for SymbolExtractError {}

impl SymbolLanguage {
    /// Stable per-file coverage identity. TSX uses a distinct grammar from TS.
    #[must_use]
    pub const fn coverage_identity(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Go => "go",
            Self::Python => "python",
            Self::JavaScript => "javascript",
            Self::TypeScript { is_tsx: false } => "typescript",
            Self::TypeScript { is_tsx: true } => "typescript_tsx",
        }
    }

    /// Map a repo-relative path to its language by extension. `None` is an
    /// explicit unsupported file, not an error to hide.
    #[must_use]
    pub fn from_path(path: &str) -> Option<Self> {
        let extension = path.rsplit_once('.')?.1;
        match extension {
            "rs" => Some(Self::Rust),
            "go" => Some(Self::Go),
            "py" => Some(Self::Python),
            "js" | "mjs" | "cjs" | "jsx" => Some(Self::JavaScript),
            "ts" | "mts" | "cts" => Some(Self::TypeScript { is_tsx: false }),
            "tsx" => Some(Self::TypeScript { is_tsx: true }),
            _ => None,
        }
    }

    fn grammar(self) -> Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::TypeScript { is_tsx: true } => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::TypeScript { is_tsx: false } => {
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
            }
        }
    }

    fn language_code(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Go => "go",
            Self::Python => "python",
            Self::JavaScript => "javascript",
            Self::TypeScript { .. } => "typescript",
        }
    }

    fn separator(self) -> &'static str {
        match self {
            Self::Rust => "::",
            Self::Go | Self::Python | Self::JavaScript | Self::TypeScript { .. } => ".",
        }
    }

    fn query(self) -> &'static str {
        match self {
            Self::Rust => RUST_QUERY,
            Self::Go => GO_QUERY,
            Self::Python => PYTHON_QUERY,
            Self::JavaScript => JAVASCRIPT_QUERY,
            Self::TypeScript { .. } => TYPESCRIPT_QUERY,
        }
    }

    /// Map a matched definition node kind onto a canonical symbol kind.
    fn kind_for(self, node_kind: &str, container_is_type: bool) -> Option<&'static str> {
        match (self, node_kind) {
            (
                _,
                "function_declaration"
                | "generator_function_declaration"
                | "function_expression"
                | "generator_function"
                | "function_signature"
                | "function_item"
                | "function_signature_item"
                | "function_definition",
            ) => Some(if container_is_type {
                "method"
            } else {
                "function"
            }),
            (
                _,
                "method_declaration"
                | "method_definition"
                | "method_elem"
                | "method_signature"
                | "abstract_method_signature",
            ) => Some("method"),
            (
                _,
                "class" | "class_declaration" | "abstract_class_declaration" | "class_definition",
            ) => Some("class"),
            (_, "struct_item") => Some("struct"),
            (_, "enum_item" | "enum_declaration") => Some("enum"),
            (_, "trait_item") => Some("trait"),
            (_, "interface_declaration") => Some("interface"),
            (_, "type_item" | "type_alias" | "type_alias_declaration") => Some("type_alias"),
            (_, "mod_item" | "module" | "internal_module") => Some("module"),
            _ => None,
        }
    }

    /// Container kinds that make a DIRECT child fn a method rather than
    /// a local function.
    fn is_type_container(self, node_kind: &str) -> bool {
        match self {
            Self::Rust => matches!(node_kind, "impl_item" | "trait_item"),
            Self::Go => matches!(node_kind, "method_declaration"),
            Self::Python => matches!(node_kind, "class_definition"),
            Self::JavaScript | Self::TypeScript { .. } => matches!(
                node_kind,
                "class_declaration" | "abstract_class_declaration"
            ),
        }
    }

    /// Node kinds that introduce a named container for qualification.
    fn is_container(self, node_kind: &str) -> bool {
        match self {
            Self::Rust => matches!(
                node_kind,
                "impl_item"
                    | "trait_item"
                    | "struct_item"
                    | "enum_item"
                    | "mod_item"
                    | "function_item"
            ),
            Self::Go => matches!(
                node_kind,
                "function_declaration" | "method_declaration" | "type_spec" | "type_alias"
            ),
            Self::Python => matches!(node_kind, "class_definition" | "function_definition"),
            Self::JavaScript | Self::TypeScript { .. } => matches!(
                node_kind,
                "class_declaration"
                    | "class"
                    | "abstract_class_declaration"
                    | "interface_declaration"
                    | "type_alias_declaration"
                    | "function_declaration"
                    | "generator_function_declaration"
                    | "function_expression"
                    | "generator_function"
                    | "method_definition"
                    | "module"
                    | "internal_module"
                    | "enum_declaration"
            ),
        }
    }

    /// The name text of a container node, if it has one.
    fn container_name(self, node: Node<'_>, source: &str) -> Result<String, SymbolExtractError> {
        if self == Self::Rust && node.kind() == "impl_item" {
            // impl blocks name their container through the `type` field.
            // Generic argument lists (`Foo<T>`) are stripped so the same
            // logical type yields one qualified-name spelling.
            let ty = required_child(node, "type", "Rust impl container")?;
            let base = if ty.kind() == "generic_type" {
                required_child(ty, "type", "Rust impl generic type")?
            } else {
                ty
            };
            return Ok(node_text(base, source, "Rust impl type")?.to_string());
        }
        let name = required_child(node, "name", "named container")?;
        Ok(node_text(name, source, "container name")?.to_string())
    }
}

fn required_child<'tree>(
    node: Node<'tree>,
    field: &str,
    context: &str,
) -> Result<Node<'tree>, SymbolExtractError> {
    node.child_by_field_name(field)
        .ok_or_else(|| SymbolExtractError::ProducerDefect {
            detail: format!(
                "{context} node {} at {}..{} has no `{field}` field",
                node.kind(),
                node.start_byte(),
                node.end_byte()
            ),
        })
}

fn node_text<'source>(
    node: Node<'_>,
    source: &'source str,
    context: &str,
) -> Result<&'source str, SymbolExtractError> {
    node.utf8_text(source.as_bytes())
        .map_err(|error| SymbolExtractError::ProducerDefect {
            detail: format!(
                "{context} span {}..{} is not valid UTF-8: {error}",
                node.start_byte(),
                node.end_byte()
            ),
        })
}

/// Depth-first search for the first descendant of `node` whose kind
/// matches. Tree-sitter 0.25 exposes no descendants iterator, so this is
/// an explicit cursor walk bounded by the subtree.
fn find_descendant_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    loop {
        if cursor.node().kind() == kind {
            return Some(cursor.node());
        }
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.node().id() == node.id() {
                return None;
            }
            // Never leave the subtree: sibling moves are only valid from
            // a child of `node`, and returning to `node` itself means the
            // walk is exhausted.
            if cursor.node().id() != node.id() && cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() || cursor.node().id() == node.id() {
                return None;
            }
        }
    }
}

/// Go methods qualify through their receiver type: the first
/// `type_identifier` inside the receiver parameter list (value or pointer
/// receiver alike).
fn go_receiver_type(def_node: Node<'_>, source: &str) -> Result<String, SymbolExtractError> {
    let receiver = required_child(def_node, "receiver", "Go method")?;
    let type_node = find_descendant_kind(receiver, "type_identifier").ok_or_else(|| {
        SymbolExtractError::ProducerDefect {
            detail: format!(
                "Go method receiver at {}..{} has no type_identifier",
                receiver.start_byte(),
                receiver.end_byte()
            ),
        }
    })?;
    Ok(node_text(type_node, source, "Go receiver type")?.to_string())
}

const RUST_QUERY: &str = r"
(function_item name: (identifier) @name) @def
(trait_item body: (declaration_list (function_signature_item name: (identifier) @name) @def))
(struct_item name: (type_identifier) @name) @def
(enum_item name: (type_identifier) @name) @def
(trait_item name: (type_identifier) @name) @def
(type_item name: (type_identifier) @name) @def
(mod_item name: (identifier) @name) @def
";

const GO_QUERY: &str = r"
(function_declaration name: (identifier) @name) @def
(method_declaration name: (field_identifier) @name) @def
(type_spec name: (type_identifier) @name) @def
(type_alias name: (type_identifier) @name) @def
(type_spec type: (interface_type (method_elem name: (field_identifier) @name) @def))
(type_alias type: (interface_type (method_elem name: (field_identifier) @name) @def))
";

const PYTHON_QUERY: &str = r"
(function_definition name: (identifier) @name) @def
(class_definition name: (identifier) @name) @def
";

const JAVASCRIPT_QUERY: &str = r"
(function_declaration name: (identifier) @name) @def
(generator_function_declaration name: (identifier) @name) @def
(function_expression name: (identifier) @name) @def
(generator_function name: (identifier) @name) @def
(class name: (identifier) @name) @def
(class_declaration name: (identifier) @name) @def
(method_definition name: [(property_identifier) (private_property_identifier)] @name) @def
";

const TYPESCRIPT_QUERY: &str = r"
(function_declaration name: (identifier) @name) @def
(generator_function_declaration name: (identifier) @name) @def
(function_expression name: (identifier) @name) @def
(generator_function name: (identifier) @name) @def
(function_signature name: (identifier) @name) @def
(class name: (type_identifier) @name) @def
(class_declaration name: (type_identifier) @name) @def
(abstract_class_declaration name: (type_identifier) @name) @def
(method_definition name: [(property_identifier) (private_property_identifier)] @name) @def
(method_signature name: [(property_identifier) (private_property_identifier)] @name) @def
(abstract_method_signature name: (property_identifier) @name) @def
(interface_declaration name: (type_identifier) @name) @def
(type_alias_declaration name: (type_identifier) @name) @def
(enum_declaration name: (identifier) @name) @def
(internal_module name: [(identifier) (nested_identifier) (string)] @name) @def
(module name: [(identifier) (nested_identifier) (string)] @name) @def
";

struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(source: &str) -> Self {
        // Single line-model source of truth: the corpus splitter mirrors
        // Python splitlines over `\n`, `\r\n`, and lone `\r`. A private
        // `\n`-only model would silently disagree with the corpus/prove
        // path on lone-CR files (audit finding: line-model divergence).
        Self {
            starts: crate::corpus::split_line_starts(source).0,
        }
    }

    fn line(&self, byte_offset: usize) -> Result<u32, std::num::TryFromIntError> {
        let line = self.starts.partition_point(|start| *start <= byte_offset);
        u32::try_from(line.max(1))
    }
}

/// One extracted definition before record assembly.
struct RawDefinition {
    kind: &'static str,
    local_name: String,
    containers: Vec<String>,
    byte_start: usize,
    byte_end: usize,
}

fn parse(
    language: SymbolLanguage,
    path: &str,
    source: &str,
) -> Result<tree_sitter::Tree, SymbolExtractError> {
    let tree = parse_tree(language, path, source, None)?;
    if tree.root_node().has_error() {
        return Err(SymbolExtractError::ParseFailure {
            path: path.to_string(),
        });
    }
    Ok(tree)
}

fn parse_tree(
    language: SymbolLanguage,
    path: &str,
    source: &str,
    control: Option<&ExtractionControl<'_>>,
) -> Result<tree_sitter::Tree, SymbolExtractError> {
    if let Some(control) = control {
        control.check()?;
    }
    let mut parser = Parser::new();
    let grammar = language.grammar();
    parser
        .set_language(&grammar)
        .map_err(|error| SymbolExtractError::ProducerDefect {
            detail: format!("failed to install pinned grammar for {path}: {error}"),
        })?;
    let mut progress =
        |_: &tree_sitter::ParseState| control.is_some_and(|control| control.check().is_err());
    let mut input = |offset, _| source.as_bytes().get(offset..).unwrap_or_default();
    let tree = parser.parse_with_options(
        &mut input,
        None,
        Some(tree_sitter::ParseOptions::new().progress_callback(&mut progress)),
    );
    if let Some(control) = control {
        control.check()?;
    }
    tree.ok_or_else(|| SymbolExtractError::ProducerDefect {
        detail: format!("parser returned no tree without a cancellation or deadline: {path}"),
    })
}

fn query_definitions(
    language: SymbolLanguage,
    tree: &tree_sitter::Tree,
    source: &str,
    control: Option<&ExtractionControl<'_>>,
) -> Result<Vec<RawDefinition>, SymbolExtractError> {
    let grammar = language.grammar();
    let query = Query::new(&grammar, language.query()).map_err(|error| {
        // Query construction fails only when the pinned grammar and the
        // shipped query drift: a producer defect, never a file failure.
        SymbolExtractError::ProducerDefect {
            detail: format!("grammar query construction failed: {error}"),
        }
    })?;
    let mut cursor = QueryCursor::new();
    let mut definitions: Vec<RawDefinition> = Vec::new();
    let mut progress =
        |_: &tree_sitter::QueryCursorState| control.is_some_and(|control| control.check().is_err());
    let mut stream = cursor.matches_with_options(
        &query,
        tree.root_node(),
        source.as_bytes(),
        tree_sitter::QueryCursorOptions::new().progress_callback(&mut progress),
    );
    while let Some(matched) = stream.next() {
        if let Some(control) = control {
            control.check_symbols(definitions.len())?;
        }
        let mut def_node: Option<Node<'_>> = None;
        let mut name_node: Option<Node<'_>> = None;
        for capture in matched.captures {
            let capture_index = usize::try_from(capture.index).map_err(|error| {
                SymbolExtractError::ProducerDefect {
                    detail: format!(
                        "capture index {} does not fit usize: {error}",
                        capture.index
                    ),
                }
            })?;
            let capture_name = query.capture_names().get(capture_index).ok_or_else(|| {
                SymbolExtractError::ProducerDefect {
                    detail: format!(
                        "capture index {capture_index} exceeds capture-name count {}",
                        query.capture_names().len()
                    ),
                }
            })?;
            match *capture_name {
                "def" => def_node = Some(capture.node),
                "name" => name_node = Some(capture.node),
                _ => {}
            }
        }
        let def_node = def_node.ok_or_else(|| SymbolExtractError::ProducerDefect {
            detail: "definition query matched without its required @def capture".to_string(),
        })?;
        let name_node = name_node.ok_or_else(|| SymbolExtractError::ProducerDefect {
            detail: "definition query matched without its required @name capture".to_string(),
        })?;
        let local_name = node_text(name_node, source, "definition name")?;
        let mut containers: Vec<String> = Vec::new();
        // Named ancestors qualify definitions without inventing names for
        // anonymous scopes. Python class bodies share their namespace through
        // control-flow blocks, so its method decision uses the nearest named
        // scope. Rust requires direct impl/trait declaration ownership below.
        let mut nearest_type_container = false;
        let mut seen_container = false;
        let mut parent = def_node.parent();
        while let Some(node) = parent {
            if let Some(control) = control {
                control.check()?;
            }
            if language.is_container(node.kind()) {
                // Expressions may have an explicit inner name. Anonymous
                // expressions do not acquire the surrounding variable's name.
                if matches!(
                    language,
                    SymbolLanguage::JavaScript | SymbolLanguage::TypeScript { .. }
                ) && matches!(
                    node.kind(),
                    "class" | "function_expression" | "generator_function"
                ) && node.child_by_field_name("name").is_none()
                {
                    parent = node.parent();
                    continue;
                }
                let name = language.container_name(node, source)?;
                if !seen_container {
                    seen_container = true;
                    nearest_type_container = language.is_type_container(node.kind());
                }
                containers.push(name);
                if language == SymbolLanguage::Go && node.kind() == "method_declaration" {
                    // Descendants need the same receiver owner as the method
                    // itself. The list is reversed once after the walk.
                    containers.push(go_receiver_type(node, source)?);
                }
            }
            parent = node.parent();
        }
        if language == SymbolLanguage::Go && def_node.kind() == "method_declaration" {
            // Go methods carry their container (the receiver type) on the
            // node itself rather than through an ancestor.
            let receiver_type = go_receiver_type(def_node, source)?;
            nearest_type_container = true;
            containers.push(receiver_type);
        }
        containers.reverse();
        let container_is_type = match language {
            SymbolLanguage::Rust => def_node.parent().is_some_and(|body| {
                body.kind() == "declaration_list"
                    && body
                        .parent()
                        .is_some_and(|owner| language.is_type_container(owner.kind()))
            }),
            // Function declarations are local functions even inside class
            // initializers; actual methods use method_definition explicitly.
            SymbolLanguage::JavaScript | SymbolLanguage::TypeScript { .. } => false,
            SymbolLanguage::Go | SymbolLanguage::Python => nearest_type_container,
        };
        let kind = if language == SymbolLanguage::Go && def_node.kind() == "type_spec" {
            // `type X ...` is classified by its type child: struct_type,
            // interface_type, or a plain definition (type_alias).
            Some(
                match required_child(def_node, "type", "Go type specification")?.kind() {
                    "interface_type" => "interface",
                    "struct_type" => "struct",
                    _ => "type_alias",
                },
            )
        } else {
            language.kind_for(def_node.kind(), container_is_type)
        };
        let kind = kind.ok_or_else(|| SymbolExtractError::ProducerDefect {
            detail: format!(
                "definition query emitted unclassified node kind `{}`",
                def_node.kind()
            ),
        })?;
        definitions.push(RawDefinition {
            kind,
            local_name: local_name.to_string(),
            containers,
            byte_start: def_node.start_byte(),
            byte_end: def_node.end_byte(),
        });
    }
    if let Some(control) = control {
        control.check()?;
    }
    drop(stream);
    if cursor.did_exceed_match_limit() {
        return Err(SymbolExtractError::ProducerDefect {
            detail: "symbol query exceeded its native match limit".to_string(),
        });
    }
    Ok(definitions)
}

fn deterministic_symbol_id(
    path: &str,
    language: &LanguageCode,
    kind: &SymbolKindCode,
    qualified_name: &str,
    byte_start: usize,
    byte_end: usize,
) -> String {
    let canonical = format!(
        "{path}\u{0}{language}\u{0}{kind}\u{0}{qualified_name}\u{0}{byte_start}\u{0}{byte_end}"
    );
    format!("sym-v1:{}", sha256_hex(canonical.as_bytes()))
}

/// Extract every named definition from one source file.
///
/// # Errors
///
/// Returns a typed [`SymbolExtractError`] for unsupported files, parse
/// failures, or deterministic-id collisions. Empty output is a legitimate
/// result (a file with no named definitions), not a failure.
pub fn extract_symbols(path: &str, source: &str) -> Result<Vec<SymbolRecord>, SymbolExtractError> {
    let Some(language) = SymbolLanguage::from_path(path) else {
        return Err(SymbolExtractError::Unsupported {
            path: path.to_string(),
        });
    };
    let tree = parse(language, path, source)?;
    extract_parsed_symbols(language, path, source, &tree, None)
}

fn extract_parsed_symbols(
    language: SymbolLanguage,
    path: &str,
    source: &str,
    tree: &tree_sitter::Tree,
    control: Option<&ExtractionControl<'_>>,
) -> Result<Vec<SymbolRecord>, SymbolExtractError> {
    let definitions = query_definitions(language, tree, source, control)?;
    let line_index = LineIndex::new(source);
    let language_code = LanguageCode::from_code_str(language.language_code()).ok_or_else(|| {
        SymbolExtractError::Unsupported {
            path: path.to_string(),
        }
    })?;
    let repo_path = RepoRelativePath::new(path.to_string());
    let mut records = Vec::with_capacity(definitions.len());
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    let mut raw = definitions;
    raw.sort_by(|left, right| {
        left.byte_start
            .cmp(&right.byte_start)
            .then(left.byte_end.cmp(&right.byte_end))
            .then(left.local_name.cmp(&right.local_name))
    });
    for definition in raw {
        if let Some(control) = control {
            control.check()?;
        }
        let separator = language.separator();
        let qualified_name = if definition.containers.is_empty() {
            definition.local_name.clone()
        } else {
            format!(
                "{}{separator}{}",
                definition.containers.join(separator),
                definition.local_name
            )
        };
        let container_qualified_name = if definition.containers.is_empty() {
            None
        } else {
            Some(definition.containers.join(separator))
        };
        let kind = SymbolKindCode::from_code_str(definition.kind).ok_or_else(|| {
            SymbolExtractError::ProducerDefect {
                detail: format!(
                    "unregistered symbol kind emitted for {path}: {}",
                    definition.kind
                ),
            }
        })?;
        let symbol_id = deterministic_symbol_id(
            path,
            &language_code,
            &kind,
            &qualified_name,
            definition.byte_start,
            definition.byte_end,
        );
        if !seen_ids.insert(symbol_id.clone()) {
            return Err(SymbolExtractError::IdCollision {
                path: path.to_string(),
                symbol_id,
            });
        }
        records.push(SymbolRecord {
            symbol_id: SymbolId::new(symbol_id),
            repo_relative_path: repo_path.clone(),
            language: language_code.clone(),
            symbol_kind: kind,
            symbol_kind_family: None,
            local_name: definition.local_name.clone().into_boxed_str(),
            qualified_name: qualified_name.into_boxed_str(),
            signature: None,
            visibility: None,
            definition_span: SymbolSpan {
                path: path.to_string().into_boxed_str(),
                byte_start: u32::try_from(definition.byte_start).map_err(|error| {
                    SymbolExtractError::ProducerDefect {
                        detail: format!(
                            "definition byte_start {} exceeds u32 for {path}: {error}",
                            definition.byte_start
                        ),
                    }
                })?,
                byte_end: u32::try_from(definition.byte_end).map_err(|error| {
                    SymbolExtractError::ProducerDefect {
                        detail: format!(
                            "definition byte_end {} exceeds u32 for {path}: {error}",
                            definition.byte_end
                        ),
                    }
                })?,
                line_start: line_index.line(definition.byte_start).map_err(|error| {
                    SymbolExtractError::ProducerDefect {
                        detail: format!("line_start exceeds u32 for {path}: {error}"),
                    }
                })?,
                line_end: line_index
                    .line(definition.byte_end.saturating_sub(1))
                    .map_err(|error| SymbolExtractError::ProducerDefect {
                        detail: format!("line_end exceeds u32 for {path}: {error}"),
                    })?,
            },
            container_qualified_name: container_qualified_name.map(String::into_boxed_str),
            relationship: SymbolRelationship::Def,
        });
    }
    Ok(records)
}

/// Extract symbols for a whole admitted corpus.
///
/// Unsupported admitted files and parse failures abort the run before any
/// batch is assembled or published.
pub struct CorpusSymbolExtraction {
    pub symbols: std::collections::BTreeMap<String, Vec<SymbolRecord>>,
}

pub fn extract_corpus_symbols(
    files: &std::collections::BTreeMap<String, crate::corpus::SourceFile>,
) -> crate::BenchResult<CorpusSymbolExtraction> {
    let preflight = preflight_corpus_symbols(files, &SymbolPreflightOptions::default())?;
    preflight
        .admit(SymbolCoveragePolicy::RequireComplete)
        .map_err(|error| {
            if let crate::BenchError::Chunk { path, message } = error {
                crate::BenchError::Chunk {
                    path,
                    message: format!("symbol extraction coverage failure: {message}"),
                }
            } else {
                error
            }
        })?;
    Ok(CorpusSymbolExtraction {
        symbols: preflight.into_symbols(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qualified(record: &SymbolRecord) -> &str {
        &record.qualified_name
    }

    fn find<'a>(records: &'a [SymbolRecord], name: &str) -> &'a SymbolRecord {
        records
            .iter()
            .find(|record| &*record.local_name == name)
            .unwrap_or_else(|| panic!("symbol {name} missing"))
    }

    #[test]
    fn rust_definitions_qualify_methods_inside_impls() {
        let source = "pub struct Engine { pub rpm: u32 }\n\
                      impl Engine {\n    \
                      pub fn start(&self) {}\n    \
                      fn secret_helper(&self) {}\n\
                      }\n\
                      fn main() {}\n\
                      mod inner {\n    \
                      pub fn nested() {}\n\
                      }\n";
        let records = extract_symbols("src/engine.rs", source).expect("rust parses");
        let method = find(&records, "start");
        assert_eq!(method.symbol_kind.as_str(), "method");
        assert_eq!(qualified(method), "Engine::start");
        assert_eq!(method.container_qualified_name.as_deref(), Some("Engine"));
        let helper = find(&records, "secret_helper");
        assert_eq!(helper.symbol_kind.as_str(), "method");
        assert_eq!(qualified(helper), "Engine::secret_helper");
        let free = find(&records, "main");
        assert_eq!(free.symbol_kind.as_str(), "function");
        assert_eq!(qualified(free), "main");
        assert!(free.container_qualified_name.is_none());
        let nested = find(&records, "nested");
        assert_eq!(qualified(nested), "inner::nested");
        let structure = find(&records, "Engine");
        assert_eq!(structure.symbol_kind.as_str(), "struct");
        assert!(
            records
                .iter()
                .all(|record| record.definition_span.byte_start <= record.definition_span.byte_end)
        );
        assert_eq!(
            records.first().expect("first symbol").local_name.as_ref(),
            "Engine"
        );
    }

    #[test]
    fn go_functions_methods_and_types() {
        let source = "package geom\n\n\
                      type Rect struct {\n\tW float64\n}\n\n\
                      type Shape interface {\n\tArea() float64\n}\n\n\
                      func (r *Rect) Area() float64 { return r.W }\n\n\
                      func NewRect(w float64) *Rect { return &Rect{W: w} }\n";
        let records = extract_symbols("geom/rect.go", source).expect("go parses");
        let method = records
            .iter()
            .find(|record| qualified(record) == "Rect.Area")
            .expect("concrete receiver method");
        assert!(
            records
                .iter()
                .any(|record| qualified(record) == "Shape.Area")
        );
        assert_eq!(method.symbol_kind.as_str(), "method");
        let free = find(&records, "NewRect");
        assert_eq!(free.symbol_kind.as_str(), "function");
        // Go qualifies methods by receiver type.
        assert_eq!(qualified(method), "Rect.Area");
        let structure = find(&records, "Rect");
        assert_eq!(structure.symbol_kind.as_str(), "struct");
        let interface = records
            .iter()
            .find(|record| &*record.local_name == "Shape")
            .expect("interface");
        assert_eq!(interface.symbol_kind.as_str(), "interface");
    }

    #[test]
    fn python_definitions_and_decorator_span() {
        let source = "class Service:\n    \
                      def start(self):\n        pass\n\n    \
                      @staticmethod\n    \
                      def build():\n        return Service()\n\n\
                      def standalone():\n    pass\n";
        let records = extract_symbols("svc/service.py", source).expect("python parses");
        let method = find(&records, "start");
        assert_eq!(method.symbol_kind.as_str(), "method");
        assert_eq!(qualified(method), "Service.start");
        // The decorated definition's span starts at `def`, excluding the
        // decorator line by contract.
        let decorated = find(&records, "build");
        let def_offset = source.find("def build").expect("def offset");
        assert_eq!(
            usize::try_from(decorated.definition_span.byte_start).expect("byte offset fits usize"),
            def_offset,
            "decorator is not part of the definition span"
        );
        let free = find(&records, "standalone");
        assert_eq!(free.symbol_kind.as_str(), "function");
        assert!(free.container_qualified_name.is_none());
    }

    #[test]
    fn javascript_and_typescript_definitions() {
        let js = "export class Queue {\n  \
                  push(item) {}\n  \
                  static make() {}\n\
                  }\n\
                  function main() {}\n";
        let records = extract_symbols("src/queue.js", js).expect("javascript parses");
        assert_eq!(find(&records, "push").symbol_kind.as_str(), "method");
        assert_eq!(qualified(find(&records, "push")), "Queue.push");
        assert_eq!(find(&records, "make").symbol_kind.as_str(), "method");
        assert_eq!(find(&records, "main").symbol_kind.as_str(), "function");

        let ts = "export interface Node {\n  id: string;\n}\n\
                  export type Alias = Node;\n\
                  export class Tree {\n  \
                  insert(node: Node) {}\n\
                  }\n";
        let records = extract_symbols("src/tree.ts", ts).expect("typescript parses");
        assert_eq!(find(&records, "Node").symbol_kind.as_str(), "interface");
        assert_eq!(find(&records, "Alias").symbol_kind.as_str(), "type_alias");
        assert_eq!(find(&records, "insert").symbol_kind.as_str(), "method");
        assert_eq!(qualified(find(&records, "insert")), "Tree.insert");
    }

    #[test]
    fn tsx_files_use_the_tsx_grammar() {
        let source = "export function Card(props: { title: string }) {\n  \
                      return <section>{props.title}</section>;\n\
                      }\n";
        let records = extract_symbols("ui/card.tsx", source).expect("tsx parses with jsx");
        assert_eq!(find(&records, "Card").symbol_kind.as_str(), "function");
        assert_eq!(
            records.first().expect("Card symbol").language.as_str(),
            "typescript"
        );
    }

    #[test]
    fn crlf_line_spans_stay_consistent() {
        let source = "fn one() {}\r\nfn two() {}\r\n";
        let records = extract_symbols("src/crlf.rs", source).expect("rust parses");
        let one = find(&records, "one");
        let two = find(&records, "two");
        assert_eq!(one.definition_span.line_start, 1);
        assert_eq!(two.definition_span.line_start, 2);
        assert!(two.definition_span.byte_start > one.definition_span.byte_end);
    }

    #[test]
    fn unknown_extensions_and_parse_failures_are_typed() {
        let unsupported = extract_symbols("docs/readme.md", "# hi");
        assert_eq!(
            unsupported.unwrap_err(),
            SymbolExtractError::Unsupported {
                path: "docs/readme.md".to_string()
            }
        );
        let broken = extract_symbols("src/broken.rs", "fn incomplete( {");
        assert_eq!(
            broken.unwrap_err(),
            SymbolExtractError::ParseFailure {
                path: "src/broken.rs".to_string()
            }
        );

        let text = "# hi".to_string();
        let digest = sha256_hex(text.as_bytes());
        let file = crate::corpus::SourceFile {
            path: "docs/readme.md".to_string(),
            bytes: text.as_bytes().to_vec(),
            text,
            line_starts: vec![0],
            sha256: digest.clone(),
        };
        let other_text = "notes".to_string();
        let other_digest = sha256_hex(other_text.as_bytes());
        let other = crate::corpus::SourceFile {
            path: "docs/znotes.toml".to_string(),
            bytes: other_text.as_bytes().to_vec(),
            text: other_text,
            line_starts: vec![0],
            sha256: other_digest.clone(),
        };
        let files = std::collections::BTreeMap::from([
            (file.path.clone(), file),
            (other.path.clone(), other),
        ]);
        let coverage_error = extract_corpus_symbols(&files)
            .err()
            .expect("the first unsupported admitted file must abort the corpus");
        assert!(matches!(coverage_error, crate::BenchError::Chunk { .. }));
        let coverage_error = coverage_error.to_string();
        assert!(coverage_error.contains("docs/readme.md"));
        assert!(coverage_error.contains(&digest));
        assert!(coverage_error.contains("unsupported_language"));
        assert!(!coverage_error.contains(&other_digest));

        let malformed = "fn incomplete( {".to_string();
        let malformed_sha = sha256_hex(malformed.as_bytes());
        let supported_file = crate::corpus::SourceFile {
            path: "src/broken.rs".to_string(),
            bytes: malformed.as_bytes().to_vec(),
            text: malformed,
            line_starts: vec![0],
            sha256: malformed_sha.clone(),
        };
        let supported =
            std::collections::BTreeMap::from([(supported_file.path.clone(), supported_file)]);
        let parse_error = extract_corpus_symbols(&supported)
            .err()
            .expect("supported parse failure must abort")
            .to_string();
        assert!(parse_error.contains("src/broken.rs"));
        assert!(parse_error.contains(&malformed_sha));
        assert!(parse_error.contains("symbol extraction coverage failure"));

        let mut forged = files;
        forged
            .get_mut("docs/readme.md")
            .expect("fixture file")
            .sha256 = "0".repeat(64);
        assert!(
            extract_corpus_symbols(&forged)
                .err()
                .expect("forged source hash must abort")
                .to_string()
                .contains("symbol source path/hash/text/line model differs")
        );
    }

    #[test]
    fn extraction_is_deterministic_and_ids_bind_identity() {
        let source = "struct A;\nimpl A {\n  fn go(&self) {}\n}\n";
        let first = extract_symbols("src/a.rs", source).expect("parses");
        let second = extract_symbols("src/a.rs", source).expect("parses");
        let ids: Vec<&str> = first
            .iter()
            .map(|record| record.symbol_id.as_str())
            .collect();
        let ids_again: Vec<&str> = second
            .iter()
            .map(|record| record.symbol_id.as_str())
            .collect();
        assert_eq!(ids, ids_again);
        assert_eq!(first.len(), ids.len());
        assert_eq!(
            ids.iter().copied().collect::<BTreeSet<&str>>().len(),
            ids.len()
        );

        // A different path changes the id; a moved span changes it too.
        let moved_source = "\nstruct A;\nimpl A {\n  fn go(&self) {}\n}\n";
        let moved = extract_symbols("src/a.rs", moved_source).expect("parses");
        let other_path = extract_symbols("src/b.rs", source).expect("parses");
        assert_ne!(
            find(&first, "go").symbol_id.as_str(),
            find(&moved, "go").symbol_id.as_str()
        );
        assert_ne!(
            find(&first, "go").symbol_id.as_str(),
            find(&other_path, "go").symbol_id.as_str()
        );
    }

    #[test]
    fn lone_cr_line_model_matches_the_corpus_splitter() {
        // Audit finding 1 regression: with lone-\r separators the symbol
        // lines must agree with the corpus line model that prove_hit
        // resolves bytes through.
        let source = "fn a() {}\rfn b() {}\rfn c() {}\r";
        let records = extract_symbols("src/cr.rs", source).expect("parses");
        let b = find(&records, "b");
        let starts = crate::corpus::split_line_starts(source).0;
        let byte_start =
            usize::try_from(b.definition_span.byte_start).expect("byte offset fits usize");
        let line = starts.partition_point(|start| *start <= byte_start);
        assert_eq!(
            usize::try_from(b.definition_span.line_start).expect("line number fits usize"),
            line.max(1),
            "symbol line must match the corpus model"
        );
    }

    #[test]
    fn go_value_receivers_and_plain_type_definitions() {
        let source = "package t\n\ntype Celsius float64\n\ntype Rect struct { W float64 }\n\ntype Shape interface { Area() float64 }\n\ntype Config struct { logger interface{} }\n\nfunc (r Rect) Area() float64 { return r.W }\n";
        let records = extract_symbols("t/temp.go", source).expect("go parses");
        assert_eq!(
            find(&records, "Celsius").symbol_kind.as_str(),
            "type_alias",
            "plain Go type definitions are not structs"
        );
        assert_eq!(find(&records, "Rect").symbol_kind.as_str(), "struct");
        assert_eq!(find(&records, "Shape").symbol_kind.as_str(), "interface");
        assert_eq!(find(&records, "Config").symbol_kind.as_str(), "struct");
        let area = records
            .iter()
            .find(|record| qualified(record) == "Rect.Area")
            .expect("concrete receiver method");
        assert!(
            records
                .iter()
                .any(|record| qualified(record) == "Shape.Area")
        );
        assert_eq!(area.symbol_kind.as_str(), "method");
        assert_eq!(qualified(area), "Rect.Area", "value receivers qualify too");
    }

    #[test]
    fn typescript_namespaces_qualify_their_members() {
        let source = "namespace Outer {\n  export function inner() {}\n}\nfunction outside() {}\n";
        let records = extract_symbols("ns/a.ts", source).expect("ts parses");
        assert_eq!(qualified(find(&records, "inner")), "Outer.inner");
        assert_eq!(find(&records, "inner").symbol_kind.as_str(), "function");
        assert_eq!(qualified(find(&records, "outside")), "outside");
    }

    #[test]
    fn rust_trait_methods_are_methods_and_body_fns_stay_functions() {
        let source = "trait Store {\n    fn load(&self);\n}\nimpl Store for u8 {\n    fn load(&self) {\n        let helper = || 1;\n        fn nested() {}\n    }\n}\n";
        let records = extract_symbols("src/store.rs", source).expect("rust parses");
        assert_eq!(
            records.len(),
            4,
            "trait, trait method, impl method, local fn"
        );
        for name in ["Store::load", "u8::load"] {
            let method = records
                .iter()
                .find(|record| qualified(record) == name)
                .expect("each distinct method definition must be present");
            assert_eq!(method.symbol_kind.as_str(), "method");
        }
        assert_eq!(
            find(&records, "load").symbol_kind.as_str(),
            "method",
            "trait methods and impl methods share the method kind"
        );
        let body_fn = find(&records, "nested");
        assert_eq!(
            body_fn.symbol_kind.as_str(),
            "function",
            "a fn inside a method body is a local function, not a method"
        );
        // The impl names its own type (`u8`), so the body fn carries the
        // honest chain through it.
        assert_eq!(qualified(body_fn), "u8::load::nested");
    }

    #[test]
    fn rust_impl_generics_stripped_from_container_names() {
        let source = "struct Vec2<T> { x: T }\nstruct Bar<T>(T);\nstruct Baz;\nimpl<T> Vec2<T> {\n    fn first(&self) -> &T { &self.x }\n}\nimpl Vec2<u8> {\n    fn second(&self) {}\n}\nimpl Vec2<Bar<Baz>> {\n    fn nested(&self) {}\n}\n";
        let records = extract_symbols("src/generic.rs", source).expect("rust parses");
        assert_eq!(qualified(find(&records, "first")), "Vec2::first");
        assert_eq!(qualified(find(&records, "second")), "Vec2::second");
        assert_eq!(qualified(find(&records, "nested")), "Vec2::nested");
        for source in [
            "struct Foo<T>(T); impl Foo<fn() -> ()> { fn first(&self) {} }",
            "struct Foo<const B: bool>; impl Foo<{1 < 2}> { fn first(&self) {} }",
            "struct Foo<const B: bool>; impl Foo<{2 > 1}> { fn first(&self) {} }",
            "struct Foo<const B: bool>; impl Foo<{1 << 2 == 4}> { fn first(&self) {} }",
        ] {
            let records = extract_symbols("src/generic.rs", source).expect("rust parses");
            assert_eq!(qualified(find(&records, "first")), "Foo::first");
        }
        for (source, expected) in [
            (
                "impl module::Foo<fn() -> ()> { fn first(&self) {} }",
                "module::Foo::first",
            ),
            (
                "impl Foo<Bar>::Assoc { fn first(&self) {} }",
                "Foo<Bar>::Assoc::first",
            ),
        ] {
            let records = extract_symbols("src/generic.rs", source).expect("rust parses");
            assert_eq!(qualified(find(&records, "first")), expected);
        }
        // The pinned grammar rejects a turbofish in this type position.
        // Do not weaken strict parsing to manufacture another positive.
        assert!(matches!(
            extract_symbols(
                "src/generic.rs",
                "struct Foo<T>(T); impl Foo::<u8> { fn first(&self) {} }"
            ),
            Err(SymbolExtractError::ParseFailure { path }) if path == "src/generic.rs"
        ));
    }

    #[test]
    fn nested_python_and_typescript_functions_stay_functions() {
        let python = "class Service:\n    def run(self):\n        def helper():\n            pass\n        helper()\n";
        let records = extract_symbols("src/service.py", python).expect("python parses");
        assert_eq!(find(&records, "run").symbol_kind.as_str(), "method");
        assert_eq!(find(&records, "helper").symbol_kind.as_str(), "function");
        assert_eq!(qualified(find(&records, "helper")), "Service.run.helper");
        for path in ["src/service.ts", "src/service.js", "src/service.tsx"] {
            for source in [
                "class Service { field = () => { function helper() {} }; }",
                "class Service { field = function () { function helper() {} }; }",
                "class Service { field = function* () { function helper() {} }; }",
                "class Service { static { function helper() {} } }",
            ] {
                let records = extract_symbols(path, source).expect("javascript/typescript parses");
                assert_eq!(find(&records, "helper").symbol_kind.as_str(), "function");
                assert_eq!(qualified(find(&records, "helper")), "Service.helper");
            }
            let source = "class Service { field = () => { class Inner { run() {} } }; }";
            let records = extract_symbols(path, source).expect("nested class parses");
            assert_eq!(find(&records, "run").symbol_kind.as_str(), "method");
            assert_eq!(qualified(find(&records, "run")), "Service.Inner.run");
        }
        for source in [
            "struct Service; impl Service { const F: fn() = || { fn helper() {} }; }",
            "struct Service; impl Service { const F: () = { fn helper() {} }; }",
            "trait T { type Item; } struct Service; impl T for Service { type Item = [(); { fn helper() {} 0 }]; }",
        ] {
            let records = extract_symbols("src/service.rs", source).expect("rust parses");
            assert_eq!(find(&records, "helper").symbol_kind.as_str(), "function");
            assert_eq!(qualified(find(&records, "helper")), "Service::helper");
        }
        for source in [
            "trait Service { fn run(&self) { fn helper() {} } }",
            "struct Service; impl Service { fn run(&self) { fn helper() {} } }",
        ] {
            let records = extract_symbols("src/service.rs", source).expect("rust method parses");
            assert_eq!(find(&records, "run").symbol_kind.as_str(), "method");
            assert_eq!(find(&records, "helper").symbol_kind.as_str(), "function");
            assert_eq!(qualified(find(&records, "helper")), "Service::run::helper");
        }

        let python = "class Service:\n    if True:\n        @staticmethod\n        def run():\n            def helper():\n                pass\n";
        let records = extract_symbols("src/service.py", python).expect("decorated python parses");
        assert_eq!(find(&records, "run").symbol_kind.as_str(), "method");
        assert_eq!(qualified(find(&records, "run")), "Service.run");
        assert_eq!(find(&records, "helper").symbol_kind.as_str(), "function");
        assert_eq!(qualified(find(&records, "helper")), "Service.run.helper");

        let typescript =
            "class Service {\n  run() {\n    function helper() {}\n    helper();\n  }\n}\n";
        let records = extract_symbols("src/service.ts", typescript).expect("typescript parses");
        assert_eq!(find(&records, "run").symbol_kind.as_str(), "method");
        assert_eq!(find(&records, "helper").symbol_kind.as_str(), "function");
        assert_eq!(qualified(find(&records, "helper")), "Service.run.helper");
    }

    #[test]
    fn unicode_identifiers_and_attribute_span() {
        // Python allows Unicode identifiers; Rust attributes are sibling
        // nodes, so a struct span starts at the struct itself.
        let py = "def 관리자():\n    pass\n";
        let records = extract_symbols("u/py.py", py).expect("python parses");
        assert_eq!(find(&records, "관리자").symbol_kind.as_str(), "function");

        let rs = "#[derive(Debug)]\nstruct Tagged;\n";
        let records = extract_symbols("u/a.rs", rs).expect("rust parses");
        let tagged = find(&records, "Tagged");
        let struct_offset = rs.find("struct Tagged").expect("offset");
        assert_eq!(
            usize::try_from(tagged.definition_span.byte_start).expect("byte offset fits usize"),
            struct_offset,
            "Rust attribute lines are not part of the definition span"
        );
    }

    #[test]
    fn jsx_and_versioned_ts_extensions_map() {
        assert_eq!(
            SymbolLanguage::from_path("ui/card.jsx")
                .expect("jsx grammar")
                .coverage_identity(),
            "javascript"
        );
        assert_eq!(
            SymbolLanguage::from_path("ui/card.tsx")
                .expect("tsx grammar")
                .coverage_identity(),
            "typescript_tsx"
        );
        let jsx = "export function Card() { return null; }\n";
        let records = extract_symbols("ui/card.jsx", jsx).expect("jsx parses");
        assert_eq!(
            records.first().expect("JS symbol").language.as_str(),
            "javascript"
        );
        let mts = "export const x: number = 1;\nexport function go(): void {}\n";
        let records = extract_symbols("m/a.mts", mts).expect("mts parses");
        assert_eq!(
            records
                .first()
                .expect("TypeScript symbol")
                .language
                .as_str(),
            "typescript"
        );
    }
    #[test]
    fn homonymous_definitions_keep_distinct_ids() {
        let source = "struct Walker;\nimpl Walker {\n  fn step(&self) {}\n}\n\
                      struct Runner;\nimpl Runner {\n  fn step(&self) {}\n}\n";
        let records = extract_symbols("src/duo.rs", source).expect("rust parses");
        let ids: Vec<&str> = records
            .iter()
            .filter(|record| &*record.local_name == "step")
            .map(|record| record.symbol_id.as_str())
            .collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(
            ids.first().expect("first homonymous symbol"),
            ids.get(1).expect("second homonymous symbol")
        );
        let qualifieds: Vec<&str> = records
            .iter()
            .filter(|record| &*record.local_name == "step")
            .map(|record| record.qualified_name.as_ref())
            .collect();
        assert_eq!(qualifieds, ["Walker::step", "Runner::step"]);
    }
}
