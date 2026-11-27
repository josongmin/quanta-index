//! Source-bound chunk, parse-tree, and runtime catalog projection contracts.

use sha2::{Digest as _, Sha256};

use crate::lex::DirtyRecord;
use crate::{
    ChunkId, ChunkRecord, GenerationPin, QueryResultWindowV1, RepoId, RepoRelativePath, RevisionId,
    RuntimeCatalogIngestBatch, StructuralCandidate, StructuralTreeRecord, TextQueryRequest,
    TextQuerySyntax,
    index::{
        Digest, ObjectRef, OperationKey, OwnerKey, ProducerId, ProjectionId, ProjectionReplayStamp, SchemaId, SourceCut,
        StableOwnerId, StreamId, ViewRef,
    },
};

use super::NativeReadSessionId;

pub const NATIVE_DSL_PRODUCER_V1: ProducerId = ProducerId(*b"native-dsl-v1\0\0\0");
pub const NATIVE_DSL_SCHEMA_V1: SchemaId = SchemaId(*b"native-dsl-v1\0\0\0");
pub const NATIVE_DSL_POLICY_V1: Digest = Digest(*b"native-dsl-projection-policy-v1\0");

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeDslScopeV1 {
    pub repository: RepoId,
    pub revision: RevisionId,
}
crate::native_serde_struct!([serde_identifier] NativeDslScopeV1 {
    repository: RepoId,
    revision: RevisionId,
});

impl NativeDslScopeV1 {
    pub fn for_pin(pin: &GenerationPin) -> Self {
        Self {
            repository: pin.repo_id.clone(),
            revision: pin.revision_id.clone(),
        }
    }

    pub fn matches_pin(&self, pin: &GenerationPin) -> bool {
        self.repository == pin.repo_id && self.revision == pin.revision_id
    }

    /// The repository and revision are byte-delimited and form one projection
    /// namespace independently for each authority.
    pub fn projection(&self, kind: NativeDslKindV1) -> ProjectionId {
        // This namespace is for Source-validated corpora and catalogs bound
        // to their sealed chunk indexes. Previous projections need a fresh
        // publication from the current producer.
        self.projection_in_domain(kind, b"quanta-native-dsl-sealed-projection-v1\0")
    }

    /// Verify historical Structural acknowledgements that predate Source sealing.
    pub fn retired_structural_projection_v1(&self) -> ProjectionId {
        self.projection_in_domain(
            NativeDslKindV1::Structural,
            b"quanta-native-dsl-projection-v1\0",
        )
    }

    fn projection_in_domain(&self, kind: NativeDslKindV1, domain: &[u8]) -> ProjectionId {
        let mut hash = Sha256::new();
        hash.update(domain);
        hash.update(kind.tag());
        for component in [self.repository.as_str(), self.revision.as_str()] {
            hash.update(
                u64::try_from(component.len())
                    .unwrap_or(u64::MAX)
                    .to_be_bytes(),
            );
            hash.update(component.as_bytes());
        }
        ProjectionId(truncated_digest(hash))
    }

