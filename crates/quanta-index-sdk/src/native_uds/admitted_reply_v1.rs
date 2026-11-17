//! Type-specific, bounded decoding for one-shot Native publication replies.
//!
//! The IPC layer already owns the admitted raw response body. This decoder
//! accepts only publication variants before asking Serde to decode a payload.

use std::{fmt, io::Cursor};

use quanta_index_contract::{
    index::{
        CatalogView, IngressRef, MapRoot, ObjectRef, OperationReply, OperationStatus,
        OwnerMutationSet, SubmitOutcome, ViewRef,
    },
    native_protocol::{
        MAX_NATIVE_SOURCE_OWNER_LOOKUP, NativeIndexErrorCode, NativeIndexResponse,
        NativeIndexResponseEnvelope, NativeIndexWireError, NativeProjectionOwnerBinding,
        NativeProjectionOwnerLookup, NativeProjectionRead, NativeSourceOwnerBinding,
        NativeSourceOwnerLookup, NativeStubReply,
    },
};
use serde::de::{self, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor};

use super::NativeUdsError;

pub(super) const MAX_PUBLICATION_SOURCE_REFS: usize = super::MAX_NATIVE_STAGE_SOURCES_INPUTS;
const SOURCE_REFS_BOUND_ERROR: &str = "source reference count exceeds native bound";
pub(super) const MAX_PUBLICATION_ERROR_MESSAGE_BYTES: usize = 4096;
// The longest current publication field or enum identifier is shorter than
// 32 bytes. A longer token is not produced by the canonical wire encoder.
const MAX_PUBLICATION_IDENTIFIER_BYTES: usize = 32;
const MAX_PUBLICATION_CBOR_DEPTH: usize = 32;
// The publication variant list has 22 names and 260 identifier bytes.
// Its quotes, separators, prefix and one offending known identifier, as well
// as the smaller wire-error list, fit within 512 diagnostic bytes. No unknown
// identifier reaches Ciborium after the token preflight.
const MAX_PUBLICATION_DIAGNOSTIC_BYTES: usize = 512;
const MAX_STUB_REPLY_BODY_BYTES: usize = 1 << 20;
const MAX_STUB_EDITS: u64 = 64;
const MAX_STUB_REPLACEMENT_BYTES: u64 = 256 << 10;

pub(super) fn decoded_retained_limit() -> Result<u64, NativeUdsError> {
    let references = MAX_PUBLICATION_SOURCE_REFS
        .checked_mul(std::mem::size_of::<ObjectRef>())
        .ok_or_else(|| NativeUdsError::Protocol("publication reply memory overflow".into()))?;
    let owner_rows = MAX_NATIVE_SOURCE_OWNER_LOOKUP
        .checked_mul(std::mem::size_of::<NativeSourceOwnerBinding>())
        .ok_or_else(|| NativeUdsError::Protocol("publication reply memory overflow".into()))?;
    let projection_rows = MAX_NATIVE_SOURCE_OWNER_LOOKUP
        .checked_mul(std::mem::size_of::<NativeProjectionOwnerBinding>())
        .and_then(|value| value.checked_mul(2))
        .and_then(|value| value.checked_add(std::mem::size_of::<NativeProjectionOwnerLookup>()))
        .ok_or_else(|| NativeUdsError::Protocol("projection-owner reply memory overflow".into()))?;
    u64::try_from(
        references
            .max(owner_rows)
            .max(projection_rows)
            .max(MAX_PUBLICATION_ERROR_MESSAGE_BYTES),
    )
    .map_err(|_error| NativeUdsError::Protocol("publication reply memory overflow".into()))
}

pub(super) fn stub_decoded_retained_limit() -> Result<u64, NativeUdsError> {
    let edits = usize::try_from(MAX_STUB_EDITS)
        .map_err(|error| NativeUdsError::Protocol(format!("Stub edit bound overflow: {error}")))?;
    let replacements = usize::try_from(MAX_STUB_REPLACEMENT_BYTES).map_err(|error| {
        NativeUdsError::Protocol(format!("Stub replacement bound overflow: {error}"))
    })?;
    let edit_bytes = edits
        .checked_mul(std::mem::size_of::<
            quanta_index_contract::native_protocol::NativeStubByteEdit,
        >())
        .and_then(|bytes| bytes.checked_mul(2))
        .ok_or_else(|| NativeUdsError::Protocol("Stub decoded charge overflow".into()))?;
    u64::try_from(
        replacements
            .checked_mul(2)
            .and_then(|value| value.checked_add(edit_bytes))
            .and_then(|value| {
                MAX_PUBLICATION_ERROR_MESSAGE_BYTES
                    .checked_add(64)
                    .and_then(|overhead| value.checked_add(overhead))
            })
            .ok_or_else(|| NativeUdsError::Protocol("Stub decoded charge overflow".into()))?,
    )
    .map_err(|error| NativeUdsError::Protocol(format!("Stub decoded charge overflow: {error}")))
}

pub(super) fn parser_temporary_limit() -> Result<u64, NativeUdsError> {
    // One canonical String map/enum identifier can remain live at each
    // recursive level. Every known identifier is <=32 bytes and is copied in
    // one Ciborium scratch chunk; a failure may additionally format one
    // bounded Serde diagnostic. The free-text Error.message is separately
    // precharged as retained output and uses deserialize_str, not String.
    let bytes = MAX_PUBLICATION_CBOR_DEPTH
        .checked_add(1)
        .and_then(|levels| levels.checked_mul(MAX_PUBLICATION_IDENTIFIER_BYTES))
        .and_then(|identifiers| identifiers.checked_add(MAX_PUBLICATION_DIAGNOSTIC_BYTES))
        .ok_or_else(|| NativeUdsError::Protocol("parser memory overflow".into()))?;
    u64::try_from(bytes).map_err(|_error| NativeUdsError::Protocol("parser memory overflow".into()))
}

#[cfg(test)]
pub(super) fn decode(bytes: &[u8]) -> Result<NativeIndexResponseEnvelope, NativeUdsError> {
    decode_with_mode(bytes, false)
}

#[cfg(test)]
pub(super) fn decode_stub(bytes: &[u8]) -> Result<NativeIndexResponseEnvelope, NativeUdsError> {
    if bytes.len() > MAX_STUB_REPLY_BODY_BYTES {
        return Err(NativeUdsError::Protocol(
            "Stub reply exceeds admitted body bound".into(),
        ));
    }
    decode_with_mode(bytes, true)
}

