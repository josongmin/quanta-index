//! FD6 Source-only evidence. The receiver must authenticate all physical roots
//! and Q publications independently before using these comparison values.

use super::{
    NativeCutoverByteStringV1, NativeCutoverDigest64V1, NativeCutoverEvidenceBeforeSelectionV1,
    NativeCutoverEvidenceDecodeErrorV1 as DecodeError, NativeCutoverEvidenceFreshSdkV1,
    NativeCutoverEvidenceHeaderV1, NativeCutoverEvidenceHostFactsV1,
    NativeCutoverEvidenceLegacyPublicationV1, NativeCutoverEvidencePreviousNativeSourceCoverageV1,
    NativeCutoverEvidenceSourceFactV1, NativeCutoverEvidenceSourcePublicationBindingV1,
    NativeCutoverEvidenceSyntaxErrorV1 as SyntaxError, NativeCutoverEvidenceTargetV1,
    NativeCutoverEvidenceV1, SyntaxReaderV1, MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1,
    canonical_digest64,
    decode_preflighted_v5_v1, read_native_cutover_before_native_selector_v1,
    validate_custody_shape_v1, validate_fact_bounds_v1, validate_legacy_publication_v1,
    validate_native_cutover_evidence_cbor_v1, validate_source_facts_v1,
    validate_v5_predecessor_pair_v1,
};
use crate::index::{Digest, ObjectRef, OperationKey, SourceOperationMode, UpdateRequest};
use crate::{RepoMapMutationPhaseV2, RepoMapTerminalReceiptV2};

/// Signed S replay data compared with the actual immutable artifact and QMP
/// receipt under the installed Original before the FD6 writer copies it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverWriterAckV7 {
    pub publish_generation: u64,
    pub state_revision: u64,
    pub closeout_id: String,
    pub attempt_id: String,
    pub writer_root_locator: NativeCutoverDigest64V1,
    pub search_root_locator: NativeCutoverDigest64V1,
    pub frozen_request_digest: NativeCutoverDigest64V1,
    pub product_scope_digest: NativeCutoverDigest64V1,
    pub artifact_content_digest: NativeCutoverDigest64V1,
    pub handoff_set_digest: NativeCutoverDigest64V1,
    pub source_closure_binding_digest: NativeCutoverDigest64V1,
    pub source_terminal_digest: NativeCutoverDigest64V1,
    pub source_transition_digest: NativeCutoverDigest64V1,
    pub q_source_storage_root_digest: NativeCutoverDigest64V1,
    pub q_source_transition_digest: NativeCutoverDigest64V1,
    pub manifest_digest: NativeCutoverDigest64V1,
    pub issued_marker_digest: NativeCutoverDigest64V1,
    pub qmp_receipt_digest: NativeCutoverDigest64V1,
    pub ack_evidence_digest: NativeCutoverDigest64V1,
}
crate::native_serde_struct!(NativeCutoverWriterAckV7 {
    publish_generation: u64,
    state_revision: u64,
    closeout_id: String,
    attempt_id: String,
    writer_root_locator: NativeCutoverDigest64V1,
    search_root_locator: NativeCutoverDigest64V1,
    frozen_request_digest: NativeCutoverDigest64V1,
    product_scope_digest: NativeCutoverDigest64V1,
    artifact_content_digest: NativeCutoverDigest64V1,
    handoff_set_digest: NativeCutoverDigest64V1,
    source_closure_binding_digest: NativeCutoverDigest64V1,
    source_terminal_digest: NativeCutoverDigest64V1,
    source_transition_digest: NativeCutoverDigest64V1,
    q_source_storage_root_digest: NativeCutoverDigest64V1,
    q_source_transition_digest: NativeCutoverDigest64V1,
    manifest_digest: NativeCutoverDigest64V1,
    issued_marker_digest: NativeCutoverDigest64V1,
    qmp_receipt_digest: NativeCutoverDigest64V1,
    ack_evidence_digest: NativeCutoverDigest64V1
});

/// Signed Source and immutable Q request evidence. The held Q Catalog
/// independently supplies the Published view, Source cut and profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverSourceOnlyPublicationV7 {
    pub target: NativeCutoverEvidenceTargetV1,
    pub source: NativeCutoverEvidenceSourcePublicationBindingV1,
    pub request_ref: ObjectRef,
    pub request: UpdateRequest,
    pub writer_ack: NativeCutoverWriterAckV7,
}
crate::native_serde_struct!(NativeCutoverSourceOnlyPublicationV7 {
    target: NativeCutoverEvidenceTargetV1,
    source: NativeCutoverEvidenceSourcePublicationBindingV1,
    request_ref: ObjectRef,
    request: UpdateRequest,
    writer_ack: NativeCutoverWriterAckV7
});

impl NativeCutoverSourceOnlyPublicationV7 {
    fn validate_structure_v6(&self) -> Result<(), SyntaxError> {
        let target = &self.target;
        let op = target.operation;
        let ack = &self.writer_ack;
        let source_request = self
            .request
            .kind
            .source_transition_v1()
            .ok_or(SyntaxError::NonCanonical)?;
        let [event] = source_request.events.as_slice() else {
            return Err(SyntaxError::NonCanonical);
        };
        validate_source_binding_v6(&self.source)?;
        if op.sequence == 0
            || target.event_predecessor.checked_add(1) != Some(target.event_sequence)
            || target.source_mode == SourceOperationMode::Unchanged
            || target.expected_view.workspace != op.workspace
            || target.expected_view.epoch != op.epoch
            || target.expected_view.digest == Digest([0; 32])
            || target.expected_view.sequence.checked_add(1).is_none()
            || self.request.operation != op
            || self.request.expected_view != target.expected_view
            || self.request.kind.source_mode() != target.source_mode
            || source_request.expected_source != target.expected_source
            || event.producer != op.producer
            || event.stream != op.stream
            || event.stream_epoch != op.stream_epoch
            || event.predecessor != target.event_predecessor
            || event.sequence != target.event_sequence
            || self.request_ref.0 == Digest([0; 32])
            || ack.publish_generation == 0
            || ack.state_revision == 0
            || ![&ack.closeout_id, &ack.attempt_id]
                .into_iter()
                .all(|id| !id.is_empty() && id.trim() == id.as_str() && !id.contains('\0'))
            || ack.writer_root_locator == ack.search_root_locator
            || ack.source_terminal_digest != self.source.physical_target_source_digest
            || ack.source_transition_digest != self.source.transition_digest
            || ![
                ack.writer_root_locator,
                ack.search_root_locator,
                ack.frozen_request_digest,
                ack.product_scope_digest,
                ack.artifact_content_digest,
                ack.handoff_set_digest,
                ack.source_closure_binding_digest,
                ack.source_terminal_digest,
                ack.q_source_storage_root_digest,
                ack.q_source_transition_digest,
                ack.manifest_digest,
                ack.issued_marker_digest,
                ack.qmp_receipt_digest,
                ack.ack_evidence_digest,
            ]
            .iter()
            .all(|digest| canonical_digest64(digest) && digest.0 != [b'0'; 64])
        {
            return Err(SyntaxError::NonCanonical);
        }
        Ok(())
    }
}