    pub fn stream(&self, kind: NativeDslKindV1) -> StreamId {
        StreamId(self.projection(kind).0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeDslKindV1 {
    Structural,
    RuntimeMetadata,
}
crate::native_serde_enum!([serde_identifier] NativeDslKindV1 {
    units { Structural = 0, RuntimeMetadata = 1 }
    newtypes {}
    records {}
});

impl NativeDslKindV1 {
    fn tag(self) -> [u8; 1] {
        match self {
            Self::Structural => [0],
            Self::RuntimeMetadata => [1],
        }
    }
}

pub const NATIVE_DSL_HEADER_OWNER_V1: OwnerKey = OwnerKey {
    producer: NATIVE_DSL_PRODUCER_V1,
    stable_id: StableOwnerId([0; 16]),
};

/// Distinguish the source producers as well as their stable local IDs.
pub fn native_dsl_file_owner_v1(source: OwnerKey) -> OwnerKey {
    let mut hash = Sha256::new();
    hash.update(b"quanta-native-dsl-file-v1\0");
    hash.update(source.producer.0);
    hash.update(source.stable_id.0);
    OwnerKey {
        producer: NATIVE_DSL_PRODUCER_V1,
        stable_id: StableOwnerId(truncated_digest(hash)),
    }
}

/// Projection context records the event frontier together with the producer's
/// frozen input identity. It is carried by the original projection binding.
pub fn native_dsl_context_v1(event_sequence: u64, authority: Digest) -> Digest {
    let mut hash = Sha256::new();
    hash.update(b"quanta-native-dsl-context-v1\0");
    hash.update(authority.0);
    let digest = hash.finalize();
    let mut value = [0; 32];
    value[..8].copy_from_slice(&event_sequence.to_be_bytes());
    value[8..].copy_from_slice(&digest[..24]);
    Digest(value)
}

pub fn native_dsl_sequence_v1(context: Digest) -> u64 {
    let mut sequence = [0; 8];
    sequence.copy_from_slice(&context.0[..8]);
    u64::from_be_bytes(sequence)
}

/// Match a replay against the request's exact input base, or against the
/// authenticated Source on which an installed producer froze its facts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeDslReplayBasisV1 {
    Exact {
        view: ViewRef,
        event_predecessor: u64,
    },
    Source(SourceCut),
    /// Reconcile an immutable operation only if its event is no newer than
    /// the authenticated projection currently serving on this Source.
    SourceWithServingSequence {
        source: SourceCut,
        serving_event_sequence: u64,
    },
}
crate::native_serde_enum!([serde_identifier] NativeDslReplayBasisV1 {
    units {}
    newtypes { Source = 1 => SourceCut }
    records {
        Exact = 0 { view: ViewRef, event_predecessor: u64 },
        SourceWithServingSequence = 2 { source: SourceCut, serving_event_sequence: u64 },
    }
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeDslReplayV1 {
    pub operation: OperationKey,
    pub projection: ProjectionId,
    pub basis: NativeDslReplayBasisV1,
    pub authority: Digest,
}
crate::native_serde_struct!([serde_identifier] NativeDslReplayV1 {
    operation: OperationKey,
    projection: ProjectionId,
    basis: NativeDslReplayBasisV1,
    authority: Digest,
});

impl NativeDslReplayV1 {
    #[must_use]
    pub fn matches_stamp_v1(&self, stamp: ProjectionReplayStamp) -> bool {
        let sequence = native_dsl_sequence_v1(stamp.context);
        let same_base = match self.basis {
            NativeDslReplayBasisV1::Exact { view, event_predecessor } =>
                stamp.expected_view == view && event_predecessor.checked_add(1) == Some(sequence),
            NativeDslReplayBasisV1::Source(source) => source == stamp.source,
            NativeDslReplayBasisV1::SourceWithServingSequence { source, serving_event_sequence } =>
                source == stamp.source && sequence <= serving_event_sequence,
        };
        same_base
            && stamp.projection == self.projection
            && sequence != 0
            && stamp.context == native_dsl_context_v1(sequence, self.authority)
    }
}

fn truncated_digest(hash: Sha256) -> [u8; 16] {
    let digest = hash.finalize();
    let mut identity = [0; 16];
    identity.copy_from_slice(&digest[..16]);
    identity
}

/// One complete file contribution. Trees name chunks in this same file.
/// The projection envelope owns the corresponding Source record reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeDslFileV1 {
    pub chunks: Vec<ChunkRecord>,
    pub trees: Vec<StructuralTreeRecord>,
}
crate::native_serde_struct!([serde_identifier] NativeDslFileV1 {
    chunks: Vec<ChunkRecord>,
    trees: Vec<StructuralTreeRecord>,
});

/// The complete chunk projection's independent Structural readiness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeDslCorpusV1 {
    pub pin: GenerationPin,
    pub source_producer: ProducerId,
    pub structural_ready: bool,
}
crate::native_serde_struct!([serde_identifier] NativeDslCorpusV1 {
    pin: GenerationPin,
    source_producer: ProducerId,
    structural_ready: bool,
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeDslRuntimeCatalogV1 {
    /// Exact chunk universe that owns every document named by this catalog.
    pub corpus_certificate: ObjectRef,
    /// Complete dirty membership for this published Source cut.
    pub dirty: Vec<DirtyRecord>,
    pub catalog: RuntimeCatalogIngestBatch,
}
crate::native_serde_struct!([serde_identifier] NativeDslRuntimeCatalogV1 {
    corpus_certificate: ObjectRef,
    dirty: Vec<DirtyRecord>,
    catalog: RuntimeCatalogIngestBatch,
});

const NATIVE_DSL_RUNTIME_CATALOG_FIELDS_V1: &[&str] =
    &["corpus_certificate", "dirty", "catalog"];

struct NativeDslRuntimeCatalogFieldSeedV1;

impl<'de> serde::de::DeserializeSeed<'de> for NativeDslRuntimeCatalogFieldSeedV1 {
    type Value = &'static str;

    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl serde::de::Visitor<'_> for NativeDslRuntimeCatalogFieldSeedV1 {
    type Value = &'static str;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a borrowed RuntimeMetadata header field")
    }

    fn visit_str<E: serde::de::Error>(self, key: &str) -> Result<Self::Value, E> {
        NATIVE_DSL_RUNTIME_CATALOG_FIELDS_V1.iter().copied()
            .find(|field| *field == key)
            .ok_or_else(|| E::unknown_field(key, NATIVE_DSL_RUNTIME_CATALOG_FIELDS_V1))
    }

    fn visit_bytes<E: serde::de::Error>(self, key: &[u8]) -> Result<Self::Value, E> {
        let key = core::str::from_utf8(key)
            .map_err(|_| E::invalid_value(serde::de::Unexpected::Bytes(key), &self))?;
        self.visit_str(key)
    }

    fn visit_string<E: serde::de::Error>(self, _key: String) -> Result<Self::Value, E> {
        Err(E::custom("admitted RuntimeMetadata field must be borrowed"))
    }
}

struct NativeDslRuntimeCatalogSeedV1<'a, A: ?Sized> {
    admission: &'a A,
    limits: crate::CorpusDecodeLimitsV1,
}

struct NativeDslRuntimeCatalogVisitorV1<'a, A: ?Sized> {
    admission: &'a A,
    limits: crate::CorpusDecodeLimitsV1,
}

impl<'de, A: quanta_index_contract_base::SourceIdentityDecodeAdmissionV1 + ?Sized>
    serde::de::Visitor<'de> for NativeDslRuntimeCatalogVisitorV1<'_, A>
{
    type Value = NativeDslRuntimeCatalogV1;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a NativeDslRuntimeCatalogV1 map")
    }

