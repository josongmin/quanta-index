//! Deterministic HNSW (Hierarchical Navigable Small World) index.
//!
//! This module ships the spec-bound HNSW backend for corpora above
//! [`crate::types::EXACT_NN_CUTOFF`]. The build is **fully
//! deterministic**: two builders fed the same `(seed, insertion sequence)`
//! produce a byte-identical [`HnswIndex`] every run.
//!
//! ## Determinism contract
//!
//! Standard HNSW uses a floating-point PRNG to assign a node's max
//! level. Floats are platform-dependent (`f64::ln` is **not**
//! IEEE-bit-exact across libc implementations) and a PRNG seed alone
//! does not protect against compiler / target drift. Both problems are
//! sidestepped here:
//!
//! 1. Level assignment uses a hand-rolled
//!    [`SipHash-2-4`](https://www.aumasson.jp/siphash/) of
//!    `seed || doc_id`. The hash output is consumed in
//!    `log2(m)`-bit chunks, advancing the level by one each time a
//!    chunk reads zero. This approximates the canonical `1/M^k`
//!    geometric distribution while staying in pure integer
//!    arithmetic — bit-exact across compilers and targets.
//! 2. Neighbour search ties break by **ascending [`DocId`]** at every
//!    layer, mirroring the exact-NN executor in [`crate::query`].
//!
//! ## Parameter constraints
//!
//! - `m` must be a power of two in `[MIN_M, MAX_M]`. The defaults
//!   (`m = 16`) keep `log2(m)` integer and let the level loop consume
//!   the `SipHash` output in fixed-width chunks.
//! - `ef_construction` must be `>= m` and `<= MAX_EF`.
//! - `ef_search` must be `>= 1` and `<= MAX_EF`.
//!
//! Out-of-range values surface [`SemanticErrorCode::SemHnswParamsInvalid`].
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use crate::cosine::cosine_similarity;
use crate::errors::{SemanticError, SemanticErrorCode};
use crate::query::AnnResult;
use crate::types::{DocId, Embedding, MAX_EMBEDDING_DIM};

/// Minimum permitted `m` (graph degree). Power-of-two-constrained.
pub const MIN_M: u32 = 4;
/// Maximum permitted `m`. Anything above this would push per-node
/// neighbour storage past the practical bound.
pub const MAX_M: u32 = 64;
/// Maximum permitted `ef_construction` / `ef_search`. Bounded so a
/// pathological query cannot allocate unboundedly.
pub const MAX_EF: u32 = 10_000;
/// Hard ceiling on the layered-graph top level.
///
/// With `m=16` and the SipHash-driven 1/M^k geometric distribution, a
/// 16-level cap covers >> 2^64 hypothetical nodes — plenty of headroom
/// for any realistic corpus.
pub const MAX_LEVEL: u32 = 16;

/// HNSW build / search parameters.
///
/// All fields are public so callers can opt in to a specific `seed`
/// without going through a builder pattern; construction goes through
/// [`HnswParams::new`] (or [`HnswParams::DEFAULTS`]) so the validation
/// gates fire once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HnswParams {
    pub m: u32,
    pub ef_construction: u32,
    pub ef_search: u32,
    pub seed: u64,
}

impl HnswParams {
    /// Spec-locked defaults. `seed` is a fixed 64-bit constant so two
    /// callers who pin the defaults get the same graph shape.
    pub const DEFAULTS: Self = Self {
        m: 16,
        ef_construction: 200,
        ef_search: 100,
        seed: 0xdead_beef_cafe_f00d,
    };

    /// Validated constructor.
    pub fn new(
        m: u32,
        ef_construction: u32,
        ef_search: u32,
        seed: u64,
    ) -> Result<Self, SemanticError> {
        let p = Self {
            m,
            ef_construction,
            ef_search,
            seed,
        };
        p.validate()?;
        Ok(p)
    }

    /// Returns `Ok(())` if every field is within range; otherwise
    /// returns [`SemanticErrorCode::SemHnswParamsInvalid`].
    pub fn validate(&self) -> Result<(), SemanticError> {
        if self.m < MIN_M || self.m > MAX_M {
            return Err(SemanticError::new(
                SemanticErrorCode::SemHnswParamsInvalid,
                format!("m={} not in [{MIN_M},{MAX_M}]", self.m),
            ));
        }
        if !self.m.is_power_of_two() {
            return Err(SemanticError::new(
                SemanticErrorCode::SemHnswParamsInvalid,
                format!(
                    "m={} must be a power of two for deterministic level assignment",
                    self.m
                ),
            ));
        }
        if self.ef_construction < self.m {
            return Err(SemanticError::new(
                SemanticErrorCode::SemHnswParamsInvalid,
                format!(
                    "ef_construction={} must be >= m={}",
                    self.ef_construction, self.m
                ),
            ));
        }
        if self.ef_construction > MAX_EF {
            return Err(SemanticError::new(
                SemanticErrorCode::SemHnswParamsInvalid,
                format!(
                    "ef_construction={} exceeds MAX_EF={MAX_EF}",
                    self.ef_construction
                ),
            ));
        }
        if self.ef_search == 0 || self.ef_search > MAX_EF {
            return Err(SemanticError::new(
                SemanticErrorCode::SemHnswParamsInvalid,
                format!("ef_search={} not in [1,{MAX_EF}]", self.ef_search),
            ));
        }
        Ok(())
    }

    /// `log2(m)` as a `u32`. Validation guarantees `m` is a power of
    /// two so the value is exact.
    #[must_use]
    pub fn bits_per_level_step(&self) -> u32 {
        self.m.trailing_zeros()
    }
}

// ─────────────────────────── SipHash-2-4 ───────────────────────────

/// Hand-rolled SipHash-2-4 of `seed_lo || seed_hi || doc_id`.
///
/// All u64, little-endian. The output is byte-exact across compilers
/// because the algorithm is pure integer arithmetic — no
/// platform-specific `ln`/PRNG. The 16-byte `SipHash` key is
/// `(seed, seed ^ 0xa55a_a55a_a55a_a55a)` so a caller-supplied `u64`
/// seed expands to the full 128-bit key in a documented way.
fn siphash_2_4(seed: u64, doc_id: DocId) -> u64 {
    let k0 = seed;
    let k1 = seed ^ 0xa55a_a55a_a55a_a55a_u64;

    let mut v0 = k0 ^ 0x736f_6d65_7073_6575_u64;
    let mut v1 = k1 ^ 0x646f_7261_6e64_6f6d_u64;
    let mut v2 = k0 ^ 0x6c79_6765_6e65_7261_u64;
    let mut v3 = k1 ^ 0x7465_6462_7974_6573_u64;

    // Message: 8 bytes (doc_id as little-endian u64).
    // SipHash finalisation expects the input length mod 256 in the
    // top byte of the final message word. Here we have one 8-byte
    // message word; the final word is `(len & 0xff) << 56` OR'd into
    // the message word — but because our message is exactly 8 bytes
    // we have one full word and a separate zero-padded length word
    // with the length byte at the top.
    let m_word: u64 = doc_id.0;
    v3 ^= m_word;
    sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
    sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
    v0 ^= m_word;

    // Final padding word: length is 8 -> top byte is 0x08.
    let pad: u64 = 0x0800_0000_0000_0000_u64;
    v3 ^= pad;
    sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
    sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
    v0 ^= pad;

    // Finalisation: 4 rounds with v2 ^= 0xff.
    v2 ^= 0xff;
    sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
    sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
    sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
    sip_round(&mut v0, &mut v1, &mut v2, &mut v3);

    v0 ^ v1 ^ v2 ^ v3
}

fn sip_round(v0: &mut u64, v1: &mut u64, v2: &mut u64, v3: &mut u64) {
    *v0 = v0.wrapping_add(*v1);
    *v1 = v1.rotate_left(13);
    *v1 ^= *v0;
    *v0 = v0.rotate_left(32);
    *v2 = v2.wrapping_add(*v3);
    *v3 = v3.rotate_left(16);
    *v3 ^= *v2;
    *v0 = v0.wrapping_add(*v3);
    *v3 = v3.rotate_left(21);
    *v3 ^= *v0;
    *v2 = v2.wrapping_add(*v1);
    *v1 = v1.rotate_left(17);
    *v1 ^= *v2;
    *v2 = v2.rotate_left(32);
}

