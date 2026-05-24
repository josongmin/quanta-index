//! Golden extractor corpus — per-language expected symbol set.
//!
//! Until the real tree-sitter-backed extractors land (see crate-level doc
//! § Tree-sitter deferral), this corpus exercises the [`SymbolExtractor`]
//! trait surface and the [`ExtractorRegistry`] routing path with
//! deterministic fixtures. The shape of these tests is the shape the
//! real per-language extractors must satisfy: given a small source
//! snippet, the registry returns a canonical `Vec<Symbol>` matching the
//! expected `(name, kind)` set.

use quanta_index_lq_symbol::{
    ByteSpan, DocId, ExtractorRegistry, LangId, MockExtractor, Symbol, SymbolErrorCode, SymbolKind,
};

fn must_span(start: u32, end: u32) -> ByteSpan {
    let Ok(s) = ByteSpan::new(start, end) else {
        std::process::abort();
    };
    s
}

fn rust_corpus() -> Vec<Symbol> {
    // Models the symbols that a real `tree-sitter-rust` + `tags.scm` pass
    // would emit for the snippet:
    //
    //   pub fn handle_request(req: Request) -> Response { ... }
    //   pub struct Handler { state: AppState }
    //   pub trait HasState { fn state(&self) -> &AppState; }
    //   impl Handler { fn new() -> Self { ... } }
    //
    // Method `Handler::new` is flat-listed with `parent = Some("Handler")`.
    vec![
        Symbol::new(
            "handle_request",
            SymbolKind::Function,
            DocId(1),
            must_span(7, 21),
            LangId::Rust,
            None,
        ),
        Symbol::new(
            "Handler",
            SymbolKind::Struct,
            DocId(1),
            must_span(70, 77),
            LangId::Rust,
            None,
        ),
        Symbol::new(
            "HasState",
            SymbolKind::Trait,
            DocId(1),
            must_span(110, 118),
            LangId::Rust,
            None,
        ),
        Symbol::new(
            "state",
            SymbolKind::Method,
            DocId(1),
            must_span(125, 130),
            LangId::Rust,
            Some("HasState".into()),
        ),
        Symbol::new(
            "new",
            SymbolKind::Method,
            DocId(1),
            must_span(180, 183),
            LangId::Rust,
            Some("Handler".into()),
        ),
    ]
}

fn python_corpus() -> Vec<Symbol> {
    // Models:  class Foo: def bar(self): pass
    //          def top(): pass
    //          CONST = 1
    vec![
        Symbol::new(
            "Foo",
            SymbolKind::Class,
            DocId(2),
            must_span(6, 9),
            LangId::Python,
            None,
        ),
        Symbol::new(
            "bar",
            SymbolKind::Method,
            DocId(2),
            must_span(20, 23),
            LangId::Python,
            Some("Foo".into()),
        ),
        Symbol::new(
            "top",
            SymbolKind::Function,
            DocId(2),
            must_span(40, 43),
            LangId::Python,
            None,
        ),
        Symbol::new(
            "CONST",
            SymbolKind::Constant,
            DocId(2),
            must_span(60, 65),
            LangId::Python,
            None,
        ),
    ]
}

fn javascript_corpus() -> Vec<Symbol> {
    // Models: function greet() {} ; class Greeter { hello() {} } ;
    //         const VALUE = 1
    vec![
        Symbol::new(
            "greet",
            SymbolKind::Function,
            DocId(3),
            must_span(9, 14),
            LangId::JavaScript,
            None,
        ),
        Symbol::new(
            "Greeter",
            SymbolKind::Class,
            DocId(3),
            must_span(25, 32),
            LangId::JavaScript,
            None,
        ),
        Symbol::new(
            "hello",
            SymbolKind::Method,
            DocId(3),
            must_span(40, 45),
            LangId::JavaScript,
            Some("Greeter".into()),
        ),
        Symbol::new(
            "VALUE",
            SymbolKind::Constant,
            DocId(3),
            must_span(70, 75),
            LangId::JavaScript,
            None,
        ),
    ]
}

fn typescript_corpus() -> Vec<Symbol> {
    // Models: interface Foo {} ; type Bar = string ; enum Color { Red }
    vec![
        Symbol::new(
            "Foo",
            SymbolKind::Interface,
            DocId(4),
            must_span(10, 13),
            LangId::TypeScript,
            None,
        ),
        Symbol::new(
            "Bar",
            SymbolKind::TypeAlias,
            DocId(4),
            must_span(25, 28),
            LangId::TypeScript,
            None,
        ),
        Symbol::new(
            "Color",
            SymbolKind::Enum,
            DocId(4),
            must_span(45, 50),
            LangId::TypeScript,
            None,
        ),
    ]
}

