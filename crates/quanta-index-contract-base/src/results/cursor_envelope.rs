//! Authenticated continuation cursors (`CursorEnvelopeV2`, S21-06).
//!
//! A continuation token is an integrity-protected statement of the exact
//! request context it continues: route, full generation pin, read
//! identity, normalized query and constraint digests, order, cap, aux
//! epochs, key id, issued-at/expiry and the version, all bound by
//! HMAC-SHA256 under a server-held key. The wire form is
//! base64url-without-padding of `version || canonical-context || mac`.
//!
//! Decode verifies the signature, version and expiry before any resource
//! acquisition; binding to the current request is the caller's
//! comparison of [`CursorBindingV2`], which covers every context field so
//! a token minted for one query, repo, route, order, cap or epoch cannot
//! be replayed against another.

use core::fmt;

use serde::{Deserialize, Serialize, de};
use sha2::{Digest, Sha256};

use crate::ids::{ManifestGeneration, RepoId, RevisionId};
use crate::query::GenerationPin;

/// Envelope format version. One byte on the wire, inside the MAC.
pub const CURSOR_ENVELOPE_V2_VERSION: u8 = 2;

/// Byte domain separator for the canonical context encoding.
const CURSOR_CONTEXT_DOMAIN: &[u8] = b"quanta-index-cursor-context-v2";

/// Byte domain separator for the MAC.
const CURSOR_MAC_DOMAIN: &[u8] = b"quanta-index-cursor-mac-v2";

/// Length of the HMAC-SHA256 tag carried on the wire.
const CURSOR_MAC_LEN: usize = 32;

/// Pageable routes that may issue a continuation cursor. Bounded top-k
/// routes never do: they must not imply pagination.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CursorRouteV2 {
    Lexical,
    Symbol,
    History,
    RuntimeMetadata,
    Structural,
}

impl CursorRouteV2 {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::Symbol => "symbol",
            Self::History => "history",
            Self::RuntimeMetadata => "runtime_metadata",
            Self::Structural => "structural",
        }
    }

    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::Lexical,
            Self::Symbol,
            Self::History,
            Self::RuntimeMetadata,
            Self::Structural,
        ]
    }

    #[must_use]
    pub fn from_wire_str(value: &str) -> Option<Self> {
        match value {
            "lexical" => Some(Self::Lexical),
            "symbol" => Some(Self::Symbol),
            "history" => Some(Self::History),
            "runtime_metadata" => Some(Self::RuntimeMetadata),
            "structural" => Some(Self::Structural),
            _ => None,
        }
    }

    const fn discriminant(self) -> u64 {
        match self {
            Self::Lexical => 1,
            Self::Symbol => 2,
            Self::History => 3,
            Self::RuntimeMetadata => 4,
            Self::Structural => 5,
        }
    }
}

/// Auxiliary-store epochs a cursor binds so a stale continuation cannot
/// read a rotated authority silently.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CursorAuxEpochKindV2 {
    History,
    RuntimeMetadata,
    Structural,
}

impl CursorAuxEpochKindV2 {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::History => "history",
            Self::RuntimeMetadata => "runtime_metadata",
            Self::Structural => "structural",
        }
    }

    const fn discriminant(self) -> u64 {
        match self {
            Self::History => 1,
            Self::RuntimeMetadata => 2,
            Self::Structural => 3,
        }
    }
}

/// One bound aux epoch.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CursorAuxEpochV2 {
    pub kind: CursorAuxEpochKindV2,
    pub epoch: u64,
}