    fn visit_map<M: serde::de::MapAccess<'de>>(
        self, mut map: M,
    ) -> Result<Self::Value, M::Error> {
        let (mut corpus_certificate, mut dirty, mut catalog) = (None, None, None);
        while let Some(field) = map.next_key_seed(NativeDslRuntimeCatalogFieldSeedV1)? {
            match field {
                "corpus_certificate" => {
                    if corpus_certificate.replace(map.next_value()?).is_some() {
                        return Err(serde::de::Error::duplicate_field("corpus_certificate"));
                    }
                }
                "dirty" => {
                    if dirty.replace(map.next_value_seed(
                        quanta_index_contract_base::AdmittedSequenceSeedV1::new(
                            self.admission, self.limits.collection_items,
                            || DirtyRecord::deserialize_seed_with_admission_v1(
                                self.admission, self.limits.text_bytes,
                            ),
                        ),
                    )?).is_some() {
                        return Err(serde::de::Error::duplicate_field("dirty"));
                    }
                }
                "catalog" => {
                    if catalog.replace(map.next_value_seed(
                        RuntimeCatalogIngestBatch::deserialize_seed_with_admission_v1(
                            self.admission, self.limits,
                        ),
                    )?).is_some() {
                        return Err(serde::de::Error::duplicate_field("catalog"));
                    }
                }
                _ => return Err(serde::de::Error::unknown_field(
                    field, NATIVE_DSL_RUNTIME_CATALOG_FIELDS_V1,
                )),
            }
        }
        Ok(NativeDslRuntimeCatalogV1 {
            corpus_certificate: corpus_certificate.ok_or_else(||
                serde::de::Error::missing_field("corpus_certificate"))?,
            dirty: dirty.ok_or_else(|| serde::de::Error::missing_field("dirty"))?,
            catalog: catalog.ok_or_else(|| serde::de::Error::missing_field("catalog"))?,
        })
    }
}

