//! One canonical incremental IR shared by native SDKs and the application core.
//! Live leases and reclaim authority are intentionally absent from this DTO surface.
pub use quanta_index_contract_base::index_refs::*;

/// Source path byte limit, leaving five bytes for the canonical path map key
/// namespace within the native 4096-byte map key format.
pub const MAX_SOURCE_PATH_BYTES: usize = 4091;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MapRoot(pub Option<ObjectRef>);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceCut {
    pub owners: MapRoot,
    pub paths: MapRoot,
    pub frontiers: MapRoot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRecord {
    pub path: String,
    pub bytes: ObjectRef,
    pub byte_len: u64,
}

/// One bounded native source intake item. A batch preserves this order in its
/// returned source-record references.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceInput {
    pub path: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnerMutation {
    pub owner: OwnerKey,
    pub expected: Option<ObjectRef>,
    pub next: Option<ObjectRef>,
}

impl OwnerMutation {
    /// An expected stamp is historical CAS metadata, never a physical edge.
    #[must_use]
    pub fn object_children(&self) -> Vec<ObjectRef> {
        self.next.into_iter().collect()
    }
}

/// A complete, sorted owner-key mutation map. `digest` commits to the
/// canonical item stream, independently of map page layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnerMutationSet {
    pub root: MapRoot,
    pub count: u64,
    pub digest: Digest,
}

impl OwnerMutationSet {
    #[must_use]
    pub fn object_children(self) -> Vec<ObjectRef> {
        self.root.0.into_iter().collect()
    }
}

