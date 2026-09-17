//! Scope-streamed semantic build (QI-BB-021 follow-up #2).
//!
//! A semantic batch's vectors never sit resident all at once. The source of
//! a batch hands the build port one bounded *window* of embedded replace
//! scopes at a time; the port validates the window against the model
//! contract, appends it to the generation's working dataset, drops it, and
//! asks for the next. The seal at the end commits the whole dataset exactly
//! as an all-at-once build did: the row commitment is computed over the
//! table, in canonical row order, so where the windows fell leaves no trace
//! in the sealed manifest.
//!
//! The unit a window is cut on is the *owner scope*: every embedding of one
//! `(corpus_kind, owner_kind, owner_id)` travels in one window, because that
//! triple is what the build deletes before it appends. A window carries at
//! most [`SEMANTIC_STREAM_WINDOW_SCOPES`] owner scopes and at most
//! [`SEMANTIC_STREAM_WINDOW_VECTOR_BYTES`] of vectors; an owner scope whose
//! vectors alone exceed the byte bound cannot be carried by any window and
//! is refused typed rather than carried over the bound or split across a
//! delete boundary.
//!
//! Residency is observable, not assumed: every window carries a lease on
//! its source's [`SemanticWindowResidencyV1`], released when the window is
//! dropped, and a source refuses to issue a second window while one is
//! still out. The sink's receipt and the source's tally are computed
//! independently and compared by the caller, so a window the sink did not
//! append, or appended twice, is a typed mismatch and not a silent gap.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use quanta_index_contract::{
    BatchIngestMode, EmbeddingModelContract, EmbeddingRecord, GenerationPin, ManifestGeneration,
    SearchScopeSurface, SemanticCorpusKindV1, SemanticIngestBatch, SemanticReplaceScope,
    SemanticTombstoneScope,
};

use crate::error::CoreError;

/// Most owner scopes one window carries.
///
/// An owner scope is the unit the build replaces atomically: one delete
/// predicate, then its rows. 1,024 owner scopes of one record each is the
/// number of texts the default provider tuning holds in flight per round
/// (four requests of 256 inputs), so a window of small scopes fills one
/// round of requests instead of forcing a round trip per scope, the cost
/// the all-at-once derivation was written to avoid; it also bounds the
/// delete predicates one window issues before its append.
pub const SEMANTIC_STREAM_WINDOW_SCOPES: usize = 1_024;

/// Most bytes of `f32` vectors one window carries resident.
///
/// 32 MiB is one full in-flight round at the widest admitted dimension
/// (1,024 texts × 8,192 components × 4 bytes), so the byte bound never cuts
/// a window below one round; at 1,536 components it is 5,461 rows, one
/// eighth of the default per-batch vector envelope, so a build's resident
/// vectors are bounded by the window and not by the batch.
pub const SEMANTIC_STREAM_WINDOW_VECTOR_BYTES: u64 = 32 * 1024 * 1024;

/// Wire code for a window the sink received that exceeds the policy.
pub const SEMANTIC_STREAM_WINDOW_EXCEEDED_CODE: &str = "SEMANTIC_STREAM_WINDOW_EXCEEDED";

/// Wire code for an owner scope whose vectors alone exceed the window byte
/// bound: no window can carry it.
pub const SEMANTIC_STREAM_OWNER_SCOPE_OVER_WINDOW_CODE: &str =
    "SEMANTIC_STREAM_OWNER_SCOPE_OVER_WINDOW";

/// Wire code for a source asked for the next window while the previous one
/// is still resident.
pub const SEMANTIC_STREAM_WINDOW_STILL_RESIDENT_CODE: &str =
    "SEMANTIC_STREAM_WINDOW_STILL_RESIDENT";

/// Bytes of one `f32` vector component.
const VECTOR_COMPONENT_BYTES: u64 = 4;

/// Ceilings for one window: owner scopes it carries and bytes of vectors it
/// holds resident.
///
/// Every bound is a strict maximum; zero is refused at construction because
/// a zero ceiling admits nothing and is a configuration defect, not a
/// disabled bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SemanticStreamWindowPolicy {
    owner_scopes: usize,
    vector_bytes: u64,
}

