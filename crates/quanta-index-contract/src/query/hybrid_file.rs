//! Explicit ordinary file retrieval over the two bounded hybrid chunk lanes.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser::SerializeStruct};

use crate::{HybridQueryRequest, validate_public_top_k};

const MAX_QUERY_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct HybridFileQueryRequestV1 {
    /// `query.top_k` is the per-lane chunk budget, not a file count.
    pub query: HybridQueryRequest,
    pub file_top_k: u32,
}

impl HybridFileQueryRequestV1 {
    pub fn validate_v1(&self) -> Result<(), String> {
        let chunk_cap = validate_public_top_k(self.query.top_k)
            .map_err(|error| format!("{}: {error}", error.code()))?;
        let _text_cap = validate_public_top_k(self.query.text_query.top_k)
            .map_err(|error| format!("{}: {error}", error.code()))?;
        let file_cap = validate_public_top_k(self.file_top_k)
            .map_err(|error| format!("{}: {error}", error.code()))?;
        if file_cap > chunk_cap || self.query.text_query.top_k != chunk_cap {
            return Err("file cap exceeds or differs from the chunk-lane cap".into());
        }
        for text in [
            self.query.text_query.query_text.as_str(),
            self.query.semantic_query_text.as_str(),
        ] {
            if text.trim().is_empty() || text.contains('\0') || text.len() > MAX_QUERY_BYTES {
                return Err("hybrid file query text is empty or exceeds 16 KiB".into());
            }
        }
        Ok(())
    }
}

impl Serialize for HybridFileQueryRequestV1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.validate_v1().map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("HybridFileQueryRequestV1", 2)?;
        state.serialize_field("query", &self.query)?;
        state.serialize_field("file_top_k", &self.file_top_k)?;
        state.end()
    }
}

struct WireRequest {
    query: HybridQueryRequest,
    file_top_k: u32,
}
crate::native_serde_struct!([serde_identifier] WireRequest {
    query: HybridQueryRequest,
    file_top_k: u32,
});

impl<'de> Deserialize<'de> for HybridFileQueryRequestV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = WireRequest::deserialize(deserializer)?;
        let request = Self {
            query: wire.query,
            file_top_k: wire.file_top_k,
        };
        request.validate_v1().map_err(de::Error::custom)?;
        Ok(request)
    }
}