/// Assign a max-level to `doc_id` deterministically.
///
/// The `SipHash` output is consumed in `bits_per_level_step()`-wide
/// chunks from the bottom up. Each zero chunk promotes the level by
/// one. With `m=16` (`bits_per_level_step()=4`) the chance of a chunk
/// being zero is `1/16`, so the resulting distribution approximates
/// the canonical HNSW `P(level>=k) = 1/m^k` while staying pure-int.
fn assign_level(params: &HnswParams, doc_id: DocId) -> u32 {
    let mut h: u64 = siphash_2_4(params.seed, doc_id);
    let bits = params.bits_per_level_step();
    if bits == 0 {
        // Defense in depth: validate() guarantees m >= 4 so bits >= 2.
        return 0;
    }
    let mask: u64 = (1u64.wrapping_shl(bits)).wrapping_sub(1);
    let mut level: u32 = 0;
    loop {
        if level >= MAX_LEVEL {
            return MAX_LEVEL;
        }
        if (h & mask) != 0 {
            return level;
        }
        level = level.saturating_add(1);
        h = h.wrapping_shr(bits);
        if h == 0 {
            return level;
        }
    }
}

// ─────────────────────────── Data ───────────────────────────

/// Per-doc layered-graph node.
#[derive(Clone, Debug, PartialEq)]
pub struct HnswNode {
    pub vector: Vec<f32>,
    /// `levels[0]` is the bottom (layer 0) neighbour list. Each list
    /// is sorted ascending by [`DocId`] for byte-stable persistence.
    pub levels: Vec<Vec<DocId>>,
}

impl HnswNode {
    /// `true` if `doc_id` already appears in the layer-`level`
    /// neighbour list.
    fn has_neighbor_at(&self, level: usize, doc_id: DocId) -> bool {
        self.levels
            .get(level)
            .is_some_and(|v| v.binary_search(&doc_id).is_ok())
    }
}

/// Persisted HNSW index for one generation.
#[derive(Clone, Debug, PartialEq)]
pub struct HnswIndex {
    generation: u64,
    dim: u32,
    params: HnswParams,
    nodes: BTreeMap<DocId, HnswNode>,
    entry: Option<DocId>,
    top_level: u32,
}

impl HnswIndex {
    /// Generation this index was built for.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }
    /// Embedding dimension every vector is pinned to.
    #[must_use]
    pub const fn dim(&self) -> u32 {
        self.dim
    }
    /// Build parameters carried alongside the graph so search can
    /// honour `ef_search` and verify the seed.
    #[must_use]
    pub const fn params(&self) -> &HnswParams {
        &self.params
    }
    /// Number of doc nodes in the graph.
    #[must_use]
    pub fn corpus_size(&self) -> usize {
        self.nodes.len()
    }
    /// `true` if no docs were inserted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    /// Top layer index. `0` if the graph is empty.
    #[must_use]
    pub const fn top_level(&self) -> u32 {
        self.top_level
    }
    /// Borrow the entry-point doc id, if any.
    #[must_use]
    pub const fn entry(&self) -> Option<DocId> {
        self.entry
    }

    /// Iterate `(DocId, &HnswNode)` pairs in ascending [`DocId`]
    /// order — the canonical wire iteration order.
    pub fn iter(&self) -> impl Iterator<Item = (DocId, &HnswNode)> + '_ {
        self.nodes.iter().map(|(d, n)| (*d, n))
    }
}

// ─────────────────────────── Builder ───────────────────────────

/// Deterministic HNSW builder. Same `(params, insertion sequence)`
/// produces a byte-identical [`HnswIndex`].
pub struct HnswIndexBuilder {
    generation: u64,
    dim: u32,
    params: HnswParams,
    nodes: BTreeMap<DocId, HnswNode>,
    /// Per-doc max level recorded at insertion time so finishing
    /// doesn't need to recompute the `SipHash`.
    levels: BTreeMap<DocId, u32>,
    entry: Option<DocId>,
    top_level: u32,
}

