//! Fixed 256-row evidence for the pinned Lance IVF-HNSW-SQ branch.

use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Instant,
};

use quanta_index_core::index::{
    MemoryLeaseOwner, MemoryScope, MemoryScopePort, ObjectRef, WorkLimits,
};

use super::*;

const ROWS: usize = 256;
const DIMENSION: usize = 8;
const TOP_K: usize = 5;
const QUERY_ORDINAL: usize = 64;

#[derive(Default)]
struct CountedMemory {
    live: AtomicU64,
    invalid_release: AtomicBool,
}

impl MemoryLeaseOwner for CountedMemory {
    fn release(&self, bytes: u64) {
        if self
            .live
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                live.checked_sub(bytes)
            })
            .is_err()
        {
            self.invalid_release.store(true, Ordering::SeqCst);
        }
    }
}

struct BoundedMemory(Arc<CountedMemory>);

impl MemoryScopePort for BoundedMemory {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        if bytes == 0 {
            return Err(IndexError::InvalidInput(
                "trained ANN test zero-byte reservation".into(),
            ));
        }
        let _previous = self
            .0
            .live
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                live.checked_add(bytes)
                    .filter(|next| *next <= 64 * 1024 * 1024)
            })
            .map_err(|_live| IndexError::ResourceLimited)?;
        Ok(MemoryLease::new(bytes, self.0.clone()))
    }
}

fn verify(condition: bool, message: &'static str) -> Result<(), Box<dyn std::error::Error>> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn owner(ordinal: usize) -> Result<OwnerKey, IndexError> {
    let ordinal = u16::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?;
    let mut stable = [0u8; 16];
    stable
        .get_mut(14..)
        .ok_or(IndexError::ResourceLimited)?
        .copy_from_slice(&ordinal.to_be_bytes());
    Ok(OwnerKey {
        producer: ProducerId([1; 16]),
        stable_id: StableOwnerId(stable),
    })
}

fn input(recipe: Digest) -> OwnerInputBinding {
    OwnerInputBinding {
        source: Some(ObjectRef(Digest([3; 32]))),
        projection: None,
        producer_context: Digest([4; 32]),
        derivation_policy: recipe,
    }
}

fn vector(ordinal: u16) -> [f32; DIMENSION] {
    let theta = f32::from(ordinal) * std::f32::consts::TAU / 256.0;
    [
        theta.cos(),
        theta.sin(),
        (3.0 * theta).cos() * 0.25,
        (3.0 * theta).sin() * 0.25,
        0.0,
        0.0,
        0.0,
        0.0,
    ]
}

// This oracle uses the fixed input vectors and f64 arithmetic, not Lance
// distances, production cosine, or the candidates returned by the index.
fn exact_cosine(left: &[f32; DIMENSION], right: &[f32; DIMENSION]) -> f64 {
    let mut dot = 0.0f64;
    let mut left_norm = 0.0f64;
    let mut right_norm = 0.0f64;
    for (a, b) in left.iter().zip(right) {
        let a = f64::from(*a);
        let b = f64::from(*b);
        dot = a.mul_add(b, dot);
        left_norm = a.mul_add(a, left_norm);
        right_norm = b.mul_add(b, right_norm);
    }
    dot / (left_norm.sqrt() * right_norm.sqrt())
}

fn ordinal(candidate: &RegionCandidate) -> Result<usize, IndexError> {
    let [high, low] = candidate.vector_id.as_slice() else {
        return Err(corrupt("trained ANN candidate vector ID width differs"));
    };
    let value = usize::from(u16::from_be_bytes([*high, *low]));
    if value >= ROWS {
        return Err(corrupt("trained ANN candidate vector ID is out of fixture"));
    }
    Ok(value)
}

