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

/// Explicit producer admission for lexical text. Raw-byte and projection
/// consumers retain their separate input policies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceTextAdmission {
    Excluded,
    Utf8 { policy: Digest },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRecord {
    pub path: String,
    pub bytes: ObjectRef,
    pub byte_len: u64,
    pub text: SourceTextAdmission,
}

/// One bounded native source intake item. A batch preserves this order in its
/// returned source-record references.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceInput {
    pub path: String,
    pub bytes: Vec<u8>,
    pub text: SourceTextAdmission,
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
/// canonical item stream, including each owner's expected and next stamp,
/// independently of map page layout.
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

/// Persisted native index state format, shared by the catalog and runtime.
pub const NATIVE_INDEX_STATE_VERSION: u32 = 18;
/// Persisted native source-event capture payload format.
pub const NATIVE_SOURCE_CAPTURE_VERSION: u32 = 2;
/// Hash domain for logical owner mutation capture, independent of execution base.
pub const NATIVE_SOURCE_CAPTURE_HASH_DOMAIN: &[u8] = b"quanta-index-native-source-capture-v2\0";

impl SourceTransition {
    /// Logical capture identity for immutable source-event claims. The count
    /// and digest commit to the canonical mutation stream, including each
    /// owner's expected and next stamp. The execution `SourceCut`, physical map
    /// root and event positions are excluded so a refused operation can retry
    /// the same capture against a new exact execution base.
    pub fn owner_digest(&self) -> Result<Digest, serde_json::Error> {
        use sha2::{Digest as _, Sha256};
        struct CaptureWriter(Sha256);
        impl std::io::Write for CaptureWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.update(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writer = CaptureWriter(Sha256::new());
        writer.0.update(NATIVE_SOURCE_CAPTURE_HASH_DOMAIN);
        // Preserve the canonical encoder and byte sequence without creating
        // an intermediate owned JSON buffer in producer/replay validation.
        serde_json::to_writer(&mut writer, &(self.owners.count, self.owners.digest))?;
        Ok(Digest(writer.0.finalize().into()))
    }
}

#[cfg(test)]
mod source_capture_tests {
    use super::{
        Digest, MapRoot, ObjectRef, OwnerMutationSet, SourceCut, SourceTransition,
        NATIVE_INDEX_STATE_VERSION, NATIVE_SOURCE_CAPTURE_HASH_DOMAIN,
        NATIVE_SOURCE_CAPTURE_VERSION,
    };

    #[test]
    fn source_capture_v2_has_independent_golden_and_exact_logical_inputs() {
        // Independently computed as SHA-256(domain || JSON([2, [165; 32]])).
        const GOLDEN: [u8; 32] = [
            0x6e, 0x7d, 0x2e, 0xa9, 0xaa, 0xec, 0xd9, 0x46, 0x23, 0xb7, 0xbb, 0x2e, 0x4b, 0x46,
            0x40, 0x8b, 0x26, 0xa6, 0xa1, 0xca, 0xa5, 0xab, 0x69, 0xcf, 0xb6, 0xd0, 0x1d, 0xc1,
            0xbe, 0x88, 0x40, 0xe2,
        ];
        assert_eq!(NATIVE_INDEX_STATE_VERSION, 18);
        assert_eq!(NATIVE_SOURCE_CAPTURE_VERSION, 2);
        assert_eq!(
            NATIVE_SOURCE_CAPTURE_HASH_DOMAIN,
            b"quanta-index-native-source-capture-v2\0"
        );
        let mut transition = SourceTransition {
            expected_source: SourceCut {
                owners: MapRoot(None),
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            },
            owners: OwnerMutationSet {
                root: MapRoot(None),
                count: 2,
                digest: Digest([165; 32]),
            },
            events: Vec::new(),
        };
        let captured = transition.owner_digest().map_err(|error| error.to_string());
        assert_eq!(captured, Ok(Digest(GOLDEN)));

        transition.expected_source.owners = MapRoot(Some(ObjectRef(Digest([1; 32]))));
        transition.expected_source.paths = MapRoot(Some(ObjectRef(Digest([2; 32]))));
        transition.expected_source.frontiers = MapRoot(Some(ObjectRef(Digest([3; 32]))));
        transition.owners.root = MapRoot(Some(ObjectRef(Digest([4; 32]))));
        assert_eq!(
            transition.owner_digest().map_err(|error| error.to_string()),
            captured
        );

        transition.owners.count = 3;
        let changed_count = transition.owner_digest().map_err(|error| error.to_string());
        assert!(changed_count.is_ok());
        assert_ne!(changed_count, captured);
        transition.owners.count = 2;
        transition.owners.digest = Digest([166; 32]);
        let changed_digest = transition.owner_digest().map_err(|error| error.to_string());
        assert!(changed_digest.is_ok());
        assert_ne!(changed_digest, captured);
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
        match self.payload_digest_with_work_v3(&mut |_| Ok::<(), core::convert::Infallible>(())) {
            Ok(digest) => Ok(digest),
            Err(ProjectionPayloadDigestErrorV3::Encode(cause)) => Err(cause),
            Err(ProjectionPayloadDigestErrorV3::Arithmetic) => {
                Err(<serde_json::Error as serde::ser::Error>::custom(
                    "projection payload JSON length overflow",
                ))
            }
            Err(ProjectionPayloadDigestErrorV3::Work(never)) => match never {},
        }
    }