impl HnswIndexBuilder {
    /// Construct a builder for `(generation, dim)` with the supplied
    /// `params`. Rejects `generation == 0`, `dim == 0`,
    /// `dim > MAX_EMBEDDING_DIM`, and any invalid HNSW parameter.
    pub fn new(generation: u64, dim: u32, params: HnswParams) -> Result<Self, SemanticError> {
        if generation == 0 {
            return Err(SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                "generation must be non-zero",
            ));
        }
        if dim == 0 {
            return Err(SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                "dim must be non-zero",
            ));
        }
        let max = u32::try_from(MAX_EMBEDDING_DIM).map_err(|e| {
            SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                format!("MAX_EMBEDDING_DIM cast: {e}"),
            )
        })?;
        if dim > max {
            return Err(SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                format!("dim {dim} exceeds MAX_EMBEDDING_DIM {max}"),
            ));
        }
        params.validate()?;
        Ok(Self {
            generation,
            dim,
            params,
            nodes: BTreeMap::new(),
            levels: BTreeMap::new(),
            entry: None,
            top_level: 0,
        })
    }

    /// Generation id this builder is bound to.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }
    /// Embedding dim every staged vector must match.
    #[must_use]
    pub const fn dim(&self) -> u32 {
        self.dim
    }
    /// Number of staged docs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    /// `true` if no docs have been staged.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Insert `(doc_id, embedding)` into the layered graph. Fails
    /// closed on dim mismatch and on duplicate ids.
    ///
    /// **Not replay-safe.** Re-issuing `add_embedding` for the same
    /// `doc_id` returns [`SemanticErrorCode::IndexCorrupted`]. Channel
    /// subscribers that may replay events after a crash MUST use
    /// [`Self::upsert_embedding`] instead.
    pub fn add_embedding(
        &mut self,
        doc_id: DocId,
        embedding: &Embedding,
    ) -> Result<(), SemanticError> {
        if embedding.dim() != self.dim {
            return Err(SemanticError::new(
                SemanticErrorCode::SemDimMismatch,
                format!(
                    "embedding dim {} != builder dim {} for doc {doc_id}",
                    embedding.dim(),
                    self.dim
                ),
            ));
        }
        if self.nodes.contains_key(&doc_id) {
            return Err(SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                format!("duplicate doc {doc_id}"),
            ));
        }
        self.insert_new_node(doc_id, embedding)
    }

    /// Idempotent upsert of a document's embedding into the layered
    /// graph.
    ///
    /// If `doc_id` is already present, REMOVE the prior node first
    /// (along with every neighbour back-reference) then re-insert the
    /// new vector. Re-applying the same `(doc_id, embedding)` after
    /// itself is a no-op at the wire level provided the graph was
    /// quiescent on the first call: same `SipHash` seed → same level
    /// assignment → byte-identical graph reconstruction.
    ///
    /// Fails closed on dim mismatch with
    /// [`SemanticErrorCode::SemDimMismatch`].
    pub fn upsert_embedding(
        &mut self,
        doc_id: DocId,
        embedding: &Embedding,
    ) -> Result<(), SemanticError> {
        if embedding.dim() != self.dim {
            return Err(SemanticError::new(
                SemanticErrorCode::SemDimMismatch,
                format!(
                    "embedding dim {} != builder dim {} for doc {doc_id}",
                    embedding.dim(),
                    self.dim
                ),
            ));
        }
        if self.nodes.contains_key(&doc_id) {
            let _existed: bool = self.remove_node_internal(doc_id);
        }
        self.insert_new_node(doc_id, embedding)
    }

    /// Idempotent removal of a document from the layered graph.
    ///
    /// Returns `Ok(true)` if `doc_id` was present and removed; `Ok(false)`
    /// if `doc_id` was not present (no-op). The `false` case is NOT an
    /// error: channel-replay tolerance per the producer/search-plane
    /// contract treats redundant deletes as idempotent.
    ///
    /// Removal purges every back-reference from neighbour nodes' layer
    /// lists so the graph stays bidirectionally consistent. If the
    /// removed doc was the entry point, the next-highest surviving node
    /// (lowest [`DocId`] at the new top level) is elected as the new
    /// entry.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "API stability: leave Result room for future builder caps per spec"
    )]
    pub fn remove_embedding(&mut self, doc_id: DocId) -> Result<bool, SemanticError> {
        Ok(self.remove_node_internal(doc_id))
    }

    /// Seed a fresh builder for `new_generation` from a prior finished
    /// [`HnswIndex`].
    ///
    /// The builder's `generation` becomes `new_generation` (must be
    /// non-zero); `dim` and `params` are inherited from `prior`. Every
    /// `(doc_id, vector)` pair in `prior.iter()` is re-inserted into a
    /// fresh graph in ascending [`DocId`] order. Because level
    /// assignment is a pure function of `(seed, doc_id)`, the resulting
    /// builder is deterministic per `(seed, prior.docs)`.
    ///
    /// **Note on graph topology**: this rebuild does NOT necessarily
    /// reproduce `prior`'s exact neighbour lists. HNSW graph shape is
    /// path-dependent on insertion order, and `from_prior` insertion
    /// order is the sorted-DocId order, which may differ from the
    /// original build order. The reconstructed graph remains a valid
    /// HNSW with the same docs and params; subsequent deltas applied via
    /// [`Self::upsert_embedding`] / [`Self::remove_embedding`] /
    /// [`Self::finish`] produce a byte-identical artifact across two
    /// calls of `from_prior` against the same prior.
    ///
    /// Rejects `new_generation == 0` with
    /// [`SemanticErrorCode::IndexCorrupted`].
    pub fn from_prior(prior: &HnswIndex, new_generation: u64) -> Result<Self, SemanticError> {
        if new_generation == 0 {
            return Err(SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                "new_generation must be non-zero",
            ));
        }
        let mut next = Self::new(new_generation, prior.dim, prior.params)?;
        for (doc_id, node) in &prior.nodes {
            let emb = Embedding::new(node.vector.clone())?;
            next.insert_new_node(*doc_id, &emb)?;
        }
        Ok(next)
    }

    /// Core insert path (no duplicate-check / no remove-prior). Used by
    /// [`Self::add_embedding`], [`Self::upsert_embedding`], and
    /// [`Self::from_prior`]. Assumes `embedding.dim() == self.dim` and
    /// the node is NOT already present.
    fn insert_new_node(
        &mut self,
        doc_id: DocId,
        embedding: &Embedding,
    ) -> Result<(), SemanticError> {
        let new_level = assign_level(&self.params, doc_id);
        let levels_count = usize::try_from(new_level.saturating_add(1)).map_err(|e| {
            SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                format!("level cast: {e}"),
            )
        })?;
        let new_node = HnswNode {
            vector: embedding.as_slice().to_vec(),
            levels: vec![Vec::new(); levels_count],
        };
        let prev = self.nodes.insert(doc_id, new_node);
        if prev.is_some() {
            return Err(SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                format!("duplicate doc {doc_id} (race)"),
            ));
        }
        let prev_level = self.levels.insert(doc_id, new_level);
        if prev_level.is_some() {
            return Err(SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                format!("duplicate doc {doc_id} level (race)"),
            ));
        }

        // First node becomes the entry point and the graph is done.
        let Some(entry_id) = self.entry else {
            self.entry = Some(doc_id);
            self.top_level = new_level;
            return Ok(());
        };

        // Greedy descent from current top down to new_level + 1: find
        // the nearest single neighbour at each layer.
        let q_vec = embedding.as_slice();
        let mut current = entry_id;
        let mut lvl = self.top_level;
        while lvl > new_level {
            current = self.greedy_descend(q_vec, current, lvl)?;
            if lvl == 0 {
                break;
            }
            lvl = lvl.saturating_sub(1);
        }

        // From min(top_level, new_level) down to 0: find ef_construction
        // candidates, prune to m neighbours, wire bidirectional links.
        let start_level = core::cmp::min(self.top_level, new_level);
        let mut layer_entry = current;
        let mut lvl_i = start_level;
        loop {
            let candidates =
                self.search_layer(q_vec, layer_entry, self.params.ef_construction, lvl_i)?;
            let neighbours = select_m_neighbours(&candidates, self.params.m)?;
            // Link new -> neighbour.
            for nb in &neighbours {
                self.link(doc_id, *nb, lvl_i)?;
                self.link(*nb, doc_id, lvl_i)?;
                // Prune neighbour's outgoing list to <= m by distance.
                self.prune_neighbours(*nb, lvl_i)?;
            }
            // Next-layer entry is the nearest of `neighbours`.
            if let Some((nearest, _)) = candidates.first() {
                layer_entry = *nearest;
            }
            if lvl_i == 0 {
                break;
            }
            lvl_i = lvl_i.saturating_sub(1);
        }

        if new_level > self.top_level {
            self.top_level = new_level;
            self.entry = Some(doc_id);
        }
        Ok(())
    }

    /// Internal helper shared by [`Self::upsert_embedding`] and
    /// [`Self::remove_embedding`]. Returns `true` if the doc existed.
    ///
    /// Purges the node from `self.nodes` and `self.levels`, and removes
    /// every back-reference from neighbour nodes' per-layer lists. If
    /// the removed doc was the entry point, re-elects a new entry as
    /// the surviving node with the highest `levels` value, breaking
    /// ties by ascending [`DocId`]. The graph stays bidirectionally
    /// consistent on every removal so subsequent searches do not chase
    /// a dangling reference.
    fn remove_node_internal(&mut self, doc_id: DocId) -> bool {
        let Some(removed_node) = self.nodes.remove(&doc_id) else {
            return false;
        };
        let _removed_level: Option<u32> = self.levels.remove(&doc_id);

        // Purge back-references. The neighbour set at every layer of
        // the removed node tells us exactly which other nodes might
        // still carry `doc_id` in their per-layer lists.
        for (lvl_us, neighbours) in removed_node.levels.iter().enumerate() {
            for nb in neighbours {
                if let Some(other) = self.nodes.get_mut(nb)
                    && let Some(olist) = other.levels.get_mut(lvl_us)
                    && let Ok(pos) = olist.binary_search(&doc_id)
                {
                    let _evicted: DocId = olist.remove(pos);
                }
            }
        }

        // Re-elect entry point if needed.
        if self.entry == Some(doc_id) {
            // Find the node with the highest recorded level, tiebreak
            // by smallest DocId. Iterating `self.levels` (a `BTreeMap`)
            // gives us ascending DocId order so the first occurrence
            // of the max level wins the tiebreak by construction.
            let mut best: Option<(DocId, u32)> = None;
            for (id, lvl) in &self.levels {
                match best {
                    None => best = Some((*id, *lvl)),
                    Some((_, bl)) => {
                        if *lvl > bl {
                            best = Some((*id, *lvl));
                        }
                    }
                }
            }
            if let Some((id, lvl)) = best {
                self.entry = Some(id);
                self.top_level = lvl;
            } else {
                self.entry = None;
                self.top_level = 0;
            }
        }
        true
    }

    /// Greedy single-step descent: from `start`, walk to the nearest
    /// neighbour at `level` until no neighbour is closer than the
    /// current node. Used for layers above the inserted node's level
    /// and for the search-phase descent.
    fn greedy_descend(&self, q: &[f32], start: DocId, level: u32) -> Result<DocId, SemanticError> {
        let lvl_us = usize::try_from(level).map_err(|e| {
            SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                format!("level cast: {e}"),
            )
        })?;
        let mut current = start;
        let mut current_dist = self.distance(q, current)?;
        loop {
            let Some(node) = self.nodes.get(&current) else {
                return Err(SemanticError::new(
                    SemanticErrorCode::IndexCorrupted,
                    format!("greedy_descend: missing node {current}"),
                ));
            };
            let Some(neighbours) = node.levels.get(lvl_us) else {
                return Ok(current);
            };
            let mut next: Option<(DocId, f32)> = None;
            for nb in neighbours {
                if *nb == current {
                    continue;
                }
                let d = self.distance(q, *nb)?;
                match next {
                    None => next = Some((*nb, d)),
                    Some((bid, bd)) => {
                        #[expect(
                            clippy::float_cmp,
                            reason = "deterministic tie-break: bit-identical distances tie on DocId"
                        )]
                        let take = d < bd || (d == bd && *nb < bid);
                        if take {
                            next = Some((*nb, d));
                        }
                    }
                }
            }
            match next {
                Some((nb, nd)) if nd < current_dist => {
                    current = nb;
                    current_dist = nd;
                }
                _ => return Ok(current),
            }
        }
    }

    /// Best-first search at one layer. Returns `(doc, distance)` pairs
    /// sorted by ascending distance, tied by ascending [`DocId`], cap
    /// at `ef`.
    fn search_layer(
        &self,
        q: &[f32],
        entry: DocId,
        ef: u32,
        level: u32,
    ) -> Result<Vec<(DocId, f32)>, SemanticError> {
        let lvl_us = usize::try_from(level).map_err(|e| {
            SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                format!("level cast: {e}"),
            )
        })?;
        let ef_us = usize::try_from(ef).map_err(|e| {
            SemanticError::new(SemanticErrorCode::IndexCorrupted, format!("ef cast: {e}"))
        })?;
        let mut visited: BTreeSet<DocId> = BTreeSet::new();
        let mut results: Vec<(DocId, f32)> = Vec::new();
        let mut frontier: Vec<(DocId, f32)> = Vec::new();

        let entry_dist = self.distance(q, entry)?;
        let _ins_entry: bool = visited.insert(entry);
        results.push((entry, entry_dist));
        frontier.push((entry, entry_dist));

        while let Some(idx) = pop_nearest(&mut frontier) {
            let (current, current_dist) = idx;
            // Termination: if the frontier's nearest is worse than
            // the worst kept result and we already have ef results,
            // we are done.
            if results.len() >= ef_us
                && let Some(worst) = worst_distance(&results)
                && current_dist > worst
            {
                break;
            }
            let Some(node) = self.nodes.get(&current) else {
                return Err(SemanticError::new(
                    SemanticErrorCode::IndexCorrupted,
                    format!("search_layer: missing node {current}"),
                ));
            };
            let Some(neighbours) = node.levels.get(lvl_us) else {
                continue;
            };
            for nb in neighbours {
                if !visited.insert(*nb) {
                    continue;
                }
                let d = self.distance(q, *nb)?;
                let mut accept = false;
                if results.len() < ef_us {
                    accept = true;
                } else if let Some(worst) = worst_distance(&results) {
                    #[expect(
                        clippy::float_cmp,
                        reason = "deterministic tie-break: bit-identical distances tie on DocId"
                    )]
                    let take = d < worst || (d == worst && *nb < worst_docid(&results));
                    if take {
                        accept = true;
                    }
                }
                if accept {
                    results.push((*nb, d));
                    frontier.push((*nb, d));
                    if results.len() > ef_us {
                        // Drop the worst entry.
                        sort_by_dist_then_id(&mut results);
                        let _popped: Option<(DocId, f32)> = results.pop();
                    }
                }
            }
        }
        sort_by_dist_then_id(&mut results);
        Ok(results)
    }

    fn distance(&self, q: &[f32], doc: DocId) -> Result<f32, SemanticError> {
        let Some(node) = self.nodes.get(&doc) else {
            return Err(SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                format!("distance: missing node {doc}"),
            ));
        };
        let sim = cosine_similarity(q, node.vector.as_slice())?;
        // Distance = 1 - cosine_similarity. Bounded `[0.0, 2.0]` for
        // any well-formed input.
        Ok(1.0_f32 - sim)
    }

    fn link(&mut self, from: DocId, to: DocId, level: u32) -> Result<(), SemanticError> {
        if from == to {
            return Ok(());
        }
        let lvl_us = usize::try_from(level).map_err(|e| {
            SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                format!("level cast: {e}"),
            )
        })?;
        let Some(node) = self.nodes.get_mut(&from) else {
            return Err(SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                format!("link: missing node {from}"),
            ));
        };
        // Ensure layer-list capacity. Nodes are allocated with their
        // own max-level depth at insert time; the existence check here
        // is defense-in-depth in case `link` is called for a layer
        // above the from-node's level (shouldn't happen but we fail
        // closed).
        if node.levels.get(lvl_us).is_none() {
            return Err(SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                format!("link: node {from} has no layer {level}"),
            ));
        }
        if node.has_neighbor_at(lvl_us, to) {
            return Ok(());
        }
        let Some(list) = node.levels.get_mut(lvl_us) else {
            return Err(SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                format!("link: layer {level} missing on {from}"),
            ));
        };
        let insert_at = list.partition_point(|d| *d < to);
        list.insert(insert_at, to);
        Ok(())
    }

    fn prune_neighbours(&mut self, doc: DocId, level: u32) -> Result<(), SemanticError> {
        let lvl_us = usize::try_from(level).map_err(|e| {
            SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                format!("level cast: {e}"),
            )
        })?;
        let m_us = usize::try_from(self.params.m).map_err(|e| {
            SemanticError::new(SemanticErrorCode::IndexCorrupted, format!("m cast: {e}"))
        })?;
        // Snapshot current neighbour list and the host vector.
        let (mine_vec, current): (Vec<f32>, Vec<DocId>) = {
            let Some(node) = self.nodes.get(&doc) else {
                return Err(SemanticError::new(
                    SemanticErrorCode::IndexCorrupted,
                    format!("prune: missing node {doc}"),
                ));
            };
            let Some(list) = node.levels.get(lvl_us) else {
                return Ok(());
            };
            if list.len() <= m_us {
                return Ok(());
            }
            (node.vector.clone(), list.clone())
        };
        // Rank neighbours by distance to host, tiebreak DocId asc.
        let mut scored: Vec<(DocId, f32)> = Vec::with_capacity(current.len());
        for nb in &current {
            let d = self.distance(mine_vec.as_slice(), *nb)?;
            scored.push((*nb, d));
        }
        sort_by_dist_then_id(&mut scored);
        let kept: Vec<DocId> = scored.iter().take(m_us).map(|p| p.0).collect();
        let dropped: Vec<DocId> = scored.iter().skip(m_us).map(|p| p.0).collect();
        // Keep neighbour list sorted ascending.
        let mut kept_sorted = kept;
        kept_sorted.sort_unstable();
        let Some(node) = self.nodes.get_mut(&doc) else {
            return Err(SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                format!("prune: missing node {doc} (post)"),
            ));
        };
        let Some(list) = node.levels.get_mut(lvl_us) else {
            return Ok(());
        };
        *list = kept_sorted;
        // Mirror-unlink: remove `doc` from each dropped neighbour's
        // list at this level so the graph stays bidirectionally
        // consistent.
        for d in &dropped {
            let Some(other) = self.nodes.get_mut(d) else {
                continue;
            };
            let Some(olist) = other.levels.get_mut(lvl_us) else {
                continue;
            };
            if let Ok(pos) = olist.binary_search(&doc) {
                let _removed: DocId = olist.remove(pos);
            }
        }
        Ok(())
    }

    /// Finalise into an [`HnswIndex`]. The builder's `nodes` map is
    /// already in ascending [`DocId`] order; per-layer neighbour
    /// lists are kept sorted by [`HnswIndexBuilder::link`] so the
    /// wire encoding is byte-stable across two identical builds.
    #[must_use]
    pub fn finish(self) -> HnswIndex {
        HnswIndex {
            generation: self.generation,
            dim: self.dim,
            params: self.params,
            nodes: self.nodes,
            entry: self.entry,
            top_level: self.top_level,
        }
    }
}