#[cfg(test)]
fn decode_with_mode(
    bytes: &[u8],
    allow_stub: bool,
) -> Result<NativeIndexResponseEnvelope, NativeUdsError> {
    prepare(bytes, allow_stub, false)?.decode()
}

/// Allocation-free preflight result; it borrows the already admitted raw owner.
/// No alternate response representation or second CBOR decoder is introduced.
pub(super) struct PreparedReply<'a> {
    bytes: &'a [u8],
    decoded_bytes: u64,
}

impl PreparedReply<'_> {
    pub(super) fn decoded_retained_limit(&self) -> Result<u64, NativeUdsError> {
        Ok(self.decoded_bytes)
    }

    pub(super) fn decode(self) -> Result<NativeIndexResponseEnvelope, NativeUdsError> {
        let mut reader = Cursor::new(self.bytes);
        let envelope: PublicationEnvelope =
            ciborium::de::from_reader(&mut reader).map_err(|_error| {
                NativeUdsError::Protocol("malformed or unsupported Native publication reply".into())
            })?;
        if usize::try_from(reader.position()).ok() != Some(self.bytes.len()) {
            return Err(NativeUdsError::Protocol(
                "Native publication reply has trailing CBOR".into(),
            ));
        }
        Ok(envelope.0)
    }
}

fn read_size(value: usize) -> Result<u64, NativeUdsError> {
    u64::try_from(value)
        .map_err(|_overflow| NativeUdsError::Protocol("retained read backing overflow".into()))
}

pub(super) fn prepare(
    bytes: &[u8],
    allow_stub: bool,
    allow_read: bool,
) -> Result<PreparedReply<'_>, NativeUdsError> {
    if allow_stub && bytes.len() > MAX_STUB_REPLY_BODY_BYTES {
        return Err(NativeUdsError::Protocol(
            "Stub reply exceeds admitted body bound".into(),
        ));
    }
    let mut scan = CborScan {
        bytes,
        offset: 0,
        allow_stub,
        allow_read,
        stub_replacement_bytes: 0,
        read_decoded_bytes: read_size(std::mem::size_of::<NativeIndexResponseEnvelope>())?,
    };
    scan.item(0, None)?;
    if scan.offset != bytes.len() {
        return Err(NativeUdsError::Protocol(
            "Native publication reply has trailing CBOR".into(),
        ));
    }
    let decoded_bytes = if allow_read {
        // Fixed fields are inline in the envelope. The scan includes every
        // dynamic vector/string, including the canonical Error.message.
        scan.read_decoded_bytes
    } else if allow_stub {
        stub_decoded_retained_limit()?
    } else {
        decoded_retained_limit()?
    };
    Ok(PreparedReply {
        bytes,
        decoded_bytes,
    })
}

#[cfg(test)]
fn preflight_canonical_cbor(bytes: &[u8], allow_stub: bool) -> Result<(), NativeUdsError> {
    prepare(bytes, allow_stub, false).map(|_| ())
}

struct CborScan<'a> {
    bytes: &'a [u8],
    offset: usize,
    allow_stub: bool,
    allow_read: bool,
    read_decoded_bytes: u64,
    stub_replacement_bytes: u64,
}

impl<'a> CborScan<'a> {
    fn byte(&mut self) -> Result<u8, NativeUdsError> {
        let byte =
            self.bytes.get(self.offset).copied().ok_or_else(|| {
                NativeUdsError::Protocol("truncated Native publication CBOR".into())
            })?;
        self.offset = self
            .offset
            .checked_add(1)
            .ok_or_else(|| NativeUdsError::Protocol("CBOR offset overflow".into()))?;
        Ok(byte)
    }