    /// Hash the canonical event payload without allocating an encoded copy.
    /// The callback pays before every JSON chunk and the fixed hash domain.
    pub fn payload_digest_with_work_v3<E>(
        &self,
        work: &mut impl FnMut(usize) -> Result<(), E>,
    ) -> Result<Digest, ProjectionPayloadDigestErrorV3<E>> {
        use sha2::{Digest as _, Sha256};
        const DOMAIN: &[u8] = b"quanta-index-projection-transition-v1\0";
        let payload = (
            self.projection,
            self.producer,
            &self.expected,
            self.target_source,
            self.schema,
            self.producer_policy,
            self.dependency_context,
            self.completeness,
            &self.owners,
        );
        let mut hash = Sha256::new();
        work(DOMAIN.len()).map_err(ProjectionPayloadDigestErrorV3::Work)?;
        hash.update(DOMAIN);
        emit_paid_identity_json_v3(&payload, ViewDigestSinkV3::Hash(&mut hash), work).map_err(
            |cause| match cause {
                CatalogViewDigestErrorV3::Work(cause) => {
                    ProjectionPayloadDigestErrorV3::Work(cause)
                }
                CatalogViewDigestErrorV3::Arithmetic => ProjectionPayloadDigestErrorV3::Arithmetic,
                CatalogViewDigestErrorV3::Encode(cause) => {
                    ProjectionPayloadDigestErrorV3::Encode(cause)
                }
            },
        )?;
        Ok(Digest(hash.finalize().into()))
    }
}

#[derive(Debug)]
pub enum ProjectionPayloadDigestErrorV3<E> {
    Work(E),
    Arithmetic,
    Encode(serde_json::Error),
}

#[cfg(test)]
#[path = "projection_payload_digest_tests_v3.rs"]
mod projection_payload_digest_tests_v3;

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
        struct HashWriter<'a>(&'a mut Sha256);
        impl std::io::Write for HashWriter<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.update(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut hash = Sha256::new();
        hash.update(b"quanta-index-owner-work-v1\0");
        serde_json::to_writer(HashWriter(&mut hash), self)?;
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
    Blocked(Refusal),
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
        match self.identity_digest_with_work_v3(&mut |_| Ok::<(), core::convert::Infallible>(())) {
            Ok(digest) => Ok(digest),
            Err(CatalogViewDigestErrorV3::Encode(cause)) => Err(cause),
            Err(CatalogViewDigestErrorV3::Arithmetic) => {
                Err(<serde_json::Error as serde::ser::Error>::custom(
                    "catalog view JSON length overflow",
                ))
            }
            Err(CatalogViewDigestErrorV3::Work(never)) => match never {},
        }
    }

    /// Stream the same canonical JSON identity without cloning the view or
    /// building an unadmitted Vec. The callback pays for both serialization
    /// passes and the fixed domain/length commitment bytes.
    pub fn identity_digest_with_work_v3<E>(
        &self,
        work: &mut impl FnMut(usize) -> Result<(), E>,
    ) -> Result<Digest, CatalogViewDigestErrorV3<E>> {
        use sha2::{Digest as _, Sha256};
        const DOMAIN: &[u8] = b"quanta-index-vnext\0";
        const LABEL: &[u8] = b"catalog-view";

        let view = CatalogViewIdentityRefV3(self);
        let mut encoded_len = 0_u64;
        emit_paid_identity_json_v3(&view, ViewDigestSinkV3::Count(&mut encoded_len), work)?;
        let mut hash = Sha256::new();
        let commitment_len = DOMAIN
            .len()
            .checked_add(8)
            .and_then(|len| len.checked_add(LABEL.len()))
            .and_then(|len| len.checked_add(8))
            .ok_or(CatalogViewDigestErrorV3::Arithmetic)?;
        work(commitment_len).map_err(CatalogViewDigestErrorV3::Work)?;
        hash.update(DOMAIN);
        hash.update(12_u64.to_be_bytes());
        hash.update(LABEL);
        hash.update(encoded_len.to_be_bytes());
        emit_paid_identity_json_v3(&view, ViewDigestSinkV3::Hash(&mut hash), work)?;
        Ok(Digest(hash.finalize().into()))
    }
}

