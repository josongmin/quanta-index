//! Hand-written Hierarchical Navigable Small World (HNSW) index.
//!
//! Cosine-similarity ANN search. Vectors are L2-normalized at insertion so
//! cosine reduces to a dot product. Adjacency is stored as `BTreeMap`
//! keyed by node id (`usize`); the workspace bans `HashMap`/`HashSet`.
//!
//! Level assignment is deterministic per embedding id (a small LCG seeded
//! from an FNV-1a hash of the id string) so that tests are reproducible
//! without depending on `std::collections::DefaultHasher` (also banned).
//! `log_M(n)` for any realistic `n` stays well below 16.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) items are the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_core::CoreError;

/// Standard HNSW connectivity at the upper layers.
const M: usize = 16;
/// Connectivity at layer 0 (typically 2 * M).
const M_MAX0: usize = 32;
/// Candidate-list size during insertion.
const EF_CONSTRUCTION: usize = 200;
/// Candidate-list size during query.
const EF_SEARCH: usize = 64;
/// Hard cap on assigned levels. `log_M(n)` for any realistic `n` stays well
/// below 16.
const MAX_LEVEL_CAP: usize = 16;

#[derive(Clone, Debug)]
struct Node {
    id: String,
    /// L2-normalized vector. `deleted == true` means tombstoned; the vector
    /// is preserved for adjacency cleanup.
    vector: Vec<f32>,
    /// `adjacency[layer]` = neighbor node indexes at that layer.
    /// `adjacency.len() - 1` is the top layer this node participates in.
    adjacency: Vec<Vec<usize>>,
    deleted: bool,
}

/// Hand-written HNSW index with cosine-similarity search.
pub(crate) struct HnswIndex {
    dim: usize,
    nodes: Vec<Node>,
    /// id → index in `nodes`.
    id_to_idx: BTreeMap<String, usize>,
    /// Current top layer.
    max_level: usize,
    /// Entry-point node index, or `None` if the index is empty / fully deleted.
    entry: Option<usize>,
}

impl HnswIndex {
    #[must_use]
    pub(crate) fn new(dim: usize) -> Self {
        Self {
            dim,
            nodes: Vec::new(),
            id_to_idx: BTreeMap::new(),
            max_level: 0,
            entry: None,
        }
    }

    #[must_use]
    pub(crate) fn dim(&self) -> usize {
        self.dim
    }

    /// Insert (or replace) an embedding. `vector.len()` must equal `self.dim`.
    pub(crate) fn insert(&mut self, id: String, vector: &[f32]) -> Result<(), CoreError> {
        if vector.len() != self.dim {
            return Err(CoreError::InvalidContract(format!(
                "hnsw: vector dim {} != index dim {}",
                vector.len(),
                self.dim
            )));
        }
        let Some(normalized) = l2_normalize(vector) else {
            return Err(CoreError::InvalidContract(
                "hnsw: zero-norm vector rejected".to_string(),
            ));
        };

        // If the id already exists, tombstone the old node before inserting
        // a fresh one. Cheaper than in-place graph rewire.
        if let Some(existing_idx) = self.id_to_idx.get(&id).copied() {
            self.tombstone_node(existing_idx);
            let _removed = self.id_to_idx.remove(&id);
        }

        let level = assign_level(&id);
        let new_idx = self.nodes.len();
        let new_node = Node {
            id: id.clone(),
            vector: normalized.clone(),
            adjacency: vec![Vec::new(); level.saturating_add(1)],
            deleted: false,
        };
        self.nodes.push(new_node);
        let _prior = self.id_to_idx.insert(id, new_idx);

        // First live node: becomes the entry point.
        let Some(entry) = self.entry_alive() else {
            self.entry = Some(new_idx);
            self.max_level = level;
            return Ok(());
        };

        // Descend from the current top down to `level + 1` with greedy search.
        let mut current = entry;
        let mut layer = self.max_level;
        while layer > level {
            current = self.greedy_descent(current, &normalized, layer);
            if layer == 0 {
                break;
            }
            layer = layer.saturating_sub(1);
        }

        // From `level` down to 0: do ef_construction search, then connect.
        let mut ep: Vec<(f32, usize)> = vec![(self.score(current, &normalized), current)];
        let mut layer_down = level;
        loop {
            let candidates = self.search_layer(&normalized, &ep, EF_CONSTRUCTION, layer_down);
            let m_layer = if layer_down == 0 { M_MAX0 } else { M };
            let selected = select_neighbors(&candidates, m_layer);
            self.connect(new_idx, &selected, layer_down);

            ep = candidates;
            if layer_down == 0 {
                break;
            }
            layer_down = layer_down.saturating_sub(1);
        }

        if level > self.max_level {
            self.max_level = level;
            self.entry = Some(new_idx);
        }
        Ok(())
    }