    fn take(&mut self, length: u64) -> Result<&'a [u8], NativeUdsError> {
        let length = usize::try_from(length)
            .map_err(|_error| NativeUdsError::Protocol("CBOR length overflow".into()))?;
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| NativeUdsError::Protocol("CBOR offset overflow".into()))?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| NativeUdsError::Protocol("truncated Native publication CBOR".into()))?;
        self.offset = end;
        Ok(bytes)
    }

    fn argument(&mut self, additional: u8) -> Result<u64, NativeUdsError> {
        match additional {
            0..=23 => Ok(u64::from(additional)),
            24 => self.byte().map(u64::from),
            25 => {
                let bytes = self.take(2)?;
                let array: [u8; 2] = bytes.try_into().map_err(|_error| {
                    NativeUdsError::Protocol("invalid CBOR integer width".into())
                })?;
                Ok(u64::from(u16::from_be_bytes(array)))
            }
            26 => {
                let bytes = self.take(4)?;
                let array: [u8; 4] = bytes.try_into().map_err(|_error| {
                    NativeUdsError::Protocol("invalid CBOR integer width".into())
                })?;
                Ok(u64::from(u32::from_be_bytes(array)))
            }
            27 => {
                let bytes = self.take(8)?;
                let array: [u8; 8] = bytes.try_into().map_err(|_error| {
                    NativeUdsError::Protocol("invalid CBOR integer width".into())
                })?;
                Ok(u64::from_be_bytes(array))
            }
            28..=31 => Err(NativeUdsError::Protocol(
                "Native publication CBOR must use definite valid lengths".into(),
            )),
            _ => Err(NativeUdsError::Protocol("invalid CBOR argument".into())),
        }
    }

    fn map_key(&mut self) -> Result<&'a str, NativeUdsError> {
        let header = self.byte()?;
        if header >> 5 != 3 {
            return Err(NativeUdsError::Protocol(
                "Native publication CBOR map key is not text".into(),
            ));
        }
        let length = self.argument(header & 0x1f)?;
        if length
            > u64::try_from(MAX_PUBLICATION_IDENTIFIER_BYTES)
                .map_err(|_error| NativeUdsError::Protocol("identifier bound overflow".into()))?
        {
            return Err(NativeUdsError::Protocol(
                "Native publication identifier exceeds 32 bytes".into(),
            ));
        }
        let key = std::str::from_utf8(self.take(length)?)
            .map_err(|_error| NativeUdsError::Protocol("invalid CBOR map key UTF-8".into()))?;
        if !known_identifier(key) {
            return Err(NativeUdsError::Protocol(
                "unknown Native publication CBOR identifier".into(),
            ));
        }
        if !self.allow_read && read_identifier(key) {
            return Err(NativeUdsError::Protocol(
                "retained read is outside admitted publication reply".into(),
            ));
        }
        if !self.allow_stub && matches!(key, "Stub" | "Prepared") {
            return Err(NativeUdsError::Protocol(
                "Stub is outside admitted publication reply".into(),
            ));
        }
        Ok(key)
    }

    fn item(&mut self, depth: usize, field: Option<&str>) -> Result<(), NativeUdsError> {
        if depth > MAX_PUBLICATION_CBOR_DEPTH {
            return Err(NativeUdsError::Protocol(
                "Native publication CBOR nesting exceeds contract".into(),
            ));
        }
        let header = self.byte()?;
        let major = header >> 5;
        let value = self.argument(header & 0x1f)?;
        match major {
            0 | 1 => Ok(()),
            2 if self.allow_stub && field == Some("replacement") => {
                self.charge_stub_replacement(value)?;
                let _skipped = self.take(value)?;
                Ok(())
            }
            2 => Err(NativeUdsError::Protocol(
                "Native publication CBOR byte strings are outside the canonical wire".into(),
            )),
            3 => {
                let limit = if field == Some("message") {
                    MAX_PUBLICATION_ERROR_MESSAGE_BYTES
                } else if self.allow_read && matches!(field, Some("repo_id" | "revision_id")) {
                    quanta_index_contract::RepoId::MAX_UTF8_BYTES_V1
                } else if self.allow_read && field == Some("path") {
                    super::MAX_SOURCE_PATH_BYTES
                } else if self.allow_stub && field == Some("language") {
                    64
                } else {
                    MAX_PUBLICATION_IDENTIFIER_BYTES
                };
                if value
                    > u64::try_from(limit)
                        .map_err(|_error| NativeUdsError::Protocol("text bound overflow".into()))?
                {
                    return Err(NativeUdsError::Protocol(
                        "Native publication text exceeds its typed bound".into(),
                    ));
                }
                if self.allow_read
                    && matches!(field, Some("message" | "path" | "repo_id" | "revision_id"))
                {
                    self.charge_read(value.max(8), 1)?;
                }
                let text = self.take(value)?;
                if field != Some("message")
                    && field != Some("language")
                    && !(self.allow_read
                        && matches!(field, Some("path" | "repo_id" | "revision_id")))
                {
                    let identifier = std::str::from_utf8(text).map_err(|_error| {
                        NativeUdsError::Protocol("invalid CBOR identifier UTF-8".into())
                    })?;
                    if !known_identifier(identifier)
                        || (!self.allow_stub && matches!(identifier, "Stub" | "Prepared"))
                        || (!self.allow_read && read_identifier(identifier))
                    {
                        return Err(NativeUdsError::Protocol(
                            "unknown Native publication CBOR identifier".into(),
                        ));
                    }
                }
                Ok(())
            }
            4 => {
                if self.allow_read {
                    self.read_array(field, value)?;
                }
                if self.allow_stub && field == Some("edits") && value > MAX_STUB_EDITS {
                    return Err(NativeUdsError::Protocol(
                        "Stub edit count exceeds bound".into(),
                    ));
                }
                if self.allow_stub && field == Some("replacement") {
                    self.charge_stub_replacement(value)?;
                }
                if value
                    > u64::try_from(self.bytes.len().saturating_sub(self.offset))
                        .map_err(|_error| NativeUdsError::Protocol("array bound overflow".into()))?
                {
                    return Err(NativeUdsError::Protocol("CBOR array exceeds frame".into()));
                }
                let next = depth
                    .checked_add(1)
                    .ok_or_else(|| NativeUdsError::Protocol("CBOR depth overflow".into()))?;
                for _item in 0..value {
                    self.item(next, None)?;
                }
                Ok(())
            }
            5 => {
                let remaining = self.bytes.len().saturating_sub(self.offset);
                let minimum = value
                    .checked_mul(2)
                    .ok_or_else(|| NativeUdsError::Protocol("map bound overflow".into()))?;
                if minimum
                    > u64::try_from(remaining)
                        .map_err(|_error| NativeUdsError::Protocol("map bound overflow".into()))?
                {
                    return Err(NativeUdsError::Protocol("CBOR map exceeds frame".into()));
                }
                let next = depth
                    .checked_add(1)
                    .ok_or_else(|| NativeUdsError::Protocol("CBOR depth overflow".into()))?;
                for _entry in 0..value {
                    let key = self.map_key()?;
                    self.item(next, Some(key))?;
                }
                Ok(())
            }
            6 => Err(NativeUdsError::Protocol(
                "Native publication CBOR tag is outside the canonical wire".into(),
            )),
            7 if header & 0x1f <= 27 => Ok(()),
            7 => Err(NativeUdsError::Protocol("invalid CBOR simple value".into())),
            _ => Err(NativeUdsError::Protocol("invalid CBOR major type".into())),
        }
    }

    fn charge_read(&mut self, count: u64, size: usize) -> Result<(), NativeUdsError> {
        // Serde's Vec cautious size hint may grow geometrically. Include its
        // spare capacity, and String/byte-vector minimum allocation capacity.
        let bytes = count
            .checked_mul(read_size(size)?)
            .and_then(|bytes| bytes.checked_mul(2))
            .ok_or_else(|| NativeUdsError::Protocol("retained read backing overflow".into()))?;
        self.read_decoded_bytes = self
            .read_decoded_bytes
            .checked_add(bytes)
            .ok_or_else(|| NativeUdsError::Protocol("retained read backing overflow".into()))?;
        Ok(())
    }

    fn read_array(&mut self, field: Option<&str>, count: u64) -> Result<(), NativeUdsError> {
        use quanta_index_contract::{
            index::InstanceSpec,
            native_protocol::{
                MAX_NATIVE_READ_PROFILE_INSTANCES, MAX_NATIVE_SEMANTIC_TOP_K,
                MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES, MAX_NATIVE_TEXT_TOP_K, NativeSemanticFileHit,
                NativeTextHit,
            },
        };
        let (limit, size) = match field {
            Some("instances") => (
                read_size(MAX_NATIVE_READ_PROFILE_INSTANCES)?,
                std::mem::size_of::<InstanceSpec>(),
            ),
            // Text/Page rows and fused rows share the hits field; the largest
            // canonical row is conservative for each of those schemas.
            Some("hits" | "lexical_pool") => (
                u64::from(MAX_NATIVE_TEXT_TOP_K),
                std::mem::size_of::<NativeTextHit>().max(std::mem::size_of::<
                    quanta_index_contract::native_protocol::NativeHybridFileHit,
                >()),
            ),
            Some("dense_pool") => (
                u64::from(MAX_NATIVE_SEMANTIC_TOP_K),
                std::mem::size_of::<NativeSemanticFileHit>(),
            ),
            Some("vector_id") => (read_size(MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES)?, 1),
            // Canonical fixed-size IDs/roots are inline fields, not heaps.
            Some(
                "session" | "query" | "tenant" | "collection" | "worktree" | "slot" | "producer"
                | "stable_id" | "digest" | "profile" | "owners" | "paths" | "frontiers"
                | "projections" | "members" | "coverage" | "root" | "artifact" | "lexical_artifact"
                | "source" | "instance" | "text_instance" | "semantic_instance" | "kind" | "group"
                | "schema" | "policy" | "format" | "projection" | "context" | "id" | "payload"
                | "input_digest" | "searchable" | "producer_context" | "derivation_policy"
                | "missing" | "previous_missing" | "member" | "snapshot_digest" | "replay_start"
                | "source_input" | "original" | "repo_commit_recency" | "file_contributor"
                | "predecessor_context",
            ) => (32, 0),
            _ => {
                return Err(NativeUdsError::Protocol(
                    "noncanonical retained read array".into(),
                ));
            }
        };
        if count > limit {
            return Err(NativeUdsError::Protocol(
                "retained read array exceeds typed bound".into(),
            ));
        }
        if size != 0 {
            self.charge_read(
                if field == Some("vector_id") {
                    count.max(8)
                } else {
                    count
                },
                size,
            )?;
        }
        Ok(())
    }

    fn charge_stub_replacement(&mut self, value: u64) -> Result<(), NativeUdsError> {
        if value > 64 << 10 {
            return Err(NativeUdsError::Protocol(
                "Stub replacement exceeds per-edit bound".into(),
            ));
        }
        self.stub_replacement_bytes = self
            .stub_replacement_bytes
            .checked_add(value)
            .ok_or_else(|| NativeUdsError::Protocol("Stub replacement size overflow".into()))?;
        if self.stub_replacement_bytes > MAX_STUB_REPLACEMENT_BYTES {
            return Err(NativeUdsError::Protocol(
                "Stub replacement total exceeds bound".into(),
            ));
        }
        Ok(())
    }
}