impl SemanticStreamWindowPolicy {
    /// The production window: [`SEMANTIC_STREAM_WINDOW_SCOPES`] owner scopes
    /// and [`SEMANTIC_STREAM_WINDOW_VECTOR_BYTES`] of vectors.
    pub const DEFAULT: Self = Self {
        owner_scopes: SEMANTIC_STREAM_WINDOW_SCOPES,
        vector_bytes: SEMANTIC_STREAM_WINDOW_VECTOR_BYTES,
    };

    /// A policy with explicit ceilings; each must be at least one.
    pub fn new(max_owner_scopes: usize, max_vector_bytes: u64) -> Result<Self, CoreError> {
        if max_owner_scopes == 0 || max_vector_bytes == 0 {
            return Err(CoreError::InvalidContract(
                "semantic stream window policy: every ceiling must be at least one".to_string(),
            ));
        }
        Ok(Self {
            owner_scopes: max_owner_scopes,
            vector_bytes: max_vector_bytes,
        })
    }

    /// Most owner scopes one window carries.
    #[must_use]
    pub const fn max_owner_scopes(&self) -> usize {
        self.owner_scopes
    }

    /// Most bytes of `f32` vectors one window holds resident.
    #[must_use]
    pub const fn max_vector_bytes(&self) -> u64 {
        self.vector_bytes
    }

    /// The bytes `records` vectors of `dimension` components occupy.
    pub fn vector_bytes(records: usize, dimension: usize) -> Result<u64, CoreError> {
        let (Ok(records), Ok(components)) = (u64::try_from(records), u64::try_from(dimension))
        else {
            return Err(vector_bytes_overflow());
        };
        records
            .checked_mul(components)
            .and_then(|total| total.checked_mul(VECTOR_COMPONENT_BYTES))
            .ok_or_else(vector_bytes_overflow)
    }

    /// Where an owner scope of `owner_bytes` of vectors goes relative to a
    /// window already holding `fill`.
    ///
    /// The first owner scope always opens a window; a later one joins while
    /// both ceilings hold and opens the next window otherwise. An owner
    /// scope over the byte ceiling by itself is refused typed: it fits no
    /// window, and splitting it would put a delete boundary inside one
    /// owner's rows.
    pub fn place(
        &self,
        fill: SemanticWindowFillV1,
        owner_bytes: u64,
    ) -> Result<SemanticWindowPlacementV1, CoreError> {
        if owner_bytes > self.vector_bytes {
            return Err(CoreError::Typed {
                code: SEMANTIC_STREAM_OWNER_SCOPE_OVER_WINDOW_CODE.to_string(),
                message: format!(
                    "semantic stream window: one owner scope expands to {owner_bytes} bytes of vectors, the window ceiling is {}",
                    self.vector_bytes
                ),
            });
        }
        if fill.owner_scopes == 0 {
            return Ok(SemanticWindowPlacementV1::Joins);
        }
        let joined_scopes = fill.owner_scopes.checked_add(1);
        let joined_bytes = fill.vector_bytes.checked_add(owner_bytes);
        let (Some(joined_scopes), Some(joined_bytes)) = (joined_scopes, joined_bytes) else {
            return Ok(SemanticWindowPlacementV1::OpensNext);
        };
        if joined_scopes > self.owner_scopes || joined_bytes > self.vector_bytes {
            return Ok(SemanticWindowPlacementV1::OpensNext);
        }
        Ok(SemanticWindowPlacementV1::Joins)
    }

