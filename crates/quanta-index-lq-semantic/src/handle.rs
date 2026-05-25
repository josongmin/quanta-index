//! Semantic vector handle resolution per ADR-026 Option B.
//!
//! `LqExprExt::SemanticVector { vector_ref: SemanticVectorRef::Handle(h), .. }`
//! carries a producer-assigned `embedding_id` string. Per [ADR-026 Option B]
//! the handle is server-side equivalent to `EmbeddingRecord.embedding_id`,
//! and via [`producer-handoff.md §3.5.1`] wire-identity equivalence the
//! embedding's shard identity collapses onto the corpus's
//! [`crate::types::DocId`] (a `u64` newtype).
//!
//! At resolution time the query planner queries the active-generation
//! [`crate::index::SemanticIndex`] for that handle. There is no separate
//! handle store (Option B); the index's `(DocId, Vec<f32>)` map is the
//! single source of truth. The handle string is parsed as a [`u64`] —
//! a non-numeric handle, or one that doesn't resolve, fails closed with
//! [`crate::errors::SemanticErrorCode::SemHandleNotFound`] and the original
//! handle string is carried in [`crate::errors::SemanticError::detail`].
//!
//! There is **no fallback to an empty result**: per
//! [`producer-handoff.md §6.5`] an unresolved handle surfaces a typed
//! `SEM_HANDLE_NOT_FOUND` on the observability rail rather than degrading
//! the query.
//!
//! D18 — no proc-macro derives in this module.

use crate::errors::SemanticError;
use crate::index::SemanticIndex;
use crate::types::DocId;

/// Resolves a `SemanticVectorRef::Handle` string to the corpus [`DocId`]
/// that owns the matching embedding, or fails closed.
///
/// Implementations must NEVER substitute an empty result, a placeholder
/// vector, or a heuristic match for an absent handle. The only success
/// shape is `Ok(DocId)` for an exact, present-in-corpus match.
pub trait SemanticHandleResolver {
    /// Look up `handle` against the active-generation embedding store.
    ///
    /// Returns:
    /// - `Ok(DocId)` when the handle resolves to a present corpus entry,
    /// - `Err(SemanticError { code: SemHandleNotFound, detail: handle, .. })`
    ///   when no entry is found for `handle`.
    fn resolve(&self, handle: &str) -> Result<DocId, SemanticError>;
}

/// Resolves handles against a borrowed [`SemanticIndex`].
///
/// Holds an immutable reference to the index, so the lifetime of the
/// resolver is bounded by the index it was constructed from. This matches
/// the planner's call shape: a single index is borrowed for the duration
/// of a query, and the resolver is built per-query.
pub struct SemanticIndexHandleResolver<'a> {
    index: &'a SemanticIndex,
}

impl<'a> SemanticIndexHandleResolver<'a> {
    /// Construct a resolver bound to `index`.
    #[must_use]
    pub const fn new(index: &'a SemanticIndex) -> Self {
        Self { index }
    }

    /// Borrow the backing index.
    #[must_use]
    pub const fn index(&self) -> &'a SemanticIndex {
        self.index
    }
}

