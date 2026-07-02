//! Lancedb-flavored SQL helpers shared by the build and search paths.
//!
//! Lancedb's `DataFusion`-backed predicate language has no parameterized-query
//! API, so identifier values that flow into `table.delete(...)` and
//! `vector_search(...).only_if(...)` must be string-escaped here. Single quote
//! is the only `DataFusion` string-literal escape; embedded `'` is doubled.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use std::collections::BTreeSet;

use crate::layout::COLUMN_EMBEDDING_ID;

/// Wrap `value` in single quotes and escape any embedded `'` for use as a
/// `DataFusion` string literal (the only literal kind lancedb predicates accept).
pub(crate) fn quote_sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Build an `embedding_id IN ('a', 'b', ...)` filter for `vector_search.only_if`
/// from the search-time allowlist.
pub(crate) fn build_id_in_filter(allowed_ids: &BTreeSet<String>) -> String {
    let mut joined = String::new();
    let mut first = true;
    for id in allowed_ids {
        if first {
            first = false;
        } else {
            joined.push_str(", ");
        }
        joined.push_str(&quote_sql_string(id));
    }
    format!("{COLUMN_EMBEDDING_ID} IN ({joined})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_sql_string_wraps_in_single_quotes() {
        assert_eq!(quote_sql_string(""), "''");
        assert_eq!(quote_sql_string("foo"), "'foo'");
        assert_eq!(quote_sql_string("src/main.rs"), "'src/main.rs'");
    }

    #[test]
    fn quote_sql_string_doubles_embedded_single_quotes() {
        // DataFusion-style escape: each `'` becomes `''` inside the literal.
        assert_eq!(quote_sql_string("O'Reilly"), "'O''Reilly'");
        assert_eq!(
            quote_sql_string("a' OR 1=1 --"),
            "'a'' OR 1=1 --'",
            "SQL-injection-shaped input must stay inside the single-quoted literal"
        );
        assert_eq!(
            quote_sql_string("'multiple''quotes'"),
            "'''multiple''''quotes'''",
        );
    }

    #[test]
    fn quote_sql_string_passes_other_meta_chars_through() {
        // Inside a DataFusion string literal, `--`, `;`, `/* */`, `\` are all
        // ordinary content — only `'` is special. Confirm.
        assert_eq!(quote_sql_string("a--b"), "'a--b'");
        assert_eq!(quote_sql_string("a;b"), "'a;b'");
        assert_eq!(quote_sql_string("a/*b*/c"), "'a/*b*/c'");
        assert_eq!(quote_sql_string("a\\nb"), "'a\\nb'");
    }

    #[test]
    fn build_id_in_filter_empty_allowlist_uses_empty_in_clause() {
        let empty: BTreeSet<String> = BTreeSet::new();
        assert_eq!(build_id_in_filter(&empty), "embedding_id IN ()");
    }

    #[test]
    fn build_id_in_filter_single_id() {
        let mut allow: BTreeSet<String> = BTreeSet::new();
        let _inserted: bool = allow.insert("emb-1".to_string());
        assert_eq!(build_id_in_filter(&allow), "embedding_id IN ('emb-1')");
    }

    #[test]
    fn build_id_in_filter_multiple_ids_sorted() {
        let mut allow: BTreeSet<String> = BTreeSet::new();
        let _b: bool = allow.insert("b".to_string());
        let _a: bool = allow.insert("a".to_string());
        let _c: bool = allow.insert("c".to_string());
        // BTreeSet iteration is sorted -> the IN list is deterministic.
        assert_eq!(
            build_id_in_filter(&allow),
            "embedding_id IN ('a', 'b', 'c')"
        );
    }

    #[test]
    fn build_id_in_filter_escapes_hostile_id() {
        let mut allow: BTreeSet<String> = BTreeSet::new();
        let _inserted: bool = allow.insert("'); SELECT * FROM secret; --".to_string());
        assert_eq!(
            build_id_in_filter(&allow),
            "embedding_id IN ('''); SELECT * FROM secret; --')",
            "injection-shaped id must remain a literal inside the IN clause"
        );
    }
}