impl<'de, A: quanta_index_contract_base::SourceIdentityDecodeAdmissionV1 + ?Sized>
    serde::de::DeserializeSeed<'de> for NativeDslRuntimeCatalogSeedV1<'_, A>
{
    type Value = NativeDslRuntimeCatalogV1;

    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let result = decoder.deserialize_struct(
            "NativeDslRuntimeCatalogV1", NATIVE_DSL_RUNTIME_CATALOG_FIELDS_V1,
            NativeDslRuntimeCatalogVisitorV1 { admission: self.admission, limits: self.limits },
        );
        if result.is_err() {
            self.admission.abort_decode();
        }
        result
    }
}

impl NativeDslRuntimeCatalogV1 {
    /// The caller must retain the admission owner with the decoded header.
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A, limits: crate::CorpusDecodeLimitsV1,
    ) -> impl for<'de> serde::de::DeserializeSeed<'de, Value = Self> + 'a
    where A: quanta_index_contract_base::SourceIdentityDecodeAdmissionV1 + ?Sized {
        NativeDslRuntimeCatalogSeedV1 { admission, limits }
    }
}

pub const NATIVE_DSL_RUNTIME_CATALOG_PAGE_BYTES_V1: u32 = 4 * 1024 * 1024;
pub const NATIVE_DSL_RUNTIME_CATALOG_MAX_BYTES_V1: u32 = 16 * 1024 * 1024;
pub const NATIVE_DSL_STRUCTURAL_CHUNK_PAGE_ITEMS_V1: usize = 256;
pub const NATIVE_DSL_STRUCTURAL_CHUNK_PAGE_TEXT_BYTES_V1: usize = 1024 * 1024;