/// Pop the (`DocId`, dist) with the smallest distance (tiebreak: smaller
/// `DocId`). Mutates the frontier in place.
fn pop_nearest(frontier: &mut Vec<(DocId, f32)>) -> Option<(DocId, f32)> {
    if frontier.is_empty() {
        return None;
    }
    let mut best_idx: usize = 0;
    let mut best = match frontier.first() {
        Some(v) => *v,
        None => return None,
    };
    for (i, item) in frontier.iter().enumerate().skip(1) {
        let (id, d) = *item;
        #[expect(
            clippy::float_cmp,
            reason = "deterministic tie-break: bit-identical distances tie on DocId"
        )]
        let take = d < best.1 || (d == best.1 && id < best.0);
        if take {
            best = (id, d);
            best_idx = i;
        }
    }
    // Swap-remove preserves O(1) pop but breaks ordering; we recompute
    // `pop_nearest` each call so ordering is irrelevant.
    let _swapped: (DocId, f32) = frontier.swap_remove(best_idx);
    Some(best)
}

fn worst_distance(results: &[(DocId, f32)]) -> Option<f32> {
    let mut worst: Option<f32> = None;
    for (_, d) in results {
        worst = Some(worst.map_or(*d, |w| if *d > w { *d } else { w }));
    }
    worst
}

fn worst_docid(results: &[(DocId, f32)]) -> DocId {
    let mut worst_dist: Option<f32> = None;
    let mut worst_id: DocId = DocId(0);
    for (id, d) in results {
        match worst_dist {
            None => {
                worst_dist = Some(*d);
                worst_id = *id;
            }
            Some(w) => {
                #[expect(
                    clippy::float_cmp,
                    reason = "deterministic tie-break: bit-identical distances tie on DocId"
                )]
                let take = *d > w || (*d == w && *id > worst_id);
                if take {
                    worst_dist = Some(*d);
                    worst_id = *id;
                }
            }
        }
    }
    worst_id
}

fn sort_by_dist_then_id(v: &mut [(DocId, f32)]) {
    v.sort_by(|a, b| {
        let by_d = a.1.total_cmp(&b.1);
        match by_d {
            Ordering::Less | Ordering::Greater => by_d,
            Ordering::Equal => a.0.cmp(&b.0),
        }
    });
}

/// Pick the M nearest candidates (already sorted) — simple heuristic
/// matching the SEM-01 deterministic contract.
fn select_m_neighbours(candidates: &[(DocId, f32)], m: u32) -> Result<Vec<DocId>, SemanticError> {
    let m_us = usize::try_from(m).map_err(|e| {
        SemanticError::new(SemanticErrorCode::IndexCorrupted, format!("m cast: {e}"))
    })?;
    let mut out: Vec<DocId> = Vec::with_capacity(core::cmp::min(m_us, candidates.len()));
    for (id, _) in candidates.iter().take(m_us) {
        out.push(*id);
    }
    Ok(out)
}

// ─────────────────────────── Query ───────────────────────────