fn read_identifier(value: &str) -> bool {
    matches!(
        value,
        "ReadSession"
            | "ReadSessionProfile"
            | "HistoryCut"
            | "HistoryGeneration"
            | "TextRanked"
            | "TextRankedPage"
            | "HybridFileRanked"
    )
}

fn known_identifier(value: &str) -> bool {
    matches!(
        value,
        "ReadSession"
            | "ReadSessionProfile"
            | "HistoryCut"
            | "HistoryGeneration"
            | "member"
            | "source_input"
            | "predecessor_context"
            | "inputs"
            | "original"
            | "repo_commit_recency"
            | "file_contributor"
            | "cut"
            | "generation"
            | "repo_id"
            | "revision_id"
            | "durable_epoch"
            | "snapshot_digest"
            | "replay_start"
            | "TextRanked"
            | "TextRankedPage"
            | "HybridFileRanked"
            | "Released"
            | "Settlement"
            | "Active"
            | "Closing"
            | "SettledAtSnapshot"
            | "Incomplete"
            | "expires_at_ms"
            | "state"
            | "status"
            | "has_remaining_target_pins"
            | "head"
            | "exact_floor"
            | "instances"
            | "instance"
            | "kind"
            | "group"
            | "activation"
            | "input_domain"
            | "policy"
            | "format"
            | "CoreRequired"
            | "AsyncCoverage"
            | "SourceLocal"
            | "matched"
            | "exhaustive"
            | "hits"
            | "path"
            | "score"
            | "artifact"
            | "remaining"
            | "after"
            | "query"
            | "text_instance"
            | "semantic_instance"
            | "lexical_artifact"
            | "lexical_matched"
            | "semantic_coverage"
            | "semantic_traversal"
            | "semantic_exhaustive"
            | "scanned_owners"
            | "skipped_stale"
            | "file_top_k"
            | "lexical_pool"
            | "dense_pool"
            | "projection_owner"
            | "vector_id"
            | "chunk"
            | "lexical_rank"
            | "dense_rank"
            | "desired"
            | "covered"
            | "searchable"
            | "Complete"
            | "Partial"
            | "previous_missing"
            | "missing"
            | "context"
            | "ExactExhausted"
            | "ExactCapped"
            | "Approximate"
            | "capped"
            | "id"
            | "input"
            | "producer_context"
            | "derivation_policy"
            | "input_digest"
            | "span"
            | "SourcePayload"
            | "ProjectionPayload"
            | "RenderedMember"
            | "start_byte"
            | "end_byte"
            | "ordinal"
            | "request_id"
            | "payload"
            | "Created"
            | "Staged"
            | "Submitted"
            | "View"
            | "PublishedViewMetadata"
            | "RootScopedView"
            | "root_nonce"
            | "root_device"
            | "root_inode"
            | "Status"
            | "Error"
            | "InputOpened"
            | "MutationPageStaged"
            | "MutationsStaged"
            | "InputDiscarded"
            | "StagedSources"
            | "SourceOwners"
            | "Projection"
            | "ProjectionOwners"
            | "ProjectionSourceDiff"
            | "expected"
            | "next"
            | "has_more"
            | "contribution"
            | "Stub"
            | "Prepared"
            | "document"
            | "session"
            | "publisher"
            | "revision"
            | "selected"
            | "language"
            | "selected_id"
            | "edits"
            | "target_id"
            | "start"
            | "end"
            | "replacement"
            | "binding"
            | "projection"
            | "schema"
            | "producer_policy"
            | "dependency_context"
            | "certificate"
            | "view"
            | "rows"
            | "owner"
            | "stable_id"
            | "workspace"
            | "epoch"
            | "sequence"
            | "digest"
            | "tenant"
            | "collection"
            | "worktree"
            | "slot"
            | "producer"
            | "stream"
            | "stream_epoch"
            | "operation"
            | "attempt"
            | "fence"
            | "root"
            | "count"
            | "identity"
            | "profile"
            | "source"
            | "projections"
            | "members"
            | "coverage"
            | "owners"
            | "paths"
            | "frontiers"
            | "code"
            | "message"
            | "Accepted"
            | "Terminal"
            | "Unknown"
            | "Pending"
            | "Published"
            | "Refused"
            | "Conflict"
            | "FenceLost"
            | "InvalidInput"
            | "ResourceLimited"
            | "BackendViolation"
            | "RebuildRequired"
            | "MaintenanceRequired"
            | "Denied"
            | "DigestConflict"
            | "Cancelled"
            | "SnapshotUnavailable"
            | "CorruptObject"
            | "Storage"
            | "Overloaded"
            | "ResponseTooLarge"
            | "CapabilityUnavailable"
            | "FocusSubjectNotFound"
    )
}