/// Durable pre-admission input custody. The catalog owns this fence; callers
/// cannot turn an ingress into an operation attempt or publisher.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IngressRef {
    pub operation: OperationKey,
    pub fence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceEvent {
    pub producer: ProducerId,
    pub stream: StreamId,
    pub stream_epoch: u64,
    pub predecessor: u64,
    pub sequence: u64,
    pub payload: Digest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceTransition {
    pub expected_source: SourceCut,
    pub owners: OwnerMutationSet,
    pub events: Vec<SourceEvent>,
}

impl SourceTransition {
    /// Native capture payload identity. Event positions are excluded so an
    /// operation retry cannot redefine the immutable captured owner transition.
    pub fn owner_digest(&self) -> Result<Digest, serde_json::Error> {
        use sha2::{Digest as _, Sha256};
        let bytes = serde_json::to_vec(&(self.expected_source, &self.owners))?;
        let mut hash = Sha256::new();
        hash.update(b"quanta-index-native-source-transition-v1\0");
        hash.update(bytes);
        Ok(Digest(hash.finalize().into()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputBinding {
    pub root: MapRoot,
    pub schema: SchemaId,
    pub policy: Digest,
    pub context: Digest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoverageStatus {
    Pending { previous_missing: MapRoot },
    Partial { missing: MapRoot },
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoverageBinding {
    pub instance: IndexInstanceId,
    pub desired: InputBinding,
    pub covered: Option<InputBinding>,
    pub searchable: Option<ObjectRef>,
    pub status: CoverageStatus,
    /// Owner-local desired/covered state. Missing owners are rooted separately
    /// so one completion never scans every owner to establish completeness.
    pub owners: MapRoot,
}

/// Per-owner materialization identity. Admission limits are excluded from this
/// binding; output-affecting recipe/model/producer context are included.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnerInputBinding {
    pub source: Option<ObjectRef>,
    pub projection: Option<ObjectRef>,
    pub producer_context: Digest,
    pub derivation_policy: Digest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnerCoverage {
    pub owner: OwnerKey,
    pub desired: OwnerInputBinding,
    pub covered: Option<OwnerInputBinding>,
    pub searchable: Option<ObjectRef>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectionBinding {
    pub projection: ProjectionId,
    pub producer: ProducerId,
    pub owners: MapRoot,
    pub source: SourceCut,
    pub schema: SchemaId,
    pub producer_policy: Digest,
    pub dependency_context: Digest,
    pub certificate: ObjectRef,
}

impl ProjectionBinding {
    /// Global projection context; owner reuse still uses each contribution stamp.
    #[must_use]
    pub fn materialization_context(&self) -> Digest {
        use sha2::{Digest as _, Sha256};
        let mut hash = Sha256::new();
        hash.update(b"quanta-index-projection-context-v1\0");
        hash.update(self.projection.0);
        hash.update(self.producer.0);
        hash.update(self.schema.0);
        hash.update(self.producer_policy.0);
        hash.update(self.dependency_context.0);
        Digest(hash.finalize().into())
    }

    /// Only current dependencies are physical children. Historical stamps are
    /// checked against a pinned base and must not retain an ancestor chain.
    #[must_use]
    pub fn object_children(self) -> Vec<ObjectRef> {
        let mut children = vec![self.certificate];
        children.extend(self.owners.0);
        children.extend(source_object_children(self.source));
        canonical_object_children(children)
    }
}

/// One complete producer-owned contribution. A present envelope with no output
/// is a certified empty result; failed or skipped owners have no envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectionOwnerContribution {
    pub projection: ProjectionId,
    pub producer: ProducerId,
    pub owner: OwnerKey,
    /// Canonical source owner when the contribution is tied to a source file.
    pub source_owner: Option<OwnerKey>,
    pub source: Option<ObjectRef>,
    pub dependency_stamp: Digest,
    pub output: Option<ObjectRef>,
}

impl ProjectionOwnerContribution {
    /// Stable owner context shared by publication and adapter verification.
    #[must_use]
    pub fn materialization_context(&self) -> Digest {
        use sha2::{Digest as _, Sha256};
        let mut hash = Sha256::new();
        hash.update(b"quanta-index-projection-owner-context-v1\0");
        hash.update(self.projection.0);
        hash.update(self.producer.0);
        hash.update(self.dependency_stamp.0);
        Digest(hash.finalize().into())
    }

    #[must_use]
    pub fn object_children(self) -> Vec<ObjectRef> {
        canonical_object_children([self.source, self.output].into_iter().flatten().collect())
    }
}

/// Producer attestation for the entire affected owner set, including owners
/// whose source bytes did not change. Index validates binding, not truthfulness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectionCertificate {
    pub projection: ProjectionId,
    pub producer: ProducerId,
    pub prior: Option<ProjectionBinding>,
    pub target_source: SourceCut,
    pub schema: SchemaId,
    pub producer_policy: Digest,
    pub dependency_context: Digest,
    pub affected: OwnerMutationSet,
}

impl ProjectionCertificate {
    /// `prior` is a comparison stamp, not live custody of historical roots.
    #[must_use]
    pub fn object_children(&self) -> Vec<ObjectRef> {
        let mut children = source_object_children(self.target_source);
        children.extend(self.affected.object_children());
        canonical_object_children(children)
    }
}

fn source_object_children(source: SourceCut) -> Vec<ObjectRef> {
    canonical_object_children(
        [source.owners.0, source.paths.0, source.frontiers.0]
            .into_iter()
            .flatten()
            .collect(),
    )
}

fn canonical_object_children(mut children: Vec<ObjectRef>) -> Vec<ObjectRef> {
    children.sort_unstable();
    children.dedup();
    children
}

/// The producer owns affected-set completeness. A missing attestation never
/// becomes an empty successful replacement or evidence of unaffected owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProducerCompleteness {
    CompleteAffectedDomain { certificate: ObjectRef },
    Partial { missing: MapRoot },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectionTransition {
    pub projection: ProjectionId,
    pub producer: ProducerId,
    pub expected: Option<ProjectionBinding>,
    pub target_source: SourceCut,
    pub schema: SchemaId,
    pub producer_policy: Digest,
    pub dependency_context: Digest,
    pub completeness: ProducerCompleteness,
    /// Complete replacement per affected owner; omission inherits other owners.
    pub owners: OwnerMutationSet,
    /// Separate producer stream frontiers; every payload binds this transition.
    pub events: Vec<SourceEvent>,
}

impl ProjectionTransition {
    pub fn payload_digest(&self) -> Result<Digest, serde_json::Error> {
        use sha2::{Digest as _, Sha256};
        let bytes = serde_json::to_vec(&(
            self.projection,
            self.producer,
            &self.expected,
            self.target_source,
            self.schema,
            self.producer_policy,
            self.dependency_context,
            self.completeness,
            &self.owners,
        ))?;
        let mut hash = Sha256::new();
        hash.update(b"quanta-index-projection-transition-v1\0");
        hash.update(bytes);
        Ok(Digest(hash.finalize().into()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkScope {
    Owner {
        owner: OwnerKey,
        expected: OwnerInputBinding,
    },
    ProjectionDomain {
        projection: ProjectionId,
        source: SourceCut,
        producer_policy: Digest,
        dependency_context: Digest,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkIntent {
    pub workspace: WorkspaceKey,
    pub epoch: u64,
    pub instance: IndexInstanceId,
    pub scope: WorkScope,
}

impl WorkIntent {
    pub fn identity_digest(&self) -> Result<Digest, serde_json::Error> {
        use sha2::{Digest as _, Sha256};
        let mut hash = Sha256::new();
        hash.update(b"quanta-index-owner-work-v1\0");
        hash.update(serde_json::to_vec(self)?);
        Ok(Digest(hash.finalize().into()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkClaim {
    pub intent: WorkIntent,
    pub fence: u64,
    /// Server catalog wall-clock lease; never inferred from a view sequence.
    pub lease_until_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkStatus {
    Unknown,
    Pending,
    Claimed(WorkClaim),
    Completed(ViewRef),
    Superseded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnerMemberBinding {
    pub instance: IndexInstanceId,
    pub owner: OwnerKey,
    pub input: OwnerInputBinding,
    pub format: StorageFormatId,
    pub artifact: ObjectRef,
    pub logical_commitment: Digest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemberBinding {
    pub instance: IndexInstanceId,
    pub format: StorageFormatId,
    pub artifact: ObjectRef,
    pub input: InputBinding,
    pub logical_commitment: Digest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupActivation {
    CoreRequired,
    AsyncCoverage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstanceSpec {
    pub instance: IndexInstanceId,
    pub kind: IndexKind,
    pub group: PublicationGroupId,
    pub activation: GroupActivation,
    pub input_domain: InputDomain,
    pub schema: SchemaId,
    pub policy: Digest,
    pub format: StorageFormatId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputDomain {
    SourceLocal,
    Projection {
        projection: ProjectionId,
        producer: ProducerId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexProfile {
    /// Bounded configuration, not corpus-sized membership.
    pub instances: Vec<InstanceSpec>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogView {
    pub identity: ViewRef,
    pub profile: ObjectRef,
    pub source: SourceCut,
    pub projections: MapRoot,
    pub members: MapRoot,
    /// Desired/covered/searchable state for async instances is independently rooted.
    pub coverage: MapRoot,
}

impl CatalogView {
    /// Fixed canonical field order, excluding this digest's own value.
    pub fn identity_digest(&self) -> Result<Digest, serde_json::Error> {
        use sha2::{Digest as _, Sha256};
        let mut view = self.clone();
        view.identity.digest = Digest([0; 32]);
        let bytes = serde_json::to_vec(&view)?;
        let mut hash = Sha256::new();
        hash.update(b"quanta-index-vnext\0");
        hash.update(12_u64.to_be_bytes());
        hash.update(b"catalog-view");
        let encoded_len =
            u64::try_from(bytes.len()).map_err(<serde_json::Error as serde::ser::Error>::custom)?;
        hash.update(encoded_len.to_be_bytes());
        hash.update(bytes);
        Ok(Digest(hash.finalize().into()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpdateKind {
    SourceAdvance(SourceTransition),
    ProjectionAdvance(Box<ProjectionTransition>),
    Reconfigure {
        profile: ObjectRef,
    },
    /// Completion can rebase over unrelated source/view advances. The owner's
    /// exact desired input and live claim fence remain mandatory.
    Materialize {
        claim: WorkClaim,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateRequest {
    pub operation: OperationKey,
    pub expected_view: ViewRef,
    pub kind: UpdateKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImmutableOperation {
    pub key: OperationKey,
    /// Digest of the complete canonical request; no self-referential digest field.
    pub request: ObjectRef,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    Conflict,
    FenceLost,
    InvalidInput,
    ResourceLimited,
    BackendViolation,
    RebuildRequired,
    MaintenanceRequired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationStatus {
    Unknown,
    Pending(AttemptRef),
    Published(ViewRef),
    Refused(Refusal),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewSelection {
    Latest { workspace: WorkspaceKey, epoch: u64 },
    Exact(ViewRef),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubmitOutcome {
    Accepted(AttemptRef),
    Terminal(OperationStatus),
    Unknown(OperationKey),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationReply<T> {
    pub operation: OperationKey,
    pub payload: T,
}

// Closed manual codecs preserve the named JSON/CBOR representation.
quanta_index_contract_base::native_serde_newtype!(MapRoot, Option<ObjectRef>);
quanta_index_contract_base::native_serde_struct!(SourceCut {
    owners: MapRoot,
    paths: MapRoot,
    frontiers: MapRoot
});
quanta_index_contract_base::native_serde_struct!(SourceRecord {
    path: String,
    bytes: ObjectRef,
    byte_len: u64
});
quanta_index_contract_base::native_serde_struct!(SourceInput {
    path: String,
    bytes: Vec<u8>
});
quanta_index_contract_base::native_serde_struct!(OwnerMutation { owner: OwnerKey, expected: Option<ObjectRef>, next: Option<ObjectRef> });
quanta_index_contract_base::native_serde_struct!(OwnerMutationSet {
    root: MapRoot,
    count: u64,
    digest: Digest
});
quanta_index_contract_base::native_serde_struct!(IngressRef {
    operation: OperationKey,
    fence: u64
});
quanta_index_contract_base::native_serde_struct!(SourceEvent {
    producer: ProducerId,
    stream: StreamId,
    stream_epoch: u64,
    predecessor: u64,
    sequence: u64,
    payload: Digest
});
quanta_index_contract_base::native_serde_struct!(SourceTransition { expected_source: SourceCut, owners: OwnerMutationSet, events: Vec<SourceEvent> });
quanta_index_contract_base::native_serde_struct!(InputBinding {
    root: MapRoot,
    schema: SchemaId,
    policy: Digest,
    context: Digest
});
quanta_index_contract_base::native_serde_enum!(CoverageStatus { units { Complete = 2 } newtypes {  } records { Pending = 0 { previous_missing: MapRoot }, Partial = 1 { missing: MapRoot } } });
quanta_index_contract_base::native_serde_struct!(CoverageBinding { instance: IndexInstanceId, desired: InputBinding, covered: Option<InputBinding>, searchable: Option<ObjectRef>, status: CoverageStatus, owners: MapRoot });
quanta_index_contract_base::native_serde_struct!(OwnerInputBinding { source: Option<ObjectRef>, projection: Option<ObjectRef>, producer_context: Digest, derivation_policy: Digest });
quanta_index_contract_base::native_serde_struct!(OwnerCoverage { owner: OwnerKey, desired: OwnerInputBinding, covered: Option<OwnerInputBinding>, searchable: Option<ObjectRef> });
quanta_index_contract_base::native_serde_struct!(ProjectionBinding {
    projection: ProjectionId,
    producer: ProducerId,
    owners: MapRoot,
    source: SourceCut,
    schema: SchemaId,
    producer_policy: Digest,
    dependency_context: Digest,
    certificate: ObjectRef
});
quanta_index_contract_base::native_serde_struct!(ProjectionOwnerContribution { projection: ProjectionId, producer: ProducerId, owner: OwnerKey, source_owner: Option<OwnerKey>, source: Option<ObjectRef>, dependency_stamp: Digest, output: Option<ObjectRef> });
quanta_index_contract_base::native_serde_struct!(ProjectionCertificate { projection: ProjectionId, producer: ProducerId, prior: Option<ProjectionBinding>, target_source: SourceCut, schema: SchemaId, producer_policy: Digest, dependency_context: Digest, affected: OwnerMutationSet });
quanta_index_contract_base::native_serde_enum!(ProducerCompleteness { units {  } newtypes {  } records { CompleteAffectedDomain = 0 { certificate: ObjectRef }, Partial = 1 { missing: MapRoot } } });
quanta_index_contract_base::native_serde_struct!(ProjectionTransition { projection: ProjectionId, producer: ProducerId, expected: Option<ProjectionBinding>, target_source: SourceCut, schema: SchemaId, producer_policy: Digest, dependency_context: Digest, completeness: ProducerCompleteness, owners: OwnerMutationSet, events: Vec<SourceEvent> });
quanta_index_contract_base::native_serde_enum!(WorkScope { units {  } newtypes {  } records { Owner = 0 { owner: OwnerKey, expected: OwnerInputBinding }, ProjectionDomain = 1 { projection: ProjectionId, source: SourceCut, producer_policy: Digest, dependency_context: Digest } } });
quanta_index_contract_base::native_serde_struct!(WorkIntent {
    workspace: WorkspaceKey,
    epoch: u64,
    instance: IndexInstanceId,
    scope: WorkScope
});
quanta_index_contract_base::native_serde_struct!(WorkClaim {
    intent: WorkIntent,
    fence: u64,
    lease_until_ms: u64
});
quanta_index_contract_base::native_serde_enum!(WorkStatus { units { Unknown = 0, Pending = 1, Superseded = 4 } newtypes { Claimed = 2 => WorkClaim, Completed = 3 => ViewRef } records {  } });
quanta_index_contract_base::native_serde_struct!(OwnerMemberBinding {
    instance: IndexInstanceId,
    owner: OwnerKey,
    input: OwnerInputBinding,
    format: StorageFormatId,
    artifact: ObjectRef,
    logical_commitment: Digest
});
quanta_index_contract_base::native_serde_struct!(MemberBinding {
    instance: IndexInstanceId,
    format: StorageFormatId,
    artifact: ObjectRef,
    input: InputBinding,
    logical_commitment: Digest
});
quanta_index_contract_base::native_serde_enum!(GroupActivation { units { CoreRequired = 0, AsyncCoverage = 1 } newtypes {  } records {  } });
quanta_index_contract_base::native_serde_struct!(InstanceSpec {
    instance: IndexInstanceId,
    kind: IndexKind,
    group: PublicationGroupId,
    activation: GroupActivation,
    input_domain: InputDomain,
    schema: SchemaId,
    policy: Digest,
    format: StorageFormatId
});
quanta_index_contract_base::native_serde_enum!(InputDomain { units { SourceLocal = 0 } newtypes {  } records { Projection = 1 { projection: ProjectionId, producer: ProducerId } } });
quanta_index_contract_base::native_serde_struct!(IndexProfile { instances: Vec<InstanceSpec> });
quanta_index_contract_base::native_serde_struct!(CatalogView {
    identity: ViewRef,
    profile: ObjectRef,
    source: SourceCut,
    projections: MapRoot,
    members: MapRoot,
    coverage: MapRoot
});
quanta_index_contract_base::native_serde_enum!(UpdateKind { units {  } newtypes { SourceAdvance = 0 => SourceTransition, ProjectionAdvance = 1 => Box<ProjectionTransition> } records { Reconfigure = 2 { profile: ObjectRef }, Materialize = 3 { claim: WorkClaim } } });
quanta_index_contract_base::native_serde_struct!(UpdateRequest {
    operation: OperationKey,
    expected_view: ViewRef,
    kind: UpdateKind
});
quanta_index_contract_base::native_serde_struct!(ImmutableOperation {
    key: OperationKey,
    request: ObjectRef
});
quanta_index_contract_base::native_serde_enum!(Refusal { units { Conflict = 0, FenceLost = 1, InvalidInput = 2, ResourceLimited = 3, BackendViolation = 4, RebuildRequired = 5, MaintenanceRequired = 6 } newtypes {  } records {  } });
quanta_index_contract_base::native_serde_enum!(OperationStatus { units { Unknown = 0 } newtypes { Pending = 1 => AttemptRef, Published = 2 => ViewRef, Refused = 3 => Refusal } records {  } });
quanta_index_contract_base::native_serde_enum!(ViewSelection { units {  } newtypes { Exact = 1 => ViewRef } records { Latest = 0 { workspace: WorkspaceKey, epoch: u64 } } });
quanta_index_contract_base::native_serde_enum!(SubmitOutcome { units {  } newtypes { Accepted = 0 => AttemptRef, Terminal = 1 => OperationStatus, Unknown = 2 => OperationKey } records {  } });

impl<T: serde::Serialize> serde::Serialize for OperationReply<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("OperationReply", 2)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "operation", &self.operation)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "payload", &self.payload)?;
        serde::ser::SerializeStruct::end(state)
    }
}

impl<'de, T: serde::Deserialize<'de>> serde::Deserialize<'de> for OperationReply<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &["operation", "payload"];
        struct ReplyVisitor<T>(std::marker::PhantomData<fn() -> T>);
        impl<'de, T: serde::Deserialize<'de>> serde::de::Visitor<'de> for ReplyVisitor<T> {
            type Value = OperationReply<T>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a complete original-operation reply")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut operation = None;
                let mut payload = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "operation" => {
                            if operation.is_some() {
                                return Err(serde::de::Error::duplicate_field("operation"));
                            }
                            operation = Some(map.next_value::<OperationKey>()?);
                        }
                        "payload" => {
                            if payload.is_some() {
                                return Err(serde::de::Error::duplicate_field("payload"));
                            }
                            payload = Some(map.next_value::<T>()?);
                        }
                        _ => return Err(serde::de::Error::unknown_field(&key, FIELDS)),
                    }
                }
                Ok(OperationReply {
                    operation: operation
                        .ok_or_else(|| serde::de::Error::missing_field("operation"))?,
                    payload: payload.ok_or_else(|| serde::de::Error::missing_field("payload"))?,
                })
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let operation = seq
                    .next_element::<OperationKey>()?
                    .ok_or_else(|| serde::de::Error::missing_field("operation"))?;
                let payload = seq
                    .next_element::<T>()?
                    .ok_or_else(|| serde::de::Error::missing_field("payload"))?;
                if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(serde::de::Error::invalid_length(3, &self));
                }
                Ok(OperationReply { operation, payload })
            }
        }
        deserializer.deserialize_struct(
            "OperationReply",
            FIELDS,
            ReplyVisitor(std::marker::PhantomData),
        )
    }
}