#[derive(Debug)]
pub enum CatalogViewDigestErrorV3<E> {
    Work(E),
    Arithmetic,
    Encode(serde_json::Error),
}

struct CatalogViewIdentityRefV3<'a>(&'a CatalogView);

impl serde::Serialize for CatalogViewIdentityRefV3<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut identity = self.0.identity;
        identity.digest = Digest([0; 32]);
        let mut state = serializer.serialize_struct("CatalogView", 6)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "identity", &identity)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "profile", &self.0.profile)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "source", &self.0.source)?;
        serde::ser::SerializeStruct::serialize_field(
            &mut state,
            "projections",
            &self.0.projections,
        )?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "members", &self.0.members)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "coverage", &self.0.coverage)?;
        serde::ser::SerializeStruct::end(state)
    }
}

enum ViewDigestSinkV3<'a> {
    Count(&'a mut u64),
    Hash(&'a mut sha2::Sha256),
}

struct ViewDigestWriterV3<'a, E, F> {
    sink: ViewDigestSinkV3<'a>,
    work: &'a mut F,
    first_work_refusal: &'a mut Option<E>,
    overflow: &'a mut bool,
}

impl<E, F: FnMut(usize) -> Result<(), E>> std::io::Write for ViewDigestWriterV3<'_, E, F> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if let Err(cause) = (self.work)(bytes.len()) {
            *self.first_work_refusal = Some(cause);
            return Err(std::io::ErrorKind::Other.into());
        }
        match &mut self.sink {
            ViewDigestSinkV3::Count(count) => {
                let len = match u64::try_from(bytes.len()) {
                    Ok(len) => len,
                    Err(_overflow) => {
                        *self.overflow = true;
                        return Err(std::io::ErrorKind::InvalidData.into());
                    }
                };
                let Some(next) = count.checked_add(len) else {
                    *self.overflow = true;
                    return Err(std::io::ErrorKind::InvalidData.into());
                };
                **count = next;
            }
            ViewDigestSinkV3::Hash(hash) => {
                use sha2::Digest as _;
                hash.update(bytes);
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn emit_paid_identity_json_v3<E, T: serde::Serialize>(
    view: &T,
    sink: ViewDigestSinkV3<'_>,
    work: &mut impl FnMut(usize) -> Result<(), E>,
) -> Result<(), CatalogViewDigestErrorV3<E>> {
    let mut first_work_refusal = None;
    let mut overflow = false;
    let result = serde_json::to_writer(
        ViewDigestWriterV3 {
            sink,
            work,
            first_work_refusal: &mut first_work_refusal,
            overflow: &mut overflow,
        },
        view,
    );
    if let Some(cause) = first_work_refusal {
        return Err(CatalogViewDigestErrorV3::Work(cause));
    }
    if overflow {
        return Err(CatalogViewDigestErrorV3::Arithmetic);
    }
    result.map_err(CatalogViewDigestErrorV3::Encode)
}

#[cfg(test)]
mod catalog_view_digest_tests_v3 {
    use super::*;
    use sha2::{Digest as _, Sha256};

    fn sample_view() -> CatalogView {
        CatalogView {
            identity: ViewRef {
                workspace: WorkspaceKey {
                    tenant: TenantId([1; 16]),
                    collection: CollectionId([2; 16]),
                    worktree: WorktreeId([3; 16]),
                    slot: SlotId([4; 16]),
                },
                epoch: 7,
                sequence: 11,
                digest: Digest([9; 32]),
            },
            profile: ObjectRef(Digest([5; 32])),
            source: SourceCut {
                owners: MapRoot(Some(ObjectRef(Digest([6; 32])))),
                paths: MapRoot(None),
                frontiers: MapRoot(Some(ObjectRef(Digest([7; 32])))),
            },
            projections: MapRoot(None),
            members: MapRoot(Some(ObjectRef(Digest([8; 32])))),
            coverage: MapRoot(None),
        }
    }

    #[test]
    fn borrowed_catalog_digest_matches_original_owned_json_commitment() {
        let view = sample_view();
        let mut old = view.clone();
        old.identity.digest = Digest([0; 32]);
        let bytes = serde_json::to_vec(&old).expect("original catalog JSON");
        let mut hash = Sha256::new();
        hash.update(b"quanta-index-vnext\0");
        hash.update(12_u64.to_be_bytes());
        hash.update(b"catalog-view");
        hash.update(
            u64::try_from(bytes.len())
                .expect("bounded test JSON")
                .to_be_bytes(),
        );
        hash.update(&bytes);
        let expected = Digest(hash.finalize().into());

        let mut charged = 0_usize;
        let actual = view
            .identity_digest_with_work_v3(&mut |bytes| {
                charged += bytes;
                Ok::<_, &'static str>(())
            })
            .expect("borrowed digest");
        assert_eq!(actual, expected);
        assert_eq!(view.identity_digest().expect("ordinary facade"), expected);
        let commitment_bytes = b"quanta-index-vnext\0"
            .len()
            .checked_add(16)
            .and_then(|bytes| bytes.checked_add(b"catalog-view".len()))
            .expect("bounded commitment");
        let expected_work = bytes
            .len()
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(commitment_bytes))
            .expect("bounded fixture work");
        assert_eq!(charged, expected_work);
    }

    #[test]
    fn borrowed_catalog_digest_retains_typed_work_refusal() {
        let cause = sample_view()
            .identity_digest_with_work_v3(&mut |_| Err::<(), _>("work cap"))
            .expect_err("first byte must refuse");
        assert!(matches!(cause, CatalogViewDigestErrorV3::Work("work cap")));
    }

    #[test]
    fn closed_nested_catalog_keys_preserve_json_and_cbor_shape() {
        let view = sample_view();
        let json = serde_json::to_vec(&view).expect("catalog JSON");
        assert_eq!(
            serde_json::from_slice::<CatalogView>(&json).expect("decode"),
            view
        );
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&view, &mut cbor).expect("catalog CBOR");
        assert_eq!(
            ciborium::de::from_reader::<CatalogView, _>(cbor.as_slice()).expect("decode"),
            view
        );
        let invalid = String::from_utf8(json)
            .expect("JSON is UTF-8")
            .replace("\"frontiers\":", "\"unexpected\":");
        assert!(serde_json::from_str::<CatalogView>(&invalid).is_err());
    }

    #[test]
    fn operation_reply_keys_preserve_duplicate_missing_and_unknown_refusal() {
        let operation = OperationKey {
            workspace: sample_view().identity.workspace,
            epoch: 7,
            producer: ProducerId([8; 16]),
            stream: StreamId([9; 16]),
            stream_epoch: 10,
            sequence: 11,
        };
        let reply = OperationReply {
            operation,
            payload: 17_u64,
        };
        let json = serde_json::to_string(&reply).expect("reply JSON");
        assert_eq!(
            serde_json::from_str::<OperationReply<u64>>(&json).expect("decode"),
            reply
        );
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&reply, &mut cbor).expect("reply CBOR");
        assert_eq!(
            ciborium::de::from_reader::<OperationReply<u64>, _>(cbor.as_slice()).expect("decode"),
            reply
        );
        for invalid in [
            json.replace("\"payload\":17", "\"payload\":17,\"payload\":18"),
            json.replace(",\"payload\":17", ""),
            json.replace("\"payload\":17", "\"other\":17"),
        ] {
            assert!(serde_json::from_str::<OperationReply<u64>>(&invalid).is_err());
        }
    }
}

