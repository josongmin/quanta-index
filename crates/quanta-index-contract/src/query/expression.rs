// D18 — every wire shape on the underlying types is hand-rolled serde in
// `quanta_index_lq_norm::ast`.

pub use quanta_index_lq_norm::{
    LQ_VERSION_TAG, LqExpr, LqLeaf, LqMetaVar, LqPredicateArg, LqSpan, LqStructuralBlock,
    LqStructuralConstraint, LqStructuralConstraintOperand, LqStructuralExpr,
    LqStructuralHoleMultiplicity, LqStructuralHoleRef, LqStructuralNode,
};

/// Maximum distinct `where` regexes admitted by one structural request.
///
/// Boolean dispatch enforces this across leaves; a direct producer request
/// enforces it within its single block. This is not a physical heap ceiling.
pub const MAX_STRUCTURAL_WHERE_REGEX_ENGINES_V1: usize = 8;

pub type LqQuery = quanta_index_lq_norm::LqNormalizedQuery;

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