/// The full request context a continuation is valid for. Compared as a
/// whole against the executing request; any single differing field fails
/// the match.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CursorBindingV2 {
    /// The pageable route the token continues.
    pub route: CursorRouteV2,
    /// The full generation pin of the read the token continues: repo,
    /// revision and manifest generation. A continuation executes under
    /// exactly this pin — never under a re-resolved active generation.
    pub pin: GenerationPin,
    /// Digest of the canonical plan / read identity.
    pub plan_digest: [u8; 32],
    /// Digest of the normalized query.
    pub query_digest: [u8; 32],
    /// Digest of the constraint set.
    pub constraints_digest: [u8; 32],
    /// Stable wire name of the ordering.
    pub order: String,
    /// The cap the original request ran under.
    pub cap: u32,
    /// Aux epochs at issue time, sorted by kind.
    pub aux_epochs: Vec<CursorAuxEpochV2>,
}

impl CursorBindingV2 {
    /// Canonical context bytes; `None` only when a field length cannot
    /// fit the wire length width, which no supported target allows.
    ///
    /// The context layout changed with the full-pin binding: tokens
    /// minted under the old `(repo_id, pinned_generation)` layout fail
    /// the MAC under the new layout and are refused as tampered. No
    /// migration exists or is needed — continuations live at most the
    /// one-hour TTL.
    fn canonical_bytes(&self, buffer: &mut Vec<u8>) -> Option<()> {
        push_domain(buffer, CURSOR_CONTEXT_DOMAIN);
        push_u64(buffer, self.route.discriminant());
        push_str(buffer, self.pin.repo_id.as_str())?;
        push_str(buffer, self.pin.revision_id.as_str())?;
        push_u64(buffer, self.pin.manifest_generation.get());
        push_digest(buffer, &self.plan_digest);
        push_digest(buffer, &self.query_digest);
        push_digest(buffer, &self.constraints_digest);
        push_str(buffer, &self.order)?;
        push_u32(buffer, self.cap);
        push_count(buffer, self.aux_epochs.len())?;
        for epoch in &self.aux_epochs {
            push_u64(buffer, epoch.kind.discriminant());
            push_u64(buffer, epoch.epoch);
        }
        Some(())
    }
}

/// A minted cursor: binding plus the boundary row key, key id and
/// validity window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CursorEnvelopeV2 {
    binding: CursorBindingV2,
    /// Route-specific ordering key of the last returned row. Opaque to
    /// the envelope; the route validates it before use.
    boundary: String,
    key_id: u64,
    issued_at_unix: u64,
    expires_at_unix: u64,
}

/// Server-held cursor signing key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CursorKeyV2 {
    id: u64,
    key: [u8; 32],
}

impl CursorKeyV2 {
    /// Construct a key from its id and raw bytes.
    #[must_use]
    pub const fn new(id: u64, key: [u8; 32]) -> Self {
        Self { id, key }
    }

    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }
}

/// Fail-closed cursor decode failures. The search-plane maps these onto
/// the stable refusal codes (`CURSOR_INVALID`, `CURSOR_EXPIRED`, ...).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CursorEnvelopeError {
    /// The token is not valid base64url or has an impossible length.
    MalformedToken,
    /// The version byte is not `CURSOR_ENVELOPE_V2_VERSION`.
    Version { found: u8 },
    /// The HMAC does not verify: any tampered or foreign-key token.
    Signature,
    /// The token's expiry is in the past for `now_unix`.
    Expired,
    /// The validity window itself is inconsistent.
    InvalidWindow,
}

impl fmt::Display for CursorEnvelopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedToken => f.write_str("cursor envelope: malformed token"),
            Self::Version { found } => {
                write!(
                    f,
                    "cursor envelope: unsupported version {found} (expected {CURSOR_ENVELOPE_V2_VERSION})"
                )
            }
            Self::Signature => f.write_str("cursor envelope: signature mismatch"),
            Self::Expired => f.write_str("cursor envelope: expired"),
            Self::InvalidWindow => f.write_str("cursor envelope: invalid validity window"),
        }
    }
}

impl std::error::Error for CursorEnvelopeError {}

