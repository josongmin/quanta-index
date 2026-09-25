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

/// Pinned grammar identity bound into every scope digest (batch.rs). The
/// versions mirror the workspace lockfile; changing a grammar changes the
/// digest and invalidates frozen evidence.
pub const SYMBOL_PRODUCER_GRAMMARS: &str = concat!(
    "tree-sitter@0.25;",
    "rust@0.24;",
    "go@0.25;",
    "javascript@0.25;",
    "python@0.25;",
    "typescript@0.23",
);

/// Producer identity for batch digests.
pub const SYMBOL_PRODUCER_IDENTITY: &str = "source-bound-symbols-v1";

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
    Unsupported { path: String },
    /// tree-sitter reported parse errors in the file.
    ParseFailure { path: String },
    /// Two definitions collapsed onto one deterministic id.
    IdCollision { path: String, symbol_id: String },
    /// The producer itself is inconsistent with the pinned grammar (query
    /// construction failed or a kind code is unregistered). A producer
    /// defect aborts the whole corpus run; it is never a per-file parse
    /// failure.
    ProducerDefect { detail: String },
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
        }
    }
}

impl std::error::Error for SymbolExtractError {}

impl SymbolLanguage {
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
            _ => ".",
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
            (_, "function_declaration")
            | (_, "generator_function_declaration")
            | (_, "function_item")
            | (_, "function_definition") => Some(if container_is_type {
                "method"
            } else {
                "function"
            }),
            (_, "method_declaration") | (_, "method_definition") => Some("method"),
            (_, "class_declaration")
            | (_, "abstract_class_declaration")
            | (_, "class_definition") => Some("class"),
            (_, "struct_item") => Some("struct"),
            (_, "enum_item") => Some("enum"),
            (_, "trait_item") => Some("trait"),
            (_, "interface_declaration") => Some("interface"),
            (_, "type_item") | (_, "type_alias_declaration") => Some("type_alias"),
            (_, "mod_item") => Some("module"),
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
            Self::Go => matches!(node_kind, "method_declaration"),
            Self::Python => matches!(node_kind, "class_definition"),
            Self::JavaScript | Self::TypeScript { .. } => matches!(
                node_kind,
                "class_declaration"
                    | "abstract_class_declaration"
                    | "function_declaration"
                    | "generator_function_declaration"
                    | "module"
                    | "internal_module"
                    | "enum_declaration"
            ),
        }
    }

    /// The name text of a container node, if it has one.
    fn container_name<'tree>(self, node: Node<'tree>, source: &str) -> Option<String> {
        if self == Self::Rust && node.kind() == "impl_item" {
            // impl blocks name their container through the `type` field.
            // Generic argument lists (`Foo<T>`) are stripped so the same
            // logical type yields one qualified-name spelling.
            let ty = node.child_by_field_name("type")?;
            let text = ty.utf8_text(source.as_bytes()).ok()?;
            let Some(open) = text.find('<') else {
                return Some(text.to_string());
            };
            if !text.ends_with('>') || text.bytes().filter(|b| *b == b'<').count() != 1 {
                // Malformed or operator-heavy generics: keep the raw text
                // rather than mangling it.
                return Some(text.to_string());
            }
            return Some(text[..open].to_string());
        }
        let name = node.child_by_field_name("name")?;
        Some(name.utf8_text(source.as_bytes()).ok()?.to_string())
    }
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
fn go_receiver_type<'tree>(def_node: Node<'tree>, source: &str) -> Option<String> {
    let receiver = def_node.child_by_field_name("receiver")?;
    let type_node = find_descendant_kind(receiver, "type_identifier")?;
    type_node
        .utf8_text(source.as_bytes())
        .ok()
        .map(str::to_string)
}

const RUST_QUERY: &str = r#"
(function_item name: (identifier) @name) @def
(struct_item name: (type_identifier) @name) @def
(enum_item name: (type_identifier) @name) @def
(trait_item name: (type_identifier) @name) @def
(type_item name: (type_identifier) @name) @def
(mod_item name: (identifier) @name) @def
"#;

const GO_QUERY: &str = r#"
(function_declaration name: (identifier) @name) @def
(method_declaration name: (field_identifier) @name) @def
(type_spec name: (type_identifier) @name) @def
"#;

const PYTHON_QUERY: &str = r#"
(function_definition name: (identifier) @name) @def
(class_definition name: (identifier) @name) @def
"#;

