//! Deterministic, immutable HNSW regions. Catalog remains the publication owner.
//!
//! Graph distances select candidates only. The caller authenticates the selected
//! member and re-scores original owner vectors before admitting a result.

use std::{cmp::Ordering, collections::BinaryHeap};

use quanta_index_core::index::ports::ImmutableObjectPort;
use quanta_index_core::index::{
    AttemptContext, Digest, IndexError, MemoryLease, MemoryLifetime, ObjectRef, OwnerKey,
    ProducerId, StableOwnerId, WorkBudget, WorkUsage,
};
use sha2::{Digest as _, Sha256};

pub(super) use super::super::ann_region_policy::MAX_REGION_ROWS;
use super::super::ann_region_policy::{MAX_REGION_CELLS, MAX_REGION_DIMENSION};
use super::vector_norm;

pub(super) const MAX_REGION_CANDIDATES: usize = 1024;
const MAX_ID_BYTES: usize = 1024;
const M: usize = 16;
const EF_CONSTRUCTION: usize = 64;
const MAX_CONSTRUCTION_VISITS: usize = 256;
const MAX_LEVEL: usize = 6;
const SEED: u64 = 42;
const MAGIC: &[u8; 8] = b"QNHNSW02";
const HEADER_BYTES: usize = 8 + 32 + 8 + 4 + 4 + 8 + 4 + 4 + 4;
const MAX_ARTIFACT_BYTES: usize = 32 * 1024 * 1024;

fn base_cycle_neighbors(index: usize, count: usize) -> (usize, usize) {
    ((index + count - 1) % count, (index + 1) % count)
}

#[derive(Clone, Copy)]
pub(super) struct RegionRow<'a> {
    pub owner: OwnerKey,
    pub member: Digest,
    pub vector_id: &'a [u8],
    pub vector: &'a [f32],
}