impl CursorEnvelopeV2 {
    /// Mint a cursor. `expires_at_unix` must be after `issued_at_unix`.
    pub fn mint(
        binding: CursorBindingV2,
        boundary: impl Into<String>,
        key: &CursorKeyV2,
        issued_at_unix: u64,
        expires_at_unix: u64,
    ) -> Result<Self, CursorEnvelopeError> {
        if expires_at_unix <= issued_at_unix {
            return Err(CursorEnvelopeError::InvalidWindow);
        }
        Ok(Self {
            binding,
            boundary: boundary.into(),
            key_id: key.id(),
            issued_at_unix,
            expires_at_unix,
        })
    }

    #[must_use]
    pub const fn binding(&self) -> &CursorBindingV2 {
        &self.binding
    }

    #[must_use]
    pub fn boundary(&self) -> &str {
        &self.boundary
    }

    #[must_use]
    pub const fn key_id(&self) -> u64 {
        self.key_id
    }

    #[must_use]
    pub const fn issued_at_unix(&self) -> u64 {
        self.issued_at_unix
    }

    #[must_use]
    pub const fn expires_at_unix(&self) -> u64 {
        self.expires_at_unix
    }

    /// Whether this envelope's context is exactly `expected`. Any field
    /// difference — route, repo, generation, digests, order, cap, aux
    /// epochs — fails the match. Key id is compared too: a token signed
    /// by another key is a context mismatch even before its MAC check.
    #[must_use]
    pub fn matches(&self, expected: &CursorBindingV2, key_id: u64) -> bool {
        self.key_id == key_id && &self.binding == expected
    }

    fn mac_input(&self, buffer: &mut Vec<u8>) -> Option<()> {
        push_domain(buffer, CURSOR_MAC_DOMAIN);
        push_u64(buffer, u64::from(CURSOR_ENVELOPE_V2_VERSION));
        push_u64(buffer, self.key_id);
        push_u64(buffer, self.issued_at_unix);
        push_u64(buffer, self.expires_at_unix);
        push_str(buffer, &self.boundary)?;
        self.binding.canonical_bytes(buffer)
    }

    /// Encode to the wire form: base64url without padding of
    /// `version || canonical-context || mac`.
    pub fn encode(&self, key: &CursorKeyV2) -> Result<String, CursorEnvelopeError> {
        let mut body = Vec::new();
        self.mac_input(&mut body)
            .ok_or(CursorEnvelopeError::MalformedToken)?;
        let mut mac = Vec::with_capacity(CURSOR_MAC_LEN);
        push_digest(&mut mac, &hmac_sha256(&key.key, &body));
        let mut token = Vec::with_capacity(body.len().saturating_add(CURSOR_MAC_LEN));
        token.push(CURSOR_ENVELOPE_V2_VERSION);
        token.extend_from_slice(&body);
        token.extend_from_slice(&mac);
        Ok(base64url_nopad_encode(&token))
    }