    /// Measure a window the sink received and admit it, or refuse it typed.
    ///
    /// Counts the distinct owner scopes across the window's replace scopes
    /// and sums the bytes of every vector it carries. An empty window is a
    /// source defect and is refused as an invalid contract.
    pub fn admit(&self, window: &SemanticScopeWindowV1) -> Result<SemanticWindowFillV1, CoreError> {
        let mut owners = BTreeSet::new();
        let mut rows = 0_usize;
        for scope in window.scopes() {
            for embedding in &scope.embeddings {
                let _seen_before = owners.insert(owner_key_v1(embedding));
                rows = rows.checked_add(1).ok_or_else(|| {
                    CoreError::InvalidContract(
                        "semantic stream window: row count overflow".to_string(),
                    )
                })?;
            }
        }
        if rows == 0 {
            return Err(CoreError::InvalidContract(
                "semantic stream window: a window carries at least one embedding".to_string(),
            ));
        }
        let fill = SemanticWindowFillV1 {
            owner_scopes: owners.len(),
            vector_bytes: window.vector_bytes(),
        };
        if fill.owner_scopes > self.owner_scopes || fill.vector_bytes > self.vector_bytes {
            return Err(CoreError::Typed {
                code: SEMANTIC_STREAM_WINDOW_EXCEEDED_CODE.to_string(),
                message: format!(
                    "semantic stream window: window carries {} owner scopes and {} bytes of vectors, the ceilings are {} and {}",
                    fill.owner_scopes, fill.vector_bytes, self.owner_scopes, self.vector_bytes
                ),
            });
        }
        Ok(fill)
    }
}

/// What a window holds so far: owner scopes and bytes of vectors.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SemanticWindowFillV1 {
    pub owner_scopes: usize,
    pub vector_bytes: u64,
}

/// Whether an owner scope joins the window being filled or opens the next.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticWindowPlacementV1 {
    Joins,
    OpensNext,
}

fn vector_bytes_overflow() -> CoreError {
    CoreError::InvalidContract("semantic stream window: vector bytes overflow".to_string())
}

/// The bytes one `f32` vector occupies.
fn vector_bytes_of(vector: &[f32]) -> Result<u64, CoreError> {
    SemanticStreamWindowPolicy::vector_bytes(vector.len(), 1)
}

/// The `(corpus_kind, owner_kind, owner_id)` triple the build deletes by.
#[must_use]
pub fn owner_key_v1(embedding: &EmbeddingRecord) -> (&'static str, &'static str, &str) {
    (
        embedding.corpus_kind.as_code_str(),
        embedding.owner_kind.as_code_str(),
        embedding.owner_id.as_ref(),
    )
}

/// The windows a source has handed out and not yet seen dropped, and the
/// most it ever had out at once.
///
/// Shared between the source and every lease it issues; a test double reads
/// it as the oracle that the sink held one window at a time.
#[derive(Debug, Default)]
pub struct SemanticWindowResidencyV1 {
    outstanding_windows: AtomicU64,
    outstanding_vector_bytes: AtomicU64,
    peak_windows: AtomicU64,
    peak_vector_bytes: AtomicU64,
}

impl SemanticWindowResidencyV1 {
    /// Windows issued and not yet dropped.
    #[must_use]
    pub fn outstanding_windows(&self) -> u64 {
        self.outstanding_windows.load(Ordering::Acquire)
    }

    /// Bytes of vectors in the windows issued and not yet dropped.
    #[must_use]
    pub fn outstanding_vector_bytes(&self) -> u64 {
        self.outstanding_vector_bytes.load(Ordering::Acquire)
    }

    /// Most windows ever out at once.
    #[must_use]
    pub fn peak_windows(&self) -> u64 {
        self.peak_windows.load(Ordering::Acquire)
    }

    /// Most bytes of vectors ever out at once.
    #[must_use]
    pub fn peak_vector_bytes(&self) -> u64 {
        self.peak_vector_bytes.load(Ordering::Acquire)
    }

    fn acquire(self: &Arc<Self>, vector_bytes: u64) -> SemanticWindowLeaseV1 {
        let windows = self
            .outstanding_windows
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        let bytes = self
            .outstanding_vector_bytes
            .fetch_add(vector_bytes, Ordering::AcqRel)
            .saturating_add(vector_bytes);
        let _prior_windows = self.peak_windows.fetch_max(windows, Ordering::AcqRel);
        let _prior_bytes = self.peak_vector_bytes.fetch_max(bytes, Ordering::AcqRel);
        SemanticWindowLeaseV1 {
            residency: Arc::clone(self),
            vector_bytes,
        }
    }

    fn release(&self, vector_bytes: u64) {
        let _windows = self.outstanding_windows.fetch_sub(1, Ordering::AcqRel);
        let _bytes = self
            .outstanding_vector_bytes
            .fetch_sub(vector_bytes, Ordering::AcqRel);
    }
}

