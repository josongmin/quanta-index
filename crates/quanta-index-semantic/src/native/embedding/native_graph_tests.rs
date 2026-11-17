use super::*;
use quanta_index_core::index::WorkLimits;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

fn budget() -> Result<WorkBudget, Box<dyn std::error::Error>> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 536_870_912,
            mutations: 10_000_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or("deadline")?,
    ))
}
fn owner(index: u16) -> OwnerKey {
    let mut stable = [0; 16];
    stable[..2].copy_from_slice(&index.to_be_bytes());
    OwnerKey {
        producer: ProducerId([1; 16]),
        stable_id: StableOwnerId(stable),
    }
}
fn rows(vectors: &[Vec<f32>]) -> Vec<RegionRow<'_>> {
    vectors
        .iter()
        .enumerate()
        .map(|(i, vector)| RegionRow {
            owner: owner(i as u16),
            member: Digest([2; 32]),
            vector_id: b"v",
            vector,
        })
        .collect()
}

fn bounded_partial_recall_count(
    candidate_indices: &[usize],
    oracle: &[(usize, f64)],
) -> Result<usize, &'static str> {
    let unique_candidates: BTreeSet<_> = candidate_indices.iter().copied().collect();
    if unique_candidates.len() != candidate_indices.len() {
        return Err("duplicate partial ANN candidate");
    }
    let candidate_top_ten: BTreeSet<_> = candidate_indices.iter().take(10).copied().collect();
    let gold_top_ten: BTreeSet<_> = oracle
        .iter()
        .take(10)
        .map(|(index, _score)| *index)
        .collect();
    Ok(candidate_top_ten.intersection(&gold_top_ten).count())
}

#[test]
fn native_graph_partial_recall_rejects_duplicate_candidate_page() {
    // Fixed gold IDs are independent of graph traversal and ranking helpers.
    let oracle: Vec<_> = (0..20).map(|index| (index, 0.0)).collect();
    let unique: Vec<_> = (0..20).collect();
    let missed: Vec<_> = (10..20).collect();
    assert_eq!(bounded_partial_recall_count(&unique, &oracle), Ok(10));
    assert_eq!(bounded_partial_recall_count(&missed, &oracle), Ok(0));
    // A repeated gold ID can otherwise inflate ten returned slots to 10/10.
    assert_eq!(
        bounded_partial_recall_count(&[0; 10], &oracle),
        Err("duplicate partial ANN candidate")
    );
    // Duplicates beyond top ten still make the candidate page invalid.
    let mut duplicate_tail = unique;
    duplicate_tail.push(19);
    assert_eq!(
        bounded_partial_recall_count(&duplicate_tail, &oracle),
        Err("duplicate partial ANN candidate")
    );
}

#[test]
fn native_graph_reopen_is_deterministic_and_matches_independent_exact()
-> Result<(), Box<dyn std::error::Error>> {
    let vectors: Vec<_> = (0..128)
        .map(|i| {
            let x = i as f32 - 64.0;
            vec![1.0, x / 64.0]
        })
        .collect();
    let input = rows(&vectors);
    let mut work = budget()?;
    let graph = build(&input, Digest([3; 32]), 7, 2, &mut work)?;
    let (bytes, _memory) = encode(&graph, &mut work)?;
    let again = build(&input, Digest([3; 32]), 7, 2, &mut work)?;
    let (same, _same_memory) = encode(&again, &mut work)?;
    if bytes != same {
        return Err("same seed/recipe/sorted input changed seal".into());
    }
    let parallel = std::thread::scope(|scope| {
        let worker = || -> Result<Vec<u8>, String> {
            let mut work = budget().map_err(|error| error.to_string())?;
            let graph = build(&input, Digest([3; 32]), 7, 2, &mut work)
                .map_err(|error| error.to_string())?;
            let (bytes, _memory) = encode(&graph, &mut work).map_err(|error| error.to_string())?;
            Ok(bytes)
        };
        let first = scope.spawn(worker);
        let second = scope.spawn(worker);
        Ok::<_, String>([
            first
                .join()
                .map_err(|_| "first graph worker panicked".to_owned())??,
            second
                .join()
                .map_err(|_| "second graph worker panicked".to_owned())??,
        ])
    })?;
    for rebuilt in &parallel {
        if rebuilt != &bytes {
            return Err("serial/parallel graph seals differ".into());
        }
    }
    let reopened = decode(&bytes, &mut work)?;
    for query in [[1.0f32, 0.0], [1.0, -0.75], [1.0, 0.75]] {
        let candidates = reopened.candidates(&query, 1024, 128, &mut work)?;
        // Independent brute-force implementation uses f64 directly, with no
        // graph distance/rank helper in its expected-value calculation.
        let score = |vector: &[f32]| {
            let dot = f64::from(query[0]) * f64::from(vector[0])
                + f64::from(query[1]) * f64::from(vector[1]);
            dot / ((f64::from(query[0]).powi(2) + f64::from(query[1]).powi(2)).sqrt()
                * (f64::from(vector[0]).powi(2) + f64::from(vector[1]).powi(2)).sqrt())
        };
        let mut oracle: Vec<_> = vectors.iter().enumerate().collect();
        oracle.sort_by(|(ia, a), (ib, b)| score(b).total_cmp(&score(a)).then_with(|| ia.cmp(ib)));
        let expected: Vec<_> = oracle.iter().take(10).map(|(index, _)| *index).collect();
        if candidates.indices.get(..10) != Some(expected.as_slice()) {
            return Err(format!(
                "native recall/reopen differs: {:?} vs {expected:?}",
                candidates.indices
            )
            .into());
        }
    }
    Ok(())
}