/// Execute a cosine top-k search against an [`HnswIndex`]. Same
/// determinism contract as the exact-NN path: same `(query, index)`
/// produces a byte-identical result list two runs later.
pub fn query_hnsw(
    idx: &HnswIndex,
    query: &Embedding,
    top_k: u32,
) -> Result<Vec<AnnResult>, SemanticError> {
    if top_k == 0 {
        return Ok(Vec::new());
    }
    if query.dim() != idx.dim {
        return Err(SemanticError::new(
            SemanticErrorCode::SemDimMismatch,
            format!("query dim {} != index dim {}", query.dim(), idx.dim),
        ));
    }
    let Some(entry) = idx.entry else {
        return Ok(Vec::new());
    };

    let q = query.as_slice();
    // Read-only proxy that re-uses the builder's search primitives.
    let view = HnswSearchView { idx };

    // Greedy descent from top_level down to layer 1.
    let mut current = entry;
    let mut lvl = idx.top_level;
    while lvl > 0 {
        current = view.greedy_descend(q, current, lvl)?;
        lvl = lvl.saturating_sub(1);
    }

    // Best-first search at layer 0 with ef_search.
    let candidates = view.search_layer(q, current, idx.params.ef_search, 0)?;
    let k_us = usize::try_from(top_k).map_err(|e| {
        SemanticError::new(
            SemanticErrorCode::IndexCorrupted,
            format!("top_k cast: {e}"),
        )
    })?;

    // Convert distance to similarity for the result type.
    let mut hits: Vec<AnnResult> = Vec::with_capacity(core::cmp::min(k_us, candidates.len()));
    for (doc, _dist) in candidates.iter().take(k_us) {
        // Recompute sim via the cosine kernel directly so the
        // surfaced score is byte-identical to the exact-NN path
        // (we deliberately do not reuse the ranking distance which
        // came back as `1 - sim`).
        let Some(node) = idx.nodes.get(doc) else {
            return Err(SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                format!("query_hnsw: missing node {doc}"),
            ));
        };
        let sim = cosine_similarity(q, node.vector.as_slice())?;
        hits.push(AnnResult::new(*doc, sim)?);
    }
    // Order results by score DESC, doc_id ASC.
    hits.sort_by(|a, b| {
        let by_score = b.score.total_cmp(&a.score);
        match by_score {
            Ordering::Less | Ordering::Greater => by_score,
            Ordering::Equal => a.doc_id.cmp(&b.doc_id),
        }
    });
    Ok(hits)
}

/// Read-only search-side view of an [`HnswIndex`]. Holds a reference
/// so the builder's algorithms can be reused without cloning.
struct HnswSearchView<'a> {
    idx: &'a HnswIndex,
}

impl HnswSearchView<'_> {
    fn distance(&self, q: &[f32], doc: DocId) -> Result<f32, SemanticError> {
        let Some(node) = self.idx.nodes.get(&doc) else {
            return Err(SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                format!("distance: missing node {doc}"),
            ));
        };
        let sim = cosine_similarity(q, node.vector.as_slice())?;
        Ok(1.0_f32 - sim)
    }

    fn greedy_descend(&self, q: &[f32], start: DocId, level: u32) -> Result<DocId, SemanticError> {
        let lvl_us = usize::try_from(level).map_err(|e| {
            SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                format!("level cast: {e}"),
            )
        })?;
        let mut current = start;
        let mut current_dist = self.distance(q, current)?;
        loop {
            let Some(node) = self.idx.nodes.get(&current) else {
                return Err(SemanticError::new(
                    SemanticErrorCode::IndexCorrupted,
                    format!("greedy_descend: missing node {current}"),
                ));
            };
            let Some(neighbours) = node.levels.get(lvl_us) else {
                return Ok(current);
            };
            let mut next: Option<(DocId, f32)> = None;
            for nb in neighbours {
                if *nb == current {
                    continue;
                }
                let d = self.distance(q, *nb)?;
                match next {
                    None => next = Some((*nb, d)),
                    Some((bid, bd)) => {
                        #[expect(
                            clippy::float_cmp,
                            reason = "deterministic tie-break: bit-identical distances tie on DocId"
                        )]
                        let take = d < bd || (d == bd && *nb < bid);
                        if take {
                            next = Some((*nb, d));
                        }
                    }
                }
            }
            match next {
                Some((nb, nd)) if nd < current_dist => {
                    current = nb;
                    current_dist = nd;
                }
                _ => return Ok(current),
            }
        }
    }

    fn search_layer(
        &self,
        q: &[f32],
        entry: DocId,
        ef: u32,
        level: u32,
    ) -> Result<Vec<(DocId, f32)>, SemanticError> {
        let lvl_us = usize::try_from(level).map_err(|e| {
            SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                format!("level cast: {e}"),
            )
        })?;
        let ef_us = usize::try_from(ef).map_err(|e| {
            SemanticError::new(SemanticErrorCode::IndexCorrupted, format!("ef cast: {e}"))
        })?;
        let mut visited: BTreeSet<DocId> = BTreeSet::new();
        let mut results: Vec<(DocId, f32)> = Vec::new();
        let mut frontier: Vec<(DocId, f32)> = Vec::new();

        let entry_dist = self.distance(q, entry)?;
        let _ins_entry: bool = visited.insert(entry);
        results.push((entry, entry_dist));
        frontier.push((entry, entry_dist));

        while let Some(idx) = pop_nearest(&mut frontier) {
            let (current, current_dist) = idx;
            if results.len() >= ef_us
                && let Some(worst) = worst_distance(&results)
                && current_dist > worst
            {
                break;
            }
            let Some(node) = self.idx.nodes.get(&current) else {
                return Err(SemanticError::new(
                    SemanticErrorCode::IndexCorrupted,
                    format!("search_layer: missing node {current}"),
                ));
            };
            let Some(neighbours) = node.levels.get(lvl_us) else {
                continue;
            };
            for nb in neighbours {
                if !visited.insert(*nb) {
                    continue;
                }
                let d = self.distance(q, *nb)?;
                let mut accept = false;
                if results.len() < ef_us {
                    accept = true;
                } else if let Some(worst) = worst_distance(&results) {
                    #[expect(
                        clippy::float_cmp,
                        reason = "deterministic tie-break: bit-identical distances tie on DocId"
                    )]
                    let take = d < worst || (d == worst && *nb < worst_docid(&results));
                    if take {
                        accept = true;
                    }
                }
                if accept {
                    results.push((*nb, d));
                    frontier.push((*nb, d));
                    if results.len() > ef_us {
                        sort_by_dist_then_id(&mut results);
                        let _popped: Option<(DocId, f32)> = results.pop();
                    }
                }
            }
        }
        sort_by_dist_then_id(&mut results);
        Ok(results)
    }
}

// ─────────────────────────── Manual serde ───────────────────────────
//
// Wire shape (canonical CBOR map):
//
// {
//   "generation": u64,
//   "dim": u32,
//   "params": { "m": u32, "ef_construction": u32, "ef_search": u32, "seed": u64 },
//   "entry": <DocId>?,
//   "top_level": u32,
//   "nodes": [ [DocId, HnswNode], ... ],   // sorted ascending by DocId
// }
//
// `HnswNode` wire shape:
//
// {
//   "vector": [f32, ...],
//   "levels": [ [DocId, ...], ... ],       // per-layer neighbour list, sorted asc
// }

impl serde::Serialize for HnswParams {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(4))?;
        m.serialize_entry("m", &self.m)?;
        m.serialize_entry("ef_construction", &self.ef_construction)?;
        m.serialize_entry("ef_search", &self.ef_search)?;
        m.serialize_entry("seed", &self.seed)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for HnswParams {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = HnswParams;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("HnswParams map")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<HnswParams, M::Error> {
                let mut m: Option<u32> = None;
                let mut ef_c: Option<u32> = None;
                let mut ef_s: Option<u32> = None;
                let mut seed: Option<u64> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "m" => {
                            if m.is_some() {
                                return Err(serde::de::Error::duplicate_field("m"));
                            }
                            m = Some(map.next_value()?);
                        }
                        "ef_construction" => {
                            if ef_c.is_some() {
                                return Err(serde::de::Error::duplicate_field("ef_construction"));
                            }
                            ef_c = Some(map.next_value()?);
                        }
                        "ef_search" => {
                            if ef_s.is_some() {
                                return Err(serde::de::Error::duplicate_field("ef_search"));
                            }
                            ef_s = Some(map.next_value()?);
                        }
                        "seed" => {
                            if seed.is_some() {
                                return Err(serde::de::Error::duplicate_field("seed"));
                            }
                            seed = Some(map.next_value()?);
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(
                                other,
                                &["m", "ef_construction", "ef_search", "seed"],
                            ));
                        }
                    }
                }
                let m = m.ok_or_else(|| serde::de::Error::missing_field("m"))?;
                let ef_c =
                    ef_c.ok_or_else(|| serde::de::Error::missing_field("ef_construction"))?;
                let ef_s = ef_s.ok_or_else(|| serde::de::Error::missing_field("ef_search"))?;
                let seed = seed.ok_or_else(|| serde::de::Error::missing_field("seed"))?;
                let p = HnswParams {
                    m,
                    ef_construction: ef_c,
                    ef_search: ef_s,
                    seed,
                };
                p.validate()
                    .map_err(|e| serde::de::Error::custom(format!("{e}")))?;
                Ok(p)
            }
        }
        de.deserialize_map(V)
    }
}