    pub(crate) fn delete(&mut self, id: &str) {
        if let Some(idx) = self.id_to_idx.get(id).copied() {
            self.tombstone_node(idx);
            let _removed = self.id_to_idx.remove(id);
        }
    }

    /// Returns `(id, cosine_similarity)` tuples sorted descending by score.
    ///
    /// Rejects zero-norm and dim-mismatched queries with
    /// [`CoreError::InvalidContract`] rather than returning an empty hit list,
    /// so caller-side contract bugs surface loudly instead of being absorbed
    /// as "no results".
    pub(crate) fn search(
        &self,
        query: &[f32],
        top_k: usize,
    ) -> Result<Vec<(String, f32)>, CoreError> {
        if top_k == 0 {
            return Ok(Vec::new());
        }
        if query.len() != self.dim {
            return Err(CoreError::InvalidContract(format!(
                "hnsw: query dim {} != index dim {}",
                query.len(),
                self.dim
            )));
        }
        let normalized = l2_normalize(query).ok_or_else(|| {
            CoreError::InvalidContract("hnsw: zero-norm query rejected".to_string())
        })?;
        let Some(entry) = self.entry_alive() else {
            return Ok(Vec::new());
        };

        // Greedy descent through upper layers.
        let mut current = entry;
        let mut layer = self.max_level;
        while layer > 0 {
            current = self.greedy_descent(current, &normalized, layer);
            layer = layer.saturating_sub(1);
        }

        let ep = vec![(self.score(current, &normalized), current)];
        let ef = top_k.max(EF_SEARCH);
        let mut candidates = self.search_layer(&normalized, &ep, ef, 0);
        candidates.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let mut out: Vec<(String, f32)> = Vec::with_capacity(top_k.min(candidates.len()));
        for (score, idx) in candidates {
            if out.len() >= top_k {
                break;
            }
            if let Some(node) = self.nodes.get(idx)
                && !node.deleted
            {
                out.push((node.id.clone(), score));
            }
        }
        Ok(out)
    }

    /// Exact allowlist search over normalized vectors. This closes the
    /// lexical-scope starvation hole that appears when a global ANN top-k is
    /// post-filtered after the fact.
    pub(crate) fn search_scoped(
        &self,
        query: &[f32],
        allowed_ids: &BTreeSet<String>,
        top_k: usize,
    ) -> Result<Vec<(String, f32)>, CoreError> {
        if top_k == 0 || allowed_ids.is_empty() {
            return Ok(Vec::new());
        }
        if query.len() != self.dim {
            return Err(CoreError::InvalidContract(format!(
                "hnsw: scoped query dim {} != index dim {}",
                query.len(),
                self.dim
            )));
        }
        let normalized = l2_normalize(query).ok_or_else(|| {
            CoreError::InvalidContract("hnsw: zero-norm scoped query rejected".to_string())
        })?;
        let mut out: Vec<(String, f32)> = Vec::new();
        for id in allowed_ids {
            let Some(idx) = self.id_to_idx.get(id).copied() else {
                continue;
            };
            let Some(node) = self.nodes.get(idx) else {
                continue;
            };
            if node.deleted {
                continue;
            }
            out.push((node.id.clone(), dot(&node.vector, &normalized)));
        }
        out.sort_by(|lhs, rhs| {
            rhs.1
                .partial_cmp(&lhs.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| lhs.0.as_str().cmp(rhs.0.as_str()))
        });
        out.truncate(top_k);
        Ok(out)
    }

    fn entry_alive(&self) -> Option<usize> {
        if let Some(idx) = self.entry
            && self.nodes.get(idx).is_some_and(|n| !n.deleted)
        {
            return Some(idx);
        }
        // Fallback: any live node.
        for (i, n) in self.nodes.iter().enumerate() {
            if !n.deleted {
                return Some(i);
            }
        }
        None
    }

    fn tombstone_node(&mut self, idx: usize) {
        if let Some(node) = self.nodes.get_mut(idx) {
            node.deleted = true;
            node.adjacency.clear();
        }
    }