#[test]
fn native_graph_rejects_noncanonical_rows_and_corrupt_seal()
-> Result<(), Box<dyn std::error::Error>> {
    let vectors = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
    let mut input = rows(&vectors);
    let mut work = budget()?;
    input.reverse();
    if !matches!(
        build(&input, Digest([3; 32]), 1, 2, &mut work),
        Err(IndexError::InvalidInput(_))
    ) {
        return Err("unsorted row admission succeeded".into());
    }
    input.reverse();
    let graph = build(&input, Digest([3; 32]), 1, 2, &mut work)?;
    let (bytes, _memory) = encode(&graph, &mut work)?;
    for length in [0, 8, HEADER_BYTES, bytes.len() - 1] {
        if decode(&bytes[..length], &mut work).is_ok() {
            return Err("truncated graph admitted".into());
        }
    }
    let mut foreign = bytes.clone();
    foreign[0] ^= 1;
    if !matches!(
        decode(&foreign, &mut work),
        Err(IndexError::RebuildRequired)
    ) {
        return Err("foreign format admitted".into());
    }
    let mut old_recipe = bytes.clone();
    old_recipe[..8].copy_from_slice(b"QNHNSW01");
    if !matches!(
        decode(&old_recipe, &mut work),
        Err(IndexError::RebuildRequired)
    ) {
        return Err("retired construction recipe admitted".into());
    }
    let mut wrong_bound = bytes.clone();
    wrong_bound[72..76].copy_from_slice(&255u32.to_le_bytes());
    if !matches!(
        decode(&wrong_bound, &mut work),
        Err(IndexError::RebuildRequired)
    ) {
        return Err("foreign construction bound admitted".into());
    }
    // Fixed v2 wire golden: header76 + owner32 + member32 + id_len2 + id1.
    // Nonfinite stored vectors must refuse even with an otherwise valid seal.
    let mut nonfinite = bytes.clone();
    nonfinite[143..147].copy_from_slice(&f32::NAN.to_le_bytes());
    if !matches!(
        decode(&nonfinite, &mut work),
        Err(IndexError::CorruptObject(_))
    ) {
        return Err("nonfinite graph vector admitted".into());
    }
    let mut invalid_edge = build(&input, Digest([3; 32]), 1, 2, &mut work)?;
    invalid_edge.rows[0].links[0][0] = 0;
    let (invalid_edge, _edge_memory) = encode(&invalid_edge, &mut work)?;
    if !matches!(
        decode(&invalid_edge, &mut work),
        Err(IndexError::CorruptObject(_))
    ) {
        return Err("self-referential graph edge admitted".into());
    }
    // Keep the valid envelope, rows and at-level edge shape. Removing both
    // directed base edges still makes one row unreachable from the entry.
    let mut disconnected = graph;
    for row in &mut disconnected.rows {
        row.links[0].clear();
    }
    let (disconnected, _edge_memory) = encode(&disconnected, &mut work)?;
    match decode(&disconnected, &mut work) {
        Err(IndexError::CorruptObject(_)) => {}
        Err(error) => return Err(error.into()),
        Ok(graph) => {
            let result = graph.candidates(&[1.0, 0.0], 2, 2, &mut work)?;
            return Err(format!(
                "disconnected persisted graph admitted: candidates={:?}, capped={}",
                result.indices, result.capped
            )
            .into());
        }
    }
    let mut trailing = bytes;
    trailing.push(0);
    if decode(&trailing, &mut work).is_ok() {
        return Err("trailing graph bytes admitted".into());
    }
    Ok(())
}

