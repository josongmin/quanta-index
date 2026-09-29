// D18 — every wire shape on the underlying types is hand-rolled serde in
// `quanta_index_lq_norm::ast`.

pub use quanta_index_lq_norm::{
    LQ_VERSION_TAG, LqExpr, LqLeaf, LqMetaVar, LqPredicateArg, LqSpan, LqStructuralBlock,
    LqStructuralConstraint, LqStructuralConstraintOperand, LqStructuralExpr,
    LqStructuralHoleMultiplicity, LqStructuralHoleRef, LqStructuralNode,
};

/// Maximum distinct `where` regex engines retained by one structural block
/// request. This is a cardinality guard, not a physical heap-byte ceiling.
pub const MAX_STRUCTURAL_WHERE_REGEX_ENGINES_V1: usize = 8;

pub type TextQueryAst = quanta_index_lq_norm::LqNormalizedQuery;
pub type LqQuery = TextQueryAst;

#[cfg(test)]
mod tests {
    use super::{LqExpr, LqLeaf};

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
    fn lq_expr_lq_cbor_roundtrip() -> TestRes {
        let v = LqExpr::Leaf(LqLeaf::Keyword("foo".to_owned()));
        let bytes = encode(&v)?;
        let back: LqExpr = decode(&bytes)?;
        if back != v {
            return Err("roundtrip mismatch".into());
        }
        Ok(())
    }
}