fn validate_publication_lineage_v6(
    initial: &NativeCutoverSourceOnlyPublicationV7,
    successors: &[NativeCutoverSourceOnlyPublicationV7],
) -> Result<(), SyntaxError> {
    initial.validate_structure_v6()?;
    let full = initial.target.operation;
    if successors.len() >= MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1 {
        return Err(SyntaxError::SequenceTooLarge);
    }
    if initial.target.source_mode != SourceOperationMode::Advance
        || initial.target.event_predecessor != 0
        || initial.target.event_sequence != 1
    {
        return Err(SyntaxError::NonCanonical);
    }
    let mut prior = initial;
    for successor in successors {
        successor.validate_structure_v6()?;
        let op = successor.target.operation;
        if op.workspace != full.workspace
            || op.epoch != full.epoch
            || op.producer != full.producer
            || op.stream != full.stream
            || successor.target.text_admission != initial.target.text_admission
            || (op.stream_epoch, op.sequence)
                <= (prior.target.operation.stream_epoch, prior.target.operation.sequence)
            || successor.writer_ack.publish_generation
                <= prior.writer_ack.publish_generation
            || successor.writer_ack.writer_root_locator != initial.writer_ack.writer_root_locator
            || successor.writer_ack.search_root_locator != initial.writer_ack.search_root_locator
        {
            return Err(SyntaxError::NonCanonical);
        }
        prior = successor;
    }
    Ok(())
}

/// The signed predecessor has the receipts and locator. Only the receiver
/// resolves the physical root from the retained installation selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeCutoverHistoricalRepoMapV6 {
    Required {
        locator_digest: NativeCutoverDigest64V1,
        publish: RepoMapTerminalReceiptV2,
        activation: RepoMapTerminalReceiptV2,
    },
    NotApplicable {
        reason_code: String,
        policy_digest: NativeCutoverDigest64V1,
        product_scope_digest: NativeCutoverDigest64V1,
    },
}
crate::native_serde_enum!(NativeCutoverHistoricalRepoMapV6 {
    units {}
    newtypes {}
    records {
        Required = 0 {
            locator_digest: NativeCutoverDigest64V1,
            publish: RepoMapTerminalReceiptV2,
            activation: RepoMapTerminalReceiptV2
        },
        NotApplicable = 1 {
            reason_code: String,
            policy_digest: NativeCutoverDigest64V1,
            product_scope_digest: NativeCutoverDigest64V1
        }
    }
});