/// The residency a window holds while it is alive; released on drop.
#[derive(Debug)]
pub struct SemanticWindowLeaseV1 {
    residency: Arc<SemanticWindowResidencyV1>,
    vector_bytes: u64,
}

impl Drop for SemanticWindowLeaseV1 {
    fn drop(&mut self) {
        self.residency.release(self.vector_bytes);
    }
}

/// One bounded window of embedded replace scopes, leased from its source.
///
/// The scopes are whole owner scopes: every embedding of one owner key is in
/// exactly one window across the stream. A replace scope here may be one
/// fragment of the scope the producer sent (a path with many chunk owners
/// spans windows), which changes nothing the build does, because it deletes
/// and appends by owner.
#[derive(Debug)]
pub struct SemanticScopeWindowV1 {
    scopes: Vec<SemanticReplaceScope>,
    lease: SemanticWindowLeaseV1,
}

impl SemanticScopeWindowV1 {
    /// Lease `scopes` as one window against `residency`, measuring the
    /// bytes of every vector they carry.
    pub fn lease(
        scopes: Vec<SemanticReplaceScope>,
        residency: &Arc<SemanticWindowResidencyV1>,
    ) -> Result<Self, CoreError> {
        let mut vector_bytes = 0_u64;
        for scope in &scopes {
            for embedding in &scope.embeddings {
                vector_bytes = vector_bytes
                    .checked_add(vector_bytes_of(&embedding.vector)?)
                    .ok_or_else(vector_bytes_overflow)?;
            }
        }
        Ok(Self {
            scopes,
            lease: residency.acquire(vector_bytes),
        })
    }

    /// The replace scopes this window carries.
    #[must_use]
    pub fn scopes(&self) -> &[SemanticReplaceScope] {
        &self.scopes
    }

    /// Bytes of `f32` vectors this window holds resident.
    #[must_use]
    pub const fn vector_bytes(&self) -> u64 {
        self.lease.vector_bytes
    }

    /// Rows this window carries.
    pub fn rows(&self) -> Result<u64, CoreError> {
        let overflow =
            || CoreError::InvalidContract("semantic stream window: row count overflow".to_string());
        let mut rows = 0_u64;
        for scope in &self.scopes {
            let count = u64::try_from(scope.embeddings.len()).map_err(|_err| overflow())?;
            rows = rows.checked_add(count).ok_or_else(overflow)?;
        }
        Ok(rows)
    }
}

/// What a stream added up to, as counted on one side of it.
///
/// A source tallies what it issued and the sink tallies what it appended;
/// the two are computed independently and must agree exactly.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SemanticStreamTallyV1 {
    /// Windows issued or appended.
    pub windows: u64,
    /// Replace scopes across those windows.
    pub replace_scopes: u64,
    /// Rows across those windows.
    pub rows: u64,
    /// Most bytes of vectors one window held.
    pub peak_vector_bytes: u64,
}

impl SemanticStreamTallyV1 {
    /// Count one more window of `replace_scopes` scopes, `rows` rows and
    /// `vector_bytes` of vectors.
    pub fn count_window(
        &mut self,
        replace_scopes: usize,
        rows: u64,
        vector_bytes: u64,
    ) -> Result<(), CoreError> {
        let overflow = || CoreError::InvalidContract("semantic stream tally overflow".to_string());
        self.windows = self.windows.checked_add(1).ok_or_else(overflow)?;
        let replace_scopes = u64::try_from(replace_scopes).map_err(|_err| overflow())?;
        self.replace_scopes = self
            .replace_scopes
            .checked_add(replace_scopes)
            .ok_or_else(overflow)?;
        self.rows = self.rows.checked_add(rows).ok_or_else(overflow)?;
        self.peak_vector_bytes = self.peak_vector_bytes.max(vector_bytes);
        Ok(())
    }
}

/// Issues windows on behalf of a source: one lease at a time, tallied.
///
/// Every source funnels its windows through one issuer so the residency
/// rule and the tally live in one place.
#[derive(Debug)]
pub struct SemanticWindowIssuerV1 {
    residency: Arc<SemanticWindowResidencyV1>,
    tally: SemanticStreamTallyV1,
}

impl Default for SemanticWindowIssuerV1 {
    fn default() -> Self {
        Self::new()
    }
}