#[test]
fn native_graph_rejects_multiple_member_commitments_for_one_owner()
-> Result<(), Box<dyn std::error::Error>> {
    let owner = owner(1);
    let recipe = Digest([3; 32]);
    let mut work = budget()?;
    let mut input = [
        RegionRow {
            owner,
            member: Digest([1; 32]),
            vector_id: b"a",
            vector: &[1.0, 0.0],
        },
        RegionRow {
            owner,
            member: Digest([1; 32]),
            vector_id: b"b",
            vector: &[0.0, 1.0],
        },
    ];
    let mut graph = build(&input, recipe, 1, 2, &mut work)?;
    let (valid, _memory) = encode(&graph, &mut work)?;
    let _valid = decode(&valid, &mut work)?;

    // Construct a wire-valid second row with its own deterministic height.
    // Its owner can resolve only one member reference in the region map.
    input[1].member = Digest([2; 32]);
    let height = level(recipe, input[1]);
    graph.rows[1].member = input[1].member;
    graph.rows[1].links.resize_with(height + 1, Vec::new);
    for links in &mut graph.rows[1].links[1..] {
        links.clear();
    }
    for links in &mut graph.rows[0].links[1..] {
        links.clear();
    }
    let (invalid, _memory) = encode(&graph, &mut work)?;
    let mut accepted = Vec::new();
    if !matches!(
        build(&input, recipe, 1, 2, &mut work),
        Err(IndexError::InvalidInput(_))
    ) {
        accepted.push("build");
    }
    if !matches!(
        decode(&invalid, &mut work),
        Err(IndexError::CorruptObject(_))
    ) {
        accepted.push("reopen");
    }
    if !accepted.is_empty() {
        return Err(format!("multiple commitments for one owner accepted: {accepted:?}").into());
    }
    Ok(())
}

#[test]
fn native_graph_visit_and_rescore_caps_are_explicit() -> Result<(), Box<dyn std::error::Error>> {
    let vectors: Vec<_> = (0..32).map(|i| vec![1.0, i as f32]).collect();
    let mut work = budget()?;
    let graph = build(&rows(&vectors), Digest([3; 32]), 1, 2, &mut work)?;
    let candidates = graph.candidates(&[1.0, 0.0], 1, 10, &mut work)?;
    if !candidates.capped || candidates.indices.len() != 1 {
        return Err("visit cap not preserved".into());
    }
    let candidates = graph.candidates(&[1.0, 0.0], 1024, 2, &mut work)?;
    if !candidates.capped || candidates.indices.len() > 2 {
        return Err("rescore cap not preserved".into());
    }
    if !matches!(
        graph.candidates(&[1.0, 0.0], 0, 2, &mut work),
        Err(IndexError::InvalidInput(_))
    ) {
        return Err("zero visit cap admitted".into());
    }
    Ok(())
}

#[test]
fn native_graph_small_region_preserves_fixed_order_under_maximum_visit_ceiling()
-> Result<(), Box<dyn std::error::Error>> {
    let vectors = vec![
        vec![1.0, 0.0],
        vec![0.0, 1.0],
        vec![1.0, 0.0],
        vec![-1.0, 0.0],
    ];
    let mut work = budget()?;
    let graph = build(&rows(&vectors), Digest([3; 32]), 1, 2, &mut work)?;
    let candidates = graph.candidates(
        &[1.0, 0.0],
        MAX_REGION_ROWS,
        MAX_REGION_CANDIDATES,
        &mut work,
    )?;
    // Independent cosine order: +X ties by row ID, then orthogonal, then -X.
    if candidates.indices != [0, 2, 1, 3] || candidates.capped {
        return Err("small-region candidates or cap changed under a generous visit ceiling".into());
    }
    Ok(())
}