    /// Greedy descent: from `entry` at `layer`, hop to neighbor with strictly
    /// higher score until local maximum.
    fn greedy_descent(&self, entry: usize, query: &[f32], layer: usize) -> usize {
        let mut current = entry;
        let mut current_score = self.score(current, query);
        loop {
            let Some(neighbors) = self.neighbors(current, layer) else {
                return current;
            };
            let mut best = current;
            let mut best_score = current_score;
            for &nb in neighbors {
                if self.nodes.get(nb).is_some_and(|n| n.deleted) {
                    continue;
                }
                let s = self.score(nb, query);
                if s > best_score {
                    best = nb;
                    best_score = s;
                }
            }
            if best == current {
                return current;
            }
            current = best;
            current_score = best_score;
        }
    }

    /// HNSW layer search: returns up to `ef` candidates by cosine similarity.
    /// Used both at construction time (M selection) and query time.
    fn search_layer(
        &self,
        query: &[f32],
        entry_points: &[(f32, usize)],
        ef: usize,
        layer: usize,
    ) -> Vec<(f32, usize)> {
        let mut visited: BTreeSet<usize> = BTreeSet::new();
        // candidates: ordered by descending score; we pop the most-similar
        // first to expand greedily.
        let mut candidates: Vec<(f32, usize)> = Vec::new();
        // dynamic: best `ef` nodes seen so far, ordered by descending score.
        let mut dynamic: Vec<(f32, usize)> = Vec::new();

        for &(score, idx) in entry_points {
            if visited.insert(idx) {
                candidates.push((score, idx));
                dynamic.push((score, idx));
            }
        }
        candidates.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        dynamic.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        while let Some((c_score, c_idx)) = candidates.first().copied() {
            let _removed = candidates.remove(0);
            let worst_in_dynamic = dynamic.last().map_or(f32::NEG_INFINITY, |&(s, _)| s);
            if dynamic.len() >= ef && c_score < worst_in_dynamic {
                break;
            }
            let neighbors_opt = self.neighbors(c_idx, layer).cloned();
            let Some(neighbors) = neighbors_opt else {
                continue;
            };
            for nb in neighbors {
                if !visited.insert(nb) {
                    continue;
                }
                if self.nodes.get(nb).is_some_and(|n| n.deleted) {
                    continue;
                }
                let s = self.score(nb, query);
                let worst = dynamic.last().map_or(f32::NEG_INFINITY, |&(sc, _)| sc);
                if dynamic.len() < ef || s > worst {
                    insert_sorted_desc(&mut candidates, (s, nb));
                    insert_sorted_desc(&mut dynamic, (s, nb));
                    if dynamic.len() > ef {
                        let _popped = dynamic.pop();
                    }
                }
            }
        }
        dynamic
    }

    fn connect(&mut self, new_idx: usize, selected: &[(f32, usize)], layer: usize) {
        // Add forward edges new_idx -> selected.
        if let Some(node) = self.nodes.get_mut(new_idx)
            && let Some(adj) = node.adjacency.get_mut(layer)
        {
            for &(_score, nb) in selected {
                if !adj.contains(&nb) {
                    adj.push(nb);
                }
            }
        }
        // Add reverse edges and prune if over capacity.
        let m_layer = if layer == 0 { M_MAX0 } else { M };
        for &(_score, nb) in selected {
            let needs_prune = self.add_back_edge(nb, new_idx, layer, m_layer);
            if needs_prune {
                self.prune_neighbors(nb, layer, m_layer);
            }
        }
    }

    /// Returns `true` if the back-edge insert caused the layer's adjacency to
    /// exceed `m_layer` (i.e. caller must prune).
    fn add_back_edge(&mut self, nb: usize, new_idx: usize, layer: usize, m_layer: usize) -> bool {
        let Some(node) = self.nodes.get_mut(nb) else {
            return false;
        };
        let Some(adj) = node.adjacency.get_mut(layer) else {
            return false;
        };
        if !adj.contains(&new_idx) {
            adj.push(new_idx);
        }
        adj.len() > m_layer
    }