impl serde::Serialize for HnswNode {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        m.serialize_entry("vector", &F32Seq(self.vector.as_slice()))?;
        m.serialize_entry("levels", &LevelsSeq(self.levels.as_slice()))?;
        m.end()
    }
}

struct F32Seq<'a>(&'a [f32]);
impl serde::Serialize for F32Seq<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq as _;
        let mut s = ser.serialize_seq(Some(self.0.len()))?;
        for f in self.0 {
            s.serialize_element(f)?;
        }
        s.end()
    }
}

struct LevelsSeq<'a>(&'a [Vec<DocId>]);
impl serde::Serialize for LevelsSeq<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq as _;
        let mut s = ser.serialize_seq(Some(self.0.len()))?;
        for lvl in self.0 {
            s.serialize_element(&DocIdSeq(lvl.as_slice()))?;
        }
        s.end()
    }
}

struct DocIdSeq<'a>(&'a [DocId]);
impl serde::Serialize for DocIdSeq<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq as _;
        let mut s = ser.serialize_seq(Some(self.0.len()))?;
        for d in self.0 {
            s.serialize_element(d)?;
        }
        s.end()
    }
}

impl<'de> serde::Deserialize<'de> for HnswNode {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = HnswNode;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("HnswNode map")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<HnswNode, M::Error> {
                let mut vector: Option<Vec<f32>> = None;
                let mut levels: Option<Vec<Vec<DocId>>> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "vector" => {
                            if vector.is_some() {
                                return Err(serde::de::Error::duplicate_field("vector"));
                            }
                            vector = Some(map.next_value()?);
                        }
                        "levels" => {
                            if levels.is_some() {
                                return Err(serde::de::Error::duplicate_field("levels"));
                            }
                            levels = Some(map.next_value()?);
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(
                                other,
                                &["vector", "levels"],
                            ));
                        }
                    }
                }
                let vector = vector.ok_or_else(|| serde::de::Error::missing_field("vector"))?;
                let levels = levels.ok_or_else(|| serde::de::Error::missing_field("levels"))?;
                for (i, f) in vector.iter().enumerate() {
                    if !f.is_finite() {
                        return Err(serde::de::Error::custom(format!(
                            "vector component {i} non-finite: {f}"
                        )));
                    }
                }
                Ok(HnswNode { vector, levels })
            }
        }
        de.deserialize_map(V)
    }
}

impl serde::Serialize for HnswIndex {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(6))?;
        m.serialize_entry("generation", &self.generation)?;
        m.serialize_entry("dim", &self.dim)?;
        m.serialize_entry("params", &self.params)?;
        let entry_opt: Option<DocId> = self.entry;
        m.serialize_entry("entry", &entry_opt)?;
        m.serialize_entry("top_level", &self.top_level)?;
        m.serialize_entry("nodes", &NodesSeq(&self.nodes))?;
        m.end()
    }
}

struct NodesSeq<'a>(&'a BTreeMap<DocId, HnswNode>);
impl serde::Serialize for NodesSeq<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq as _;
        let mut s = ser.serialize_seq(Some(self.0.len()))?;
        for (doc, node) in self.0 {
            s.serialize_element(&NodesEntry(*doc, node))?;
        }
        s.end()
    }
}

struct NodesEntry<'a>(DocId, &'a HnswNode);
impl serde::Serialize for NodesEntry<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeTuple as _;
        let mut t = ser.serialize_tuple(2)?;
        t.serialize_element(&self.0)?;
        t.serialize_element(self.1)?;
        t.end()
    }
}

impl<'de> serde::Deserialize<'de> for HnswIndex {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = HnswIndex;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("HnswIndex map")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<HnswIndex, M::Error> {
                let mut generation: Option<u64> = None;
                let mut dim: Option<u32> = None;
                let mut params: Option<HnswParams> = None;
                let mut entry: Option<Option<DocId>> = None;
                let mut top_level: Option<u32> = None;
                let mut nodes_seq: Option<Vec<(DocId, HnswNode)>> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "generation" => {
                            if generation.is_some() {
                                return Err(serde::de::Error::duplicate_field("generation"));
                            }
                            generation = Some(map.next_value()?);
                        }
                        "dim" => {
                            if dim.is_some() {
                                return Err(serde::de::Error::duplicate_field("dim"));
                            }
                            dim = Some(map.next_value()?);
                        }
                        "params" => {
                            if params.is_some() {
                                return Err(serde::de::Error::duplicate_field("params"));
                            }
                            params = Some(map.next_value()?);
                        }
                        "entry" => {
                            if entry.is_some() {
                                return Err(serde::de::Error::duplicate_field("entry"));
                            }
                            entry = Some(map.next_value()?);
                        }
                        "top_level" => {
                            if top_level.is_some() {
                                return Err(serde::de::Error::duplicate_field("top_level"));
                            }
                            top_level = Some(map.next_value()?);
                        }
                        "nodes" => {
                            if nodes_seq.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodes"));
                            }
                            nodes_seq = Some(map.next_value()?);
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(
                                other,
                                &["generation", "dim", "params", "entry", "top_level", "nodes"],
                            ));
                        }
                    }
                }
                let generation =
                    generation.ok_or_else(|| serde::de::Error::missing_field("generation"))?;
                if generation == 0 {
                    return Err(serde::de::Error::custom("generation must be non-zero"));
                }
                let dim = dim.ok_or_else(|| serde::de::Error::missing_field("dim"))?;
                if dim == 0 {
                    return Err(serde::de::Error::custom("dim must be non-zero"));
                }
                let max = u32::try_from(MAX_EMBEDDING_DIM).map_err(|e| {
                    serde::de::Error::custom(format!("MAX_EMBEDDING_DIM cast: {e}"))
                })?;
                if dim > max {
                    return Err(serde::de::Error::custom(format!(
                        "dim {dim} exceeds MAX_EMBEDDING_DIM {max}"
                    )));
                }
                let params = params.ok_or_else(|| serde::de::Error::missing_field("params"))?;
                let entry_outer = entry.ok_or_else(|| serde::de::Error::missing_field("entry"))?;
                let top_level =
                    top_level.ok_or_else(|| serde::de::Error::missing_field("top_level"))?;
                if top_level > MAX_LEVEL {
                    return Err(serde::de::Error::custom(format!(
                        "top_level {top_level} exceeds MAX_LEVEL {MAX_LEVEL}"
                    )));
                }
                let entries = nodes_seq.ok_or_else(|| serde::de::Error::missing_field("nodes"))?;
                let dim_usize = usize::try_from(dim)
                    .map_err(|e| serde::de::Error::custom(format!("dim cast: {e}")))?;
                let mut nodes: BTreeMap<DocId, HnswNode> = BTreeMap::new();
                for (id, node) in entries {
                    if node.vector.len() != dim_usize {
                        return Err(serde::de::Error::custom(format!(
                            "doc {id} has vec dim {} but index dim is {dim}",
                            node.vector.len()
                        )));
                    }
                    for lvl in &node.levels {
                        let mut prev: Option<DocId> = None;
                        for d in lvl {
                            if let Some(p) = prev
                                && *d <= p
                            {
                                return Err(serde::de::Error::custom(format!(
                                    "doc {id} neighbour list not strictly ascending"
                                )));
                            }
                            prev = Some(*d);
                        }
                    }
                    if nodes.insert(id, node).is_some() {
                        return Err(serde::de::Error::custom(format!(
                            "duplicate doc {id} in nodes sequence"
                        )));
                    }
                }
                if let Some(eid) = entry_outer
                    && !nodes.contains_key(&eid)
                {
                    return Err(serde::de::Error::custom(format!(
                        "entry doc {eid} missing from nodes map"
                    )));
                }
                Ok(HnswIndex {
                    generation,
                    dim,
                    params,
                    nodes,
                    entry: entry_outer,
                    top_level,
                })
            }
        }
        de.deserialize_map(V)
    }
}

// ─────────────────────────── Tests ───────────────────────────

#[cfg(test)]
mod tests {
    use super::{
        HnswIndexBuilder, HnswParams, MAX_EF, MAX_LEVEL, MAX_M, MIN_M, assign_level, query_hnsw,
        siphash_2_4,
    };
    use crate::errors::SemanticErrorCode;
    use crate::types::{DocId, Embedding};