impl SemanticWindowIssuerV1 {
    #[must_use]
    pub fn new() -> Self {
        Self {
            residency: Arc::new(SemanticWindowResidencyV1::default()),
            tally: SemanticStreamTallyV1::default(),
        }
    }

    /// The residency every window this issuer leased is accounted in.
    #[must_use]
    pub const fn residency(&self) -> &Arc<SemanticWindowResidencyV1> {
        &self.residency
    }

    /// What this issuer has issued so far.
    #[must_use]
    pub const fn tally(&self) -> SemanticStreamTallyV1 {
        self.tally
    }

    /// Refuse to issue while a previous window is still resident.
    pub fn require_no_window_resident(&self) -> Result<(), CoreError> {
        let outstanding = self.residency.outstanding_windows();
        if outstanding != 0 {
            return Err(CoreError::Typed {
                code: SEMANTIC_STREAM_WINDOW_STILL_RESIDENT_CODE.to_string(),
                message: format!(
                    "semantic stream: {outstanding} window(s) still resident; the sink drops a window before asking for the next"
                ),
            });
        }
        Ok(())
    }

    /// Lease `scopes` as the next window and tally it.
    pub fn issue(
        &mut self,
        scopes: Vec<SemanticReplaceScope>,
    ) -> Result<SemanticScopeWindowV1, CoreError> {
        self.require_no_window_resident()?;
        let window = SemanticScopeWindowV1::lease(scopes, &self.residency)?;
        self.tally
            .count_window(window.scopes().len(), window.rows()?, window.vector_bytes())?;
        Ok(window)
    }
}

/// The scope-at-a-time source of one batch's embedded replace scopes.
///
/// Implemented by the search plane over the producer's un-embedded records
/// (embedding one window at a time) and, in core, over an already-resident
/// batch. The sink drops every window before asking for the next; a source
/// refuses a second window while one is out.
pub trait SemanticScopeSource {
    /// The next window, or `None` once every scope has been issued.
    fn next_window(&mut self) -> Result<Option<SemanticScopeWindowV1>, CoreError>;

    /// What the source has issued so far.
    fn tally(&self) -> SemanticStreamTallyV1;
}

/// Build one batch of a generation from a header and a scope source,
/// sealing on the header's `seal`.
///
/// The only build entry of the semantic track: every window the source
/// issues is admitted against the port's window policy, appended, and
/// dropped before the next is requested; the returned tally is what the
/// port appended, counted on its side.
pub trait SemanticScopeStreamBuildPort: Send + Sync {
    fn build_stream(
        &self,
        header: &SemanticIngestHeaderV1,
        scopes: &mut dyn SemanticScopeSource,
    ) -> Result<SemanticStreamTallyV1, CoreError>;
}

/// A semantic ingest batch without its replace scopes: what the build knows
/// before the first window and what the seal needs after the last.
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticIngestHeaderV1 {
    /// The generation the batch targets.
    pub pin: GenerationPin,
    /// What the generation is built under; fixed by its first batch.
    pub contract: SemanticGenerationContractV1,
    /// What names this batch and whether it seals.
    pub batch: SemanticBatchIdentityV1,
    /// The mutations that are not streamed: surfaces cleared before the
    /// first window and owners tombstoned after the last.
    pub mutations: SemanticBatchMutationsV1,
}

/// The generation-wide contract a batch carries.
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticGenerationContractV1 {
    pub mode: BatchIngestMode,
    pub base_generation: Option<ManifestGeneration>,
    pub model_contract: EmbeddingModelContract,
    pub required_corpora: Vec<SemanticCorpusKindV1>,
    pub corpus_policy_digest: Option<String>,
}

/// What names one batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticBatchIdentityV1 {
    pub manifest_digest: String,
    pub batch_digest: String,
    pub seal: bool,
}

/// The non-streamed mutations of one batch.
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticBatchMutationsV1 {
    pub clear_surfaces: Vec<SearchScopeSurface>,
    pub tombstone_scopes: Vec<SemanticTombstoneScope>,
}