struct PublicationEnvelope(NativeIndexResponseEnvelope);

enum EnvelopeField {
    RequestId,
    Payload,
}

impl<'de> serde::Deserialize<'de> for EnvelopeField {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FieldVisitor;
        impl Visitor<'_> for FieldVisitor {
            type Value = EnvelopeField;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("request_id or payload")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                match value {
                    "request_id" => Ok(EnvelopeField::RequestId),
                    "payload" => Ok(EnvelopeField::Payload),
                    _ => Err(E::unknown_field(value, &["request_id", "payload"])),
                }
            }
        }
        deserializer.deserialize_identifier(FieldVisitor)
    }
}

impl<'de> serde::Deserialize<'de> for PublicationEnvelope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EnvelopeVisitor;
        impl<'de> Visitor<'de> for EnvelopeVisitor {
            type Value = PublicationEnvelope;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("closed Native publication reply envelope")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut request_id = None;
                let mut payload = None;
                while let Some(field) = map.next_key::<EnvelopeField>()? {
                    match field {
                        EnvelopeField::RequestId => {
                            if request_id.is_some() {
                                return Err(de::Error::duplicate_field("request_id"));
                            }
                            request_id = Some(map.next_value::<u64>()?);
                        }
                        EnvelopeField::Payload => {
                            if payload.is_some() {
                                return Err(de::Error::duplicate_field("payload"));
                            }
                            payload = Some(map.next_value::<PublicationPayload>()?.0);
                        }
                    }
                }
                Ok(PublicationEnvelope(NativeIndexResponseEnvelope {
                    request_id: request_id.ok_or_else(|| de::Error::missing_field("request_id"))?,
                    payload: payload.ok_or_else(|| de::Error::missing_field("payload"))?,
                }))
            }
        }
        deserializer.deserialize_struct(
            "NativeIndexResponseEnvelope",
            &["request_id", "payload"],
            EnvelopeVisitor,
        )
    }
}

struct PublicationPayload(NativeIndexResponse);

enum PublicationVariant {
    Created,
    Staged,
    Submitted,
    View,
    PublishedViewMetadata,
    RootScopedView,
    Status,
    Error,
    InputOpened,
    MutationPageStaged,
    MutationsStaged,
    InputDiscarded,
    StagedSources,
    SourceOwners,
    Projection,
    ProjectionOwners,
    ProjectionSourceDiff,
    ReadSession,
    ReadSessionProfile,
    HistoryCut,
    HistoryGeneration,
    TextRanked,
    TextRankedPage,
    HybridFileRanked,
    Stub,
}

const PUBLICATION_VARIANTS: &[&str] = &[
    "Created",
    "Staged",
    "Submitted",
    "View",
    "PublishedViewMetadata",
    "RootScopedView",
    "Status",
    "Error",
    "InputOpened",
    "MutationPageStaged",
    "MutationsStaged",
    "InputDiscarded",
    "StagedSources",
    "SourceOwners",
    "Projection",
    "ProjectionOwners",
    "ProjectionSourceDiff",
    "ReadSession",
    "ReadSessionProfile",
    "HistoryCut",
    "HistoryGeneration",
    "TextRanked",
    "TextRankedPage",
    "HybridFileRanked",
    "Stub",
];

impl<'de> serde::Deserialize<'de> for PublicationVariant {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct VariantVisitor;
        impl Visitor<'_> for VariantVisitor {
            type Value = PublicationVariant;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("Native publication response variant")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                match value {
                    "Created" => Ok(PublicationVariant::Created),
                    "Staged" => Ok(PublicationVariant::Staged),
                    "Submitted" => Ok(PublicationVariant::Submitted),
                    "View" => Ok(PublicationVariant::View),
                    "PublishedViewMetadata" => Ok(PublicationVariant::PublishedViewMetadata),
                    "RootScopedView" => Ok(PublicationVariant::RootScopedView),
                    "Status" => Ok(PublicationVariant::Status),
                    "Error" => Ok(PublicationVariant::Error),
                    "InputOpened" => Ok(PublicationVariant::InputOpened),
                    "MutationPageStaged" => Ok(PublicationVariant::MutationPageStaged),
                    "MutationsStaged" => Ok(PublicationVariant::MutationsStaged),
                    "InputDiscarded" => Ok(PublicationVariant::InputDiscarded),
                    "StagedSources" => Ok(PublicationVariant::StagedSources),
                    "SourceOwners" => Ok(PublicationVariant::SourceOwners),
                    "Projection" => Ok(PublicationVariant::Projection),
                    "ProjectionOwners" => Ok(PublicationVariant::ProjectionOwners),
                    "ProjectionSourceDiff" => Ok(PublicationVariant::ProjectionSourceDiff),
                    "ReadSession" => Ok(PublicationVariant::ReadSession),
                    "ReadSessionProfile" => Ok(PublicationVariant::ReadSessionProfile),
                    "HistoryCut" => Ok(PublicationVariant::HistoryCut),
                    "HistoryGeneration" => Ok(PublicationVariant::HistoryGeneration),
                    "TextRanked" => Ok(PublicationVariant::TextRanked),
                    "TextRankedPage" => Ok(PublicationVariant::TextRankedPage),
                    "HybridFileRanked" => Ok(PublicationVariant::HybridFileRanked),
                    "Stub" => Ok(PublicationVariant::Stub),
                    _ => Err(E::unknown_variant(value, PUBLICATION_VARIANTS)),
                }
            }
        }
        deserializer.deserialize_identifier(VariantVisitor)
    }
}

impl<'de> serde::Deserialize<'de> for PublicationPayload {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PayloadVisitor;
        impl<'de> Visitor<'de> for PayloadVisitor {
            type Value = PublicationPayload;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("bounded Native publication response")
            }

            fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<Self::Value, A::Error> {
                let (tag, variant) = data.variant::<PublicationVariant>()?;
                let response = match tag {
                    PublicationVariant::Created => {
                        NativeIndexResponse::Created(variant.newtype_variant::<ViewRef>()?)
                    }
                    PublicationVariant::Staged => {
                        NativeIndexResponse::Staged(variant.newtype_variant::<ObjectRef>()?)
                    }
                    PublicationVariant::Submitted => NativeIndexResponse::Submitted(
                        variant.newtype_variant::<OperationReply<SubmitOutcome>>()?,
                    ),
                    PublicationVariant::Projection => NativeIndexResponse::Projection(
                        variant.newtype_variant::<NativeProjectionRead>()?,
                    ),
                    PublicationVariant::ProjectionOwners => NativeIndexResponse::ProjectionOwners(
                        variant.newtype_variant::<NativeProjectionOwnerLookup>()?,
                    ),
                    PublicationVariant::ProjectionSourceDiff => NativeIndexResponse::ProjectionSourceDiff(
                        variant.newtype_variant::<quanta_index_contract::native_protocol::NativeProjectionSourceDiff>()?,
                    ),
                    PublicationVariant::ReadSession => NativeIndexResponse::ReadSession(variant.newtype_variant::<quanta_index_contract::native_protocol::NativeReadSessionReply>()?),
                    PublicationVariant::ReadSessionProfile => NativeIndexResponse::ReadSessionProfile(variant.newtype_variant::<quanta_index_contract::native_protocol::NativeReadSessionProfile>()?),
                    PublicationVariant::HistoryCut => NativeIndexResponse::HistoryCut(variant.newtype_variant::<quanta_index_contract::native_protocol::NativeReadSessionHistoryCutV1>()?),
                    PublicationVariant::HistoryGeneration => NativeIndexResponse::HistoryGeneration(variant.newtype_variant::<quanta_index_contract::native_protocol::NativeReadSessionHistoryGenerationV1>()?),
                    PublicationVariant::TextRanked => NativeIndexResponse::TextRanked(variant.newtype_variant::<quanta_index_contract::native_protocol::NativeTextResult>()?),
                    PublicationVariant::TextRankedPage => NativeIndexResponse::TextRankedPage(variant.newtype_variant::<quanta_index_contract::native_protocol::NativeTextPage>()?),
                    PublicationVariant::HybridFileRanked => NativeIndexResponse::HybridFileRanked(variant.newtype_variant::<quanta_index_contract::native_protocol::NativeHybridFileResult>()?),
                    PublicationVariant::Stub => {
                        NativeIndexResponse::Stub(variant.newtype_variant::<NativeStubReply>()?)
                    }
                    PublicationVariant::PublishedViewMetadata => NativeIndexResponse::PublishedViewMetadata(
                        variant.newtype_variant::<OperationReply<CatalogView>>()?,
                    ),
                    PublicationVariant::RootScopedView => NativeIndexResponse::RootScopedView(
                        variant.newtype_variant::<quanta_index_contract::native_protocol::NativeRootScopedViewV1>()?,
                    ),
                    PublicationVariant::View => {
                        NativeIndexResponse::View(variant.newtype_variant::<CatalogView>()?)
                    }
                    PublicationVariant::Status => NativeIndexResponse::Status(
                        variant.newtype_variant::<OperationReply<OperationStatus>>()?,
                    ),
                    PublicationVariant::Error => {
                        NativeIndexResponse::Error(variant.newtype_variant::<BoundedError>()?.0)
                    }
                    PublicationVariant::InputOpened => {
                        NativeIndexResponse::InputOpened(variant.newtype_variant::<IngressRef>()?)
                    }
                    PublicationVariant::MutationPageStaged => {
                        NativeIndexResponse::MutationPageStaged(
                            variant.newtype_variant::<MapRoot>()?,
                        )
                    }
                    PublicationVariant::MutationsStaged => NativeIndexResponse::MutationsStaged(
                        variant.newtype_variant::<OwnerMutationSet>()?,
                    ),
                    PublicationVariant::InputDiscarded => NativeIndexResponse::InputDiscarded(
                        variant.newtype_variant::<IngressRef>()?,
                    ),
                    PublicationVariant::StagedSources => NativeIndexResponse::StagedSources(
                        variant.newtype_variant::<BoundedSourceRefs>()?.0,
                    ),
                    // The canonical row owner enforces a definite, ordered,
                    // unique table of at most 128 fixed-size bindings. Its
                    // backing is covered by decoded_retained_limit before
                    // this decoder is entered.
                    PublicationVariant::SourceOwners => NativeIndexResponse::SourceOwners(
                        variant.newtype_variant::<NativeSourceOwnerLookup>()?,
                    ),
                };
                Ok(PublicationPayload(response))
            }
        }
        deserializer.deserialize_enum("NativeIndexResponse", PUBLICATION_VARIANTS, PayloadVisitor)
    }
}

struct BoundedSourceRefs(Vec<ObjectRef>);

impl<'de> serde::Deserialize<'de> for BoundedSourceRefs {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RefsVisitor;
        impl<'de> Visitor<'de> for RefsVisitor {
            type Value = BoundedSourceRefs;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(
                    f,
                    "at most {MAX_PUBLICATION_SOURCE_REFS} source object references"
                )
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                if seq
                    .size_hint()
                    .is_some_and(|count| count > MAX_PUBLICATION_SOURCE_REFS)
                {
                    return Err(de::Error::custom(SOURCE_REFS_BOUND_ERROR));
                }
                let mut references = Vec::new();
                references
                    .try_reserve_exact(MAX_PUBLICATION_SOURCE_REFS)
                    .map_err(|_error| de::Error::custom("source reference allocation refused"))?;
                while references.len() < MAX_PUBLICATION_SOURCE_REFS {
                    let Some(reference) = seq.next_element::<ObjectRef>()? else {
                        return Ok(BoundedSourceRefs(references));
                    };
                    references.push(reference);
                }
                if seq.next_element_seed(RejectExtraElement)?.is_some() {
                    return Err(de::Error::custom(SOURCE_REFS_BOUND_ERROR));
                }
                Ok(BoundedSourceRefs(references))
            }
        }
        deserializer.deserialize_seq(RefsVisitor)
    }
}

struct RejectExtraElement;

impl<'de> de::DeserializeSeed<'de> for RejectExtraElement {
    type Value = ();

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        _deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        Err(de::Error::custom(SOURCE_REFS_BOUND_ERROR))
    }
}

