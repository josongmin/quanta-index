//! Durable batch publish acknowledgements, independent of ingest requests.
//! Canonical wire order and journal format version are unchanged.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use super::control::SemanticContentRootsV1;
use crate::ManifestGeneration;

/// The canonical-CBOR format version of a persisted
/// [`BatchPublishReceipt`] in the operation journal (SEP-21 P02B).
///
/// A journal-persisted receipt is stored as this version tag followed by
/// the receipt's canonical CBOR. A reader that finds any other version
/// refuses before any mutation (typed refusal — old receipt / new runtime
/// and new receipt / old runtime are incompatible by design); there is no
/// boot-time dual decoder and no live migration. Receipts of another
/// version are offline-migration input only.
pub const BATCH_PUBLISH_RECEIPT_FORMAT_VERSION: u32 = 1;

/// Server-side receipt for a successful batch publish.
///
/// Receipt truth is generation/materialization scoped, not channel-sequence
/// scoped. The ingest path may internally fan out to multiple storage writes,
/// but the producer-facing ack reports the generation, how many scope
/// mutations were accepted, and — since QI-BB-032 — which idempotency key it
/// answers, whether this call applied the batch or is replaying a durable
/// earlier apply, and the catalog's durable sequence of that apply.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchPublishReceipt {
    pub generation: ManifestGeneration,
    /// The generation's manifest digest for routes that carry one; `None`
    /// for auxiliary routes that name no manifest. Never the batch digest.
    pub manifest_digest: Option<String>,
    /// The batch digest the publish named: the idempotency key this receipt
    /// answers.
    pub batch_digest: String,
    pub accepted_replace_scopes: u32,
    pub accepted_tombstone_scopes: u32,
    /// Search-corpus semantic mutations accepted by the same durable apply.
    /// Auxiliary routes always report zero.
    pub accepted_semantic_replace_scopes: u32,
    pub accepted_semantic_tombstone_scopes: u32,
    pub accepted_clear_surfaces: u32,
    pub sealed: bool,
    /// `true` when this call applied the batch; `false` when the same body
    /// had already been applied and this is the durable receipt of that
    /// apply (a replay ack that mutated nothing).
    pub applied: bool,
    /// The catalog's durable sequence of the apply, unique and monotonic
    /// across the state root. A replay carries the original apply's
    /// sequence, so a producer can prove two receipts describe one apply.
    pub durable_sequence: u64,
    /// The content roots the semantic generation sealed (QI-BB-028):
    /// present exactly when this is a sealed search-corpus receipt, so the
    /// producer can name them when it activates; `None` for an unsealed
    /// publish and for every auxiliary route.
    pub semantic_content: Option<SemanticContentRootsV1>,
}

const BATCH_PUBLISH_RECEIPT_FIELDS: &[&str] = &[
    "generation",
    "manifest_digest",
    "batch_digest",
    "accepted_replace_scopes",
    "accepted_tombstone_scopes",
    "accepted_semantic_replace_scopes",
    "accepted_semantic_tombstone_scopes",
    "accepted_clear_surfaces",
    "sealed",
    "applied",
    "durable_sequence",
    "semantic_content",
];

impl Serialize for BatchPublishReceipt {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("BatchPublishReceipt", 12)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("manifest_digest", &self.manifest_digest)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("accepted_replace_scopes", &self.accepted_replace_scopes)?;
        state.serialize_field("accepted_tombstone_scopes", &self.accepted_tombstone_scopes)?;
        state.serialize_field(
            "accepted_semantic_replace_scopes",
            &self.accepted_semantic_replace_scopes,
        )?;
        state.serialize_field(
            "accepted_semantic_tombstone_scopes",
            &self.accepted_semantic_tombstone_scopes,
        )?;
        state.serialize_field("accepted_clear_surfaces", &self.accepted_clear_surfaces)?;
        state.serialize_field("sealed", &self.sealed)?;
        state.serialize_field("applied", &self.applied)?;
        state.serialize_field("durable_sequence", &self.durable_sequence)?;
        state.serialize_field("semantic_content", &self.semantic_content)?;
        state.end()
    }
}

