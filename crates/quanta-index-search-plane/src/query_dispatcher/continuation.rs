//! Central continuation admission and minting.
//!
//! Public callers only see [`ContinuationTokenV2`]. Route-specific cursor
//! structs are serialized inside the authenticated envelope and exist only
//! after this module has verified signature, expiry, route, request identity,
//! full generation pin, order, cap, and auxiliary epochs.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use quanta_index_contract::{
    ContinuationTokenV2, CursorAuxEpochV2, CursorBindingV2, CursorEnvelopeV2, CursorRouteV2,
    GenerationPin, GenerationSelector, LqQuery, QueryConstraintSetV1, SearchPlaneErrorCodeV2,
};
use quanta_index_core::CoreError;
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use super::cursor_key::CursorKeyStore;

const PLAN_DIGEST_DOMAIN: &[u8] = b"quanta-index-cursor-plan-v2\0";
const CONSTRAINT_DIGEST_DOMAIN: &[u8] = b"quanta-index-cursor-constraints-v2\0";

pub(super) trait CursorClockV2: Send + Sync {
    fn now_unix(&self) -> Result<u64, CoreError>;
}

#[derive(Debug, Default)]
pub(super) struct SystemCursorClockV2;

impl CursorClockV2 for SystemCursorClockV2 {
    fn now_unix(&self) -> Result<u64, CoreError> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .map_err(|error| {
                CoreError::Storage(format!("cursor clock is before Unix epoch: {error}"))
            })
    }
}

#[derive(Clone)]
pub(super) struct CursorAuthorityV2 {
    keys: Arc<CursorKeyStore>,
    clock: Arc<dyn CursorClockV2>,
}

impl CursorAuthorityV2 {
    pub(super) fn process_local() -> Result<Self, CoreError> {
        Ok(Self {
            keys: Arc::new(CursorKeyStore::ephemeral()?),
            clock: Arc::new(SystemCursorClockV2),
        })
    }

    pub(super) fn persistent(keys: CursorKeyStore) -> Self {
        Self {
            keys: Arc::new(keys),
            clock: Arc::new(SystemCursorClockV2),
        }
    }

    /// Authenticate and decode before any route acquires a read view.
    pub(super) fn open<T: DeserializeOwned>(
        &self,
        token: &ContinuationTokenV2,
    ) -> Result<OpenedCursorV2<T>, CoreError> {
        let envelope = self.keys.verify(token.as_str(), self.clock.now_unix()?)?;
        let boundary =
            serde_json::from_str(envelope.boundary()).map_err(|error| CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CursorInvalid,
                message: format!("cursor boundary does not decode for its route: {error}"),
            })?;
        Ok(OpenedCursorV2 { envelope, boundary })
    }

    pub(super) fn require_context<T>(
        &self,
        opened: &OpenedCursorV2<T>,
        context: &CursorRequestContextV2<'_>,
        aux_epochs: Vec<CursorAuxEpochV2>,
    ) -> Result<(), CoreError> {
        let expected = context.binding(aux_epochs)?;
        self.keys.require_binding(&opened.envelope, &expected)
    }

    pub(super) fn mint<T: Serialize>(
        &self,
        boundary: &T,
        context: &CursorRequestContextV2<'_>,
        aux_epochs: Vec<CursorAuxEpochV2>,
    ) -> Result<ContinuationTokenV2, CoreError> {
        let boundary = serde_json::to_string(boundary).map_err(|error| {
            CoreError::InvalidContract(format!("cursor boundary does not encode: {error}"))
        })?;
        let (token, _envelope) = self.keys.mint(
            context.binding(aux_epochs)?,
            &boundary,
            self.clock.now_unix()?,
            None,
        )?;
        ContinuationTokenV2::new(token).map_err(|error| {
            CoreError::InvalidContract(format!("minted cursor token is invalid: {error}"))
        })
    }
}

pub(super) struct OpenedCursorV2<T> {
    envelope: CursorEnvelopeV2,
    pub(super) boundary: T,
}

impl<T> OpenedCursorV2<T> {
    pub(super) fn binding(&self) -> &CursorBindingV2 {
        self.envelope.binding()
    }
}

pub(super) struct CursorRequestContextV2<'a> {
    pub(super) route: CursorRouteV2,
    pub(super) pin: &'a GenerationPin,
    pub(super) query: &'a LqQuery,
    pub(super) constraints: &'a QueryConstraintSetV1,
    pub(super) order: &'a str,
    pub(super) cap: u32,
}

impl CursorRequestContextV2<'_> {
    fn binding(&self, aux_epochs: Vec<CursorAuxEpochV2>) -> Result<CursorBindingV2, CoreError> {
        let query_digest = quanta_index_lq_norm::hasher::canonical_hash(self.query)
            .map_err(|error| CoreError::InvalidContract(format!("cursor query digest: {error}")))?;
        let constraints_digest = digest_serialized(CONSTRAINT_DIGEST_DOMAIN, self.constraints)?;
        let plan_digest = plan_digest(
            self.route,
            self.pin,
            &query_digest,
            &constraints_digest,
            self.order,
            self.cap,
        );
        Ok(CursorBindingV2 {
            route: self.route,
            pin: self.pin.clone(),
            plan_digest,
            query_digest,
            constraints_digest,
            order: self.order.to_string(),
            cap: self.cap,
            aux_epochs,
        })
    }
}

/// A continuation never re-resolves an active selector. The token names the
/// full immutable read identity and the request must pin that exact identity.
pub(super) fn require_token_pin(
    generation: Option<&GenerationPin>,
    selector: Option<&GenerationSelector>,
    token_pin: &GenerationPin,
) -> Result<(), CoreError> {
    if selector.is_none() && generation == Some(token_pin) {
        return Ok(());
    }
    Err(CoreError::Typed {
        code: SearchPlaneErrorCodeV2::CursorContextMismatch,
        message: "a continuation request must pin the exact generation carried by the token"
            .to_string(),
    })
}

fn digest_serialized<T: Serialize>(domain: &[u8], value: &T) -> Result<[u8; 32], CoreError> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        CoreError::InvalidContract(format!("cursor canonical value does not encode: {error}"))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(bytes);
    Ok(hasher.finalize().into())
}

fn plan_digest(
    route: CursorRouteV2,
    pin: &GenerationPin,
    query_digest: &[u8; 32],
    constraints_digest: &[u8; 32],
    order: &str,
    cap: u32,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(PLAN_DIGEST_DOMAIN);
    push_str(&mut hasher, route.as_str());
    push_str(&mut hasher, pin.repo_id.as_str());
    push_str(&mut hasher, pin.revision_id.as_str());
    hasher.update(pin.manifest_generation.get().to_be_bytes());
    hasher.update(query_digest);
    hasher.update(constraints_digest);
    push_str(&mut hasher, order);
    hasher.update(cap.to_be_bytes());
    hasher.finalize().into()
}

fn push_str(hasher: &mut Sha256, value: &str) {
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(value.as_bytes());
}