const JAVASCRIPT_QUERY: &str = r#"
(function_declaration name: (identifier) @name) @def
(generator_function_declaration name: (identifier) @name) @def
(class_declaration name: (identifier) @name) @def
(method_definition name: (property_identifier) @name) @def
"#;

const TYPESCRIPT_QUERY: &str = r#"
(function_declaration name: (identifier) @name) @def
(generator_function_declaration name: (identifier) @name) @def
(class_declaration name: (type_identifier) @name) @def
(abstract_class_declaration name: (type_identifier) @name) @def
(method_definition name: [(property_identifier) (private_property_identifier)] @name) @def
(interface_declaration name: (type_identifier) @name) @def
(type_alias_declaration name: (type_identifier) @name) @def
"#;

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

    fn line(&self, byte_offset: usize) -> u32 {
        let line = self.starts.partition_point(|start| *start <= byte_offset);
        u32::try_from(line.max(1)).unwrap_or(1)
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
    let mut parser = Parser::new();
    let grammar = language.grammar();
    if parser.set_language(&grammar).is_err() {
        return Err(SymbolExtractError::ParseFailure {
            path: path.to_string(),
        });
    }
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| SymbolExtractError::ParseFailure {
            path: path.to_string(),
        })?;
    if tree.root_node().has_error() {
        return Err(SymbolExtractError::ParseFailure {
            path: path.to_string(),
        });
    }
    Ok(tree)
}

fn query_definitions(
    language: SymbolLanguage,
    tree: &tree_sitter::Tree,
    source: &str,
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
    let mut stream = cursor.matches(&query, tree.root_node(), source.as_bytes());
    while let Some(matched) = stream.next() {
        let mut def_node: Option<Node<'_>> = None;
        let mut name_node: Option<Node<'_>> = None;
        for capture in matched.captures {
            match query.capture_names()[capture.index as usize] {
                "def" => def_node = Some(capture.node),
                "name" => name_node = Some(capture.node),
                _ => {}
            }
        }
        let (Some(def_node), Some(name_node)) = (def_node, name_node) else {
            // Anonymous definition (e.g. an unnamed impl or expression):
            // no invented public name.
            continue;
        };
        let local_name = match name_node.utf8_text(source.as_bytes()) {
            Ok(text) => text,
            Err(_) => continue,
        };
        let mut containers: Vec<String> = Vec::new();
        // The method/function decision uses the NEAREST container
        // ancestor's kind: a fn whose innermost enclosing container is a
        // type-like scope (impl/trait/class) is a method; a fn nested in
        // a method body has the enclosing function as its nearest
        // container and stays a function.
        let mut nearest_type_container = false;
        let mut seen_container = false;
        let mut parent = def_node.parent();
        while let Some(node) = parent {
            if language.is_container(node.kind()) {
                if let Some(name) = language.container_name(node, source) {
                    if !seen_container {
                        seen_container = true;
                        nearest_type_container = language.is_type_container(node.kind());
                    }
                    containers.push(name);
                }
            }
            parent = node.parent();
        }
        if language == SymbolLanguage::Go && def_node.kind() == "method_declaration" {
            // Go methods carry their container (the receiver type) on the
            // node itself rather than through an ancestor.
            if let Some(receiver_type) = go_receiver_type(def_node, source) {
                nearest_type_container = true;
                containers.push(receiver_type);
            }
        }
        containers.reverse();
        let mut kind = language.kind_for(def_node.kind(), nearest_type_container);
        if language == SymbolLanguage::Go && def_node.kind() == "type_spec" {
            // `type X ...` is classified by its type child: struct_type,
            // interface_type, or a plain definition (type_alias).
            if find_descendant_kind(def_node, "interface_type").is_some() {
                kind = Some("interface");
            } else if find_descendant_kind(def_node, "struct_type").is_some() {
                kind = Some("struct");
            } else {
                kind = Some("type_alias");
            }
        }
        let Some(kind) = kind else {
            continue;
        };
        definitions.push(RawDefinition {
            kind,
            local_name: local_name.to_string(),
            containers,
            byte_start: def_node.start_byte(),
            byte_end: def_node.end_byte(),
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
    let definitions = query_definitions(language, &tree, source)?;
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
                byte_start: u32::try_from(definition.byte_start).map_err(|_| {
                    SymbolExtractError::ParseFailure {
                        path: path.to_string(),
                    }
                })?,
                byte_end: u32::try_from(definition.byte_end).map_err(|_| {
                    SymbolExtractError::ParseFailure {
                        path: path.to_string(),
                    }
                })?,
                line_start: line_index.line(definition.byte_start),
                line_end: line_index.line(definition.byte_end.saturating_sub(1)),
            },
            container_qualified_name: container_qualified_name
                .map(|name| name.into_boxed_str())
                .map(Box::from),
            relationship: SymbolRelationship::Def,
        });
    }
    Ok(records)
}