#[test]
fn native_graph_decode_charges_original_bytes_before_decoded_memory()
-> Result<(), Box<dyn std::error::Error>> {
    use quanta_index_core::index::{MemoryScope, MemoryScopePort, WorkBudgetDenial};
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };

    struct RefuseMemory(Arc<AtomicU64>);
    impl MemoryScopePort for RefuseMemory {
        fn reserve(&self, _: u64, _: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            let _prior = self.0.fetch_add(1, Ordering::SeqCst);
            Err(IndexError::ResourceLimited)
        }
    }
    let mut setup = budget()?;
    let vectors = vec![vec![1.0, 0.0]];
    let graph = build(&rows(&vectors), Digest([3; 32]), 1, 2, &mut setup)?;
    let (bytes, _memory) = encode(&graph, &mut setup)?;
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(30))
        .ok_or("deadline")?;
    let mut original = WorkBudget::new(
        WorkLimits {
            reads: 1,
            writes: 0,
            bytes: 0,
            mutations: 1,
        },
        deadline,
    );
    let calls = Arc::new(AtomicU64::new(0));
    original.bind_memory_scope(Some(MemoryScope::new(Arc::new(RefuseMemory(Arc::clone(
        &calls,
    ))))));
    if !matches!(
        decode(&bytes, &mut original),
        Err(IndexError::ResourceLimited)
    ) || original.first_denial() != Some(WorkBudgetDenial::Bytes)
        || calls.load(Ordering::SeqCst) != 0
    {
        return Err(format!(
            "graph decoder allocated before original byte-work admission: denial={:?}, reservations={}",
            original.first_denial(), calls.load(Ordering::SeqCst),
        ).into());
    }
    Ok(())
}

#[test]
fn native_graph_independent_exact_covers_empty_sparse_and_correlated_filter_strata()
-> Result<(), Box<dyn std::error::Error>> {
    let vectors: Vec<_> = (0..256)
        .map(|i| vec![1.0, (i as f32 - 128.0) / 64.0])
        .collect();
    let mut work = budget()?;
    let graph = build(&rows(&vectors), Digest([3; 32]), 1, 2, &mut work)?;
    let query = [1.0, 0.0];
    let candidates = graph.candidates(&query, MAX_REGION_ROWS, MAX_REGION_CANDIDATES, &mut work)?;
    // Correlate eligibility with worst distance: an unfiltered small prefix
    // would omit these rows. 1/256 <=1%, 20/256 <=10%, 128/256 >10%.
    for count in [0usize, 1, 20, 128, 256] {
        let eligible = |index: usize| index < count;
        let score = |index: usize| {
            let v = &vectors[index];
            f64::from(v[0]) / (f64::from(v[0]).powi(2) + f64::from(v[1]).powi(2)).sqrt()
        };
        let mut oracle: Vec<_> = (0..256).filter(|index| eligible(*index)).collect();
        oracle.sort_by(|a, b| score(*b).total_cmp(&score(*a)).then_with(|| a.cmp(b)));
        oracle.truncate(count.min(10));
        let actual: Vec<_> = candidates
            .indices
            .iter()
            .copied()
            .filter(|index| eligible(*index))
            .take(10)
            .collect();
        if actual != oracle {
            return Err(format!("filter stratum {count}/256: {actual:?} vs {oracle:?}").into());
        }
    }
    Ok(())
}

#[test]
fn native_graph_cancel_and_work_exhaustion_refuse_without_partial_success()
-> Result<(), Box<dyn std::error::Error>> {
    let vectors = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
    let input = rows(&vectors);
    let mut work = budget()?;
    let graph = build(&input, Digest([3; 32]), 1, 2, &mut work)?;
    work.request().cancel_handle().cancel();
    if !matches!(
        graph.candidates(&[1.0, 0.0], 128, 128, &mut work),
        Err(IndexError::Cancelled)
    ) {
        return Err("cancelled graph query returned candidates".into());
    }
    let mut exhausted = WorkBudget::new(
        WorkLimits {
            reads: 0,
            writes: 0,
            bytes: 0,
            mutations: 0,
        },
        Instant::now()
            .checked_add(Duration::from_secs(1))
            .ok_or("deadline")?,
    );
    if !matches!(
        graph.candidates(&[1.0, 0.0], 128, 128, &mut exhausted),
        Err(IndexError::ResourceLimited)
    ) {
        return Err("unfunded graph query returned candidates".into());
    }
    Ok(())
}