    /// Decode and verify a token: base64url, version, length, HMAC and
    /// expiry — in that order — reconstructing the envelope. Signature,
    /// version and expiry all fail before any resource acquisition.
    pub fn decode(
        token: &str,
        key: &CursorKeyV2,
        now_unix: u64,
    ) -> Result<Self, CursorEnvelopeError> {
        let raw = base64url_nopad_decode(token).ok_or(CursorEnvelopeError::MalformedToken)?;
        if raw.len() < CURSOR_MAC_LEN + 2 {
            return Err(CursorEnvelopeError::MalformedToken);
        }
        let version = raw
            .first()
            .copied()
            .ok_or(CursorEnvelopeError::MalformedToken)?;
        if version != CURSOR_ENVELOPE_V2_VERSION {
            return Err(CursorEnvelopeError::Version { found: version });
        }
        let body_end = raw
            .len()
            .checked_sub(CURSOR_MAC_LEN)
            .ok_or(CursorEnvelopeError::MalformedToken)?;
        let body = raw
            .get(1..body_end)
            .ok_or(CursorEnvelopeError::MalformedToken)?;
        let mac = raw
            .get(body_end..)
            .ok_or(CursorEnvelopeError::MalformedToken)?;
        let expected = hmac_sha256(&key.key, body);
        if !constant_time_eq(&expected, mac) {
            return Err(CursorEnvelopeError::Signature);
        }
        let body = body
            .get(CURSOR_MAC_DOMAIN.len()..)
            .ok_or(CursorEnvelopeError::MalformedToken)?;
        let mut cursor = Reader::new(body);
        let body_version = cursor.u64().ok_or(CursorEnvelopeError::MalformedToken)?;
        if body_version != u64::from(CURSOR_ENVELOPE_V2_VERSION) {
            return Err(CursorEnvelopeError::MalformedToken);
        }
        let key_id = cursor.u64().ok_or(CursorEnvelopeError::MalformedToken)?;
        let issued_at_unix = cursor.u64().ok_or(CursorEnvelopeError::MalformedToken)?;
        let expires_at_unix = cursor.u64().ok_or(CursorEnvelopeError::MalformedToken)?;
        let boundary = cursor.str().ok_or(CursorEnvelopeError::MalformedToken)?;
        cursor
            .skip(CURSOR_CONTEXT_DOMAIN.len())
            .ok_or(CursorEnvelopeError::MalformedToken)?;
        let route = cursor.route().ok_or(CursorEnvelopeError::MalformedToken)?;
        let repo = cursor.str().ok_or(CursorEnvelopeError::MalformedToken)?;
        let Ok(repo_id) = RepoId::new(repo) else {
            return Err(CursorEnvelopeError::MalformedToken);
        };
        let revision = cursor.str().ok_or(CursorEnvelopeError::MalformedToken)?;
        let Ok(revision_id) = RevisionId::new(revision) else {
            return Err(CursorEnvelopeError::MalformedToken);
        };
        let manifest_generation = cursor.u64().ok_or(CursorEnvelopeError::MalformedToken)?;
        let plan_digest = cursor.digest().ok_or(CursorEnvelopeError::MalformedToken)?;
        let query_digest = cursor.digest().ok_or(CursorEnvelopeError::MalformedToken)?;
        let constraints_digest = cursor.digest().ok_or(CursorEnvelopeError::MalformedToken)?;
        let order = cursor.str().ok_or(CursorEnvelopeError::MalformedToken)?;
        let cap = cursor.u32().ok_or(CursorEnvelopeError::MalformedToken)?;
        let epoch_count = cursor
            .bounded_usize()
            .ok_or(CursorEnvelopeError::MalformedToken)?;
        let mut aux_epochs = Vec::new();
        for _ in 0..epoch_count {
            let kind = cursor
                .aux_kind()
                .ok_or(CursorEnvelopeError::MalformedToken)?;
            let epoch = cursor.u64().ok_or(CursorEnvelopeError::MalformedToken)?;
            aux_epochs.push(CursorAuxEpochV2 { kind, epoch });
        }
        if !cursor.is_empty() {
            return Err(CursorEnvelopeError::MalformedToken);
        }
        if key_id != key.id() {
            return Err(CursorEnvelopeError::Signature);
        }
        if expires_at_unix <= issued_at_unix {
            return Err(CursorEnvelopeError::InvalidWindow);
        }
        if now_unix > expires_at_unix {
            return Err(CursorEnvelopeError::Expired);
        }
        Ok(Self {
            binding: CursorBindingV2 {
                route,
                pin: GenerationPin::new(
                    repo_id,
                    revision_id,
                    ManifestGeneration::new(manifest_generation),
                ),
                plan_digest,
                query_digest,
                constraints_digest,
                order,
                cap,
                aux_epochs,
            },
            boundary,
            key_id,
            issued_at_unix,
            expires_at_unix,
        })
    }
}

/// Default and maximum cursor TTL (seconds): 15 minutes default, one hour
/// hard maximum.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CursorTtlPolicyV2 {
    pub default_secs: u64,
    pub max_secs: u64,
}