pub(super) struct Row {
    pub owner: OwnerKey,
    pub member: Digest,
    pub vector_id: Vec<u8>,
    vector: Vec<f32>,
    norm: f64,
    links: Vec<Vec<u32>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Seal {
    pub recipe: Digest,
    pub generation: u64,
    pub dimension: u32,
    pub rows: u32,
}

pub(super) struct Graph {
    pub seal: Seal,
    pub rows: Vec<Row>,
    entry: usize,
    // Destroy all decoded backing before releasing the producing allowance.
    _memory: Option<MemoryLease>,
}

pub(super) struct Candidates {
    pub indices: Vec<usize>,
    pub capped: bool,
    scratch_bytes: usize,
    _memory: Option<MemoryLease>,
}

impl Candidates {
    /// Traversal has finished. Ranking may reuse the portion of its admitted
    /// scratch that no longer backs the candidate index list.
    pub(super) fn reusable_scratch_bytes(&self) -> usize {
        self.scratch_bytes - self.indices.capacity() * std::mem::size_of::<usize>()
    }
}

#[derive(Clone, Copy, Debug)]
struct Scored {
    index: usize,
    cosine: f64,
}
impl PartialEq for Scored {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Scored {}
impl PartialOrd for Scored {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Scored {
    fn cmp(&self, other: &Self) -> Ordering {
        self.cosine
            .total_cmp(&other.cosine)
            .then_with(|| other.index.cmp(&self.index))
    }
}

/// Attempt-local derived distances. The immutable graph remains the only
/// persisted layout; none of this cache is serialized or reused across builds.
struct PruningDistances {
    nodes: Vec<Vec<Vec<Scored>>>,
    scores: Vec<f64>,
    epochs: Vec<u32>,
    epoch: u32,
    evaluations: usize,
    _memory: Option<MemoryLease>,
}

impl PruningDistances {
    fn new(
        input: &[RegionRow<'_>],
        recipe: Digest,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        let mut layers = 0usize;
        for row in input {
            budget.checkpoint()?;
            layers = layers
                .checked_add(level(recipe, *row) + 1)
                .ok_or(IndexError::ResourceLimited)?;
        }
        let layer_bytes = std::mem::size_of::<Vec<Scored>>()
            .checked_add(
                (M + 1)
                    .checked_mul(std::mem::size_of::<Scored>())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)?;
        let row_bytes = std::mem::size_of::<Vec<Vec<Scored>>>()
            + std::mem::size_of::<f64>()
            + std::mem::size_of::<u32>();
        let bytes = input
            .len()
            .checked_mul(row_bytes)
            .and_then(|bytes| bytes.checked_add(layers.checked_mul(layer_bytes)?))
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(
            u64::try_from(bytes).map_err(limited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut scores = reserve_vec(input.len())?;
        scores.resize(input.len(), 0.0);
        let mut epochs = reserve_vec(input.len())?;
        epochs.resize(input.len(), 0);
        Ok(Self {
            nodes: reserve_vec(input.len())?,
            scores,
            epochs,
            epoch: 0,
            evaluations: 0,
            _memory: memory,
        })
    }

    fn push_row(&mut self, height: usize) -> Result<(), IndexError> {
        let mut layers = reserve_vec(height + 1)?;
        for _layer in 0..=height {
            layers.push(reserve_vec(M + 1)?);
        }
        self.nodes.push(layers);
        Ok(())
    }

    fn begin_query(&mut self, index: usize) -> Result<(), IndexError> {
        self.evaluations = 0;
        self.epoch = u32::try_from(index.checked_add(1).ok_or(IndexError::ResourceLimited)?)
            .map_err(limited)?;
        Ok(())
    }

    fn distance(
        &mut self,
        graph: &Graph,
        index: usize,
        vector: &[f32],
        norm: f64,
        budget: &mut WorkBudget,
    ) -> Result<Option<Scored>, IndexError> {
        budget.checkpoint()?;
        if self.epoch == 0 || index >= self.epochs.len() {
            return Err(IndexError::BackendViolation(
                "native graph distance cache outside insertion".into(),
            ));
        }
        if self.epochs[index] == self.epoch {
            return Ok(Some(Scored {
                index,
                cosine: self.scores[index],
            }));
        }
        if self.evaluations == MAX_CONSTRUCTION_VISITS {
            return Ok(None);
        }
        let scored = graph.distance(index, vector, norm, budget)?;
        self.evaluations += 1;
        self.scores[index] = scored.cosine;
        self.epochs[index] = self.epoch;
        Ok(Some(scored))
    }
}

fn corrupt(message: &'static str) -> IndexError {
    IndexError::CorruptObject(message.into())
}
fn limited<T>(_: T) -> IndexError {
    IndexError::ResourceLimited
}

fn level(recipe: Digest, row: RegionRow<'_>) -> usize {
    let mut hash = Sha256::new();
    hash.update(b"quanta-native-hnsw-level-v1\0");
    hash.update(recipe.0);
    hash.update(SEED.to_le_bytes());
    hash.update(row.owner.producer.0);
    hash.update(row.owner.stable_id.0);
    hash.update(row.member.0);
    hash.update(row.vector_id);
    let digest = hash.finalize();
    let mut prefix = [0; 8];
    prefix.copy_from_slice(&digest[..8]);
    let bits = u64::from_le_bytes(prefix);
    (bits.trailing_zeros() as usize / 2).min(MAX_LEVEL)
}

fn reserve_vec<T>(count: usize) -> Result<Vec<T>, IndexError> {
    let mut values = Vec::new();
    values.try_reserve_exact(count).map_err(limited)?;
    if values.capacity() > count {
        return Err(IndexError::ResourceLimited);
    }
    Ok(values)
}

/// Admission covers all row, ID, vector, adjacency and construction scratch
/// backing before copying any input. The fixed envelope is independent of corpus.
fn memory_bound(rows: usize, dimension: usize, ids: usize) -> Result<u64, IndexError> {
    let per_row = std::mem::size_of::<Row>()
        .checked_add(
            dimension
                .checked_mul(4)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .and_then(|bytes| {
            bytes.checked_add((MAX_LEVEL + 1) * (std::mem::size_of::<Vec<u32>>() + M * 4))
        })
        .and_then(|bytes| bytes.checked_add(256))
        .ok_or(IndexError::ResourceLimited)?;
    u64::try_from(
        rows.checked_mul(per_row)
            .and_then(|bytes| bytes.checked_add(ids))
            .ok_or(IndexError::ResourceLimited)?,
    )
    .map_err(limited)
}

impl Graph {
    fn distance(
        &self,
        index: usize,
        vector: &[f32],
        norm: f64,
        budget: &mut WorkBudget,
    ) -> Result<Scored, IndexError> {
        budget.charge(WorkUsage {
            mutations: 1,
            ..WorkUsage::default()
        })?;
        let row = self
            .rows
            .get(index)
            .ok_or_else(|| corrupt("native graph row outside seal"))?;
        // Norm is derived once from the original f32 data during admission.
        let dot = vector
            .iter()
            .zip(&row.vector)
            .map(|(a, b)| f64::from(*a) * f64::from(*b))
            .sum::<f64>();
        let score = dot / (norm * row.norm);
        if !score.is_finite() {
            return Err(corrupt("native graph distance is not finite"));
        }
        Ok(Scored {
            index,
            cosine: score.clamp(-1.0, 1.0),
        })
    }

    fn greedy(
        &self,
        mut current: Scored,
        vector: &[f32],
        norm: f64,
        layer: usize,
        distances: &mut PruningDistances,
        budget: &mut WorkBudget,
    ) -> Result<Scored, IndexError> {
        loop {
            budget.checkpoint()?;
            let mut best = current;
            for &neighbor in &self.rows[current.index].links[layer] {
                let Some(candidate) =
                    distances.distance(self, neighbor as usize, vector, norm, budget)?
                else {
                    continue;
                };
                if candidate > best {
                    best = candidate;
                }
            }
            if best.index == current.index {
                return Ok(best);
            }
            current = best;
        }
    }

    /// HNSW best-first layer search. Discovery is bounded separately from the
    /// retained ef frontier; all distance work is charged to the same request.
    #[expect(
        clippy::too_many_arguments,
        reason = "geometry, bounded traversal and optional insertion-local distance custody remain explicit"
    )]
    fn layer(
        &self,
        start: Scored,
        vector: &[f32],
        norm: f64,
        layer: usize,
        ef: usize,
        max_visits: usize,
        mut distances: Option<&mut PruningDistances>,
        budget: &mut WorkBudget,
    ) -> Result<(Vec<Scored>, bool), IndexError> {
        let mut seen = reserve_vec(self.rows.len())?;
        seen.resize(self.rows.len(), false);
        let mut frontier = BinaryHeap::new();
        // The seen table admits each row at most once in this layer. A large
        // caller visit ceiling cannot justify backing beyond this graph's
        // rows, whose scratch was funded by memory_bound before traversal.
        frontier
            .try_reserve_exact(max_visits.min(self.rows.len()))
            .map_err(limited)?;
        let mut best = BinaryHeap::new();
        best.try_reserve(ef).map_err(limited)?;
        seen[start.index] = true;
        frontier.push(start);
        best.push(std::cmp::Reverse(start));
        let mut visits = 1usize;
        let mut capped = false;
        while let Some(current) = frontier.pop() {
            budget.checkpoint()?;
            if best.len() == ef && best.peek().is_some_and(|worst| current < worst.0) {
                break;
            }
            for &neighbor in &self.rows[current.index].links[layer] {
                let index = neighbor as usize;
                if seen[index] {
                    continue;
                }
                if visits == max_visits {
                    capped = true;
                    continue;
                }
                seen[index] = true;
                visits += 1;
                let candidate = if let Some(distances) = distances.as_deref_mut() {
                    let Some(candidate) = distances.distance(self, index, vector, norm, budget)?
                    else {
                        capped = true;
                        continue;
                    };
                    candidate
                } else {
                    self.distance(index, vector, norm, budget)?
                };
                if best.len() < ef || best.peek().is_some_and(|worst| candidate > worst.0) {
                    if best.len() == ef {
                        let _removed = best.pop();
                    }
                    best.push(std::cmp::Reverse(candidate));
                    frontier.push(candidate);
                }
            }
        }
        let mut result = reserve_vec(best.len())?;
        result.extend(best.into_iter().map(|row| row.0));
        result.sort_unstable_by(|a, b| b.cmp(a));
        Ok((result, capped))
    }

    fn connect(
        &mut self,
        index: usize,
        neighbors: &[Scored],
        layer: usize,
        distances: &mut PruningDistances,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        for neighbor in neighbors.iter().take(M) {
            budget.checkpoint()?;
            self.rows[index].links[layer].push(u32::try_from(neighbor.index).map_err(limited)?);
            distances.nodes[index][layer].push(*neighbor);
            let peer = neighbor.index;
            let links = &mut self.rows[peer].links[layer];
            links.push(u32::try_from(index).map_err(limited)?);
            let scores = &mut distances.nodes[peer][layer];
            // Cosine is symmetric on the same immutable f32 rows. This exact
            // score was already computed while selecting the new connection.
            scores.push(Scored {
                index,
                cosine: neighbor.cosine,
            });
            if scores.len() != links.len() {
                return Err(IndexError::BackendViolation(
                    "native graph pruning cache differs from links".into(),
                ));
            }
            if scores.len() > M {
                scores.sort_unstable_by(|a, b| b.cmp(a));
                scores.truncate(M);
                links.clear();
                links.extend(scores.iter().map(|entry| entry.index as u32));
            }
        }
        Ok(())
    }

    pub(super) fn candidates(
        &self,
        vector: &[f32],
        max_visits: usize,
        max_rescores: usize,
        budget: &mut WorkBudget,
    ) -> Result<Candidates, IndexError> {
        if max_visits == 0
            || max_rescores == 0
            || max_visits > MAX_REGION_ROWS
            || max_rescores > MAX_REGION_CANDIDATES
        {
            return Err(IndexError::InvalidInput(
                "native graph query limits invalid".into(),
            ));
        }
        let norm = vector_norm(vector, self.seal.dimension)?;
        // Charge upper-layer work as graph visits too. One seen table across
        // layers would avoid duplicate distances but must retain its allowance.
        // The counter is shared by all upper and base layer calls below.
        let scratch = memory_bound(self.rows.len(), 0, 0)?;
        let memory = budget.reserve_memory(scratch, MemoryLifetime::Temporary)?;
        let scratch_bytes = usize::try_from(scratch).map_err(limited)?;
        let mut current = self.distance(self.entry, vector, norm, budget)?;
        let mut upper_visits = 1usize;
        for layer in (1..self.rows[self.entry].links.len()).rev() {
            loop {
                let mut best = current;
                for &neighbor in &self.rows[current.index].links[layer] {
                    if upper_visits == max_visits {
                        let mut indices = reserve_vec(1)?;
                        indices.push(best.index);
                        return Ok(Candidates {
                            indices,
                            capped: true,
                            scratch_bytes,
                            _memory: memory,
                        });
                    }
                    upper_visits += 1;
                    let candidate = self.distance(neighbor as usize, vector, norm, budget)?;
                    if candidate > best {
                        best = candidate;
                    }
                }
                if best.index == current.index {
                    break;
                }
                current = best;
            }
        }
        let remaining = max_visits
            .saturating_sub(upper_visits)
            .saturating_add(1)
            .max(1);
        let (best, visit_capped) = self.layer(
            current,
            vector,
            norm,
            0,
            max_rescores.min(self.rows.len()),
            remaining,
            None,
            budget,
        )?;
        let rescore_capped = best.len() < self.rows.len();
        let mut indices = reserve_vec(best.len())?;
        indices.extend(best.iter().map(|entry| entry.index));
        Ok(Candidates {
            indices,
            capped: visit_capped || rescore_capped,
            scratch_bytes,
            _memory: memory,
        })
    }
}

pub(super) fn build(
    rows: &[RegionRow<'_>],
    recipe: Digest,
    generation: u64,
    dimension: usize,
    budget: &mut WorkBudget,
) -> Result<Graph, IndexError> {
    if rows.is_empty()
        || rows.len() > MAX_REGION_ROWS
        || dimension == 0
        || dimension > MAX_REGION_DIMENSION
        || generation == 0
        || rows
            .len()
            .checked_mul(dimension)
            .is_none_or(|cells| cells > MAX_REGION_CELLS)
    {
        return Err(IndexError::InvalidInput(
            "native graph build envelope invalid".into(),
        ));
    }
    let ids = rows.iter().try_fold(0usize, |sum, row| {
        if row.vector_id.is_empty() || row.vector_id.len() > MAX_ID_BYTES {
            return Err(IndexError::InvalidInput(
                "native graph vector ID invalid".into(),
            ));
        }
        sum.checked_add(row.vector_id.len())
            .ok_or(IndexError::ResourceLimited)
    })?;
    let memory = budget.reserve_memory(
        memory_bound(rows.len(), dimension, ids)?,
        MemoryLifetime::Temporary,
    )?;
    let mut graph = Graph {
        seal: Seal {
            recipe,
            generation,
            dimension: dimension as u32,
            rows: rows.len() as u32,
        },
        rows: reserve_vec(rows.len())?,
        entry: 0,
        _memory: memory,
    };
    let mut distances = PruningDistances::new(rows, recipe, budget)?;
    let mut previous: Option<(OwnerKey, Digest, &[u8])> = None;
    for input in rows {
        budget.checkpoint()?;
        let identity = (input.owner, input.member, input.vector_id);
        if let Some(prior) = previous {
            if prior >= identity {
                return Err(IndexError::InvalidInput(
                    "native graph rows must be strictly sorted".into(),
                ));
            }
            if prior.0 == input.owner && prior.1 != input.member {
                return Err(IndexError::InvalidInput(
                    "native graph owner has conflicting member commitments".into(),
                ));
            }
        }
        previous = Some(identity);
        let norm = vector_norm(input.vector, dimension as u32)?;
        let mut vector = reserve_vec(dimension)?;
        vector.extend_from_slice(input.vector);
        let mut vector_id = reserve_vec(input.vector_id.len())?;
        vector_id.extend_from_slice(input.vector_id);
        let height = level(recipe, *input);
        let mut links = reserve_vec(height + 1)?;
        for _layer in 0..=height {
            links.push(reserve_vec(M + 1)?);
        }
        let index = graph.rows.len();
        if index == 0 {
            graph.rows.push(Row {
                owner: input.owner,
                member: input.member,
                vector_id,
                vector,
                norm,
                links,
            });
            distances.push_row(height)?;
            continue;
        }
        let old_height = graph.rows[graph.entry].links.len() - 1;
        distances.begin_query(index)?;
        let mut current = distances
            .distance(&graph, graph.entry, input.vector, norm, budget)?
            .ok_or_else(|| {
                IndexError::BackendViolation("native graph insertion has no entry score".into())
            })?;
        for layer in ((height + 1)..=old_height).rev() {
            current = graph.greedy(current, input.vector, norm, layer, &mut distances, budget)?;
        }
        // All searches see only prior rows; the new row is appended after
        // finding its neighbors so an empty self-loop cannot win a frontier.
        let mut neighbors = reserve_vec(height.min(old_height) + 1)?;
        for layer in (0..=height.min(old_height)).rev() {
            let (found, _) = graph.layer(
                current,
                input.vector,
                norm,
                layer,
                EF_CONSTRUCTION.min(index),
                index,
                Some(&mut distances),
                budget,
            )?;
            current = found[0];
            neighbors.push((layer, found));
        }
        graph.rows.push(Row {
            owner: input.owner,
            member: input.member,
            vector_id,
            vector,
            norm,
            links,
        });
        distances.push_row(height)?;
        for (layer, found) in neighbors {
            graph.connect(index, &found, layer, &mut distances, budget)?;
        }
        if height > old_height {
            graph.entry = index;
        }
    }
    // Reserve two base edges for a deterministic cycle. Bounded-degree pruning
    // may otherwise strand a row in a directed component, defeating a full
    // region oracle even when the visit/rescore budgets cover every row.
    let count = graph.rows.len();
    if count > 1 {
        for index in 0..count {
            let (previous, next) = base_cycle_neighbors(index, count);
            let mut scored = reserve_vec(M)?;
            for &peer in &distances.nodes[index][0] {
                if peer.index != previous && peer.index != next {
                    scored.push(peer);
                }
            }
            scored.sort_unstable_by(|a, b| b.cmp(a));
            let links = &mut graph.rows[index].links[0];
            links.clear();
            links.push(previous as u32);
            if next != previous {
                links.push(next as u32);
            }
            let slots = M - links.len();
            links.extend(scored.iter().take(slots).map(|entry| entry.index as u32));
        }
    }
    drop(distances);
    for row in &mut graph.rows {
        for links in &mut row.links {
            links.sort_unstable();
        }
    }
    Ok(graph)
}

fn encoded_len(graph: &Graph) -> Result<usize, IndexError> {
    graph.rows.iter().try_fold(HEADER_BYTES, |sum, row| {
        sum.checked_add(32 + 32 + 2 + row.vector_id.len() + row.vector.len() * 4 + 1)
            .and_then(|bytes| {
                bytes.checked_add(
                    row.links
                        .iter()
                        .map(|links| 2 + links.len() * 4)
                        .sum::<usize>(),
                )
            })
            .filter(|bytes| *bytes <= MAX_ARTIFACT_BYTES)
            .ok_or(IndexError::ResourceLimited)
    })
}

fn encode(
    graph: &Graph,
    budget: &mut WorkBudget,
) -> Result<(Vec<u8>, Option<MemoryLease>), IndexError> {
    let length = encoded_len(graph)?;
    let memory = budget.reserve_memory(length as u64, MemoryLifetime::Temporary)?;
    let mut bytes = reserve_vec(length)?;
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&graph.seal.recipe.0);
    bytes.extend_from_slice(&graph.seal.generation.to_le_bytes());
    bytes.extend_from_slice(&graph.seal.dimension.to_le_bytes());
    bytes.extend_from_slice(&graph.seal.rows.to_le_bytes());
    bytes.extend_from_slice(&SEED.to_le_bytes());
    bytes.extend_from_slice(&(M as u32).to_le_bytes());
    bytes.extend_from_slice(&(EF_CONSTRUCTION as u32).to_le_bytes());
    bytes.extend_from_slice(&(MAX_CONSTRUCTION_VISITS as u32).to_le_bytes());
    for row in &graph.rows {
        budget.checkpoint()?;
        bytes.extend_from_slice(&row.owner.producer.0);
        bytes.extend_from_slice(&row.owner.stable_id.0);
        bytes.extend_from_slice(&row.member.0);
        bytes.extend_from_slice(&(row.vector_id.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&row.vector_id);
        for value in &row.vector {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.push((row.links.len() - 1) as u8);
        for links in &row.links {
            bytes.extend_from_slice(&(links.len() as u16).to_le_bytes());
            for index in links {
                bytes.extend_from_slice(&index.to_le_bytes());
            }
        }
    }
    budget.charge(WorkUsage {
        bytes: length as u64,
        ..WorkUsage::default()
    })?;
    Ok((bytes, memory))
}

pub(super) fn install(
    rows: &[RegionRow<'_>],
    recipe: Digest,
    generation: u64,
    dimension: usize,
    context: &mut AttemptContext<'_>,
) -> Result<ObjectRef, IndexError> {
    let graph = build(rows, recipe, generation, dimension, context.budget)?;
    let (bytes, _memory) = encode(&graph, context.budget)?;
    // Rows carry content commitments; source/member ObjectRefs remain in the
    // descriptor's member-map child, preserving the original Catalog graph.
    context.put(&bytes, &[])
}

struct Decoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Decoder<'a> {
    fn bytes(&mut self, count: usize) -> Result<&'a [u8], IndexError> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| corrupt("native graph truncated"))?;
        self.offset = end;
        Ok(bytes)
    }
    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], IndexError> {
        self.bytes(N)?
            .try_into()
            .map_err(|_| corrupt("native graph scalar width differs"))
    }
    fn u32(&mut self) -> Result<u32, IndexError> {
        Ok(u32::from_le_bytes(self.fixed()?))
    }
}

fn read_header(input: &mut Decoder<'_>) -> Result<Seal, IndexError> {
    if input.bytes(8)? != MAGIC {
        return Err(IndexError::RebuildRequired);
    }
    let recipe = Digest(input.fixed()?);
    let generation = u64::from_le_bytes(input.fixed()?);
    let dimension = input.u32()?;
    let count = input.u32()?;
    if u64::from_le_bytes(input.fixed()?) != SEED
        || input.u32()? != M as u32
        || input.u32()? != EF_CONSTRUCTION as u32
        || input.u32()? != MAX_CONSTRUCTION_VISITS as u32
    {
        return Err(IndexError::RebuildRequired);
    }
    let rows = count as usize;
    let width = dimension as usize;
    if generation == 0
        || rows == 0
        || rows > MAX_REGION_ROWS
        || width == 0
        || width > MAX_REGION_DIMENSION
        || rows
            .checked_mul(width)
            .is_none_or(|cells| cells > MAX_REGION_CELLS)
    {
        return Err(corrupt("native graph seal envelope invalid"));
    }
    let minimum = HEADER_BYTES
        .checked_add(
            rows.checked_mul(70 + width * 4)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .ok_or(IndexError::ResourceLimited)?;
    if input.bytes.len() < minimum {
        return Err(corrupt("native graph seal rows exceed payload"));
    }
    Ok(Seal {
        recipe,
        generation,
        dimension,
        rows: count,
    })
}

fn decode(bytes: &[u8], budget: &mut WorkBudget) -> Result<Graph, IndexError> {
    budget.checkpoint()?;
    if bytes.len() > MAX_ARTIFACT_BYTES {
        return Err(corrupt("native graph exceeds artifact bound"));
    }
    // Admit the original decode work before interpreting untrusted rows or
    // reserving their backing. A late charge can refuse only after that work.
    budget.charge(WorkUsage {
        bytes: u64::try_from(bytes.len()).map_err(limited)?,
        ..WorkUsage::default()
    })?;
    let mut input = Decoder { bytes, offset: 0 };
    let seal = read_header(&mut input)?;
    let Seal {
        recipe,
        generation,
        dimension,
        rows: count,
    } = seal;
    let rows = count as usize;
    let width = dimension as usize;
    // Encoded bytes bound total IDs; the row count/dimension bound decoded
    // structures. No untrusted adjacency count determines an allocation.
    let memory = budget.reserve_memory(
        memory_bound(rows, width, bytes.len())?,
        MemoryLifetime::Retained,
    )?;
    let mut graph = Graph {
        seal: Seal {
            recipe,
            generation,
            dimension,
            rows: count,
        },
        rows: reserve_vec(rows)?,
        entry: 0,
        _memory: memory,
    };
    for index in 0..rows {
        budget.checkpoint()?;
        let owner = OwnerKey {
            producer: ProducerId(input.fixed()?),
            stable_id: StableOwnerId(input.fixed()?),
        };
        let member = Digest(input.fixed()?);
        let length = u16::from_le_bytes(input.fixed()?) as usize;
        if length == 0 || length > MAX_ID_BYTES {
            return Err(corrupt("native graph ID length invalid"));
        }
        let mut vector_id = reserve_vec(length)?;
        vector_id.extend_from_slice(input.bytes(length)?);
        let mut vector = reserve_vec(width)?;
        for _column in 0..width {
            vector.push(f32::from_le_bytes(input.fixed()?));
        }
        let norm =
            vector_norm(&vector, dimension).map_err(|_| corrupt("native graph vector invalid"))?;
        let height = input.fixed::<1>()?[0] as usize;
        if height > MAX_LEVEL
            || height
                != level(
                    recipe,
                    RegionRow {
                        owner,
                        member,
                        vector_id: &vector_id,
                        vector: &vector,
                    },
                )
        {
            return Err(corrupt("native graph level stamp invalid"));
        }
        let mut links = reserve_vec(height + 1)?;
        for _layer in 0..=height {
            let count = u16::from_le_bytes(input.fixed()?) as usize;
            if count > M {
                return Err(corrupt("native graph degree exceeds bound"));
            }
            let mut adjacent = reserve_vec(count)?;
            for _edge in 0..count {
                let neighbor = input.u32()?;
                if neighbor as usize >= rows
                    || neighbor as usize == index
                    || adjacent.last().is_some_and(|last| *last >= neighbor)
                {
                    return Err(corrupt("native graph edge invalid"));
                }
                adjacent.push(neighbor);
            }
            links.push(adjacent);
        }
        if let Some(prior) = graph.rows.last() {
            if (prior.owner, prior.member, prior.vector_id.as_slice())
                >= (owner, member, vector_id.as_slice())
            {
                return Err(corrupt("native graph row order invalid"));
            }
            if prior.owner == owner && prior.member != member {
                return Err(corrupt(
                    "native graph owner has conflicting member commitments",
                ));
            }
        }
        if !graph.rows.is_empty() && height + 1 > graph.rows[graph.entry].links.len() {
            graph.entry = index;
        }
        graph.rows.push(Row {
            owner,
            member,
            vector_id,
            vector,
            norm,
            links,
        });
    }
    if input.offset != bytes.len() {
        return Err(corrupt("native graph trailing bytes"));
    }
    for (index, row) in graph.rows.iter().enumerate() {
        budget.checkpoint()?;
        if rows > 1 {
            let (previous, next) = base_cycle_neighbors(index, rows);
            let base = &row.links[0];
            if base.binary_search(&(previous as u32)).is_err()
                || base.binary_search(&(next as u32)).is_err()
            {
                return Err(corrupt("native graph base cycle edge missing"));
            }
        }
        for (layer, links) in row.links.iter().enumerate() {
            if links
                .iter()
                .any(|index| graph.rows[*index as usize].links.len() <= layer)
            {
                return Err(corrupt("native graph edge layer invalid"));
            }
        }
    }
    Ok(graph)
}

pub(super) fn load(
    objects: &dyn ImmutableObjectPort,
    reference: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<Graph, IndexError> {
    let stored = objects.get(reference, budget)?;
    if stored.registration.reference != reference
        || !stored.registration.children.as_slice().is_empty()
    {
        return Err(corrupt("native graph object identity or children differ"));
    }
    decode(&stored.payload, budget)
}

#[cfg(test)]
#[path = "native_graph_tests.rs"]
mod tests;

/// Maintenance authenticates the registered object and fixed header without
/// allocating vector/adjacency tables merely to decide which old root to retire.
pub(super) fn load_seal(
    objects: &dyn ImmutableObjectPort,
    reference: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<Seal, IndexError> {
    let stored = objects.get(reference, budget)?;
    if stored.registration.reference != reference
        || !stored.registration.children.as_slice().is_empty()
        || stored.payload.len() > MAX_ARTIFACT_BYTES
    {
        return Err(corrupt("native graph seal object identity or size differs"));
    }
    let mut input = Decoder {
        bytes: &stored.payload,
        offset: 0,
    };
    let seal = read_header(&mut input)?;
    budget.charge(WorkUsage {
        bytes: HEADER_BYTES as u64,
        ..WorkUsage::default()
    })?;
    Ok(seal)
}