impl SemanticHandleResolver for SemanticIndexHandleResolver<'_> {
    fn resolve(&self, handle: &str) -> Result<DocId, SemanticError> {
        // Per ADR-026 Option B: handle == embedding_id; per
        // producer-handoff.md §3.5.1 wire-identity equivalence the
        // embedding_id collapses onto the corpus DocId(u64). Parse the
        // handle string as the u64 DocId. Any parse failure is a
        // resolution failure — fail closed with SEM_HANDLE_NOT_FOUND.
        let Ok(raw) = handle.parse::<u64>() else {
            return Err(SemanticError::handle_not_found(handle));
        };
        let doc_id = DocId(raw);
        self.index.get(doc_id).map_or_else(
            || Err(SemanticError::handle_not_found(handle)),
            |_vec| Ok(doc_id),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{SemanticHandleResolver, SemanticIndexHandleResolver};
    use crate::errors::SemanticErrorCode;
    use crate::index::SemanticIndexBuilder;
    use crate::types::{DocId, Embedding};

    fn emb(v: Vec<f32>) -> Embedding {
        let Ok(e) = Embedding::new(v) else {
            std::process::abort();
        };
        e
    }

    #[test]
    fn resolver_finds_present_handle() {
        // Build a single-doc index and resolve its handle.
        let Ok(mut b) = SemanticIndexBuilder::new(1, 2) else {
            assert!(false, "builder must construct");
            return;
        };
        let v = emb(vec![1.0_f32, 0.0_f32]);
        if let Err(e) = b.add_embedding(DocId(42), &v) {
            assert!(false, "{e}");
            return;
        }
        let idx = b.finish();
        let resolver = SemanticIndexHandleResolver::new(&idx);
        match resolver.resolve("42") {
            Ok(d) => assert_eq!(d, DocId(42)),
            Err(e) => assert!(false, "expected resolution, got {e}"),
        }
    }

    #[test]
    fn resolver_returns_typed_error_on_absent_handle() {
        // Empty corpus must fail closed.
        let Ok(b) = SemanticIndexBuilder::new(1, 4) else {
            assert!(false, "builder must construct");
            return;
        };
        let idx = b.finish();
        let resolver = SemanticIndexHandleResolver::new(&idx);
        match resolver.resolve("missing") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => {
                assert_eq!(e.code, SemanticErrorCode::SemHandleNotFound);
                assert_eq!(e.detail.as_ref(), "missing");
                assert_eq!(e.dimension, None);
            }
        }
    }

    #[test]
    fn resolver_preserves_handle_in_error_detail() {
        // A numeric-but-absent handle must still surface the queried
        // handle string verbatim in the error detail.
        let Ok(b) = SemanticIndexBuilder::new(7, 3) else {
            assert!(false, "builder must construct");
            return;
        };
        let idx = b.finish();
        let resolver = SemanticIndexHandleResolver::new(&idx);
        match resolver.resolve("12345") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => {
                assert_eq!(e.code, SemanticErrorCode::SemHandleNotFound);
                assert_eq!(e.detail.as_ref(), "12345");
            }
        }
    }

    #[test]
    fn resolver_present_among_many() {
        // Index with several docs; resolver must pick the right one.
        let Ok(mut b) = SemanticIndexBuilder::new(1, 2) else {
            assert!(false, "builder must construct");
            return;
        };
        for (doc, vec) in [
            (DocId(1), vec![1.0_f32, 0.0_f32]),
            (DocId(2), vec![0.0_f32, 1.0_f32]),
            (DocId(100), vec![0.5_f32, 0.5_f32]),
        ] {
            let e = emb(vec);
            if let Err(err) = b.add_embedding(doc, &e) {
                assert!(false, "{err}");
                return;
            }
        }
        let idx = b.finish();
        let resolver = SemanticIndexHandleResolver::new(&idx);
        match resolver.resolve("100") {
            Ok(d) => assert_eq!(d, DocId(100)),
            Err(e) => assert!(false, "{e}"),
        }
        match resolver.resolve("99") {
            Ok(_) => assert!(false, "doc 99 not in corpus"),
            Err(e) => {
                assert_eq!(e.code, SemanticErrorCode::SemHandleNotFound);
                assert_eq!(e.detail.as_ref(), "99");
            }
        }
    }

    #[test]
    fn resolver_non_numeric_handle_fails_closed() {
        // Per ADR-026 Option B handles map onto DocId(u64). A
        // non-numeric handle cannot resolve and must fail closed with
        // SEM_HANDLE_NOT_FOUND carrying the verbatim handle.
        let Ok(b) = SemanticIndexBuilder::new(1, 2) else {
            assert!(false, "builder must construct");
            return;
        };
        let idx = b.finish();
        let resolver = SemanticIndexHandleResolver::new(&idx);
        match resolver.resolve("not-a-u64") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => {
                assert_eq!(e.code, SemanticErrorCode::SemHandleNotFound);
                assert_eq!(e.detail.as_ref(), "not-a-u64");
            }
        }
    }

    #[test]
    fn resolver_empty_handle_fails_closed() {
        let Ok(b) = SemanticIndexBuilder::new(1, 2) else {
            assert!(false, "builder must construct");
            return;
        };
        let idx = b.finish();
        let resolver = SemanticIndexHandleResolver::new(&idx);
        match resolver.resolve("") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => {
                assert_eq!(e.code, SemanticErrorCode::SemHandleNotFound);
                assert_eq!(e.detail.as_ref(), "");
            }
        }
    }

    #[test]
    fn resolver_index_accessor_returns_backing_index() {
        let Ok(b) = SemanticIndexBuilder::new(9, 2) else {
            assert!(false, "builder must construct");
            return;
        };
        let idx = b.finish();
        let resolver = SemanticIndexHandleResolver::new(&idx);
        // Accessor must hand back the exact same index reference.
        assert_eq!(resolver.index().generation(), 9);
        assert_eq!(resolver.index().dim(), 2);
    }

    #[test]
    fn resolver_via_upsert_path_resolves() {
        // Upsert-driven build path (replay-safe) must also resolve.
        let Ok(mut b) = SemanticIndexBuilder::new(3, 2) else {
            assert!(false, "builder must construct");
            return;
        };
        let v = emb(vec![1.0_f32, 1.0_f32]);
        if let Err(e) = b.upsert_embedding(DocId(77), &v) {
            assert!(false, "{e}");
            return;
        }
        let idx = b.finish();
        let resolver = SemanticIndexHandleResolver::new(&idx);
        match resolver.resolve("77") {
            Ok(d) => assert_eq!(d, DocId(77)),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn resolver_after_remove_fails_closed() {
        // A handle that pointed at a now-removed doc must fail closed
        // with the typed code (no stale match, no empty result).
        let Ok(mut b) = SemanticIndexBuilder::new(1, 2) else {
            assert!(false, "builder must construct");
            return;
        };
        let v = emb(vec![1.0_f32, 0.0_f32]);
        if let Err(e) = b.upsert_embedding(DocId(5), &v) {
            assert!(false, "{e}");
            return;
        }
        match b.remove_embedding(DocId(5)) {
            Ok(true) => {}
            Ok(false) => {
                assert!(false, "must remove existing");
                return;
            }
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        }
        let idx = b.finish();
        let resolver = SemanticIndexHandleResolver::new(&idx);
        match resolver.resolve("5") {
            Ok(_) => assert!(false, "removed doc must not resolve"),
            Err(e) => {
                assert_eq!(e.code, SemanticErrorCode::SemHandleNotFound);
                assert_eq!(e.detail.as_ref(), "5");
            }
        }
    }
}