impl CursorTtlPolicyV2 {
    /// The policy the search plane enforces.
    #[must_use]
    pub const fn standard() -> Self {
        Self {
            default_secs: 15 * 60,
            max_secs: 60 * 60,
        }
    }

    /// Clamp a requested TTL to `[1, max_secs]`. Fails closed (returns
    /// `None`) on zero.
    #[must_use]
    pub const fn clamp(&self, requested_secs: u64) -> Option<u64> {
        if requested_secs == 0 {
            None
        } else if requested_secs > self.max_secs {
            Some(self.max_secs)
        } else {
            Some(requested_secs)
        }
    }
}

fn push_domain(buffer: &mut Vec<u8>, domain: &[u8]) {
    buffer.extend_from_slice(domain);
}

fn push_u64(buffer: &mut Vec<u8>, value: u64) {
    buffer.extend_from_slice(&value.to_be_bytes());
}

fn push_u32(buffer: &mut Vec<u8>, value: u32) {
    buffer.extend_from_slice(&value.to_be_bytes());
}

fn push_count(buffer: &mut Vec<u8>, count: usize) -> Option<()> {
    let Ok(count) = u64::try_from(count) else {
        return None;
    };
    push_u64(buffer, count);
    Some(())
}

fn push_str(buffer: &mut Vec<u8>, value: &str) -> Option<()> {
    push_count(buffer, value.len())?;
    buffer.extend_from_slice(value.as_bytes());
    Some(())
}

fn push_digest(buffer: &mut Vec<u8>, value: &[u8; 32]) {
    buffer.extend_from_slice(value);
}

struct Reader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    const fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    fn skip(&mut self, len: usize) -> Option<()> {
        self.take(len).map(|_| ())
    }

    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.offset.checked_add(len)?;
        let slice = self.data.get(self.offset..end)?;
        self.offset = end;
        Some(slice)
    }

    const fn is_empty(&self) -> bool {
        self.offset >= self.data.len()
    }

    fn u64(&mut self) -> Option<u64> {
        let bytes = self.take(8)?;
        let mut array = [0u8; 8];
        array.copy_from_slice(bytes);
        Some(u64::from_be_bytes(array))
    }

    fn u32(&mut self) -> Option<u32> {
        let bytes = self.take(4)?;
        let mut array = [0u8; 4];
        array.copy_from_slice(bytes);
        Some(u32::from_be_bytes(array))
    }

    fn bounded_usize(&mut self) -> Option<usize> {
        let Ok(value) = usize::try_from(self.u64()?) else {
            return None;
        };
        Some(value)
    }

    fn str(&mut self) -> Option<String> {
        let len = self.bounded_usize()?;
        let bytes = self.take(len)?;
        let Ok(text) = String::from_utf8(bytes.to_vec()) else {
            return None;
        };
        Some(text)
    }

    fn digest(&mut self) -> Option<[u8; 32]> {
        let bytes = self.take(32)?;
        let mut array = [0u8; 32];
        array.copy_from_slice(bytes);
        Some(array)
    }

    fn route(&mut self) -> Option<CursorRouteV2> {
        match self.u64()? {
            1 => Some(CursorRouteV2::Lexical),
            2 => Some(CursorRouteV2::Symbol),
            3 => Some(CursorRouteV2::History),
            4 => Some(CursorRouteV2::RuntimeMetadata),
            5 => Some(CursorRouteV2::Structural),
            _ => None,
        }
    }

    fn aux_kind(&mut self) -> Option<CursorAuxEpochKindV2> {
        match self.u64()? {
            1 => Some(CursorAuxEpochKindV2::History),
            2 => Some(CursorAuxEpochKindV2::RuntimeMetadata),
            3 => Some(CursorAuxEpochKindV2::Structural),
            _ => None,
        }
    }
}