struct BoundedError(NativeIndexWireError);

enum ErrorField {
    Code,
    Message,
}

impl<'de> serde::Deserialize<'de> for ErrorField {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FieldVisitor;
        impl Visitor<'_> for FieldVisitor {
            type Value = ErrorField;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("code or message")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                match value {
                    "code" => Ok(ErrorField::Code),
                    "message" => Ok(ErrorField::Message),
                    _ => Err(E::unknown_field(value, &["code", "message"])),
                }
            }
        }
        deserializer.deserialize_identifier(FieldVisitor)
    }
}

struct BoundedMessage(String);

impl<'de> serde::Deserialize<'de> for BoundedMessage {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MessageVisitor;
        impl Visitor<'_> for MessageVisitor {
            type Value = BoundedMessage;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("Native error message of at most 4096 UTF-8 bytes")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                if value.len() > MAX_PUBLICATION_ERROR_MESSAGE_BYTES {
                    return Err(E::custom("Native error message exceeds 4096 bytes"));
                }
                let mut message = String::new();
                message
                    .try_reserve_exact(value.len())
                    .map_err(|_error| E::custom("Native error message allocation refused"))?;
                message.push_str(value);
                Ok(BoundedMessage(message))
            }
        }
        deserializer.deserialize_str(MessageVisitor)
    }
}

impl<'de> serde::Deserialize<'de> for BoundedError {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ErrorVisitor;
        impl<'de> Visitor<'de> for ErrorVisitor {
            type Value = BoundedError;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("closed bounded Native error")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut code = None;
                let mut message = None;
                while let Some(field) = map.next_key::<ErrorField>()? {
                    match field {
                        ErrorField::Code => {
                            if code.is_some() {
                                return Err(de::Error::duplicate_field("code"));
                            }
                            code = Some(map.next_value::<NativeIndexErrorCode>()?);
                        }
                        ErrorField::Message => {
                            if message.is_some() {
                                return Err(de::Error::duplicate_field("message"));
                            }
                            message = Some(map.next_value::<BoundedMessage>()?.0);
                        }
                    }
                }
                Ok(BoundedError(NativeIndexWireError {
                    code: code.ok_or_else(|| de::Error::missing_field("code"))?,
                    message: message.ok_or_else(|| de::Error::missing_field("message"))?,
                }))
            }
        }
        deserializer.deserialize_struct("NativeIndexWireError", &["code", "message"], ErrorVisitor)
    }
}

#[cfg(test)]
mod tests {
    use quanta_index_contract::{
        index::{
            CollectionId, Digest, OperationKey, OwnerKey, OwnerMutationSet, ProducerId,
            ProjectionId, SlotId, StableOwnerId, StreamId, TenantId, ViewRef, WorkspaceKey,
            WorktreeId,
        },
        native_protocol::{
            NativeIndexErrorCode, NativeIndexResponse, NativeInteractionPublisherId,
            NativeOverlayDocument, NativeOverlaySession, NativeProjectionOwnerRows,
            NativeStubBinding, NativeStubByteEdit,
        },
    };

    use super::*;

    fn request_id() -> u64 {
        41
    }

    fn operation() -> OperationKey {
        OperationKey {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 5,
            producer: ProducerId([6; 16]),
            stream: StreamId([7; 16]),
            stream_epoch: 8,
            sequence: 9,
        }
    }