struct BatchPublishReceiptVisitor;

impl<'de> Visitor<'de> for BatchPublishReceiptVisitor {
    type Value = BatchPublishReceipt;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a BatchPublishReceipt map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut generation: Option<ManifestGeneration> = None;
        let mut manifest_digest: Option<Option<String>> = None;
        let mut batch_digest: Option<String> = None;
        let mut accepted_replace_scopes: Option<u32> = None;
        let mut accepted_tombstone_scopes: Option<u32> = None;
        let mut accepted_semantic_replace_scopes: Option<u32> = None;
        let mut accepted_semantic_tombstone_scopes: Option<u32> = None;
        let mut accepted_clear_surfaces: Option<u32> = None;
        let mut sealed: Option<bool> = None;
        let mut applied: Option<bool> = None;
        let mut durable_sequence: Option<u64> = None;
        let mut semantic_content: Option<Option<SemanticContentRootsV1>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "manifest_digest" => {
                    if manifest_digest.is_some() {
                        return Err(de::Error::duplicate_field("manifest_digest"));
                    }
                    manifest_digest = Some(map.next_value()?);
                }
                "batch_digest" => {
                    if batch_digest.is_some() {
                        return Err(de::Error::duplicate_field("batch_digest"));
                    }
                    batch_digest = Some(map.next_value()?);
                }
                "accepted_replace_scopes" => {
                    if accepted_replace_scopes.is_some() {
                        return Err(de::Error::duplicate_field("accepted_replace_scopes"));
                    }
                    accepted_replace_scopes = Some(map.next_value()?);
                }
                "accepted_tombstone_scopes" => {
                    if accepted_tombstone_scopes.is_some() {
                        return Err(de::Error::duplicate_field("accepted_tombstone_scopes"));
                    }
                    accepted_tombstone_scopes = Some(map.next_value()?);
                }
                "accepted_semantic_replace_scopes" => {
                    if accepted_semantic_replace_scopes.is_some() {
                        return Err(de::Error::duplicate_field(
                            "accepted_semantic_replace_scopes",
                        ));
                    }
                    accepted_semantic_replace_scopes = Some(map.next_value()?);
                }
                "accepted_semantic_tombstone_scopes" => {
                    if accepted_semantic_tombstone_scopes.is_some() {
                        return Err(de::Error::duplicate_field(
                            "accepted_semantic_tombstone_scopes",
                        ));
                    }
                    accepted_semantic_tombstone_scopes = Some(map.next_value()?);
                }
                "accepted_clear_surfaces" => {
                    if accepted_clear_surfaces.is_some() {
                        return Err(de::Error::duplicate_field("accepted_clear_surfaces"));
                    }
                    accepted_clear_surfaces = Some(map.next_value()?);
                }
                "sealed" => {
                    if sealed.is_some() {
                        return Err(de::Error::duplicate_field("sealed"));
                    }
                    sealed = Some(map.next_value()?);
                }
                "applied" => {
                    if applied.is_some() {
                        return Err(de::Error::duplicate_field("applied"));
                    }
                    applied = Some(map.next_value()?);
                }
                "durable_sequence" => {
                    if durable_sequence.is_some() {
                        return Err(de::Error::duplicate_field("durable_sequence"));
                    }
                    durable_sequence = Some(map.next_value()?);
                }
                "semantic_content" => {
                    if semantic_content.is_some() {
                        return Err(de::Error::duplicate_field("semantic_content"));
                    }
                    semantic_content = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        BATCH_PUBLISH_RECEIPT_FIELDS,
                    ));
                }
            }
        }
        Ok(BatchPublishReceipt {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            manifest_digest: manifest_digest
                .ok_or_else(|| de::Error::missing_field("manifest_digest"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            accepted_replace_scopes: accepted_replace_scopes
                .ok_or_else(|| de::Error::missing_field("accepted_replace_scopes"))?,
            accepted_tombstone_scopes: accepted_tombstone_scopes
                .ok_or_else(|| de::Error::missing_field("accepted_tombstone_scopes"))?,
            accepted_semantic_replace_scopes: accepted_semantic_replace_scopes
                .ok_or_else(|| de::Error::missing_field("accepted_semantic_replace_scopes"))?,
            accepted_semantic_tombstone_scopes: accepted_semantic_tombstone_scopes
                .ok_or_else(|| de::Error::missing_field("accepted_semantic_tombstone_scopes"))?,
            accepted_clear_surfaces: accepted_clear_surfaces
                .ok_or_else(|| de::Error::missing_field("accepted_clear_surfaces"))?,
            sealed: sealed.ok_or_else(|| de::Error::missing_field("sealed"))?,
            applied: applied.ok_or_else(|| de::Error::missing_field("applied"))?,
            durable_sequence: durable_sequence
                .ok_or_else(|| de::Error::missing_field("durable_sequence"))?,
            semantic_content: semantic_content
                .ok_or_else(|| de::Error::missing_field("semantic_content"))?,
        })
    }
}