    fn prune_neighbors(&mut self, node_idx: usize, layer: usize, m_layer: usize) {
        let Some(node) = self.nodes.get(node_idx) else {
            return;
        };
        let Some(adj) = node.adjacency.get(layer) else {
            return;
        };
        let own_vec = node.vector.clone();
        let neighbors = adj.clone();
        let mut scored: Vec<(f32, usize)> = Vec::with_capacity(neighbors.len());
        for nb in neighbors {
            if let Some(other) = self.nodes.get(nb)
                && !other.deleted
            {
                let s = dot(&own_vec, &other.vector);
                scored.push((s, nb));
            }
        }
        let kept = select_neighbors(&scored, m_layer);
        if let Some(node) = self.nodes.get_mut(node_idx)
            && let Some(adj) = node.adjacency.get_mut(layer)
        {
            adj.clear();
            for (_score, idx) in kept {
                adj.push(idx);
            }
        }
    }

    fn neighbors(&self, idx: usize, layer: usize) -> Option<&Vec<usize>> {
        self.nodes.get(idx).and_then(|n| n.adjacency.get(layer))
    }

    fn score(&self, idx: usize, query: &[f32]) -> f32 {
        match self.nodes.get(idx) {
            Some(node) if !node.deleted => dot(&node.vector, query),
            _ => f32::NEG_INFINITY,
        }
    }
}

/// Insert into a list kept in descending-score order.
fn insert_sorted_desc(list: &mut Vec<(f32, usize)>, item: (f32, usize)) {
    let mut pos = list.len();
    for (i, existing) in list.iter().enumerate() {
        if item.0 > existing.0 {
            pos = i;
            break;
        }
    }
    list.insert(pos, item);
}

/// Simple "select M nearest" heuristic. The candidates are already scored;
/// take the top-`m` by descending similarity.
fn select_neighbors(candidates: &[(f32, usize)], m_layer: usize) -> Vec<(f32, usize)> {
    let mut sorted = candidates.to_vec();
    sorted.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    sorted.truncate(m_layer);
    sorted
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = 0.0_f32;
    for (lhs, rhs) in a.iter().zip(b.iter()) {
        acc += lhs * rhs;
    }
    acc
}

fn l2_normalize(v: &[f32]) -> Option<Vec<f32>> {
    let mut sum = 0.0_f32;
    for x in v {
        sum += x * x;
    }
    if sum <= 0.0 {
        return None;
    }
    let norm = sum.sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return None;
    }
    let mut out: Vec<f32> = Vec::with_capacity(v.len());
    for x in v {
        out.push(x / norm);
    }
    Some(out)
}

/// FNV-1a 64-bit hash of an id string. Stable across runs and platforms.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Single LCG hop (Knuth/Numerical-Recipes constants). Used to draw a uniform
/// sample from an id-derived seed; not cryptographic.
fn lcg_next(state: u64) -> u64 {
    state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407)
}

/// Convert a u64 into a uniform `[0, 1)` f64 by taking the high 53 bits.
fn u64_to_unit_f64(raw: u64) -> f64 {
    let mantissa: u64 = raw >> 11;
    // Decompose into two u32 halves to avoid `as` casts when mapping to f64.
    let lower = mantissa & 0xFFFF_FFFF;
    let upper = mantissa >> 32;
    // `lower` and `upper` are both guaranteed to fit in u32; match for safety
    // without using `unwrap_or_default` (banned on Result).
    let Ok(low_u32) = u32::try_from(lower) else {
        return 0.0_f64;
    };
    let Ok(high_u32) = u32::try_from(upper) else {
        return 0.0_f64;
    };
    let low = f64::from(low_u32);
    let high = f64::from(high_u32);
    // 2^53 as f64.
    let denom: f64 = 9_007_199_254_740_992.0_f64;
    // 2^32 as f64.
    let two_pow_32: f64 = 4_294_967_296.0_f64;
    high.mul_add(two_pow_32, low) / denom
}

/// Convert a non-negative, bounded f64 (already clamped to `[0, MAX_LEVEL_CAP]`)
/// to `usize` via a cascade of `f64::from(u32)` comparisons. Avoids `as` casts.
fn f64_clamped_to_usize(v: f64) -> usize {
    let Ok(cap_u32) = u32::try_from(MAX_LEVEL_CAP) else {
        return 0;
    };
    let mut found: u32 = 0;
    for k in 0..=cap_u32 {
        if v >= f64::from(k) {
            found = k;
        } else {
            break;
        }
    }
    usize::try_from(found).map_or(0, |v| v)
}