#[test]
fn native_graph_bounded_construction_preserves_partial_traversal_recall()
-> Result<(), Box<dyn std::error::Error>> {
    fn direction(seed: u64) -> Vec<f32> {
        let mut state = seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(0xD1B5_4A32_D192_ED03);
        (0..64)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let [low, high, ..] = state.to_le_bytes();
                f32::from(u16::from_le_bytes([low, high])) / 32_768.0 - 1.0
            })
            .collect()
    }
    let vectors: Vec<_> = (0..1024).map(direction).collect();
    let mut work = budget()?;
    let graph = build(&rows(&vectors), Digest([3; 32]), 1, 64, &mut work)?;
    // Fixed recipe bound: sum(min(i, 256), i=1..1023), not a relaxed
    // WorkBudget or a redefinition of one charged vector-distance operation.
    if work.usage().mutations > 229_248 {
        return Err("construction exceeded fixed insertion distance bound".into());
    }
    let (bytes, _memory) = encode(&graph, &mut work)?;
    let graph = decode(&bytes, &mut work)?;
    let mut recalled = 0usize;
    for ordinal in 0..32 {
        // Held-out directions; expected scores use only original f32 vectors
        // and independent f64 cosine, never a graph search/ranking helper.
        let query = direction(10_000 + ordinal);
        let query_norm = query
            .iter()
            .map(|x| f64::from(*x).powi(2))
            .sum::<f64>()
            .sqrt();
        let score = |vector: &[f32]| {
            let dot = query
                .iter()
                .zip(vector)
                .map(|(a, b)| f64::from(*a) * f64::from(*b))
                .sum::<f64>();
            let norm = vector
                .iter()
                .map(|x| f64::from(*x).powi(2))
                .sum::<f64>()
                .sqrt();
            dot / (query_norm * norm)
        };
        let mut oracle: Vec<_> = vectors
            .iter()
            .enumerate()
            .map(|(i, v)| (i, score(v)))
            .collect();
        oracle.sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let before = work.usage().mutations;
        let candidates = graph.candidates(&query, 896, 128, &mut work)?;
        let distances = work.usage().mutations - before;
        if distances > 896
            || distances >= 1024
            || !candidates.capped
            || candidates.indices.len() > 128
        {
            return Err(format!("partial ANN traversal bypassed fixed bound: {distances}").into());
        }
        recalled += bounded_partial_recall_count(&candidates.indices, &oracle)?;
    }
    eprintln!("bounded partial HNSW recalled={recalled}/320");
    if recalled < 304 {
        return Err(format!("partial HNSW recall {recalled}/320 is below .95").into());
    }
    Ok(())
}

#[test]
fn native_graph_upper_visit_cap_keeps_best_observed_candidate()
-> Result<(), Box<dyn std::error::Error>> {
    let vectors = [vec![0.0, 1.0], vec![1.0, 0.0], vec![-1.0, 0.0]];
    // Fixed identities all hash to level one for this recipe/member/seed.
    // Three rows fit every neighbor, making the upper adjacency [1, 2].
    let input: Vec<_> = [4u16, 19, 57]
        .into_iter()
        .zip(&vectors)
        .map(|(id, vector)| RegionRow {
            owner: owner(id),
            member: Digest([2; 32]),
            vector_id: b"v",
            vector,
        })
        .collect();
    let mut work = budget()?;
    let graph = build(&input, Digest([3; 32]), 1, 2, &mut work)?;
    let (bytes, _memory) = encode(&graph, &mut work)?;
    let graph = decode(&bytes, &mut work)?;
    if graph.entry != 0 || graph.rows[0].links.get(1).map(Vec::as_slice) != Some(&[1, 2]) {
        return Err("fixed level-one cap fixture changed".into());
    }
    // Cosines are independently fixed at 0, 1 and -1. With two visits the
    // entry and first neighbor have been observed; returning entry loses the
    // known nearest vector just because the next neighbor cannot be visited.
    for (visits, expected) in [(1, 0), (2, 1), (3, 1)] {
        let before = work.usage().mutations;
        let result = graph.candidates(&[1.0, 0.0], visits, 1, &mut work)?;
        let distances = work.usage().mutations - before;
        if result.indices != [expected] || !result.capped || distances > visits as u64 {
            return Err(format!(
                "upper cap visits={visits}: indices={:?}, expected={expected}, distances={distances}, capped={}",
                result.indices, result.capped
            ).into());
        }
    }
    Ok(())
}