impl<'de> Deserialize<'de> for BatchPublishReceipt {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "BatchPublishReceipt",
            BATCH_PUBLISH_RECEIPT_FIELDS,
            BatchPublishReceiptVisitor,
        )
    }
}

impl BatchPublishReceipt {
    /// An empty receipt for `generation` under `batch_digest`, before any
    /// scope is counted. It reads as applied with no durable sequence yet;
    /// the ingest dispatcher stamps the sequence once the catalog has
    /// recorded the apply, and rewrites `applied` on a replay.
    #[must_use]
    pub fn empty_for(
        generation: ManifestGeneration,
        manifest_digest: Option<String>,
        batch_digest: impl Into<String>,
    ) -> Self {
        Self {
            generation,
            manifest_digest,
            batch_digest: batch_digest.into(),
            accepted_replace_scopes: 0,
            accepted_tombstone_scopes: 0,
            accepted_semantic_replace_scopes: 0,
            accepted_semantic_tombstone_scopes: 0,
            accepted_clear_surfaces: 0,
            sealed: false,
            applied: true,
            durable_sequence: 0,
            semantic_content: None,
        }
    }

    /// Attest the content roots the semantic generation sealed.
    pub fn attest_semantic_content(&mut self, roots: SemanticContentRootsV1) {
        self.semantic_content = Some(roots);
    }

    pub fn accept_replace_scope(&mut self) {
        self.accepted_replace_scopes = self.accepted_replace_scopes.saturating_add(1);
    }

    pub fn accept_tombstone_scope(&mut self) {
        self.accepted_tombstone_scopes = self.accepted_tombstone_scopes.saturating_add(1);
    }

    pub fn accept_semantic_replace_scope(&mut self) {
        self.accepted_semantic_replace_scopes =
            self.accepted_semantic_replace_scopes.saturating_add(1);
    }

    pub fn accept_semantic_tombstone_scope(&mut self) {
        self.accepted_semantic_tombstone_scopes =
            self.accepted_semantic_tombstone_scopes.saturating_add(1);
    }

    pub fn accept_clear_surface(&mut self) {
        self.accepted_clear_surfaces = self.accepted_clear_surfaces.saturating_add(1);
    }

    pub fn mark_sealed(&mut self) {
        self.sealed = true;
    }

    /// The receipt of a fresh apply, stamped with the catalog's sequence.
    #[must_use]
    pub fn recorded_at(mut self, durable_sequence: u64) -> Self {
        self.applied = true;
        self.durable_sequence = durable_sequence;
        self
    }

    /// The receipt of an earlier apply, re-issued for a replay of the same
    /// body: the counts and sequence are the original apply's, `applied`
    /// says this call mutated nothing.
    #[must_use]
    pub fn replayed(mut self) -> Self {
        self.applied = false;
        self
    }
}

impl Default for BatchPublishReceipt {
    /// A receipt for no publish at all: generation zero, no manifest, an
    /// empty batch digest and no sequence. Only a scripted transport in a
    /// test answers with it.
    fn default() -> Self {
        Self::empty_for(ManifestGeneration::ZERO, None, String::new())
    }
}