/// Immutable source-authority intent, persisted with the original operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceOperationMode {
    Unchanged,
    Advance,
    Resync { prior_epoch: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpdateKind {
    SourceAdvance(SourceTransition),
    /// Complete replacement of the known producer owner prefix. The producer
    /// epoch switches only when the new baseline and original receipt publish.
    SourceResync {
        prior_epoch: u64,
        transition: SourceTransition,
    },
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

impl UpdateKind {
    #[must_use]
    pub const fn source_mode(&self) -> SourceOperationMode {
        match self {
            Self::SourceAdvance(_) => SourceOperationMode::Advance,
            Self::SourceResync { prior_epoch, .. } => SourceOperationMode::Resync {
                prior_epoch: *prior_epoch,
            },
            Self::ProjectionAdvance(_) | Self::Reconfigure { .. } | Self::Materialize { .. } => {
                SourceOperationMode::Unchanged
            }
        }
    }
}

#[cfg(test)]
mod source_mode_tests {
    use super::SourceOperationMode;

    #[test]
    fn source_mode_uses_a_closed_wire_contract() {
        let golden = r#"{"Resync":{"prior_epoch":7}}"#;
        let expected = SourceOperationMode::Resync { prior_epoch: 7 };
        assert_eq!(
            serde_json::from_str::<SourceOperationMode>(golden).map_err(|error| error.to_string()),
            Ok(expected)
        );
        assert_eq!(
            serde_json::to_string(&expected).map_err(|error| error.to_string()),
            Ok(golden.to_owned())
        );
        for invalid in [
            r#"{"Resync":{}}"#,
            r#"{"Resync":{"prior_epoch":7,"epoch":8}}"#,
            r#"{"Resync":{"prior_epoch":7,"prior_epoch":8}}"#,
            r#""Reset""#,
        ] {
            assert!(serde_json::from_str::<SourceOperationMode>(invalid).is_err());
        }
    }
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
    pub source_mode: SourceOperationMode,
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
    byte_len: u64,
    text: SourceTextAdmission
});
quanta_index_contract_base::native_serde_struct!(SourceInput {
    path: String,
    bytes: Vec<u8>,
    text: SourceTextAdmission
});
quanta_index_contract_base::native_serde_enum!(SourceTextAdmission {
    units { Excluded = 0 }
    newtypes { }
    records { Utf8 = 1 { policy: Digest } }
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
quanta_index_contract_base::native_serde_enum!(WorkStatus { units { Unknown = 0, Pending = 1, Superseded = 4 } newtypes { Claimed = 2 => WorkClaim, Completed = 3 => ViewRef, Blocked = 5 => Refusal } records {  } });
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
quanta_index_contract_base::native_serde_enum!(SourceOperationMode { units { Unchanged = 0, Advance = 1 } newtypes { } records { Resync = 2 { prior_epoch: u64 } } });
quanta_index_contract_base::native_serde_enum!(UpdateKind { units {  } newtypes { SourceAdvance = 0 => SourceTransition, ProjectionAdvance = 1 => Box<ProjectionTransition> } records { Reconfigure = 2 { profile: ObjectRef }, Materialize = 3 { claim: WorkClaim }, SourceResync = 4 { prior_epoch: u64, transition: SourceTransition } } });
quanta_index_contract_base::native_serde_struct!(UpdateRequest {
    operation: OperationKey,
    expected_view: ViewRef,
    kind: UpdateKind
});
quanta_index_contract_base::native_serde_struct!(ImmutableOperation {
    key: OperationKey,
    request: ObjectRef,
    source_mode: SourceOperationMode
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
                while let Some(key) =
                    map.next_key_seed(quanta_index_contract_base::ClosedFieldKeySeedV1(FIELDS))?
                {
                    match key {
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
                        unknown => return Err(serde::de::Error::unknown_field(unknown, FIELDS)),
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