/// HMAC-SHA256 (RFC 2104), implemented over `sha2` so the contract-base
/// crate needs no additional dependency. Verified against RFC 4231 test
/// vectors in the tests below.
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut key_block = [0u8; BLOCK];
    if key.len() > BLOCK {
        let hashed = Sha256::digest(key);
        key_block[..32].copy_from_slice(&hashed);
    } else {
        for (destination, source) in key_block.iter_mut().zip(key.iter()) {
            *destination = *source;
        }
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for ((ipad_byte, opad_byte), key_byte) in
        ipad.iter_mut().zip(opad.iter_mut()).zip(key_block.iter())
    {
        *ipad_byte ^= *key_byte;
        *opad_byte ^= *key_byte;
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    let result = outer.finalize();
    let mut mac = [0u8; 32];
    mac.copy_from_slice(&result);
    mac
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (l, r) in left.iter().zip(right.iter()) {
        diff |= l ^ r;
    }
    diff == 0
}

const BASE64URL_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// The masked six-bit index is in `0..=63`: the narrowing is exact and
/// the lookup cannot be out of bounds.
#[expect(
    clippy::as_conversions,
    clippy::indexing_slicing,
    reason = "the index is masked to six bits, so the narrowing is exact and the lookup is in bounds"
)]
fn alphabet_char(index: u32) -> char {
    char::from(BASE64URL_ALPHABET[(index & 0x3f) as usize])
}

fn base64url_nopad_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3).saturating_mul(4));
    for chunk in data.chunks(3) {
        let b0 = chunk.first().copied().unwrap_or(0);
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let triple = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(alphabet_char(triple >> 18));
        out.push(alphabet_char(triple >> 12));
        if chunk.len() > 1 {
            out.push(alphabet_char(triple >> 6));
        }
        if chunk.len() > 2 {
            out.push(alphabet_char(triple));
        }
    }
    out
}

/// Same bounded-by-construction arithmetic as the encoder above.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::integer_division,
    clippy::arithmetic_side_effects,
    reason = "chunk indexing is bounded by chunks(4) and each sextet shifts within 24 bits"
)]
fn base64url_nopad_decode(text: &str) -> Option<Vec<u8>> {
    fn value_of(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    }
    let bytes = text.as_bytes();
    if bytes.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len().saturating_mul(3) / 4);
    for chunk in bytes.chunks(4) {
        let mut accumulator = 0u32;
        for (index, byte) in chunk.iter().enumerate() {
            let value = u32::from(value_of(*byte)?);
            accumulator |= value << (18 - 6 * index);
        }
        out.push((accumulator >> 16) as u8);
        if chunk.len() > 2 {
            out.push((accumulator >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(accumulator as u8);
        }
    }
    Some(out)
}

// Serde surface: the route and aux kinds appear inside diagnostic JSON;
// the envelope itself travels as the opaque token string and has no
// serde impl by design.

impl Serialize for CursorRouteV2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

struct CursorRouteV2Visitor;

impl serde::de::Visitor<'_> for CursorRouteV2Visitor {
    type Value = CursorRouteV2;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a CursorRouteV2 string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        CursorRouteV2::from_wire_str(value).ok_or_else(|| {
            de::Error::unknown_variant(
                value,
                &[
                    "lexical",
                    "symbol",
                    "history",
                    "runtime_metadata",
                    "structural",
                ],
            )
        })
    }
}

