use quanta_index_contract::EmbeddingNormalization;
use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_core::{
    CoreError, EMBED_CHECKPOINT, RequestBudgetV1, SemanticPolicy, TextEmbeddingProvider,
};

/// Output dimension of the search-owned hash embedder.
pub const SEARCH_OWNED_SEMANTIC_DIMENSION: usize = 64;

/// SSOT for the search-owned hash embedder's model identity.
///
/// This lives with the embedder that defines it: both the query-time
/// [`HashingQueryTextEmbedder`] (here) and the corpus derivation contract
/// (`ingest_dispatcher`/`semantic_derive`) read it so the two sides cannot
/// silently disagree on model identity; query-time enforcement rejects a
/// mismatch (`SEM_MODEL_MISMATCH`).
pub const SEARCH_OWNED_SEMANTIC_MODEL_ID: &str = "search-owned-hash-text-v1";

/// The hash embedder's revision (QI-BB-028).
///
/// It names the slot hashing and the normalization it applies, so a change
/// to either is a new revision and never shares a cache namespace or a query
/// gate with the old one.
pub const SEARCH_OWNED_SEMANTIC_MODEL_REVISION: &str = "fnv1a64-slots-l2unit-v1";

pub trait QueryTextEmbedderPort {
    /// Embed one query text under the request's budget (QI-BB-002).
    ///
    /// An embedder that reaches a provider observes the budget inside
    /// that call — the deadline caps every attempt and a cancellation
    /// abandons the attempt in flight — and answers with the typed
    /// interruption at the `semantic:embed` checkpoint; an embedder with
    /// nothing to interrupt checks the budget once and computes.
    fn embed_query(
        &self,
        query_text: &str,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<f32>, CoreError>;

    /// Stable identity of the model this embedder produces query vectors for.
    /// Query vectors are only comparable (cosine) against a corpus indexed by the
    /// SAME model; the query path enforces this against the indexed generation's
    /// persisted model identity and fails closed (`SEM_MODEL_MISMATCH`) on drift.
    fn model_id(&self) -> &str;

    /// The model revision, compared alongside [`Self::model_id`].
    fn model_revision(&self) -> &str;
}

pub struct HashingQueryTextEmbedder {
    dimension: usize,
}

impl HashingQueryTextEmbedder {
    #[must_use]
    pub const fn new(dimension: usize) -> Self {
        Self { dimension }
    }
}

impl QueryTextEmbedderPort for HashingQueryTextEmbedder {
    fn embed_query(
        &self,
        query_text: &str,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<f32>, CoreError> {
        budget.checkpoint(EMBED_CHECKPOINT)?;
        hash_query_text(query_text, self.dimension)
    }

    fn model_id(&self) -> &str {
        SEARCH_OWNED_SEMANTIC_MODEL_ID
    }

    fn model_revision(&self) -> &str {
        SEARCH_OWNED_SEMANTIC_MODEL_REVISION
    }
}

impl TextEmbeddingProvider for HashingQueryTextEmbedder {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
        texts
            .iter()
            .map(|text| hash_query_text(text, self.dimension))
            .collect()
    }

    fn model_id(&self) -> &str {
        SEARCH_OWNED_SEMANTIC_MODEL_ID
    }

    fn model_revision(&self) -> &str {
        SEARCH_OWNED_SEMANTIC_MODEL_REVISION
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn normalization(&self) -> EmbeddingNormalization {
        // `hash_query_text` normalizes through the shared policy, so this
        // embedder promises unit vectors itself (QI-BB-031).
        EmbeddingNormalization::L2Unit
    }
}

#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-private hashing helper is shared across sibling search-plane modules"
)]
pub(crate) fn hash_query_text(text: &str, dimension: usize) -> Result<Vec<f32>, CoreError> {
    if dimension == 0 {
        return Err(CoreError::InvalidContract(
            "semantic: hashing embedder dimension must be non-zero".to_string(),
        ));
    }
    let mut vector = vec![0.0_f32; dimension];
    let dimension_u64 = u64::try_from(dimension).map_err(|err| {
        CoreError::InvalidContract(format!(
            "semantic: hashing embedder dimension conversion failed: {err}"
        ))
    })?;
    let mut saw_token = false;
    for token in text
        .split(|ch: char| !ch.is_alphanumeric())
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        saw_token = true;
        let hash = stable_fnv1a64(token.as_bytes());
        let primary_slot = hashed_slot(hash, dimension_u64)?;
        let secondary_slot = hashed_slot(hash.rotate_right(32), dimension_u64)?;
        let primary = vector.get_mut(primary_slot).ok_or_else(|| {
            CoreError::Storage(format!(
                "semantic: primary hashed slot {primary_slot} out of bounds for dimension {dimension}"
            ))
        })?;
        *primary += 1.0;
        let secondary = vector.get_mut(secondary_slot).ok_or_else(|| {
            CoreError::Storage(format!(
                "semantic: secondary hashed slot {secondary_slot} out of bounds for dimension {dimension}"
            ))
        })?;
        *secondary -= 0.5;
    }
    if !saw_token {
        return Err(CoreError::Typed {
            code: LexicalErrorCode::EmptyQuery.into(),
            message: "semantic: query text must contain at least one alphanumeric token"
                .to_string(),
        });
    }
    // The same normalization every provider gets (QI-BB-031); a text whose
    // slots cancel to a zero vector is refused typed here, not turned into
    // NaNs.
    SemanticPolicy::normalize_l2_unit_v1(&mut vector).map_err(|err| match err {
        CoreError::Typed { code, .. } => CoreError::Typed {
            code,
            message: "semantic: hashed query text collapsed to a zero-norm embedding".to_string(),
        },
        other @ (CoreError::InvalidContract(_)
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)) => other,
    })?;
    Ok(vector)
}

fn hashed_slot(hash: u64, dimension_u64: u64) -> Result<usize, CoreError> {
    let slot_u64 = hash.checked_rem(dimension_u64).ok_or_else(|| {
        CoreError::InvalidContract(
            "semantic: hashing embedder dimension must be non-zero".to_string(),
        )
    })?;
    usize::try_from(slot_u64).map_err(|err| {
        CoreError::InvalidContract(format!(
            "semantic: hashed slot conversion failed for {slot_u64}: {err}"
        ))
    })
}

fn stable_fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}
