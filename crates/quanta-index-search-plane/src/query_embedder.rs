use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_core::{CoreError, SemanticPolicy};

pub trait QueryTextEmbedderPort {
    fn embed_query(&self, query_text: &str) -> Result<Vec<f32>, CoreError>;
}

pub struct DecimalQueryTextEmbedder;

pub struct HashingQueryTextEmbedder {
    dimension: usize,
}

impl HashingQueryTextEmbedder {
    #[must_use]
    pub const fn new(dimension: usize) -> Self {
        Self { dimension }
    }
}

impl QueryTextEmbedderPort for DecimalQueryTextEmbedder {
    fn embed_query(&self, query_text: &str) -> Result<Vec<f32>, CoreError> {
        decode_query_text_as_vector(query_text)
    }
}

impl QueryTextEmbedderPort for HashingQueryTextEmbedder {
    fn embed_query(&self, query_text: &str) -> Result<Vec<f32>, CoreError> {
        hash_query_text(query_text, self.dimension)
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
            code: LexicalErrorCode::EmptyQuery.as_code_str().to_string(),
            message: "semantic: query text must contain at least one alphanumeric token"
                .to_string(),
        });
    }
    let norm_sq: f32 = vector.iter().map(|value| value * value).sum();
    if norm_sq <= f32::EPSILON {
        return Err(CoreError::Typed {
            code: LexicalErrorCode::SemInvalidVector.as_code_str().to_string(),
            message: "semantic: hashed query text collapsed to a zero-norm embedding".to_string(),
        });
    }
    let inv_norm = norm_sq.sqrt().recip();
    for value in &mut vector {
        *value *= inv_norm;
    }
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

fn decode_query_text_as_vector(text: &str) -> Result<Vec<f32>, CoreError> {
    let mut query_vector = Vec::new();
    for token in text.split_whitespace() {
        let value = token.parse::<f32>().map_err(|err| CoreError::Typed {
            code: LexicalErrorCode::SemInvalidVector.as_code_str().to_string(),
            message: format!("semantic: query vector token `{token}` is not a valid f32: {err}"),
        })?;
        if !value.is_finite() {
            return Err(CoreError::Typed {
                code: LexicalErrorCode::SemInvalidVector.as_code_str().to_string(),
                message: format!("semantic: query vector token `{token}` is not finite"),
            });
        }
        query_vector.push(value);
    }
    SemanticPolicy::validate_query_vector(&query_vector)?;
    Ok(query_vector)
}