/// Deterministic level assignment using `mL = 1 / ln(M)`.
fn assign_level(id: &str) -> usize {
    let seed = fnv1a_64(id.as_bytes());
    let raw = lcg_next(seed);
    let u_raw = u64_to_unit_f64(raw);
    // Guard u against 0 (would give +inf when log'd).
    let u_safe = if u_raw <= 0.0 {
        // Smallest representable u from our mapping is ~2^-53; use that.
        1.0_f64 / 9_007_199_254_740_992.0_f64
    } else {
        u_raw
    };
    let Ok(m_u32) = u32::try_from(M) else {
        return 0;
    };
    let m_f = f64::from(m_u32);
    let denom = m_f.ln();
    if denom == 0.0 || !denom.is_finite() {
        return 0;
    }
    let m_l = 1.0_f64 / denom;
    let level_f = -u_safe.ln() * m_l;
    if !level_f.is_finite() || level_f <= 0.0 {
        return 0;
    }
    let Ok(cap_u32) = u32::try_from(MAX_LEVEL_CAP) else {
        return 0;
    };
    let cap_f = f64::from(cap_u32);
    let bounded = if level_f > cap_f { cap_f } else { level_f };
    let truncated = bounded.trunc();
    if truncated <= 0.0 {
        return 0;
    }
    f64_clamped_to_usize(truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestRes = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn insert_search_returns_self_first() -> TestRes {
        let mut index = HnswIndex::new(4);
        let vecs: [(&str, [f32; 4]); 5] = [
            ("a", [1.0, 0.0, 0.0, 0.0]),
            ("b", [0.0, 1.0, 0.0, 0.0]),
            ("c", [0.0, 0.0, 1.0, 0.0]),
            ("d", [0.0, 0.0, 0.0, 1.0]),
            ("e", [0.5, 0.5, 0.5, 0.5]),
        ];
        for (id, v) in &vecs {
            index.insert((*id).to_string(), v.as_slice())?;
        }
        let Some(first) = vecs.first() else {
            return Err("missing first vec".into());
        };
        let results = index.search(&first.1, 1)?;
        let Some(top) = results.first() else {
            return Err("empty results".into());
        };
        if top.0 != first.0 {
            return Err(format!("expected id {}, got {}", first.0, top.0).into());
        }
        if top.1 < 0.999 {
            return Err(format!("expected score >= 0.999, got {}", top.1).into());
        }
        Ok(())
    }

    #[test]
    fn delete_then_search_excludes() -> TestRes {
        let mut index = HnswIndex::new(3);
        index.insert("x".to_string(), &[1.0, 0.0, 0.0])?;
        index.insert("y".to_string(), &[0.0, 1.0, 0.0])?;
        index.insert("z".to_string(), &[0.0, 0.0, 1.0])?;
        index.delete("y");
        let results = index.search(&[0.0, 1.0, 0.0], 5)?;
        let ids: Vec<String> = results.into_iter().map(|(id, _)| id).collect();
        if ids.iter().any(|i| i == "y") {
            return Err(format!("deleted id 'y' still present: {ids:?}").into());
        }
        Ok(())
    }

    #[test]
    fn dim_mismatch_rejected() -> TestRes {
        let mut index = HnswIndex::new(4);
        index.insert("a".to_string(), &[1.0, 0.0, 0.0, 0.0])?;
        let err = index.insert("b".to_string(), &[1.0, 0.0, 0.0]);
        if !matches!(err, Err(CoreError::InvalidContract(_))) {
            return Err(format!("expected InvalidContract error, got {err:?}").into());
        }
        Ok(())
    }

    #[test]
    fn zero_norm_insert_rejected_without_mutating_index() -> TestRes {
        let mut index = HnswIndex::new(3);
        index.insert("anchor".to_string(), &[1.0, 0.0, 0.0])?;

        let err = index.insert("zero".to_string(), &[0.0, 0.0, 0.0]);
        assert!(matches!(
            err,
            Err(CoreError::InvalidContract(message)) if message == "hnsw: zero-norm vector rejected"
        ));
        assert!(!index.id_to_idx.contains_key("zero"));

        let results = index.search(&[1.0, 0.0, 0.0], 8)?;
        let ids: Vec<String> = results.into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, vec!["anchor".to_string()]);
        Ok(())
    }

    #[test]
    fn zero_norm_replace_rejected_without_tombstoning_existing_id() -> TestRes {
        let mut index = HnswIndex::new(3);
        index.insert("anchor".to_string(), &[1.0, 0.0, 0.0])?;

        let err = index.insert("anchor".to_string(), &[0.0, 0.0, 0.0]);
        assert!(matches!(
            err,
            Err(CoreError::InvalidContract(message)) if message == "hnsw: zero-norm vector rejected"
        ));
        assert!(index.id_to_idx.contains_key("anchor"));

        let results = index.search(&[1.0, 0.0, 0.0], 1)?;
        let Some((top_id, score)) = results.first() else {
            return Err("anchor disappeared after rejected replacement".into());
        };
        assert_eq!(top_id, "anchor");
        assert!(*score > 0.99);
        Ok(())
    }

    #[test]
    fn cosine_ordering_correct() -> TestRes {
        let mut index = HnswIndex::new(2);
        index.insert("east".to_string(), &[1.0, 0.0])?;
        index.insert("diag".to_string(), &[0.7, 0.7])?;
        index.insert("north".to_string(), &[0.0, 1.0])?;
        let results = index.search(&[1.0, 0.0], 3)?;
        if results.len() != 3 {
            return Err(format!("expected 3 results, got {}", results.len()).into());
        }
        let order: Vec<String> = results.iter().map(|(id, _)| id.clone()).collect();
        let want = ["east", "diag", "north"];
        for (got, expected) in order.iter().zip(want.iter()) {
            if got != expected {
                return Err(format!("ordering mismatch: got {order:?}").into());
            }
        }
        Ok(())
    }

    #[test]
    fn scoped_search_respects_allowlist_even_when_global_top_hit_is_outside() -> TestRes {
        let mut index = HnswIndex::new(2);
        index.insert("alpha".to_string(), &[1.0, 0.0])?;
        index.insert("beta".to_string(), &[0.8, 0.2])?;
        index.insert("gamma".to_string(), &[0.0, 1.0])?;

        let mut allow: BTreeSet<String> = BTreeSet::new();
        if !allow.insert("beta".to_string()) {
            return Err("duplicate allowlist id inserted: beta".into());
        }
        if !allow.insert("gamma".to_string()) {
            return Err("duplicate allowlist id inserted: gamma".into());
        }

        let results = index.search_scoped(&[1.0, 0.0], &allow, 2)?;
        let ids: Vec<String> = results.into_iter().map(|(id, _)| id).collect();
        if ids != vec!["beta".to_string(), "gamma".to_string()] {
            return Err(format!("scoped search ordering mismatch: {ids:?}").into());
        }
        Ok(())
    }

    #[test]
    fn zero_norm_search_query_rejected() -> TestRes {
        let mut index = HnswIndex::new(3);
        index.insert("anchor".to_string(), &[1.0, 0.0, 0.0])?;

        let err = index.search(&[0.0, 0.0, 0.0], 4);
        assert!(matches!(
            err,
            Err(CoreError::InvalidContract(message)) if message == "hnsw: zero-norm query rejected"
        ));
        Ok(())
    }

    #[test]
    fn zero_norm_scoped_search_query_rejected() -> TestRes {
        let mut index = HnswIndex::new(3);
        index.insert("anchor".to_string(), &[1.0, 0.0, 0.0])?;
        let mut allow: BTreeSet<String> = BTreeSet::new();
        if !allow.insert("anchor".to_string()) {
            return Err("duplicate allowlist id".into());
        }

        let err = index.search_scoped(&[0.0, 0.0, 0.0], &allow, 4);
        assert!(matches!(
            err,
            Err(CoreError::InvalidContract(message))
                if message == "hnsw: zero-norm scoped query rejected"
        ));
        Ok(())
    }

    #[test]
    fn dim_mismatch_search_query_rejected() -> TestRes {
        let mut index = HnswIndex::new(3);
        index.insert("anchor".to_string(), &[1.0, 0.0, 0.0])?;

        let err = index.search(&[1.0, 0.0], 4);
        assert!(matches!(err, Err(CoreError::InvalidContract(_))));
        Ok(())
    }

    #[test]
    fn search_top_k_zero_returns_empty_ok() -> TestRes {
        let mut index = HnswIndex::new(3);
        index.insert("anchor".to_string(), &[1.0, 0.0, 0.0])?;

        let results = index.search(&[1.0, 0.0, 0.0], 0)?;
        assert!(results.is_empty());
        Ok(())
    }
}