impl<'de> Deserialize<'de> for CursorRouteV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        deserializer.deserialize_str(CursorRouteV2Visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CURSOR_ENVELOPE_V2_VERSION, CursorAuxEpochKindV2, CursorAuxEpochV2, CursorBindingV2,
        CursorEnvelopeError, CursorEnvelopeV2, CursorKeyV2, CursorRouteV2, CursorTtlPolicyV2,
        base64url_nopad_decode, base64url_nopad_encode, hmac_sha256,
    };
    use crate::ids::{ManifestGeneration, RepoId, RevisionId};
    use crate::query::GenerationPin;

    fn key() -> CursorKeyV2 {
        CursorKeyV2::new(7, [0x42; 32])
    }

    fn pin(repo: &str, revision: &str, generation: u64) -> GenerationPin {
        GenerationPin::new(
            RepoId::new(repo).expect("static fixture identity"),
            RevisionId::new(revision).expect("static fixture identity"),
            ManifestGeneration::new(generation),
        )
    }

    fn binding() -> CursorBindingV2 {
        CursorBindingV2 {
            route: CursorRouteV2::Lexical,
            pin: pin("repo-a", "rev-a", 12),
            plan_digest: [0x01; 32],
            query_digest: [0x02; 32],
            constraints_digest: [0x03; 32],
            order: "rank_desc".to_string(),
            cap: 50,
            aux_epochs: vec![CursorAuxEpochV2 {
                kind: CursorAuxEpochKindV2::History,
                epoch: 4,
            }],
        }
    }

    fn mint() -> CursorEnvelopeV2 {
        CursorEnvelopeV2::mint(binding(), "boundary-key-9", &key(), 1_000, 1_900)
            .expect("valid mint window")
    }

    #[test]
    fn hmac_matches_rfc4231_vectors() {
        // RFC 4231 test case 1.
        let mac = hmac_sha256(&[0x0b; 20], b"Hi There");
        assert_eq!(
            hex(&mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        // RFC 4231 test case 2.
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex(&mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().fold(String::new(), |mut acc, b| {
            use core::fmt::Write as _;
            // Writing hex digits to a `String` cannot fail; the expect
            // on the fmt Result states that invariant.
            write!(acc, "{b:02x}").expect("writing hex to a String cannot fail");
            acc
        })
    }

    #[test]
    fn base64url_round_trips_without_padding() {
        for sample in [
            &b""[..],
            b"a",
            b"ab",
            b"abc",
            b"abcd",
            &[0xff, 0xfe, 0xfd, 0xfc],
        ] {
            let encoded = base64url_nopad_encode(sample);
            assert!(!encoded.contains('='));
            assert_eq!(base64url_nopad_decode(&encoded).as_deref(), Some(sample));
        }
        assert!(base64url_nopad_decode("!!!").is_none());
        assert!(base64url_nopad_decode("abcde").is_none());
    }

    #[test]
    fn envelope_round_trips_and_binds_context() {
        let envelope = mint();
        let token = envelope.encode(&key()).expect("encode");
        let decoded = CursorEnvelopeV2::decode(&token, &key(), 1_500).expect("valid token");
        assert_eq!(decoded, envelope);
        assert!(decoded.matches(&binding(), key().id()));
    }

    #[test]
    fn tampered_tokens_fail_the_signature_before_use() {
        let envelope = mint();
        let token = envelope.encode(&key()).expect("encode");
        let raw_token = token.as_bytes().to_vec();
        for index in [0usize, 1, 10, 40, raw_token.len().saturating_sub(1)] {
            let mut tampered = raw_token.clone();
            let Some(slot) = tampered.get_mut(index) else {
                panic!("tamper index {index} must exist");
            };
            *slot = if *slot == b'A' { b'B' } else { b'A' };
            let tampered = String::from_utf8(tampered).expect("ascii token");
            match CursorEnvelopeV2::decode(&tampered, &key(), 1_500) {
                Err(
                    CursorEnvelopeError::Signature
                    | CursorEnvelopeError::MalformedToken
                    | CursorEnvelopeError::Version { .. },
                ) => {}
                other => panic!("tampered byte {index} must fail closed, got {other:?}"),
            }
        }
    }

    #[test]
    fn foreign_key_and_key_id_fail_closed() {
        let envelope = mint();
        let token = envelope.encode(&key()).expect("encode");
        let other_key = CursorKeyV2::new(8, [0x99; 32]);
        assert_eq!(
            CursorEnvelopeV2::decode(&token, &other_key, 1_500),
            Err(CursorEnvelopeError::Signature)
        );
        let wrong_id = CursorKeyV2::new(9, [0x42; 32]);
        assert_eq!(
            CursorEnvelopeV2::decode(&token, &wrong_id, 1_500),
            Err(CursorEnvelopeError::Signature)
        );
    }

    #[test]
    fn expiry_and_window_fail_closed() {
        let envelope = mint();
        let token = envelope.encode(&key()).expect("encode");
        assert_eq!(
            CursorEnvelopeV2::decode(&token, &key(), 1_901),
            Err(CursorEnvelopeError::Expired)
        );
        assert_eq!(
            CursorEnvelopeV2::decode(&token, &key(), 2_000),
            Err(CursorEnvelopeError::Expired)
        );
        assert_eq!(
            CursorEnvelopeV2::mint(binding(), "b", &key(), 1_000, 1_000),
            Err(CursorEnvelopeError::InvalidWindow)
        );
    }

    #[test]
    fn every_context_field_participates_in_the_binding_match() {
        let envelope = mint();
        let key_id = key().id();
        let check = |mutated: CursorBindingV2| !envelope.matches(&mutated, key_id);
        let base = binding();
        let mut other_repo = base.clone();
        other_repo.pin = pin("repo-b", "rev-a", 12);
        assert!(check(other_repo));
        let mut other_revision = base.clone();
        other_revision.pin = pin("repo-a", "rev-b", 12);
        assert!(check(other_revision));
        let mut other_generation = base.clone();
        other_generation.pin = pin("repo-a", "rev-a", 13);
        assert!(check(other_generation));
        let mut other_plan = base.clone();
        other_plan.plan_digest = [0xaa; 32];
        assert!(check(other_plan));
        let mut other_query = base.clone();
        other_query.query_digest = [0xbb; 32];
        assert!(check(other_query));
        let mut other_constraints = base.clone();
        other_constraints.constraints_digest = [0xcc; 32];
        assert!(check(other_constraints));
        let mut other_order = base.clone();
        other_order.order = "path_asc".to_string();
        assert!(check(other_order));
        let mut other_cap = base.clone();
        other_cap.cap = 51;
        assert!(check(other_cap));
        let mut other_route = base.clone();
        other_route.route = CursorRouteV2::Symbol;
        assert!(check(other_route));
        let mut other_epoch = base.clone();
        other_epoch.aux_epochs = vec![CursorAuxEpochV2 {
            kind: CursorAuxEpochKindV2::History,
            epoch: 5,
        }];
        assert!(check(other_epoch));
        let mut no_epoch = base;
        no_epoch.aux_epochs = Vec::new();
        assert!(check(no_epoch));
    }

    #[test]
    fn version_byte_is_bound_and_checked() {
        assert_eq!(CURSOR_ENVELOPE_V2_VERSION, 2);
        let envelope = mint();
        let token = envelope.encode(&key()).expect("encode");
        let mut raw = base64url_nopad_decode(&token).expect("valid token");
        // The version byte is MAC-covered: rewriting it must fail.
        if let Some(first) = raw.first_mut() {
            *first = 3;
        }
        let tampered = base64url_nopad_encode(&raw);
        assert_eq!(
            CursorEnvelopeV2::decode(&tampered, &key(), 1_500),
            Err(CursorEnvelopeError::Version { found: 3 })
        );
    }

    #[test]
    fn ttl_policy_clamps_fail_closed_on_zero() {
        let policy = CursorTtlPolicyV2::standard();
        assert_eq!(policy.default_secs, 900);
        assert_eq!(policy.max_secs, 3_600);
        assert_eq!(policy.clamp(0), None);
        assert_eq!(policy.clamp(1), Some(1));
        assert_eq!(policy.clamp(9_999), Some(3_600));
    }
}