fn go_corpus() -> Vec<Symbol> {
    // Models: package main ; func main() {} ; type Server struct {}
    vec![
        Symbol::new(
            "main",
            SymbolKind::Module,
            DocId(5),
            must_span(8, 12),
            LangId::Go,
            None,
        ),
        Symbol::new(
            "main",
            SymbolKind::Function,
            DocId(5),
            must_span(20, 24),
            LangId::Go,
            None,
        ),
        Symbol::new(
            "Server",
            SymbolKind::Struct,
            DocId(5),
            must_span(35, 41),
            LangId::Go,
            None,
        ),
    ]
}

fn build_registry() -> ExtractorRegistry {
    let mut r = ExtractorRegistry::new();
    let _rs: bool = r.register(LangId::Rust, Box::new(MockExtractor::new(rust_corpus())));
    let _py: bool = r.register(
        LangId::Python,
        Box::new(MockExtractor::new(python_corpus())),
    );
    let _ts: bool = r.register(
        LangId::TypeScript,
        Box::new(MockExtractor::new(typescript_corpus())),
    );
    let _js: bool = r.register(
        LangId::JavaScript,
        Box::new(MockExtractor::new(javascript_corpus())),
    );
    let _go: bool = r.register(LangId::Go, Box::new(MockExtractor::new(go_corpus())));
    r
}

#[test]
fn rust_extractor_emits_expected_set() {
    let r = build_registry();
    let got = match r.extract(LangId::Rust, b"pub fn handle_request() {}") {
        Ok(v) => v,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let names: Vec<&str> = got.iter().map(|s| s.name.as_ref()).collect();
    assert!(names.contains(&"handle_request"));
    assert!(names.contains(&"Handler"));
    assert!(names.contains(&"HasState"));
    assert!(names.contains(&"new"));
    let kinds: Vec<SymbolKind> = got.iter().map(|s| s.kind).collect();
    assert!(kinds.contains(&SymbolKind::Function));
    assert!(kinds.contains(&SymbolKind::Struct));
    assert!(kinds.contains(&SymbolKind::Trait));
    assert!(kinds.contains(&SymbolKind::Method));
}

#[test]
fn python_extractor_emits_expected_set() {
    let r = build_registry();
    let got = match r.extract(LangId::Python, b"class Foo:\n    def bar(self): pass\n") {
        Ok(v) => v,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let names: Vec<&str> = got.iter().map(|s| s.name.as_ref()).collect();
    assert!(names.contains(&"Foo"));
    assert!(names.contains(&"bar"));
    assert!(names.contains(&"top"));
    assert!(names.contains(&"CONST"));
    // bar is a Method with parent=Foo (flattened-with-container per spec §3).
    let bar = got
        .iter()
        .find(|s| s.name.as_ref() == "bar" && s.kind == SymbolKind::Method);
    let Some(bar) = bar else {
        assert!(false, "missing bar method");
        return;
    };
    assert_eq!(bar.parent.as_deref(), Some("Foo"));
}

#[test]
fn javascript_extractor_emits_expected_set() {
    let r = build_registry();
    let got = match r.extract(LangId::JavaScript, b"function greet() {}\n") {
        Ok(v) => v,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let names: Vec<&str> = got.iter().map(|s| s.name.as_ref()).collect();
    assert!(names.contains(&"greet"));
    assert!(names.contains(&"Greeter"));
    assert!(names.contains(&"hello"));
    assert!(names.contains(&"VALUE"));
}

#[test]
fn typescript_extractor_emits_typealias_interface_enum() {
    let r = build_registry();
    let got = match r.extract(LangId::TypeScript, b"interface Foo {}\n") {
        Ok(v) => v,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let kinds: Vec<SymbolKind> = got.iter().map(|s| s.kind).collect();
    assert!(kinds.contains(&SymbolKind::Interface));
    assert!(kinds.contains(&SymbolKind::TypeAlias));
    assert!(kinds.contains(&SymbolKind::Enum));
}

#[test]
fn go_extractor_emits_module_function_struct() {
    let r = build_registry();
    let got = match r.extract(LangId::Go, b"package main\nfunc main() {}\n") {
        Ok(v) => v,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let kinds: Vec<SymbolKind> = got.iter().map(|s| s.kind).collect();
    assert!(kinds.contains(&SymbolKind::Module));
    assert!(kinds.contains(&SymbolKind::Function));
    assert!(kinds.contains(&SymbolKind::Struct));
}

#[test]
fn unsupported_lang_emits_state_not_ready_symbol_lang_unsupported() {
    let mut r = ExtractorRegistry::new();
    // Register only Rust; Python is therefore unsupported.
    let _rs: bool = r.register(LangId::Rust, Box::new(MockExtractor::new(Vec::new())));
    match r.extract(LangId::Python, b"class Foo: pass\n") {
        Ok(_) => assert!(false, "must fail closed"),
        Err(e) => {
            assert_eq!(e.code, SymbolErrorCode::StateNotReady);
            assert!(e.is_lang_unsupported());
            assert!(e.detail.contains("PYTHON"));
        }
    }
}