    fn emb(v: Vec<f32>) -> Embedding {
        let Ok(e) = Embedding::new(v) else {
            std::process::abort();
        };
        e
    }

    #[test]
    fn defaults_validate() {
        match HnswParams::DEFAULTS.validate() {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
        assert_eq!(HnswParams::DEFAULTS.m, 16);
        assert_eq!(HnswParams::DEFAULTS.ef_construction, 200);
        assert_eq!(HnswParams::DEFAULTS.ef_search, 100);
        assert_eq!(HnswParams::DEFAULTS.seed, 0xdead_beef_cafe_f00d);
    }

    #[test]
    fn params_reject_zero_m() {
        match HnswParams::new(0, 16, 16, 0) {
            Ok(_) => assert!(false, "must reject"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemHnswParamsInvalid),
        }
    }

    #[test]
    fn params_reject_non_power_of_two_m() {
        match HnswParams::new(6, 16, 16, 0) {
            Ok(_) => assert!(false, "must reject"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemHnswParamsInvalid),
        }
    }

    #[test]
    fn params_reject_oversized_m() {
        match HnswParams::new(MAX_M.saturating_mul(2), 200, 100, 0) {
            Ok(_) => assert!(false, "must reject"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemHnswParamsInvalid),
        }
    }

    #[test]
    fn params_reject_ef_c_below_m() {
        match HnswParams::new(16, 8, 16, 0) {
            Ok(_) => assert!(false, "must reject"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemHnswParamsInvalid),
        }
    }

    #[test]
    fn params_reject_ef_c_oversized() {
        match HnswParams::new(16, MAX_EF.saturating_add(1), 16, 0) {
            Ok(_) => assert!(false, "must reject"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemHnswParamsInvalid),
        }
    }

    #[test]
    fn params_reject_zero_ef_search() {
        match HnswParams::new(16, 200, 0, 0) {
            Ok(_) => assert!(false, "must reject"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemHnswParamsInvalid),
        }
    }

    #[test]
    fn params_reject_min_m_below_bound() {
        match HnswParams::new(MIN_M.saturating_sub(1), 16, 16, 0) {
            Ok(_) => assert!(false, "must reject"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemHnswParamsInvalid),
        }
    }

    #[test]
    fn siphash_deterministic_same_inputs() {
        let h1 = siphash_2_4(0xdead_beef, DocId(42));
        let h2 = siphash_2_4(0xdead_beef, DocId(42));
        assert_eq!(h1, h2);
    }

    #[test]
    fn siphash_differs_on_different_seeds() {
        let h1 = siphash_2_4(0xdead_beef, DocId(42));
        let h2 = siphash_2_4(0xdead_beee, DocId(42));
        assert_ne!(h1, h2);
    }

    #[test]
    fn siphash_differs_on_different_doc_ids() {
        let h1 = siphash_2_4(0xdead_beef, DocId(42));
        let h2 = siphash_2_4(0xdead_beef, DocId(43));
        assert_ne!(h1, h2);
    }

    #[test]
    fn assign_level_bounded_by_max() {
        for id in 0u64..200u64 {
            let l = assign_level(&HnswParams::DEFAULTS, DocId(id));
            assert!(l <= MAX_LEVEL);
        }
    }

    #[test]
    fn assign_level_distribution_is_mostly_zero() {
        // 1/16 probability of level >= 1 -> across 256 docs we expect
        // ~16 promotions. We assert a loose bound to keep the test
        // robust to future `SipHash` key tweaks.
        let mut zero = 0u32;
        let mut nonzero = 0u32;
        for id in 0u64..256u64 {
            if assign_level(&HnswParams::DEFAULTS, DocId(id)) == 0 {
                zero = zero.saturating_add(1);
            } else {
                nonzero = nonzero.saturating_add(1);
            }
        }
        assert!(zero > 200, "expected mostly zero levels, got {zero}");
        assert!(nonzero > 0, "expected at least one promotion");
    }

    fn small_corpus() -> super::HnswIndex {
        let Ok(mut b) = HnswIndexBuilder::new(7, 3, HnswParams::DEFAULTS) else {
            std::process::abort();
        };
        for (id, v) in &[
            (1u64, [1.0_f32, 0.0, 0.0]),
            (2, [0.0, 1.0, 0.0]),
            (3, [0.0, 0.0, 1.0]),
            (4, [-1.0, 0.0, 0.0]),
            (5, [1.0, 1.0, 0.0]),
        ] {
            let e = emb(v.to_vec());
            if b.add_embedding(DocId(*id), &e).is_err() {
                std::process::abort();
            }
        }
        b.finish()
    }

    #[test]
    fn small_corpus_self_query_top1() {
        let idx = small_corpus();
        let q = emb(vec![1.0_f32, 0.0, 0.0]);
        let out = match query_hnsw(&idx, &q, 1) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(out.len(), 1);
        let Some(r) = out.first() else {
            assert!(false, "missing");
            return;
        };
        assert_eq!(r.doc_id, DocId(1));
        assert!((r.score - 1.0_f32).abs() < 1e-5);
    }

    #[test]
    fn query_hnsw_top_k_zero_returns_empty() {
        let idx = small_corpus();
        let q = emb(vec![1.0_f32, 0.0, 0.0]);
        match query_hnsw(&idx, &q, 0) {
            Ok(v) => assert!(v.is_empty()),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn query_hnsw_dim_mismatch_errors() {
        let idx = small_corpus();
        let q = emb(vec![1.0_f32, 0.0]);
        match query_hnsw(&idx, &q, 1) {
            Ok(_) => assert!(false, "must reject"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemDimMismatch),
        }
    }

    #[test]
    fn query_hnsw_empty_index_returns_empty() {
        let Ok(b) = HnswIndexBuilder::new(7, 3, HnswParams::DEFAULTS) else {
            assert!(false, "build");
            return;
        };
        let idx = b.finish();
        let q = emb(vec![1.0_f32, 0.0, 0.0]);
        match query_hnsw(&idx, &q, 5) {
            Ok(v) => assert!(v.is_empty()),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn builder_rejects_duplicate_docid() {
        let Ok(mut b) = HnswIndexBuilder::new(7, 3, HnswParams::DEFAULTS) else {
            assert!(false, "build");
            return;
        };
        let e1 = emb(vec![1.0_f32, 0.0, 0.0]);
        if b.add_embedding(DocId(1), &e1).is_err() {
            assert!(false, "first insert");
            return;
        }
        match b.add_embedding(DocId(1), &e1) {
            Ok(()) => assert!(false, "must reject"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::IndexCorrupted),
        }
    }

    #[test]
    fn builder_rejects_dim_mismatch() {
        let Ok(mut b) = HnswIndexBuilder::new(7, 3, HnswParams::DEFAULTS) else {
            assert!(false, "build");
            return;
        };
        let e1 = emb(vec![1.0_f32, 0.0]);
        match b.add_embedding(DocId(1), &e1) {
            Ok(()) => assert!(false, "must reject"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemDimMismatch),
        }
    }

    #[test]
    fn hnsw_cbor_roundtrip() {
        let idx = small_corpus();
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&idx, &mut buf) {
            assert!(false, "ser: {e}");
            return;
        }
        match ciborium::de::from_reader::<super::HnswIndex, _>(buf.as_slice()) {
            Ok(got) => assert_eq!(got, idx),
            Err(e) => assert!(false, "de: {e}"),
        }
    }

    #[test]
    fn hnsw_byte_identical_across_builds() {
        let i1 = small_corpus();
        let i2 = small_corpus();
        let mut b1: Vec<u8> = Vec::new();
        let mut b2: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&i1, &mut b1) {
            assert!(false, "{e}");
            return;
        }
        if let Err(e) = ciborium::ser::into_writer(&i2, &mut b2) {
            assert!(false, "{e}");
            return;
        }
        assert_eq!(b1, b2);
    }

    #[test]
    fn hnsw_query_determinism_two_runs() {
        let idx = small_corpus();
        let q = emb(vec![1.0_f32, 1.0, 0.0]);
        let a = match query_hnsw(&idx, &q, 5) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let b = match query_hnsw(&idx, &q, 5) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(a, b);
    }

    // ─────────────────────── Round-6 delta API tests ───────────────────────

    fn serialize_hnsw(idx: &super::HnswIndex) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        if ciborium::ser::into_writer(idx, &mut buf).is_err() {
            std::process::abort();
        }
        buf
    }

    #[test]
    fn hnsw_upsert_replaces_existing_doc() {
        let Ok(mut b) = HnswIndexBuilder::new(7, 3, HnswParams::DEFAULTS) else {
            assert!(false, "build");
            return;
        };
        let v1 = emb(vec![1.0_f32, 0.0_f32, 0.0_f32]);
        if b.upsert_embedding(DocId(1), &v1).is_err() {
            assert!(false, "upsert 1");
            return;
        }
        let v2 = emb(vec![0.0_f32, 1.0_f32, 0.0_f32]);
        if b.upsert_embedding(DocId(1), &v2).is_err() {
            assert!(false, "upsert 2");
            return;
        }
        let idx = b.finish();
        assert_eq!(idx.corpus_size(), 1);
        // Query the new vector — should find DocId(1) as nearest with
        // similarity ~1.0.
        let q = emb(vec![0.0_f32, 1.0_f32, 0.0_f32]);
        match query_hnsw(&idx, &q, 1) {
            Ok(out) => {
                let Some(top) = out.first() else {
                    assert!(false, "empty result");
                    return;
                };
                assert_eq!(top.doc_id, DocId(1));
                assert!((top.score - 1.0_f32).abs() < 1e-5);
            }
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn hnsw_upsert_same_embedding_is_idempotent() {
        let Ok(mut a) = HnswIndexBuilder::new(7, 3, HnswParams::DEFAULTS) else {
            assert!(false, "a");
            return;
        };
        let Ok(mut b) = HnswIndexBuilder::new(7, 3, HnswParams::DEFAULTS) else {
            assert!(false, "b");
            return;
        };
        let e = emb(vec![0.5_f32, 0.5_f32, 0.5_f32]);
        if a.upsert_embedding(DocId(3), &e).is_err() {
            assert!(false, "a upsert");
            return;
        }
        if b.upsert_embedding(DocId(3), &e).is_err() {
            assert!(false, "b upsert 1");
            return;
        }
        if b.upsert_embedding(DocId(3), &e).is_err() {
            assert!(false, "b upsert 2");
            return;
        }
        assert_eq!(serialize_hnsw(&a.finish()), serialize_hnsw(&b.finish()));
    }

    #[test]
    fn hnsw_upsert_dim_mismatch_returns_typed() {
        let Ok(mut b) = HnswIndexBuilder::new(7, 3, HnswParams::DEFAULTS) else {
            assert!(false, "build");
            return;
        };
        let e = emb(vec![1.0_f32, 0.0_f32]);
        match b.upsert_embedding(DocId(1), &e) {
            Ok(()) => assert!(false, "must reject"),
            Err(err) => assert_eq!(err.code, SemanticErrorCode::SemDimMismatch),
        }
    }

    #[test]
    fn hnsw_remove_existing_doc_returns_true() {
        let Ok(mut b) = HnswIndexBuilder::new(7, 3, HnswParams::DEFAULTS) else {
            assert!(false, "build");
            return;
        };
        let e1 = emb(vec![1.0_f32, 0.0_f32, 0.0_f32]);
        let e2 = emb(vec![0.0_f32, 1.0_f32, 0.0_f32]);
        if b.add_embedding(DocId(1), &e1).is_err() || b.add_embedding(DocId(2), &e2).is_err() {
            assert!(false, "inserts");
            return;
        }
        match b.remove_embedding(DocId(1)) {
            Ok(true) => {}
            Ok(false) => assert!(false, "should have existed"),
            Err(e) => assert!(false, "{e}"),
        }
        match b.remove_embedding(DocId(1)) {
            Ok(false) => {}
            Ok(true) => assert!(false, "second remove must be no-op"),
            Err(e) => assert!(false, "{e}"),
        }
        let idx = b.finish();
        // Query the second doc — should still be findable.
        let q = emb(vec![0.0_f32, 1.0_f32, 0.0_f32]);
        match query_hnsw(&idx, &q, 5) {
            Ok(out) => {
                let ids: Vec<DocId> = out.iter().map(|r| r.doc_id).collect();
                assert!(ids.contains(&DocId(2)));
                assert!(!ids.contains(&DocId(1)));
            }
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn hnsw_remove_nonexistent_doc_returns_false() {
        let Ok(mut b) = HnswIndexBuilder::new(7, 3, HnswParams::DEFAULTS) else {
            assert!(false, "build");
            return;
        };
        match b.remove_embedding(DocId(42)) {
            Ok(false) => {}
            Ok(true) => assert!(false, "must report false"),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn hnsw_remove_clears_entry_when_only_node() {
        let Ok(mut b) = HnswIndexBuilder::new(7, 3, HnswParams::DEFAULTS) else {
            assert!(false, "build");
            return;
        };
        let e = emb(vec![1.0_f32, 0.0_f32, 0.0_f32]);
        if b.add_embedding(DocId(1), &e).is_err() {
            assert!(false, "insert");
            return;
        }
        let Ok(removed) = b.remove_embedding(DocId(1)) else {
            assert!(false, "remove");
            return;
        };
        assert!(removed, "remove must report true");
        let idx = b.finish();
        assert!(idx.is_empty());
        assert!(idx.entry().is_none());
        assert_eq!(idx.top_level(), 0);
    }

    #[test]
    fn hnsw_from_prior_preserves_all_docs() {
        let prior = small_corpus();
        let next_b = match HnswIndexBuilder::from_prior(&prior, 8) {
            Ok(b) => b,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let next = next_b.finish();
        assert_eq!(next.generation(), 8);
        assert_eq!(next.dim(), prior.dim());
        assert_eq!(next.corpus_size(), prior.corpus_size());
        // Every doc in prior must be queryable by self-similarity.
        for (doc, node) in prior.iter() {
            let q = emb(node.vector.clone());
            match query_hnsw(&next, &q, 1) {
                Ok(out) => {
                    let Some(top) = out.first() else {
                        assert!(false, "empty result for doc {doc}");
                        return;
                    };
                    assert_eq!(top.doc_id, doc);
                }
                Err(e) => assert!(false, "{e}"),
            }
        }
    }

    #[test]
    fn hnsw_from_prior_then_remove_drops_only_target() {
        let prior = small_corpus();
        let Ok(mut b) = HnswIndexBuilder::from_prior(&prior, 8) else {
            assert!(false, "from_prior");
            return;
        };
        let Ok(removed) = b.remove_embedding(DocId(2)) else {
            assert!(false, "remove");
            return;
        };
        assert!(removed, "remove must report true");
        let out = b.finish();
        assert_eq!(out.corpus_size(), prior.corpus_size().saturating_sub(1));
        let q = emb(vec![0.0_f32, 1.0_f32, 0.0_f32]);
        match query_hnsw(&out, &q, 10) {
            Ok(hits) => {
                for r in hits {
                    assert!(r.doc_id != DocId(2));
                }
            }
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn hnsw_from_prior_with_upsert_replaces() {
        let prior = small_corpus();
        let Ok(mut b) = HnswIndexBuilder::from_prior(&prior, 8) else {
            assert!(false, "from_prior");
            return;
        };
        let e = emb(vec![0.0_f32, 1.0_f32, 0.0_f32]);
        if b.upsert_embedding(DocId(1), &e).is_err() {
            assert!(false, "upsert");
            return;
        }
        let out = b.finish();
        match query_hnsw(&out, &e, 1) {
            Ok(hits) => {
                let Some(top) = hits.first() else {
                    assert!(false, "empty");
                    return;
                };
                assert_eq!(top.doc_id, DocId(1));
                assert!((top.score - 1.0_f32).abs() < 1e-5);
            }
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn hnsw_from_prior_with_same_seed_is_deterministic() {
        let prior = small_corpus();
        let Ok(a) = HnswIndexBuilder::from_prior(&prior, 8) else {
            assert!(false, "a");
            return;
        };
        let Ok(b) = HnswIndexBuilder::from_prior(&prior, 8) else {
            assert!(false, "b");
            return;
        };
        assert_eq!(serialize_hnsw(&a.finish()), serialize_hnsw(&b.finish()));
    }

    #[test]
    fn hnsw_from_prior_rejects_zero_generation() {
        let prior = small_corpus();
        match HnswIndexBuilder::from_prior(&prior, 0) {
            Ok(_) => assert!(false, "must reject"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::IndexCorrupted),
        }
    }

    #[test]
    fn hnsw_upsert_then_remove_equals_empty() {
        let Ok(mut b) = HnswIndexBuilder::new(7, 3, HnswParams::DEFAULTS) else {
            assert!(false, "build");
            return;
        };
        let e = emb(vec![1.0_f32, 0.0_f32, 0.0_f32]);
        if b.upsert_embedding(DocId(1), &e).is_err() {
            assert!(false, "upsert");
            return;
        }
        let Ok(removed) = b.remove_embedding(DocId(1)) else {
            assert!(false, "remove");
            return;
        };
        assert!(removed, "remove must report true");
        let after = b.finish();
        let Ok(empty_b) = HnswIndexBuilder::new(7, 3, HnswParams::DEFAULTS) else {
            assert!(false, "empty");
            return;
        };
        let empty = empty_b.finish();
        assert_eq!(serialize_hnsw(&after), serialize_hnsw(&empty));
    }
}