/// Continuation within one authenticated Structural owner contribution.
/// Offset zero resumes after the owner; a positive offset resumes its chunks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeDslStructuralChunkCursorV1 {
    pub view: ViewRef,
    pub projection: ObjectRef,
    pub certificate: ObjectRef,
    pub owner: OwnerKey,
    pub offset: u32,
}
crate::native_serde_struct!(NativeDslStructuralChunkCursorV1 {
    view: ViewRef,
    projection: ObjectRef,
    certificate: ObjectRef,
    owner: OwnerKey,
    offset: u32,
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeDslStructuralChunkPageV1 {
    pub session: NativeReadSessionId,
    pub view: ViewRef,
    pub source: SourceCut,
    pub projection: ObjectRef,
    pub structural_certificate: ObjectRef,
    pub chunks: Vec<crate::ChunkId>,
    pub next: Option<NativeDslStructuralChunkCursorV1>,
}
crate::native_serde_struct!(NativeDslStructuralChunkPageV1 {
    session: NativeReadSessionId,
    view: ViewRef,
    source: SourceCut,
    projection: ObjectRef,
    structural_certificate: ObjectRef,
    chunks: Vec<crate::ChunkId>,
    next: Option<NativeDslStructuralChunkCursorV1>,
});

impl NativeDslStructuralChunkPageV1 {
    #[must_use]
    pub fn matches_request_v1(
        &self, session: NativeReadSessionId, view: ViewRef,
        after: Option<&NativeDslStructuralChunkCursorV1>,
    ) -> bool {
        let bound = |cursor: &NativeDslStructuralChunkCursorV1| {
            cursor.view == self.view
                && cursor.projection == self.projection
                && cursor.certificate == self.structural_certificate
        };
        self.session == session
            && self.view == view
            && after.is_none_or(bound)
            && self.next.as_ref().is_none_or(bound)
            && self.chunks.len() <= NATIVE_DSL_STRUCTURAL_CHUNK_PAGE_ITEMS_V1
            && self.chunks.iter().try_fold(0_usize, |bytes, chunk| {
                bytes.checked_add(chunk.as_str().len())
            }).is_some_and(|bytes| bytes <= NATIVE_DSL_STRUCTURAL_CHUNK_PAGE_TEXT_BYTES_V1)
    }
}

/// Bound page of the canonical retained RuntimeMetadata header bytes. All
/// pages of a seed carry the same exact view, Source and immutable identities.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeDslRuntimeCatalogPageV1 {
    pub session: NativeReadSessionId,
    pub view: ViewRef,
    pub source: SourceCut,
    pub structural_certificate: ObjectRef,
    pub projection: ObjectRef,
    pub output: ObjectRef,
    pub total_bytes: u32,
    pub offset: u32,
    pub bytes: Vec<u8>,
}
crate::native_serde_struct!(NativeDslRuntimeCatalogPageV1 {
    session: NativeReadSessionId,
    view: ViewRef,
    source: SourceCut,
    structural_certificate: ObjectRef,
    projection: ObjectRef,
    output: ObjectRef,
    total_bytes: u32,
    offset: u32,
    bytes: Vec<u8>,
});

impl NativeDslRuntimeCatalogPageV1 {
    #[must_use]
    pub fn matches_offset_v1(&self, offset: u32) -> bool {
        let page = NATIVE_DSL_RUNTIME_CATALOG_PAGE_BYTES_V1;
        self.offset == offset
            && offset.is_multiple_of(page)
            && self.total_bytes <= NATIVE_DSL_RUNTIME_CATALOG_MAX_BYTES_V1
            && self.total_bytes.checked_sub(offset).is_some_and(|remaining| {
                remaining != 0 && usize::try_from(remaining.min(page)).ok() == Some(self.bytes.len())
            })
    }
}

pub const MAX_NATIVE_DSL_STRUCTURAL_CHUNKS_PER_PAGE_V1: usize = 128;
pub const MAX_NATIVE_DSL_STRUCTURAL_CHUNK_BYTES_V1: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeDslStructuralChunksCursorV1 {
    pub view: ViewRef,
    pub projection: ObjectRef,
    pub structural_certificate: ObjectRef,
    pub owner: OwnerKey,
    /// A position inside this owner, or the start of the next owner.
    pub chunk_offset: Option<u32>,
}
crate::native_serde_struct!(NativeDslStructuralChunksCursorV1 {
    view: ViewRef,
    projection: ObjectRef,
    structural_certificate: ObjectRef,
    owner: OwnerKey,
    chunk_offset: Option<u32>,
});

/// One membership entry from the exact Structural projection. The owner
/// is the projection file key derived by `native_dsl_file_owner_v1`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeDslStructuralChunkMemberV1 {
    pub owner: OwnerKey,
    pub chunk_id: ChunkId,
}
crate::native_serde_struct!(NativeDslStructuralChunkMemberV1 {
    owner: OwnerKey,
    chunk_id: ChunkId,
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeDslStructuralChunksPageV1 {
    pub session: NativeReadSessionId,
    pub view: ViewRef,
    pub source: SourceCut,
    pub projection: ObjectRef,
    pub structural_certificate: ObjectRef,
    pub chunks: Vec<NativeDslStructuralChunkMemberV1>,
    pub after: Option<NativeDslStructuralChunksCursorV1>,
}
crate::native_serde_struct!(NativeDslStructuralChunksPageV1 {
    session: NativeReadSessionId,
    view: ViewRef,
    source: SourceCut,
    projection: ObjectRef,
    structural_certificate: ObjectRef,
    chunks: Vec<NativeDslStructuralChunkMemberV1>,
    after: Option<NativeDslStructuralChunksCursorV1>,
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeDslCursorV1 {
    pub view: ViewRef,
    pub projection: ObjectRef,
    pub query: Digest,
    pub candidate_id: String,
}
crate::native_serde_struct!([serde_identifier] NativeDslCursorV1 {
    view: ViewRef,
    projection: ObjectRef,
    query: Digest,
    candidate_id: String,
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeDslQueryV1 {
    pub scope: NativeDslScopeV1,
    pub kind: NativeDslKindV1,
    pub text: TextQueryRequest,
    pub after: Option<NativeDslCursorV1>,
}
crate::native_serde_struct!([serde_identifier] NativeDslQueryV1 {
    scope: NativeDslScopeV1,
    kind: NativeDslKindV1,
    text: TextQueryRequest,
    after: Option<NativeDslCursorV1> = default,
});

impl NativeDslQueryV1 {
    pub fn digest(&self) -> Result<Digest, &'static str> {
        if self.text.syntax == TextQuerySyntax::CodeSearch
            || self.text.generation.is_some()
            || self.text.generation_selector.is_some()
            || self.text.cursor.is_some()
            || self.text.query_text.len() > 16 * 1024
            || !(1..=super::MAX_NATIVE_TEXT_TOP_K).contains(&self.text.top_k)
        {
            return Err(
                "Native DSL requires a retained view, Native or Sourcegraph syntax, and a bounded page",
            );
        }
        let mut hash = Sha256::new();
        hash.update(b"quanta-native-dsl-query-v1\0");
        // Bind filters, syntax and cap without allocating an encoded query.
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
        serde_json::to_writer(HashWriter(&mut hash), &(&self.scope, self.kind, &self.text))
            .map_err(|_| "Native DSL query digest encoding failed")?;
        Ok(Digest(hash.finalize().into()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeDslRuntimeHitV1 {
    pub candidate_id: String,
    pub source_repo_id: RepoId,
    pub path: RepoRelativePath,
    pub start_line: u32,
    pub end_line: u32,
    pub snippet: String,
}
crate::native_serde_struct!([serde_identifier] NativeDslRuntimeHitV1 {
    candidate_id: String,
    source_repo_id: RepoId,
    path: RepoRelativePath,
    start_line: u32,
    end_line: u32,
    snippet: String,
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeDslStructuralHitV1 {
    pub candidate: StructuralCandidate,
    pub match_range: NativeDslMatchRangeV1,
    pub source_repo_id: RepoId,
    pub path: RepoRelativePath,
    pub pattern_kind: Option<String>,
    pub matched_node: Option<String>,
}
crate::native_serde_struct!([serde_identifier] NativeDslStructuralHitV1 {
    candidate: StructuralCandidate,
    match_range: NativeDslMatchRangeV1,
    source_repo_id: RepoId,
    path: RepoRelativePath,
    pattern_kind: Option<String> = default,
    matched_node: Option<String> = default,
});

/// Byte offsets are file-relative and end-exclusive. Lines are one-based.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeDslMatchRangeV1 {
    pub byte_start: u32,
    pub byte_end: u32,
    pub line_start: u32,
    pub line_end: u32,
}
crate::native_serde_struct!([serde_identifier] NativeDslMatchRangeV1 {
    byte_start: u32,
    byte_end: u32,
    line_start: u32,
    line_end: u32,
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeDslRowsV1 {
    Structural(Vec<NativeDslStructuralHitV1>),
    RuntimeMetadata(Vec<NativeDslRuntimeHitV1>),
}
crate::native_serde_enum!([serde_identifier] NativeDslRowsV1 {
    units {}
    newtypes {
        Structural = 0 => Vec<NativeDslStructuralHitV1>,
        RuntimeMetadata = 1 => Vec<NativeDslRuntimeHitV1>,
    }
    records {}
});

impl NativeDslRowsV1 {
    pub fn kind(&self) -> NativeDslKindV1 {
        match self {
            Self::Structural(_) => NativeDslKindV1::Structural,
            Self::RuntimeMetadata(_) => NativeDslKindV1::RuntimeMetadata,
        }
    }

    pub fn candidate_ids(&self) -> impl Iterator<Item = &str> {
        let (structural, runtime) = match self {
            Self::Structural(rows) => (rows.as_slice(), &[][..]),
            Self::RuntimeMetadata(rows) => (&[][..], rows.as_slice()),
        };
        structural
            .iter()
            .map(|row| row.candidate.candidate_id.as_str())
            .chain(runtime.iter().map(|row| row.candidate_id.as_str()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeDslPageV1 {
    pub session: NativeReadSessionId,
    pub view: ViewRef,
    pub projection: ObjectRef,
    pub query: Digest,
    pub rows: NativeDslRowsV1,
    pub window: QueryResultWindowV1,
    pub after: Option<NativeDslCursorV1>,
}
crate::native_serde_struct!([serde_identifier] NativeDslPageV1 {
    session: NativeReadSessionId,
    view: ViewRef,
    projection: ObjectRef,
    query: Digest,
    rows: NativeDslRowsV1,
    window: QueryResultWindowV1,
    after: Option<NativeDslCursorV1> = default,
});