    fn encoded(response: NativeIndexResponse) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let envelope = NativeIndexResponseEnvelope {
            request_id: request_id(),
            payload: response,
        };
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&envelope, &mut bytes)?;
        Ok(bytes)
    }

    #[test]
    fn admitted_stub_decoder_retains_exact_prepared_payload_and_bounds_replacements()
    -> Result<(), Box<dyn std::error::Error>> {
        let operation = operation();
        let binding = NativeStubBinding {
            document: NativeOverlayDocument {
                session: NativeOverlaySession {
                    workspace: operation.workspace,
                    publisher: NativeInteractionPublisherId([1; 32]),
                    epoch: 1,
                },
                owner: OwnerKey {
                    producer: operation.producer,
                    stable_id: StableOwnerId([2; 16]),
                },
            },
            revision: 1,
            selected: ViewRef {
                workspace: operation.workspace,
                epoch: operation.epoch,
                sequence: 1,
                digest: Digest([3; 32]),
            },
            source: ObjectRef(Digest([4; 32])),
            language: "rust".into(),
        };
        let reply = NativeStubReply::Prepared {
            binding,
            selected_id: Digest([5; 32]),
            edits: vec![NativeStubByteEdit {
                target_id: Digest([5; 32]),
                start: 0,
                end: 1,
                replacement: b"x".to_vec(),
            }],
        };
        let bytes = encoded(NativeIndexResponse::Stub(reply.clone()))?;
        if decode_stub(&bytes)?.payload != NativeIndexResponse::Stub(reply.clone()) {
            return Err("admitted Stub payload changed".into());
        }
        if decode(&bytes).is_ok() {
            return Err("publication decoder must reject Stub".into());
        }
        let NativeStubReply::Prepared {
            mut edits,
            binding,
            selected_id,
        } = reply
        else {
            return Err("fixture is not a prepared Stub".into());
        };
        edits.first_mut().ok_or("fixture edit missing")?.replacement = vec![b'x'; (64 << 10) + 1];
        let oversized = encoded(NativeIndexResponse::Stub(NativeStubReply::Prepared {
            binding,
            selected_id,
            edits,
        }))?;
        if decode_stub(&oversized).is_ok() {
            return Err("oversized replacement reached Serde".into());
        }
        Ok(())
    }

    #[test]
    fn bounded_publication_decodes_canonical_cbor_without_wire_twin()
    -> Result<(), Box<dyn std::error::Error>> {
        let sources: Vec<ObjectRef> = (0..MAX_PUBLICATION_SOURCE_REFS)
            .map(|id| {
                ObjectRef(Digest(
                    [u8::try_from(id).expect("source ordinal fits u8"); 32],
                ))
            })
            .collect();
        let projection_owner = NativeIndexResponse::ProjectionOwners(NativeProjectionOwnerLookup {
            view: ViewRef {
                workspace: operation().workspace,
                epoch: operation().epoch,
                sequence: 1,
                digest: Digest([7; 32]),
            },
            projection: ProjectionId([8; 16]),
            binding: None,
            rows: NativeProjectionOwnerRows::new(vec![NativeProjectionOwnerBinding {
                owner: OwnerKey {
                    producer: operation().producer,
                    stable_id: StableOwnerId([9; 16]),
                },
                contribution: Some(ObjectRef(Digest([10; 32]))),
            }])
            .expect("ordered projection owner row"),
        });
        let cases = [
            NativeIndexResponse::Staged(ObjectRef(Digest([1; 32]))),
            NativeIndexResponse::StagedSources(sources),
            NativeIndexResponse::MutationsStaged(OwnerMutationSet {
                root: MapRoot(None),
                count: 0,
                digest: Digest([2; 32]),
            }),
            NativeIndexResponse::Submitted(OperationReply {
                operation: operation(),
                payload: SubmitOutcome::Unknown(operation()),
            }),
            projection_owner,
            NativeIndexResponse::ProjectionSourceDiff(
                quanta_index_contract::native_protocol::NativeProjectionSourceDiff {
                    view: ViewRef {
                        workspace: operation().workspace,
                        epoch: operation().epoch,
                        sequence: 1,
                        digest: Digest([7; 32]),
                    },
                    projection: ProjectionId([8; 16]),
                    binding: None,
                    source: quanta_index_contract::index::SourceCut {
                        owners: MapRoot(None),
                        paths: MapRoot(None),
                        frontiers: MapRoot(None),
                    },
                    producer: operation().producer,
                    rows: quanta_index_contract::native_protocol::NativeSourceDiffRows::new(vec![
                        quanta_index_contract::index::OwnerMutation {
                            owner: OwnerKey {
                                producer: operation().producer,
                                stable_id: StableOwnerId([9; 16]),
                            },
                            expected: Some(ObjectRef(Digest([10; 32]))),
                            next: None,
                        },
                    ])?,
                    has_more: false,
                },
            ),
            NativeIndexResponse::Error(NativeIndexWireError {
                code: NativeIndexErrorCode::Conflict,
                message: "source conflict".into(),
            }),
            NativeIndexResponse::Error(NativeIndexWireError {
                code: NativeIndexErrorCode::FocusSubjectNotFound,
                message: "selected focus is absent".into(),
            }),
            NativeIndexResponse::Error(NativeIndexWireError {
                code: NativeIndexErrorCode::Storage,
                message: "x".repeat(MAX_PUBLICATION_ERROR_MESSAGE_BYTES),
            }),
        ];
        for response in cases {
            let body = encoded(response.clone())?;
            let parsed = decode(&body)?;
            if parsed.request_id != request_id() || parsed.payload != response {
                return Err("admitted publication decode differs from canonical wire".into());
            }
            let canonical: NativeIndexResponseEnvelope =
                ciborium::de::from_reader(body.as_slice())?;
            if parsed != canonical {
                return Err("canonical and admitted publication decoders disagree".into());
            }
        }
        Ok(())
    }

    #[test]
    fn publication_decoder_rejects_unbounded_or_unrelated_payloads()
    -> Result<(), Box<dyn std::error::Error>> {
        let many: Vec<ObjectRef> = (0..=MAX_PUBLICATION_SOURCE_REFS)
            .map(|id| {
                ObjectRef(Digest(
                    [u8::try_from(id).expect("source ordinal fits u8"); 32],
                ))
            })
            .collect();
        let invalid = [
            NativeIndexResponse::StagedSources(many),
            NativeIndexResponse::Error(NativeIndexWireError {
                code: NativeIndexErrorCode::Storage,
                message: "x".repeat(MAX_PUBLICATION_ERROR_MESSAGE_BYTES.saturating_add(1)),
            }),
            NativeIndexResponse::ReadyWork(Vec::new()),
        ];
        for response in invalid {
            let body = encoded(response)?;
            if decode(&body).is_ok() {
                return Err("unbounded or unrelated publication payload accepted".into());
            }
        }
        let mut body = encoded(NativeIndexResponse::Staged(ObjectRef(Digest([1; 32]))))?;
        body.push(0xf6);
        if decode(&body).is_ok() {
            return Err("trailing CBOR accepted".into());
        }
        Ok(())
    }

    #[test]
    fn token_preflight_rejects_oversized_identifier_indefinite_text_bytes_and_depth()
    -> Result<(), Box<dyn std::error::Error>> {
        use serde::ser::SerializeMap as _;

        struct OversizedIdentifier;
        impl serde::Serialize for OversizedIdentifier {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("request_id", &41_u64)?;
                let malformed = "x".repeat(MAX_PUBLICATION_IDENTIFIER_BYTES.saturating_add(1));
                map.serialize_entry(&malformed, &0_u8)?;
                map.end()
            }
        }
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&OversizedIdentifier, &mut bytes)?;
        if !matches!(
            decode(&bytes),
            Err(NativeUdsError::Protocol(message))
                if message == "Native publication identifier exceeds 32 bytes"
        ) {
            return Err("oversized identifier reached eager decoder".into());
        }
        if preflight_canonical_cbor(&[0x7f, 0xff], false).is_ok() {
            return Err("indefinite text reached eager decoder".into());
        }
        if !matches!(
            preflight_canonical_cbor(&[0x58, 0xff], false),
            Err(NativeUdsError::Protocol(message))
                if message == "Native publication CBOR byte strings are outside the canonical wire"
        ) {
            return Err("byte string reached eager decoder".into());
        }
        let byte_message = [
            0xa1, 0x67, b'm', b'e', b's', b's', b'a', b'g', b'e', 0x59, 0x10, 0x01,
        ];
        if !matches!(
            preflight_canonical_cbor(&byte_message, false),
            Err(NativeUdsError::Protocol(message))
                if message == "Native publication CBOR byte strings are outside the canonical wire"
        ) {
            return Err("oversized byte string message reached eager decoder".into());
        }
        let byte_variant = [0xa1, 0x41, b'E', 0xf6];
        if !matches!(
            preflight_canonical_cbor(&byte_variant, false),
            Err(NativeUdsError::Protocol(message))
                if message == "Native publication CBOR map key is not text"
        ) {
            return Err("byte string variant reached eager decoder".into());
        }
        let mut nested = vec![0x81; MAX_PUBLICATION_CBOR_DEPTH.saturating_add(1)];
        nested.push(0xf6);
        if preflight_canonical_cbor(&nested, false).is_ok() {
            return Err("deep CBOR reached eager decoder".into());
        }
        Ok(())
    }
}
