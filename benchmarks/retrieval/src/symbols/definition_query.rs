//! Compile each immutable grammar/query pair once per producer process.

use std::sync::OnceLock;

use tree_sitter::Query;

use super::{SymbolExtractError, SymbolLanguage};

type QueryCache = OnceLock<Result<Query, String>>;

pub(super) fn compiled(language: SymbolLanguage) -> Result<&'static Query, SymbolExtractError> {
    static RUST: QueryCache = OnceLock::new();
    static GO: QueryCache = OnceLock::new();
    static PYTHON: QueryCache = OnceLock::new();
    static JAVASCRIPT: QueryCache = OnceLock::new();
    static TYPESCRIPT: QueryCache = OnceLock::new();
    static TSX: QueryCache = OnceLock::new();
    let cache = match language {
        SymbolLanguage::Rust => &RUST,
        SymbolLanguage::Go => &GO,
        SymbolLanguage::Python => &PYTHON,
        SymbolLanguage::JavaScript => &JAVASCRIPT,
        SymbolLanguage::TypeScript { is_tsx: false } => &TYPESCRIPT,
        SymbolLanguage::TypeScript { is_tsx: true } => &TSX,
    };
    cache
        .get_or_init(|| {
            Query::new(&language.grammar(), language.query()).map_err(|error| {
                // Grammar/query drift is deterministic for this immutable binary.
                format!("grammar query construction failed: {error}")
            })
        })
        .as_ref()
        .map_err(|detail| SymbolExtractError::ProducerDefect {
            detail: detail.clone(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_shared_only_with_the_same_grammar() {
        let languages = [
            SymbolLanguage::Rust,
            SymbolLanguage::Go,
            SymbolLanguage::Python,
            SymbolLanguage::JavaScript,
            SymbolLanguage::TypeScript { is_tsx: false },
            SymbolLanguage::TypeScript { is_tsx: true },
        ];
        for language in languages {
            let query = compiled(language).expect("pinned grammar and query agree");
            assert!(std::ptr::eq(query, compiled(language).expect("same query")));
            assert!(query.capture_names().contains(&"def"));
            assert!(query.capture_names().contains(&"name"));
            for other in languages {
                if other != language {
                    assert!(!std::ptr::eq(query, compiled(other).expect("other query")));
                }
            }
        }
    }

    #[test]
    fn repeated_files_preserve_source_bound_symbols_and_reject_bad_syntax() {
        let source = "export function render() { return <div />; }";
        let first = super::super::extract_symbols("first.tsx", source).expect("valid TSX");
        let repeated = super::super::extract_symbols("first.tsx", source).expect("repeat TSX");
        assert_eq!(first, repeated);
        assert_eq!(first.len(), 1);
        let record = first.first().expect("one declaration");
        assert_eq!(&*record.local_name, "render");
        assert_eq!(record.definition_span.byte_start, 7);
        assert_eq!(
            record.definition_span.byte_end,
            u32::try_from(source.len()).expect("small fixture span")
        );
        assert!(matches!(
            super::super::extract_symbols("first.ts", source),
            Err(SymbolExtractError::ParseFailure { .. })
        ));
        assert!(matches!(
            super::super::extract_symbols("bad.tsx", "function broken("),
            Err(SymbolExtractError::ParseFailure { .. })
        ));
    }
}