impl NativeCutoverHistoricalRepoMapV6 {
    fn validate_structure_v6(&self) -> Result<(), SyntaxError> {
        let valid = match self {
            Self::Required { locator_digest, publish, activation } => {
                canonical_digest64(locator_digest)
                    && locator_digest.0 != [b'0'; 64]
                    && publish.phase == RepoMapMutationPhaseV2::Publish
                    && activation.phase == RepoMapMutationPhaseV2::Activate
            }
            Self::NotApplicable { reason_code, policy_digest, product_scope_digest } => {
                !reason_code.is_empty()
                    && canonical_digest64(policy_digest)
                    && canonical_digest64(product_scope_digest)
            }
        };
        valid.then_some(()).ok_or(SyntaxError::NonCanonical)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverHistoricalV5IdentityV6 {
    pub operation: OperationKey,
    pub published_snapshot_id: u64,
    pub lexical_revision_digest: NativeCutoverDigest64V1,
    pub prepared_artifact_digest: NativeCutoverDigest64V1,
    pub aggregate_receipt_digest: NativeCutoverDigest64V1,
    pub previous_transition_digest: Option<NativeCutoverDigest64V1>,
    pub base_source_digest: Option<NativeCutoverDigest64V1>,
}
crate::native_serde_struct!(NativeCutoverHistoricalV5IdentityV6 {
    operation: OperationKey,
    published_snapshot_id: u64,
    lexical_revision_digest: NativeCutoverDigest64V1,
    prepared_artifact_digest: NativeCutoverDigest64V1,
    aggregate_receipt_digest: NativeCutoverDigest64V1,
    previous_transition_digest: Option<NativeCutoverDigest64V1>,
    base_source_digest: Option<NativeCutoverDigest64V1>
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverHistoricalV5ArtifactV6 {
    pub identity: NativeCutoverHistoricalV5IdentityV6,
    pub repomap: NativeCutoverHistoricalRepoMapV6,
}
crate::native_serde_struct!(NativeCutoverHistoricalV5ArtifactV6 {
    identity: NativeCutoverHistoricalV5IdentityV6,
    repomap: NativeCutoverHistoricalRepoMapV6
});

/// The old installed Native root's complete signed V7 publication lineage.
/// Its BOOT operation and every Published Catalog row are verified by Q.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverPreviousSourceOnlyLineageV7 {
    pub initial_full: NativeCutoverSourceOnlyPublicationV7,
    pub successors: Vec<NativeCutoverSourceOnlyPublicationV7>,
}
crate::native_serde_struct!(NativeCutoverPreviousSourceOnlyLineageV7 {
    initial_full: NativeCutoverSourceOnlyPublicationV7,
    successors: Vec<NativeCutoverSourceOnlyPublicationV7>
});

impl NativeCutoverPreviousSourceOnlyLineageV7 {
    pub fn current_publication_v7(&self) -> &NativeCutoverSourceOnlyPublicationV7 {
        self.successors.last().unwrap_or(&self.initial_full)
    }

    fn validate_structure_v7(&self) -> Result<(), SyntaxError> {
        validate_publication_lineage_v6(&self.initial_full, &self.successors)
    }
}

/// The already-selected Native root can still belong to a historical V5
/// deployment. The new current publication has a dedicated V7 type.
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "Both bounded variants are decoded under the FD6 grant"
)]
pub enum NativeCutoverPreviousPublicationV6 {
    HistoricalV5(NativeCutoverHistoricalV5ArtifactV6),
    SourceOnlyV7(NativeCutoverPreviousSourceOnlyLineageV7),
}
crate::native_serde_enum!(NativeCutoverPreviousPublicationV6 {
    units {}
    newtypes {
        HistoricalV5 = 0 => NativeCutoverHistoricalV5ArtifactV6,
        SourceOnlyV7 = 1 => NativeCutoverPreviousSourceOnlyLineageV7
    }
    records {}
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverPreviousNativeV6 {
    pub coverage: NativeCutoverEvidencePreviousNativeSourceCoverageV1,
    pub publication: NativeCutoverPreviousPublicationV6,
}
crate::native_serde_struct!(NativeCutoverPreviousNativeV6 {
    coverage: NativeCutoverEvidencePreviousNativeSourceCoverageV1,
    publication: NativeCutoverPreviousPublicationV6
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverSourceOnlyEvidenceV6 {
    pub header: NativeCutoverEvidenceHeaderV1,
    pub before_selection: NativeCutoverEvidenceBeforeSelectionV1,
    pub fresh_sdk: NativeCutoverEvidenceFreshSdkV1,
    pub initial_full: NativeCutoverSourceOnlyPublicationV7,
    /// Signed generations after the initial FULL, in publication order.
    pub successors: Vec<NativeCutoverSourceOnlyPublicationV7>,
    pub previous_native: Option<NativeCutoverPreviousNativeV6>,
    pub legacy_publication: Option<NativeCutoverEvidenceLegacyPublicationV1>,
    pub retained_terminal: Option<NativeCutoverByteStringV1>,
    pub source_facts: Vec<NativeCutoverEvidenceSourceFactV1>,
    pub host_facts: NativeCutoverEvidenceHostFactsV1,
}
crate::native_serde_struct!(NativeCutoverSourceOnlyEvidenceV6 {
    header: NativeCutoverEvidenceHeaderV1,
    before_selection: NativeCutoverEvidenceBeforeSelectionV1,
    fresh_sdk: NativeCutoverEvidenceFreshSdkV1,
    initial_full: NativeCutoverSourceOnlyPublicationV7,
    successors: Vec<NativeCutoverSourceOnlyPublicationV7>,
    previous_native: Option<NativeCutoverPreviousNativeV6>,
    legacy_publication: Option<NativeCutoverEvidenceLegacyPublicationV1>,
    retained_terminal: Option<NativeCutoverByteStringV1>,
    source_facts: Vec<NativeCutoverEvidenceSourceFactV1>,
    host_facts: NativeCutoverEvidenceHostFactsV1
});

impl NativeCutoverSourceOnlyEvidenceV6 {
    pub const FORMAT_VERSION: u32 = 6;

    pub fn current_publication_v6(&self) -> &NativeCutoverSourceOnlyPublicationV7 {
        self.successors.last().unwrap_or(&self.initial_full)
    }

    pub fn validate_structure_v6(&self) -> Result<(), SyntaxError> {
        if self.header.format_version != Self::FORMAT_VERSION {
            return Err(SyntaxError::Unsupported);
        }
        let full = self.initial_full.target.operation;
        let current = self.current_publication_v6();
        let selected = &self.header.selected_source;
        validate_fact_bounds_v1(&self.source_facts, &self.host_facts)?;
        validate_source_facts_v1(selected, &self.source_facts, &self.host_facts)?;
        validate_legacy_publication_v1(self.legacy_publication.as_ref())?;
        validate_custody_shape_v1(
            &self.header,
            full,
            &self.before_selection,
            self.retained_terminal.as_ref(),
            &self.host_facts,
        )?;
        validate_publication_lineage_v6(&self.initial_full, &self.successors)?;
        if full.workspace != selected.workspace
            || full.epoch != selected.workspace_epoch
            || full.producer != selected.producer
            || full.stream != selected.stream
            || full.stream_epoch != selected.stream_epoch
            || self.initial_full.target.text_admission != selected.text_admission
        {
            return Err(SyntaxError::NonCanonical);
        }
        let fresh = &self.fresh_sdk;
        if current.source.target_snapshot_id != fresh.target_snapshot_id
            || current.source.physical_target_source_digest != fresh.physical_target_source_digest
            || current.source.transition_digest != fresh.source_manifest_digest
            || current.source.completion_pointer_digest != fresh.source_pointer_digest
            || fresh.committed_source_snapshot_id == 0
            || (!self.successors.is_empty()
                && fresh.committed_source_snapshot_id != fresh.target_snapshot_id)
            || fresh.committed_source_snapshot_digest != fresh.physical_target_source_digest
            || !canonical_digest64(&fresh.sdk_generation_digest)
        {
            return Err(SyntaxError::NonCanonical);
        }
        match (
            read_native_cutover_before_native_selector_v1(&self.before_selection.document.0)?,
            &self.previous_native,
        ) {
            (None, None) => {}
            (Some((nonce, boot_operation)), Some(previous))
                if self.legacy_publication.is_none()
                    && previous.coverage.previous_source_operation != current.target.operation =>
            {
                previous.validate_structure_v6(nonce, boot_operation, &self.initial_full)?;
            }
            _ => return Err(SyntaxError::NonCanonical),
        }
        Ok(())
    }
}

impl NativeCutoverPreviousNativeV6 {
    fn validate_structure_v6(
        &self,
        root_nonce: NativeCutoverDigest64V1,
        boot_operation: OperationKey,
        initial: &NativeCutoverSourceOnlyPublicationV7,
    ) -> Result<(), SyntaxError> {
        let coverage = &self.coverage;
        validate_source_binding_v6(&coverage.previous_source)?;
        if root_nonce.0 != digest_hex_v6(coverage.previous_root_nonce)
            || coverage.previous_source_operation == initial.target.operation
            || coverage.fresh_full_operation != initial.target.operation
            || coverage.fresh_source != initial.source
            || coverage.previous_source_generation == 0
            || coverage.fresh_source_generation < coverage.previous_source_generation
            || ((coverage.fresh_source_generation == coverage.previous_source_generation)
                != (coverage.previous_source == coverage.fresh_source))
        {
            return Err(SyntaxError::NonCanonical);
        }
        match &self.publication {
            NativeCutoverPreviousPublicationV6::HistoricalV5(artifact) => {
                let identity = &artifact.identity;
                validate_v5_predecessor_pair_v1(
                    identity.previous_transition_digest.as_ref(),
                    identity.base_source_digest.as_ref(),
                )?;
                artifact.repomap.validate_structure_v6()?;
                if identity.operation != coverage.previous_source_operation
                    || identity.published_snapshot_id != coverage.previous_source.target_snapshot_id
                    || ![
                        identity.lexical_revision_digest,
                        identity.prepared_artifact_digest,
                        identity.aggregate_receipt_digest,
                    ]
                    .iter()
                    .all(|digest| canonical_digest64(digest) && digest.0 != [b'0'; 64])
                {
                    return Err(SyntaxError::NonCanonical);
                }
            }
            NativeCutoverPreviousPublicationV6::SourceOnlyV7(lineage) => {
                lineage.validate_structure_v7()?;
                let publication = lineage.current_publication_v7();
                let initial_operation = lineage.initial_full.target.operation;
                if initial_operation.workspace != boot_operation.workspace
                    || initial_operation.epoch != boot_operation.epoch
                    || publication.target.operation != coverage.previous_source_operation
                    || publication.source != coverage.previous_source
                {
                    return Err(SyntaxError::NonCanonical);
                }
            }
        }
        Ok(())
    }
}

fn validate_source_binding_v6(
    source: &NativeCutoverEvidenceSourcePublicationBindingV1,
) -> Result<(), SyntaxError> {
    if source.target_snapshot_id == 0
        || ![
            source.transition_digest,
            source.completion_pointer_digest,
            source.physical_target_source_digest,
        ]
        .iter()
        .all(|digest| canonical_digest64(digest) && digest.0 != [b'0'; 64])
    {
        return Err(SyntaxError::NonCanonical);
    }
    Ok(())
}

fn digest_hex_v6(digest: Digest) -> [u8; 64] {
    let mut result = [0; 64];
    let alphabet = b"0123456789abcdef";
    for (byte, pair) in digest.0.into_iter().zip(result.chunks_exact_mut(2)) {
        pair[0] = alphabet[(byte >> 4) as usize];
        pair[1] = alphabet[(byte & 15) as usize];
    }
    result
}

/// The single top-level field selects the closed Source-only body. Historical
/// format-5 bytes retain their original unwrapped encoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverSourceOnlyWireV6 {
    pub source_only_v7: NativeCutoverSourceOnlyEvidenceV6,
}
crate::native_serde_struct!(NativeCutoverSourceOnlyWireV6 {
    source_only_v7: NativeCutoverSourceOnlyEvidenceV6
});

#[derive(Debug, Eq, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "The caller admits the full FD6 decode bank"
)]
pub enum NativeCutoverEvidenceEnvelopeV1 {
    HistoricalV5(NativeCutoverEvidenceV1),
    SourceOnlyV7(NativeCutoverSourceOnlyEvidenceV6),
}

pub fn decode_native_cutover_evidence_envelope_v1(
    bytes: &[u8],
) -> Result<NativeCutoverEvidenceEnvelopeV1, DecodeError> {
    validate_native_cutover_evidence_cbor_v1(bytes).map_err(DecodeError::Syntax)?;
    if !is_source_only_envelope_v6(bytes).map_err(DecodeError::Syntax)? {
        return decode_preflighted_v5_v1(bytes).map(NativeCutoverEvidenceEnvelopeV1::HistoricalV5);
    }
    let wire: NativeCutoverSourceOnlyWireV6 =
        ciborium::de::from_reader(bytes).map_err(DecodeError::Cbor)?;
    wire.source_only_v7
        .validate_structure_v6()
        .map_err(DecodeError::Structure)?;
    Ok(NativeCutoverEvidenceEnvelopeV1::SourceOnlyV7(
        wire.source_only_v7,
    ))
}

fn is_source_only_envelope_v6(bytes: &[u8]) -> Result<bool, SyntaxError> {
    let mut reader = SyntaxReaderV1 { bytes, cursor: 0 };
    let (major, extra) = reader.head_v1()?;
    if major != 5 || reader.length_v1(extra)? == 0 {
        return Ok(false);
    }
    let (_, extra) = reader.head_v1()?;
    let (start, end) = reader.text_v1(extra)?;
    Ok(bytes
        .get(start..end)
        .is_some_and(|key| key == b"source_only_v7"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{
        CollectionId, MapRoot, OperationKey, OwnerMutationSet, ProducerId, SlotId, SourceCut,
        SourceEvent, SourceTextAdmission, SourceTransition, StreamId, TenantId, UpdateKind,
        ViewRef, WorkspaceKey, WorktreeId,
    };
    use crate::native_cutover_evidence_v1::{
        NativeCutoverEvidenceFileIdentityV1, NativeCutoverEvidenceHostSealV1,
        NativeCutoverEvidenceSelectedSourceV1, NativeCutoverHostCustodyModeV1,
        NativeCutoverHostWatchModeV1,
    };
    use crate::{ManifestGeneration, RepoId, RepoMapMutationAck, RevisionId};

    fn encoded(value: &impl serde::Serialize) -> Vec<u8> {
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(value, &mut bytes).expect("bounded fixture encoding");
        bytes
    }

    fn fixture() -> NativeCutoverSourceOnlyWireV6 {
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        };
        let op = OperationKey {
            workspace,
            epoch: 1,
            producer: ProducerId([5; 16]),
            stream: StreamId([6; 16]),
            stream_epoch: 1,
            sequence: 1,
        };
        let selected_source = NativeCutoverEvidenceSelectedSourceV1 {
            workspace,
            workspace_epoch: 1,
            producer: op.producer,
            stream: op.stream,
            stream_epoch: 1,
            text_admission: SourceTextAdmission::Excluded,
        };
        let expected_source = SourceCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        };
        let expected_view = ViewRef {
            workspace,
            epoch: 1,
            sequence: 7,
            digest: Digest([7; 32]),
        };
        let profile = ObjectRef(Digest([8; 32]));
        let binding = NativeCutoverEvidenceSourcePublicationBindingV1 {
            transition_digest: NativeCutoverDigest64V1([b'a'; 64]),
            completion_pointer_digest: NativeCutoverDigest64V1([b'b'; 64]),
            physical_target_source_digest: NativeCutoverDigest64V1([b'c'; 64]),
            target_snapshot_id: 42,
        };
        NativeCutoverSourceOnlyWireV6 {
            source_only_v7: NativeCutoverSourceOnlyEvidenceV6 {
                header: NativeCutoverEvidenceHeaderV1 {
                    format_version: 6, canonical_workspace_root: "/repo".into(),
                    repo_id: "repo".into(), contract_key: "contract".into(),
                    writer_state_root: "/writer".into(), canonical_snapshot_namespace: "/snapshots".into(),
                    sdk_lock_path: "/sdk/lock".into(), config_path: "/q/config".into(),
                    config_sha256: Digest([9; 32]), original_full_operation: op, selected_source,
                    host: NativeCutoverEvidenceHostSealV1 {
                        canonical_workspace_root: "/repo".into(), host_instance: 1, host_generation: 1,
                        watch_mode: NativeCutoverHostWatchModeV1::Polling,
                        custody_mode: NativeCutoverHostCustodyModeV1::DurableInbox,
                        namespace_root: "/host".into(),
                        namespace_file: NativeCutoverEvidenceFileIdentityV1 { device: 1, inode: 1 },
                    },
                    sdk_lock_file: NativeCutoverEvidenceFileIdentityV1 { device: 1, inode: 2 },
                    current_selection_sha256: Digest([10; 32]),
                    candidate_selection_sha256: Digest([11; 32]),
                },
                before_selection: NativeCutoverEvidenceBeforeSelectionV1 {
                    document: NativeCutoverByteStringV1(
                        br#"{"binary":"/q/searchd","binary_sha256":"fixture","environment":{},"mode":"legacy","schema_version":2,"state_root":"/old"}"#.to_vec(),
                    ),
                    raw_sha256: Digest([12; 32]), file_version: Some(Digest([13; 32])),
                },
                fresh_sdk: NativeCutoverEvidenceFreshSdkV1 {
                    target_snapshot_id: 42, committed_source_snapshot_id: 42,
                    committed_source_snapshot_digest: binding.physical_target_source_digest,
                    sdk_generation_digest: NativeCutoverDigest64V1([b'd'; 64]),
                    source_pointer_digest: binding.completion_pointer_digest,
                    source_manifest_digest: binding.transition_digest,
                    physical_target_source_digest: binding.physical_target_source_digest,
                },
                initial_full: NativeCutoverSourceOnlyPublicationV7 {
                    target: NativeCutoverEvidenceTargetV1 {
                        operation: op, expected_view, expected_source, expected_profile: profile,
                        source_mode: SourceOperationMode::Advance, event_predecessor: 0, event_sequence: 1,
                        owner_identity_policy_digest: Digest([14; 32]),
                        text_admission: selected_source.text_admission,
                        canonical_target_digest: Digest([15; 32]),
                    },
                    source: binding, request_ref: ObjectRef(Digest([16; 32])),
                    request: UpdateRequest {
                        operation: op,
                        expected_view,
                        kind: UpdateKind::SourceAdvance(SourceTransition {
                            expected_source,
                            owners: OwnerMutationSet {
                                root: MapRoot(None), count: 0, digest: Digest([17; 32]),
                            },
                            events: vec![SourceEvent {
                                producer: op.producer, stream: op.stream, stream_epoch: op.stream_epoch,
                                predecessor: 0, sequence: 1, payload: Digest([18; 32]),
                            }],
                        }),
                    },
                    writer_ack: NativeCutoverWriterAckV7 {
                        publish_generation: 10,
                        state_revision: 3,
                        closeout_id: "fixture-closeout".into(),
                        attempt_id: "fixture-attempt".into(),
                        writer_root_locator: NativeCutoverDigest64V1([b'1'; 64]),
                        search_root_locator: NativeCutoverDigest64V1([b'2'; 64]),
                        frozen_request_digest: NativeCutoverDigest64V1([b'a'; 64]),
                        product_scope_digest: NativeCutoverDigest64V1([b'3'; 64]),
                        artifact_content_digest: NativeCutoverDigest64V1([b'4'; 64]),
                        handoff_set_digest: NativeCutoverDigest64V1([b'b'; 64]),
                        source_closure_binding_digest: NativeCutoverDigest64V1([b'5'; 64]),
                        source_terminal_digest: NativeCutoverDigest64V1([b'c'; 64]),
                        source_transition_digest: binding.transition_digest,
                        q_source_storage_root_digest: NativeCutoverDigest64V1([b'd'; 64]),
                        q_source_transition_digest: NativeCutoverDigest64V1([b'e'; 64]),
                        manifest_digest: NativeCutoverDigest64V1([b'6'; 64]),
                        issued_marker_digest: NativeCutoverDigest64V1([b'f'; 64]),
                        qmp_receipt_digest: NativeCutoverDigest64V1([b'7'; 64]),
                        ack_evidence_digest: NativeCutoverDigest64V1([b'8'; 64]),
                    },
                },
                successors: Vec::new(), previous_native: None, legacy_publication: None,
                retained_terminal: None, source_facts: Vec::new(),
                host_facts: NativeCutoverEvidenceHostFactsV1 {
                    workspace_authority: "repo".into(), capacity: 32, retained_delivery_count: 0,
                    active_units: 0, prepared_authorities: Vec::new(), deliveries: Vec::new(),
                    pending_apply_intent: None, recovery_receipts: Vec::new(),
                },
            },
        }
    }

    #[test]
    fn source_only_fd6_dispatch_preserves_v5_and_rejects_mixed_shapes() {
        let v6 = fixture();
        let bytes_v6 = encoded(&v6);
        assert_eq!(
            decode_native_cutover_evidence_envelope_v1(&bytes_v6).unwrap(),
            NativeCutoverEvidenceEnvelopeV1::SourceOnlyV7(v6.source_only_v7.clone())
        );
        assert!(super::super::decode_native_cutover_evidence_v1(&bytes_v6).is_err());
        let v6 = v6.source_only_v7;
        let v5 = NativeCutoverEvidenceV1 {
            header: NativeCutoverEvidenceHeaderV1 {
                format_version: 5,
                ..v6.header
            },
            baseline: v6.initial_full.target,
            before_selection: v6.before_selection,
            fresh_sdk: v6.fresh_sdk,
            previous_native_source_coverage: None,
            v5_artifacts: Vec::new(),
            legacy_publication: None,
            retained_terminal: None,
            source_facts: v6.source_facts,
            host_facts: v6.host_facts,
        };
        let bytes_v5 = encoded(&v5);
        assert_eq!(
            decode_native_cutover_evidence_envelope_v1(&bytes_v5).unwrap(),
            NativeCutoverEvidenceEnvelopeV1::HistoricalV5(v5)
        );
        let ciborium::Value::Map(mut old_fields) =
            ciborium::de::from_reader(bytes_v5.as_slice()).unwrap()
        else {
            panic!("historical evidence is a map");
        };
        let ciborium::Value::Map(mut new_fields) =
            ciborium::de::from_reader(bytes_v6.as_slice()).unwrap()
        else {
            panic!("Source-only evidence is a map");
        };
        let duplicate = ciborium::Value::Map(
            new_fields
                .iter()
                .cloned()
                .chain(new_fields.iter().cloned())
                .collect(),
        );
        assert!(matches!(
            decode_native_cutover_evidence_envelope_v1(&encoded(&duplicate)),
            Err(DecodeError::Syntax(SyntaxError::DuplicateField))
        ));
        old_fields.append(&mut new_fields);
        assert!(
            decode_native_cutover_evidence_envelope_v1(&encoded(&ciborium::Value::Map(
                old_fields.clone()
            )))
            .is_err()
        );
        old_fields.rotate_right(1);
        assert!(
            decode_native_cutover_evidence_envelope_v1(&encoded(&ciborium::Value::Map(old_fields)))
                .is_err()
        );
    }

    #[test]
    fn source_only_fd6_refuses_nonexact_current_axes_and_missing_previous_native() {
        let v6 = fixture();
        let mut wrong = v6.clone();
        wrong.source_only_v7.header.format_version = 5;
        assert!(decode_native_cutover_evidence_envelope_v1(&encoded(&wrong)).is_err());
        wrong = v6.clone();
        wrong
            .source_only_v7
            .initial_full
            .request
            .expected_view
            .epoch += 1;
        assert!(decode_native_cutover_evidence_envelope_v1(&encoded(&wrong)).is_err());
        wrong = v6.clone();
        let UpdateKind::SourceAdvance(transition) =
            &mut wrong.source_only_v7.initial_full.request.kind
        else {
            panic!("initial fixture must advance Source");
        };
        transition.events[0].stream_epoch += 1;
        assert!(decode_native_cutover_evidence_envelope_v1(&encoded(&wrong)).is_err());
        wrong = v6.clone();
        wrong.source_only_v7.fresh_sdk.source_pointer_digest = NativeCutoverDigest64V1([b'f'; 64]);
        assert!(decode_native_cutover_evidence_envelope_v1(&encoded(&wrong)).is_err());
        wrong = v6.clone();
        wrong
            .source_only_v7
            .initial_full
            .writer_ack
            .source_transition_digest = NativeCutoverDigest64V1([b'f'; 64]);
        assert!(decode_native_cutover_evidence_envelope_v1(&encoded(&wrong)).is_err());
        wrong = v6.clone();
        wrong
            .source_only_v7
            .initial_full
            .writer_ack
            .source_terminal_digest = NativeCutoverDigest64V1([b'f'; 64]);
        assert!(decode_native_cutover_evidence_envelope_v1(&encoded(&wrong)).is_err());
        wrong = v6.clone();
        wrong.source_only_v7.successors.push(wrong.source_only_v7.initial_full.clone());
        assert!(decode_native_cutover_evidence_envelope_v1(&encoded(&wrong)).is_err());
        let initial = &wrong.source_only_v7.initial_full;
        let mut successor = initial.clone();
        successor.target.operation.sequence += 1;
        successor.target.event_predecessor = initial.target.event_sequence;
        successor.target.event_sequence = initial.target.event_sequence + 1;
        successor.target.expected_view.sequence += 1;
        successor.target.expected_view.digest = Digest([19; 32]);
        successor.target.expected_source.frontiers = MapRoot(Some(ObjectRef(Digest([20; 32]))));
        successor.request.operation = successor.target.operation;
        successor.request.expected_view = successor.target.expected_view;
        let UpdateKind::SourceAdvance(transition) = &mut successor.request.kind else {
            panic!("successor fixture must advance Source");
        };
        transition.events[0].predecessor = successor.target.event_predecessor;
        transition.events[0].sequence = successor.target.event_sequence;
        transition.expected_source = successor.target.expected_source;
        successor.request_ref = ObjectRef(Digest([21; 32]));
        successor.writer_ack.publish_generation += 1;
        successor.source.target_snapshot_id += 1;
        successor.source.transition_digest = NativeCutoverDigest64V1([b'e'; 64]);
        successor.writer_ack.source_transition_digest = successor.source.transition_digest;
        wrong.source_only_v7.fresh_sdk.target_snapshot_id = successor.source.target_snapshot_id;
        wrong.source_only_v7.fresh_sdk.source_manifest_digest = successor.source.transition_digest;
        wrong.source_only_v7.successors = vec![successor];
        assert!(decode_native_cutover_evidence_envelope_v1(&encoded(&wrong)).is_err());
        wrong.source_only_v7.fresh_sdk.committed_source_snapshot_id =
            wrong.source_only_v7.fresh_sdk.target_snapshot_id;
        assert_eq!(
            decode_native_cutover_evidence_envelope_v1(&encoded(&wrong)).unwrap(),
            NativeCutoverEvidenceEnvelopeV1::SourceOnlyV7(wrong.source_only_v7.clone())
        );
        let mut chain = wrong.clone();
        let mut next = chain.source_only_v7.successors[0].clone();
        next.target.operation.sequence += 1;
        next.target.event_predecessor = next.target.event_sequence;
        next.target.event_sequence += 1;
        next.target.expected_view.sequence += 1;
        next.target.expected_view.digest = Digest([22; 32]);
        next.target.expected_source.frontiers = MapRoot(Some(ObjectRef(Digest([23; 32]))));
        next.request.operation = next.target.operation;
        next.request.expected_view = next.target.expected_view;
        let UpdateKind::SourceAdvance(transition) = &mut next.request.kind else {
            panic!("second successor fixture must advance Source");
        };
        transition.events[0].predecessor = next.target.event_predecessor;
        transition.events[0].sequence = next.target.event_sequence;
        transition.expected_source = next.target.expected_source;
        next.request_ref = ObjectRef(Digest([24; 32]));
        next.writer_ack.publish_generation += 1;
        next.writer_ack.state_revision += 1;
        next.source.target_snapshot_id += 1;
        next.source.transition_digest = NativeCutoverDigest64V1([b'd'; 64]);
        next.writer_ack.source_transition_digest = next.source.transition_digest;
        chain.source_only_v7.fresh_sdk.target_snapshot_id = next.source.target_snapshot_id;
        chain.source_only_v7.fresh_sdk.source_manifest_digest = next.source.transition_digest;
        chain.source_only_v7.fresh_sdk.committed_source_snapshot_id = next.source.target_snapshot_id;
        chain.source_only_v7.successors.push(next);
        assert_eq!(
            decode_native_cutover_evidence_envelope_v1(&encoded(&chain)).unwrap(),
            NativeCutoverEvidenceEnvelopeV1::SourceOnlyV7(chain.source_only_v7.clone())
        );
        chain.source_only_v7.successors.swap(0, 1);
        assert!(decode_native_cutover_evidence_envelope_v1(&encoded(&chain)).is_err());

        wrong = v6;
        let op = wrong.source_only_v7.initial_full.target.operation;
        let before_native = serde_json::json!({
            "binary":"/q/searchd","binary_sha256":"fixture","environment":{},"mode":"native",
            "schema_version":2,"state_root":"/old","native_config":"/old/config",
            "native_config_sha256":"fixture","native_boot_receipt":{
                "root_nonce_hex":"1".repeat(64),"manifest_operation":op,
                "manifest_object_hex":"2".repeat(64),"source_cut_digest_hex":"3".repeat(64),
                "install_catalog_digest_hex":"4".repeat(64)
            }
        });
        wrong.source_only_v7.before_selection.document =
            NativeCutoverByteStringV1(serde_json::to_vec(&before_native).unwrap());
        assert!(
            super::read_native_cutover_before_native_selector_v1(
                &wrong.source_only_v7.before_selection.document.0
            )
            .unwrap()
            .is_some()
        );
        assert!(decode_native_cutover_evidence_envelope_v1(&encoded(&wrong)).is_err());

        let mut previous = wrong.source_only_v7.initial_full.clone();
        previous.target.operation.sequence += 1;
        previous.request.operation = previous.target.operation;
        previous.source.target_snapshot_id = 40;
        let mut old_selection = before_native;
        old_selection["native_boot_receipt"]["manifest_operation"] =
            serde_json::to_value(previous.target.operation).unwrap();
        wrong.source_only_v7.before_selection.document =
            NativeCutoverByteStringV1(serde_json::to_vec(&old_selection).unwrap());
        let coverage = NativeCutoverEvidencePreviousNativeSourceCoverageV1 {
            previous_root_nonce: Digest([0x11; 32]),
            previous_source_operation: previous.target.operation,
            previous_source: previous.source,
            fresh_full_operation: op,
            fresh_source: wrong.source_only_v7.initial_full.source,
            previous_source_generation: 9,
            fresh_source_generation: 10,
        };
        let old_initial = previous.clone();
        let historical = NativeCutoverHistoricalV5ArtifactV6 {
            identity: NativeCutoverHistoricalV5IdentityV6 {
                operation: coverage.previous_source_operation,
                published_snapshot_id: coverage.previous_source.target_snapshot_id,
                lexical_revision_digest: NativeCutoverDigest64V1([b'1'; 64]),
                prepared_artifact_digest: NativeCutoverDigest64V1([b'2'; 64]),
                aggregate_receipt_digest: NativeCutoverDigest64V1([b'3'; 64]),
                previous_transition_digest: None,
                base_source_digest: None,
            },
            repomap: NativeCutoverHistoricalRepoMapV6::NotApplicable {
                reason_code: "source-only-profile".into(),
                policy_digest: NativeCutoverDigest64V1([b'4'; 64]),
                product_scope_digest: NativeCutoverDigest64V1([b'5'; 64]),
            },
        };
        let publish = RepoMapTerminalReceiptV2 {
            phase: RepoMapMutationPhaseV2::Publish,
            mutation: RepoMapMutationAck {
                repo_id: RepoId::new("fixture-repo").unwrap(),
                revision_id: RevisionId::new("fixture-revision").unwrap(),
                manifest_generation: ManifestGeneration::new(1),
                prior_candidate_commitment: None,
                new_candidate_commitment: "1".repeat(64),
                activation_epoch: 0,
                terminal_sequence: 1,
                replayed: false,
            },
            manifest_digest: "2".repeat(64),
            snapshot_id: "40".into(),
            projection_version: 2,
            authority_digest: "3".repeat(64),
            source_bundle_digest: "4".repeat(64),
        };
        let mut activation = publish.clone();
        activation.phase = RepoMapMutationPhaseV2::Activate;
        activation.mutation.activation_epoch = 1;
        activation.mutation.terminal_sequence = 2;
        let required = NativeCutoverHistoricalV5ArtifactV6 {
            identity: historical.identity.clone(),
            repomap: NativeCutoverHistoricalRepoMapV6::Required {
                locator_digest: NativeCutoverDigest64V1([b'6'; 64]),
                publish,
                activation,
            },
        };
        let mut foreign_path: ciborium::Value =
            ciborium::de::from_reader(encoded(&required.repomap).as_slice()).unwrap();
        let ciborium::Value::Map(variants) = &mut foreign_path else {
            panic!("expected the externally tagged RepoMap variant");
        };
        let [(ciborium::Value::Text(name), ciborium::Value::Map(fields))] =
            variants.as_mut_slice()
        else {
            panic!("expected one RepoMap record");
        };
        assert_eq!(name.as_str(), "Required");
        fields.push((
            ciborium::Value::Text("state_root".into()),
            ciborium::Value::Text("/candidate/repomap".into()),
        ));
        assert!(
            ciborium::de::from_reader::<NativeCutoverHistoricalRepoMapV6, _>(
                encoded(&foreign_path).as_slice()
            ).is_err()
        );
        for publication in [
            NativeCutoverPreviousPublicationV6::SourceOnlyV7(
                NativeCutoverPreviousSourceOnlyLineageV7 {
                    initial_full: previous,
                    successors: Vec::new(),
                },
            ),
            NativeCutoverPreviousPublicationV6::HistoricalV5(historical),
            NativeCutoverPreviousPublicationV6::HistoricalV5(required.clone()),
        ] {
            wrong.source_only_v7.previous_native = Some(NativeCutoverPreviousNativeV6 {
                coverage: coverage.clone(),
                publication,
            });
            assert_eq!(
                decode_native_cutover_evidence_envelope_v1(&encoded(&wrong)).unwrap(),
                NativeCutoverEvidenceEnvelopeV1::SourceOnlyV7(wrong.source_only_v7.clone()),
            );
            wrong
                .source_only_v7
                .previous_native
                .as_mut()
                .unwrap()
                .coverage
                .previous_root_nonce = Digest([0x22; 32]);
            assert!(decode_native_cutover_evidence_envelope_v1(&encoded(&wrong)).is_err());
        }
        let mut old_successor = old_initial.clone();
        old_successor.target.operation.sequence += 1;
        old_successor.target.event_predecessor = 1;
        old_successor.target.event_sequence = 2;
        old_successor.request.operation = old_successor.target.operation;
        let UpdateKind::SourceAdvance(transition) = &mut old_successor.request.kind else {
            panic!("old V7 successor fixture must advance Source");
        };
        transition.events[0].predecessor = 1;
        transition.events[0].sequence = 2;
        old_successor.writer_ack.publish_generation += 1;
        old_successor.source.target_snapshot_id += 1;
        old_successor.source.transition_digest = NativeCutoverDigest64V1([b'9'; 64]);
        old_successor.writer_ack.source_transition_digest = old_successor.source.transition_digest;
        let mut old_coverage = coverage.clone();
        old_coverage.previous_source_operation = old_successor.target.operation;
        old_coverage.previous_source = old_successor.source;
        wrong.source_only_v7.previous_native = Some(NativeCutoverPreviousNativeV6 {
            coverage: old_coverage,
            publication: NativeCutoverPreviousPublicationV6::SourceOnlyV7(
                NativeCutoverPreviousSourceOnlyLineageV7 {
                    initial_full: old_initial,
                    successors: vec![old_successor],
                },
            ),
        });
        assert_eq!(
            decode_native_cutover_evidence_envelope_v1(&encoded(&wrong)).unwrap(),
            NativeCutoverEvidenceEnvelopeV1::SourceOnlyV7(wrong.source_only_v7.clone())
        );
        {
            use crate::native_cutover_evidence_v1::NativeCutoverEvidenceSourceFactV1 as Fact;
            use crate::native_cutover_source_join_v1::{
                imported_native_sibling_fixture_v1, maximum_width_native_import_facts_v1,
                validate_source_only_native_imports_v7,
            };

            let (import_fixture, full) = imported_native_sibling_fixture_v1();
            let mut retirement = wrong.source_only_v7.clone();
            let old_nonce = full.origin.old_root_nonce;
            retirement
                .previous_native
                .as_mut()
                .unwrap()
                .coverage
                .previous_root_nonce = old_nonce;
            let mut before: serde_json::Value =
                serde_json::from_slice(&retirement.before_selection.document.0).unwrap();
            before["native_boot_receipt"]["root_nonce_hex"] =
                String::from_utf8(digest_hex_v6(old_nonce).to_vec())
                    .unwrap()
                    .into();
            retirement.before_selection.document.0 = serde_json::to_vec(&before).unwrap();
            retirement.source_facts.push(
                import_fixture
                    .source_facts
                    .into_iter()
                    .find(|fact| matches!(fact, Fact::ImportedNativeSource { .. }))
                    .unwrap(),
            );
            retirement.validate_structure_v6().unwrap();
            let validate = |evidence: &NativeCutoverSourceOnlyEvidenceV6, candidate_nonce| {
                validate_source_only_native_imports_v7(evidence, candidate_nonce, &mut |_| {
                    Ok::<_, ()>(())
                })
            };
            assert!(matches!(
                validate(&retirement, full.candidate_root_nonce),
                Ok(1)
            ));
            assert!(validate(&retirement, Digest([0x22; 32])).is_err());

            let binding = full.source_binding;
            for invalid_binding in [
                NativeCutoverEvidenceSourcePublicationBindingV1 {
                    target_snapshot_id: 0,
                    ..binding
                },
                NativeCutoverEvidenceSourcePublicationBindingV1 {
                    transition_digest: NativeCutoverDigest64V1([b'G'; 64]),
                    ..binding
                },
                NativeCutoverEvidenceSourcePublicationBindingV1 {
                    completion_pointer_digest: NativeCutoverDigest64V1([b'0'; 64]),
                    ..binding
                },
            ] {
                let mut invalid = retirement.clone();
                let Some(Fact::ImportedNativeSource { head, .. }) = invalid.source_facts.last_mut()
                else {
                    panic!("expected Native import");
                };
                head.original_snapshot_id = invalid_binding.target_snapshot_id;
                head.source_binding = invalid_binding;
                let bytes = encoded(&NativeCutoverSourceOnlyWireV6 {
                    source_only_v7: invalid,
                });
                assert!(matches!(
                    decode_native_cutover_evidence_envelope_v1(&bytes),
                    Err(DecodeError::Structure(SyntaxError::NonCanonical))
                ));
            }

            let mut maximum = retirement.clone();
            assert!(matches!(maximum.source_facts.pop(), Some(Fact::ImportedNativeSource { .. })));
            maximum.source_facts.extend(maximum_width_native_import_facts_v1());
            let maximum = NativeCutoverSourceOnlyWireV6 { source_only_v7: maximum };
            let bytes = encoded(&maximum);
            println!("SourceOnlyV7 FD6 with 368 maximal Native imports: {} bytes", bytes.len());
            assert!(bytes.len() <= super::super::MAX_NATIVE_CUTOVER_EVIDENCE_BYTES_V1);

            // Measure the same production V7 document with the previous named
            // CBOR import rows. JSON still uses that exact named form.
            let mut previous_wire: ciborium::Value = ciborium::de::from_reader(bytes.as_slice())
                .expect("encoded V7 envelope");
            let facts = previous_wire.as_map_mut().unwrap()[0].1.as_map_mut().unwrap().iter_mut()
                .find(|(key, _)| key.as_text() == Some("source_facts")).unwrap().1.as_array_mut().unwrap();
            for (value, fact) in facts.iter_mut().zip(&maximum.source_only_v7.source_facts) {
                if matches!(fact, Fact::ImportedNativeSource { .. }) {
                    let named = serde_json::to_value(fact).expect("historical named import");
                    *value = ciborium::Value::serialized(&named).expect("historical named CBOR import");
                }
            }
            let named_test = encoded(facts.last().unwrap());
            let original: Fact = ciborium::de::from_reader(named_test.as_slice())
                .expect("decode one retained named CBOR import");
            assert_eq!(&original, maximum.source_only_v7.source_facts.last().unwrap());
            let previous_bytes = encoded(&previous_wire);
            println!("SourceOnlyV7 FD6 with 368 named Native imports: {} bytes", previous_bytes.len());
            assert!(matches!(decode_native_cutover_evidence_envelope_v1(&previous_bytes),
                Err(DecodeError::Syntax(SyntaxError::TooLarge))));

            let NativeCutoverEvidenceEnvelopeV1::SourceOnlyV7(decoded) =
                decode_native_cutover_evidence_envelope_v1(&bytes).expect("production FD6 decode")
            else { panic!("expected Source-only FD6") };
            assert_eq!(decoded, maximum.source_only_v7);
            assert!(matches!(validate(&decoded, full.candidate_root_nonce), Ok(368)));
            let mut duplicate = decoded;
            let repeated = duplicate.source_facts.last().expect("final import").clone();
            duplicate.source_facts.push(repeated);
            assert!(validate(&duplicate, full.candidate_root_nonce).is_err());

            retirement.source_facts.insert(
                0,
                Fact::NativeSdkSourceBinding {
                    operation: full.source_operation.key,
                    aggregate_identity_digest: "a".repeat(64),
                    source: full.source_binding,
                },
            );
            assert!(validate(&retirement, full.candidate_root_nonce).is_err());
        }
        // The saved Q BOOT can select a History publication that preserves
        // this Source lane. The runtime verifies its actual SourceCut.
        let mut derived_boot = wrong.clone();
        let mut before: serde_json::Value = serde_json::from_slice(
            &derived_boot.source_only_v7.before_selection.document.0,
        ).unwrap();
        let boot = &mut before["native_boot_receipt"]["manifest_operation"];
        let mut history_operation: OperationKey = serde_json::from_value(boot.clone()).unwrap();
        history_operation.stream_epoch += 1;
        history_operation.sequence += 100;
        *boot = serde_json::to_value(history_operation).unwrap();
        derived_boot.source_only_v7.before_selection.document.0 = serde_json::to_vec(&before).unwrap();
        assert!(decode_native_cutover_evidence_envelope_v1(&encoded(&derived_boot)).is_ok());
        history_operation.epoch += 1;
        before["native_boot_receipt"]["manifest_operation"] =
            serde_json::to_value(history_operation).unwrap();
        derived_boot.source_only_v7.before_selection.document.0 = serde_json::to_vec(&before).unwrap();
        assert!(decode_native_cutover_evidence_envelope_v1(&encoded(&derived_boot)).is_err());
        if let Some(NativeCutoverPreviousNativeV6 {
            publication: NativeCutoverPreviousPublicationV6::SourceOnlyV7(lineage), ..
        }) = wrong.source_only_v7.previous_native.as_mut() {
            lineage.successors.clear();
        }
        assert!(decode_native_cutover_evidence_envelope_v1(&encoded(&wrong)).is_err());
        let mut swapped = required;
        if let NativeCutoverHistoricalRepoMapV6::Required { publish, activation, .. } = &mut swapped.repomap {
            std::mem::swap(publish, activation);
        }
        wrong.source_only_v7.previous_native = Some(NativeCutoverPreviousNativeV6 {
            coverage,
            publication: NativeCutoverPreviousPublicationV6::HistoricalV5(swapped),
        });
        assert!(decode_native_cutover_evidence_envelope_v1(&encoded(&wrong)).is_err());
    }
}
