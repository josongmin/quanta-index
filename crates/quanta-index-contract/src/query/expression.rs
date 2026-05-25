// Round-7 cleanup: the Round-6b `LqExprExt` wrapper has been absorbed into
// the canonical `LqExpr` enum in `quanta-index-lq-norm`. Producers and
// downstream consumers now construct `LqExpr::SemanticVector { vector_ref,
// top_k }` directly; the wrapper enum no longer exists and there is no
// dual surface to maintain.
//
// `SemanticVectorRef` lives on lq-norm too (so the AST can own its own
// payload type) and is re-exported here for callers that pin against the
// contract crate.
//
// D18 — every wire shape on the underlying types is hand-rolled serde in
// `quanta_index_lq_norm::ast`.

pub use quanta_index_lq_norm::{
    LQ_VERSION_TAG, LqExpr, LqLeaf, LqMetaVar, LqPredicateArg, LqSpan, LqStructuralBlock,
    LqStructuralNode, SemanticVectorRef,
};

pub type LqQuery = quanta_index_lq_norm::LqNormalizedQuery;

#[cfg(test)]
mod tests {
    use super::{LqExpr, LqLeaf, SemanticVectorRef};

    type TestRes = Result<(), Box<dyn std::error::Error>>;

    fn encode<T: serde::Serialize>(v: &T) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut buf: Vec<u8> = Vec::new();
        ciborium::ser::into_writer(v, &mut buf)?;
        Ok(buf)
    }

    fn decode<T>(bytes: &[u8]) -> Result<T, Box<dyn std::error::Error>>
    where
        T: for<'de> serde::Deserialize<'de>,
    {
        Ok(ciborium::de::from_reader(bytes)?)
    }

    #[test]
    fn semantic_vector_ref_inline_cbor_roundtrip() -> TestRes {
        let v = SemanticVectorRef::Inline(vec![0.0, 1.5, -2.0]);
        let bytes = encode(&v)?;
        let back: SemanticVectorRef = decode(&bytes)?;
        if back != v {
            return Err("roundtrip mismatch".into());
        }
        Ok(())
    }

    #[test]
    fn semantic_vector_ref_handle_cbor_roundtrip() -> TestRes {
        let v = SemanticVectorRef::Handle("vec-handle-1".into());
        let bytes = encode(&v)?;
        let back: SemanticVectorRef = decode(&bytes)?;
        if back != v {
            return Err("roundtrip mismatch".into());
        }
        Ok(())
    }

    #[test]
    fn lq_expr_lq_cbor_roundtrip() -> TestRes {
        let v = LqExpr::Leaf(LqLeaf::Keyword("foo".to_owned()));
        let bytes = encode(&v)?;
        let back: LqExpr = decode(&bytes)?;
        if back != v {
            return Err("roundtrip mismatch".into());
        }
        Ok(())
    }

    #[test]
    fn lq_expr_semantic_vector_inline_cbor_roundtrip() -> TestRes {
        let v = LqExpr::SemanticVector {
            vector_ref: SemanticVectorRef::Inline(vec![0.1, 0.2, 0.3]),
            top_k: 16,
        };
        let bytes = encode(&v)?;
        let back: LqExpr = decode(&bytes)?;
        if back != v {
            return Err("roundtrip mismatch".into());
        }
        Ok(())
    }

    #[test]
    fn lq_expr_semantic_vector_handle_cbor_roundtrip() -> TestRes {
        let v = LqExpr::SemanticVector {
            vector_ref: SemanticVectorRef::Handle("h1".into()),
            top_k: 10,
        };
        let bytes = encode(&v)?;
        let back: LqExpr = decode(&bytes)?;
        if back != v {
            return Err("roundtrip mismatch".into());
        }
        Ok(())
    }
}