impl SemanticIngestHeaderV1 {
    /// The header of an already-resident batch.
    #[must_use]
    pub fn of_batch(batch: &SemanticIngestBatch) -> Self {
        Self {
            pin: GenerationPin::new(
                batch.repo_id.clone(),
                batch.revision_id.clone(),
                batch.generation,
            ),
            contract: SemanticGenerationContractV1 {
                mode: batch.mode,
                base_generation: batch.base_generation,
                model_contract: batch.model_contract.clone(),
                required_corpora: batch.required_corpora.clone(),
                corpus_policy_digest: batch.corpus_policy_digest.clone(),
            },
            batch: SemanticBatchIdentityV1 {
                manifest_digest: batch.manifest_digest.clone(),
                batch_digest: batch.batch_digest.clone(),
                seal: batch.seal,
            },
            mutations: SemanticBatchMutationsV1 {
                clear_surfaces: batch.clear_surfaces.clone(),
                tombstone_scopes: batch.tombstone_scopes.clone(),
            },
        }
    }

    /// The model contract's dimension as a `usize`; zero is refused.
    pub fn dimension(&self) -> Result<usize, CoreError> {
        let dimension = usize::try_from(self.contract.model_contract.dimension).map_err(|err| {
            CoreError::InvalidContract(format!(
                "semantic: model contract dimension overflow: {err}"
            ))
        })?;
        if dimension == 0 {
            return Err(CoreError::InvalidContract(
                "semantic: model contract dimension must be > 0".to_string(),
            ));
        }
        Ok(dimension)
    }
}

/// One owner scope of a resident replace scope: which scope it is in and
/// which of that scope's embeddings are its rows.
struct ResidentOwnerGroup {
    scope: usize,
    embeddings: Vec<usize>,
    vector_bytes: u64,
}

/// Streams already-resident replace scopes in policy-sized windows.
///
/// A batch that is already resident gains no residency bound from this:
/// its vectors stay resident until it is dropped, and each window is one
/// more copy of its share. It exists for the legacy journal migration,
/// whose batches arrive decoded, and for fixtures; every other producer of
/// scopes streams them un-embedded through the search plane. Within a scope
/// the embeddings are grouped by owner in first-seen order, so an owner
/// whose rows the producer interleaved still travels in one window.
pub struct ResidentScopeSource<'a> {
    scopes: &'a [SemanticReplaceScope],
    pending: VecDeque<ResidentOwnerGroup>,
    policy: SemanticStreamWindowPolicy,
    issuer: SemanticWindowIssuerV1,
}

impl<'a> ResidentScopeSource<'a> {
    /// Plan `scopes` into owner groups under `policy`.
    pub fn new(
        scopes: &'a [SemanticReplaceScope],
        policy: SemanticStreamWindowPolicy,
    ) -> Result<Self, CoreError> {
        let mut pending = VecDeque::new();
        for (scope_index, scope) in scopes.iter().enumerate() {
            let mut order: Vec<ResidentOwnerGroup> = Vec::new();
            let mut position_by_owner: BTreeMap<(&str, &str, &str), usize> = BTreeMap::new();
            for (embedding_index, embedding) in scope.embeddings.iter().enumerate() {
                let bytes = vector_bytes_of(&embedding.vector)?;
                let key = owner_key_v1(embedding);
                if let Some(&position) = position_by_owner.get(&key) {
                    let group = order.get_mut(position).ok_or_else(|| {
                        CoreError::InvalidContract(
                            "semantic stream: resident owner group index out of range".to_string(),
                        )
                    })?;
                    group.embeddings.push(embedding_index);
                    group.vector_bytes = group
                        .vector_bytes
                        .checked_add(bytes)
                        .ok_or_else(vector_bytes_overflow)?;
                } else {
                    let _new = position_by_owner.insert(key, order.len());
                    order.push(ResidentOwnerGroup {
                        scope: scope_index,
                        embeddings: vec![embedding_index],
                        vector_bytes: bytes,
                    });
                }
            }
            pending.extend(order);
        }
        Ok(Self {
            scopes,
            pending,
            policy,
            issuer: SemanticWindowIssuerV1::new(),
        })
    }

    /// The residency every window of this source is accounted in.
    #[must_use]
    pub const fn residency(&self) -> &Arc<SemanticWindowResidencyV1> {
        self.issuer.residency()
    }

