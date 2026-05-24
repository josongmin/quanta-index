//! `tag:` filter primitive.
//!
//! [`tag_resolve`] returns every `(tag_name, commit_sha)` pair in the graph
//! that matches `pattern`. v1 is exact-string match; glob and regex are
//! deferred until a parser is wired up (see `dsl.md` § 3.4).
//!
//! Pattern classification:
//!
//! - bare names (no glob meta) -> "explicit lookup"; an empty match set is a
//!   typed `HistoryRefNotFound`
//! - patterns containing `*`, `?`, or `[` -> "pattern lookup"; an empty match
//!   set returns an empty `Vec` without error (matches the LEX-07 § 9 cap
//!   semantics for glob with no hits)
//!
//! D18 — manual serde everywhere; no proc-macro derives.

use crate::commit_graph::CommitGraph;
use crate::errors::{HistoryError, HistoryErrorCode};
use crate::types::CommitSha;

/// Resolve a `tag:` filter against the graph's tag map.
///
/// Returns a vector of `(name, sha)` pairs in ascending name order. Pattern
/// support is deferred per the module docs; v1 treats `pattern` as the exact
/// tag name unless it contains `*`, `?`, or `[`, in which case we fall back
/// to an empty result without error (cooperative glob hand-off).
pub fn tag_resolve(
    graph: &CommitGraph,
    pattern: &str,
) -> Result<Vec<(Box<str>, CommitSha)>, HistoryError> {
    if is_glob_pattern(pattern) {
        // Glob is deferred; pattern lookups return empty without error.
        return Ok(Vec::new());
    }

    let tags = graph.tags();
    if let Some(sha) = tags.get(pattern) {
        return Ok(vec![(Box::from(pattern), *sha)]);
    }
    Err(HistoryError::new(
        HistoryErrorCode::HistoryRefNotFound,
        format!("tag: pattern `{pattern}` matched no tags"),
    ))
}

/// Returns `true` when the pattern contains glob meta-characters.
fn is_glob_pattern(s: &str) -> bool {
    s.bytes().any(|b| matches!(b, b'*' | b'?' | b'['))
}

#[cfg(test)]
mod tests {
    use super::{is_glob_pattern, tag_resolve};
    use crate::commit_graph::CommitGraph;
    use crate::errors::HistoryErrorCode;
    use crate::types::CommitSha;

    fn sha(byte: u8) -> CommitSha {
        CommitSha::from_bytes([byte; 20])
    }

    #[test]
    fn glob_detector() {
        assert!(!is_glob_pattern("v1.0"));
        assert!(is_glob_pattern("v1.*"));
        assert!(is_glob_pattern("v?.0"));
        assert!(is_glob_pattern("v[12].0"));
    }

    #[test]
    fn exact_match_returns_pair() {
        let mut g = CommitGraph::new();
        let _prev: Option<CommitSha> = g.add_tag("v1.0", sha(3));
        let got = match tag_resolve(&g, "v1.0") {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(got.len(), 1);
        let Some((name, s)) = got.first() else {
            assert!(false, "expected pair");
            return;
        };
        assert_eq!(name.as_ref(), "v1.0");
        assert_eq!(*s, sha(3));
    }

    #[test]
    fn exact_unknown_fails_typed() {
        let g = CommitGraph::new();
        match tag_resolve(&g, "v1.0") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, HistoryErrorCode::HistoryRefNotFound),
        }
    }

    #[test]
    fn glob_pattern_returns_empty_vec_no_error() {
        let mut g = CommitGraph::new();
        let _prev: Option<CommitSha> = g.add_tag("v1.0", sha(3));
        let got = match tag_resolve(&g, "v1.*") {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert!(got.is_empty());
    }
}
