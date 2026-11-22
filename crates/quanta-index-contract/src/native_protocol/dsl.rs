//! Source-bound chunk, parse-tree, and runtime catalog projection contracts.

use sha2::{Digest as _, Sha256};

use crate::lex::DirtyRecord;
use crate::{
    ChunkRecord, GenerationPin, QueryResultWindowV1, RepoId, RepoRelativePath, RevisionId,
    RuntimeCatalogIngestBatch, StructuralCandidate, StructuralTreeRecord, TextQueryRequest,
    TextQuerySyntax,
    index::{
        Digest, ObjectRef, OperationKey, OwnerKey, ProducerId, ProjectionId, SchemaId, SourceCut,
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