    /// Take the owner groups of the next window off the plan.
    fn take_next_window_groups(&mut self) -> Result<Vec<ResidentOwnerGroup>, CoreError> {
        let mut fill = SemanticWindowFillV1::default();
        let mut taken = Vec::new();
        while let Some(group) = self.pending.front() {
            match self.policy.place(fill, group.vector_bytes)? {
                SemanticWindowPlacementV1::Joins => {}
                SemanticWindowPlacementV1::OpensNext => break,
            }
            fill.owner_scopes = fill.owner_scopes.checked_add(1).ok_or_else(|| {
                CoreError::InvalidContract(
                    "semantic stream window: owner count overflow".to_string(),
                )
            })?;
            fill.vector_bytes = fill
                .vector_bytes
                .checked_add(group.vector_bytes)
                .ok_or_else(vector_bytes_overflow)?;
            let group = self.pending.pop_front().ok_or_else(|| {
                CoreError::InvalidContract(
                    "semantic stream: resident plan emptied under its own cursor".to_string(),
                )
            })?;
            taken.push(group);
        }
        Ok(taken)
    }

    /// Clone the window's rows out of the resident scopes: consecutive
    /// groups of one scope become one replace scope carrying those rows
    /// and the memberships that name them.
    fn materialize(
        &self,
        groups: &[ResidentOwnerGroup],
    ) -> Result<Vec<SemanticReplaceScope>, CoreError> {
        let mut window: Vec<SemanticReplaceScope> = Vec::new();
        let mut current_scope: Option<usize> = None;
        for group in groups {
            let scope = self.scopes.get(group.scope).ok_or_else(|| {
                CoreError::InvalidContract(
                    "semantic stream: resident scope index out of range".to_string(),
                )
            })?;
            if current_scope != Some(group.scope) {
                window.push(SemanticReplaceScope {
                    scope: scope.scope.clone(),
                    scope_digest: scope.scope_digest.clone(),
                    embeddings: Vec::new(),
                    cluster_memberships: Vec::new(),
                });
                current_scope = Some(group.scope);
            }
            let fragment = window.last_mut().ok_or_else(|| {
                CoreError::InvalidContract(
                    "semantic stream: window fragment vanished under its own cursor".to_string(),
                )
            })?;
            for &embedding_index in &group.embeddings {
                let embedding = scope.embeddings.get(embedding_index).ok_or_else(|| {
                    CoreError::InvalidContract(
                        "semantic stream: resident embedding index out of range".to_string(),
                    )
                })?;
                fragment.cluster_memberships.extend(
                    scope
                        .cluster_memberships
                        .iter()
                        .filter(|membership| {
                            membership.cluster_record_id == embedding.record_id.as_ref()
                        })
                        .cloned(),
                );
                fragment.embeddings.push(embedding.clone());
            }
        }
        Ok(window)
    }
}

impl SemanticScopeSource for ResidentScopeSource<'_> {
    fn next_window(&mut self) -> Result<Option<SemanticScopeWindowV1>, CoreError> {
        self.issuer.require_no_window_resident()?;
        if self.pending.is_empty() {
            return Ok(None);
        }
        let groups = self.take_next_window_groups()?;
        let scopes = self.materialize(&groups)?;
        self.issuer.issue(scopes).map(Some)
    }

    fn tally(&self) -> SemanticStreamTallyV1 {
        self.issuer.tally()
    }
}

/// Build an already-resident batch through the streamed port.
///
/// Windows the batch's replace scopes by `policy` and drives `port`; see
/// [`ResidentScopeSource`] for what this does and does not bound.
pub fn build_resident_semantic_batch_v1(
    port: &dyn SemanticScopeStreamBuildPort,
    batch: &SemanticIngestBatch,
    policy: SemanticStreamWindowPolicy,
) -> Result<SemanticStreamTallyV1, CoreError> {
    let header = SemanticIngestHeaderV1::of_batch(batch);
    let mut source = ResidentScopeSource::new(&batch.replace_scopes, policy)?;
    let appended = port.build_stream(&header, &mut source)?;
    let issued = source.tally();
    if appended != issued {
        return Err(CoreError::InvalidContract(format!(
            "semantic stream: the port appended {appended:?} but the source issued {issued:?}"
        )));
    }
    Ok(appended)
}