async fn check_selected_query(
    region: &OpenRegion,
    vectors: &[[f32; DIMENSION]],
    recipe: Digest,
    budget: &mut WorkBudget,
) -> Result<(), Box<dyn std::error::Error>> {
    let query = vectors
        .get(QUERY_ORDINAL)
        .ok_or("trained ANN query absent")?;
    let mut rejected = 0usize;
    let page = query_region(
        region,
        query,
        TOP_K,
        TOP_K,
        64,
        budget,
        |candidate, _budget| {
            let value = ordinal(candidate)?;
            let eligible = value.is_multiple_of(2) && value != QUERY_ORDINAL;
            if !eligible {
                rejected = rejected.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
            Ok(eligible)
        },
    )
    .await?;
    verify(rejected > 0, "trained ANN filter rejected no candidates")?;
    verify(
        page.hits().len() == TOP_K,
        "trained ANN returned too few eligible rows",
    )?;
    verify(
        matches!(page.traversal, NativeSemanticTraversal::Approximate { .. }),
        "trained ANN claimed global exact exhaustion",
    )?;

    let mut actual = BTreeSet::new();
    for candidate in page.hits() {
        let value = ordinal(candidate)?;
        let expected = vectors.get(value).ok_or("trained ANN row absent")?;
        verify(
            value.is_multiple_of(2) && value != QUERY_ORDINAL,
            "trained ANN returned an excluded member",
        )?;
        verify(
            candidate.owner == owner(value)?
                && candidate.member == Digest([7; 32])
                && selected_input_matches(
                    candidate,
                    owner(value)?,
                    Digest([7; 32]),
                    &candidate.vector_id,
                    input(recipe),
                ),
            "trained ANN candidate escaped selected member/input binding",
        )?;
        verify(
            (f64::from(candidate.cosine) - exact_cosine(query, expected)).abs() <= 0.01,
            "trained ANN score differs from independent exact cosine",
        )?;
        verify(
            actual.insert(value),
            "trained ANN returned duplicate vector ID",
        )?;
    }
    let mut exact: Vec<_> = vectors
        .iter()
        .enumerate()
        .filter(|(value, _)| value.is_multiple_of(2) && *value != QUERY_ORDINAL)
        .map(|(value, vector)| (exact_cosine(query, vector), value))
        .collect();
    exact.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.1.cmp(&right.1))
    });
    let gold: BTreeSet<_> = exact
        .into_iter()
        .take(TOP_K)
        .map(|(_, value)| value)
        .collect();
    verify(
        actual.intersection(&gold).count() >= 4,
        "trained ANN missed more than one fixed eligible top-five row",
    )?;
    Ok(())
}

#[test]
fn trained_region_seals_opens_and_filters_fixed_vectors() -> Result<(), Box<dyn std::error::Error>>
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    crate::run_blocking(&runtime, trained_region_body())
}

async fn trained_region_body() -> Result<(), Box<dyn std::error::Error>> {
    verify(
        u64::try_from(ROWS)? >= crate::vector_index::VECTOR_INDEX_MIN_ROWS,
        "trained ANN fixture fell below index floor",
    )?;
    let root = tempfile::tempdir()?;
    let connection = lancedb::connect(&root.path().to_string_lossy())
        .execute()
        .await?;
    let recipe = Digest([9; 32]);
    let vectors: Vec<_> = (0..ROWS)
        .map(|value| u16::try_from(value).map(vector))
        .collect::<Result<_, _>>()?;
    let ids: Vec<_> = (0..ROWS)
        .map(|value| u16::try_from(value).map(u16::to_be_bytes))
        .collect::<Result<_, _>>()?;
    let rows: Vec<_> = vectors
        .iter()
        .zip(&ids)
        .enumerate()
        .map(|(value, (vector, vector_id))| {
            Ok(RegionRow {
                owner: owner(value)?,
                member: Digest([7; 32]),
                vector_id,
                input: input(recipe),
                vector,
            })
        })
        .collect::<Result<_, IndexError>>()?;
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(120))
        .ok_or(IndexError::ResourceLimited)?;
    let mut budget = WorkBudget::new(
        WorkLimits {
            reads: 1_000,
            writes: 1_000,
            bytes: 100_000_000,
            mutations: 1_000,
        },
        deadline,
    );
    let memory = Arc::new(CountedMemory::default());
    budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(BoundedMemory(
        memory.clone(),
    )))));
    let (table, seal) = build_region(
        &connection,
        "trained",
        &rows,
        recipe,
        1,
        DIMENSION,
        &mut budget,
    )
    .await?;
    verify(
        seal.vector_index.ann.is_some(),
        "256 rows did not seal a trained index",
    )?;
    verify(
        matches!(
            crate::native::ann_files::diagnostic_index_kind(&seal)?,
            quanta_index_core::DiagnosticAnnIndexKindV1::IvfHnswSq
        ),
        "trained ANN seal was not classified as IVF-HNSW-SQ",
    )?;
    verify(
        table.list_indices().await?.iter().any(|index| {
            index.name == crate::vector_index::VECTOR_INDEX_NAME
                && index.columns.iter().any(|column| column == COLUMN_VECTOR)
        }),
        "trained ANN physical table has no vector index",
    )?;
    let opened = open_region(table, seal.clone(), &mut budget).await?;
    check_selected_query(&opened, &vectors, recipe, &mut budget).await?;
    let reopened_table = connection.open_table("trained").execute().await?;
    let reopened = open_region(reopened_table, seal, &mut budget).await?;
    check_selected_query(&reopened, &vectors, recipe, &mut budget).await?;
    verify(
        memory.live.load(Ordering::SeqCst) == 0 && !memory.invalid_release.load(Ordering::SeqCst),
        "trained ANN retained an original budget lease after query",
    )
}