/// Extract symbols for a whole admitted corpus. Files whose language has
/// no pinned grammar are counted as unsupported (no symbols, not a
/// failure); parse failures in supported languages are explicit coverage
/// failures that abort the run (RBR-04).
pub struct CorpusSymbolExtraction {
    pub symbols: std::collections::BTreeMap<String, Vec<SymbolRecord>>,
    pub unsupported_files: Vec<String>,
}

pub fn extract_corpus_symbols(
    files: &std::collections::BTreeMap<String, crate::corpus::SourceFile>,
) -> crate::BenchResult<CorpusSymbolExtraction> {
    let mut symbols = std::collections::BTreeMap::new();
    let mut unsupported_files = Vec::new();
    for (path, file) in files {
        if SymbolLanguage::from_path(path).is_none() {
            unsupported_files.push(path.clone());
            continue;
        }
        let records =
            extract_symbols(path, &file.text).map_err(|error| crate::BenchError::Chunk {
                path: path.clone(),
                message: format!("symbol extraction coverage failure: {error}"),
            })?;
        let _previous = symbols.insert(path.clone(), records);
    }
    Ok(CorpusSymbolExtraction {
        symbols,
        unsupported_files,
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
        assert_eq!(records[0].local_name.as_ref(), "Engine");
    }

    #[test]
    fn go_functions_methods_and_types() {
        let source = "package geom\n\n\
                      type Rect struct {\n\tW float64\n}\n\n\
                      type Shape interface {\n\tArea() float64\n}\n\n\
                      func (r *Rect) Area() float64 { return r.W }\n\n\
                      func NewRect(w float64) *Rect { return &Rect{W: w} }\n";
        let records = extract_symbols("geom/rect.go", source).expect("go parses");
        let method = find(&records, "Area");
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
            decorated.definition_span.byte_start as usize, def_offset,
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
        assert_eq!(records[0].language.as_str(), "typescript");
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
            BTreeSet::<&str>::from_iter(ids.iter().copied()).len(),
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
        let line = starts.partition_point(|start| *start <= b.definition_span.byte_start as usize);
        assert_eq!(
            b.definition_span.line_start as usize,
            line.max(1),
            "symbol line must match the corpus model"
        );
    }

    #[test]
    fn go_value_receivers_and_plain_type_definitions() {
        let source = "package t\n\ntype Celsius float64\n\ntype Rect struct { W float64 }\n\nfunc (r Rect) Area() float64 { return r.W }\n";
        let records = extract_symbols("t/temp.go", source).expect("go parses");
        assert_eq!(
            find(&records, "Celsius").symbol_kind.as_str(),
            "type_alias",
            "plain Go type definitions are not structs"
        );
        assert_eq!(find(&records, "Rect").symbol_kind.as_str(), "struct");
        let area = find(&records, "Area");
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
        let source = "struct Vec2<T> { x: T }\nimpl<T> Vec2<T> {\n    fn first(&self) -> &T { &self.x }\n}\nimpl Vec2<u8> {\n    fn second(&self) {}\n}\n";
        let records = extract_symbols("src/generic.rs", source).expect("rust parses");
        assert_eq!(qualified(find(&records, "first")), "Vec2::first");
        assert_eq!(qualified(find(&records, "second")), "Vec2::second");
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
            tagged.definition_span.byte_start as usize, struct_offset,
            "Rust attribute lines are not part of the definition span"
        );
    }

    #[test]
    fn jsx_and_versioned_ts_extensions_map() {
        let jsx = "export function Card() { return null; }\n";
        let records = extract_symbols("ui/card.jsx", jsx).expect("jsx parses");
        assert_eq!(records[0].language.as_str(), "javascript");
        let mts = "export const x: number = 1;\nexport function go(): void {}\n";
        let records = extract_symbols("m/a.mts", mts).expect("mts parses");
        assert_eq!(records[0].language.as_str(), "typescript");
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
        assert_ne!(ids[0], ids[1]);
        let qualifieds: Vec<&str> = records
            .iter()
            .filter(|record| &*record.local_name == "step")
            .map(|record| record.qualified_name.as_ref())
            .collect();
        assert_eq!(qualifieds, ["Walker::step", "Runner::step"]);
    }
}
