//! Native multi-run recall and multi-page retirement against independent gold.

use super::*;
use quanta_index_core::index::{MapBatchRequest, MapBatchWrite, MapValue, StagedMapRoots};

// Observe the real map port during materialization or query. An 8-byte lookup is a
// region-key lookup; owner and source keys have different widths. Forward all
// operations to the same immutable map and retain its normal work charges.
pub(super) struct RegionLookupMap<'a, 'b> {
    inner: &'a GraphMap<'b>,
    region_lookups: AtomicU64,
}

impl<'a, 'b> RegionLookupMap<'a, 'b> {
    pub(super) fn new(inner: &'a GraphMap<'b>) -> Self {
        Self {
            inner,
            region_lookups: AtomicU64::new(0),
        }
    }

    pub(super) fn region_lookups(&self) -> u64 {
        self.region_lookups.load(AtomicOrdering::SeqCst)
    }
}

impl PersistentMapPort for RegionLookupMap<'_, '_> {
    fn lookup(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<MapValue>, IndexError> {
        if key.len() == 8 {
            let _previous = self.region_lookups.fetch_add(1, AtomicOrdering::SeqCst);
        }
        self.inner.lookup(root, key, budget)
    }

    fn scan(
        &self,
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        self.inner.scan(root, after, limit, budget)
    }

    fn apply(
        &self,
        base: MapRoot,
        edits: &[MapEdit],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapWrite, IndexError> {
        self.inner.apply(base, edits, sink, budget)
    }

    fn apply_batch(
        &self,
        requests: &[MapBatchRequest],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapBatchWrite, IndexError> {
        self.inner.apply_batch(requests, sink, budget)
    }

    fn apply_stream(
        &self,
        bases: &[MapRoot],
        edits: &mut dyn MapEditCursor,
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<StagedMapRoots, IndexError> {
        self.inner.apply_stream(bases, edits, sink, budget)
    }
}

fn direction(seed: u64, width: usize) -> Vec<f32> {
    let mut state = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(0xD1B5_4A32_D192_ED03);
    let mut values = Vec::new();
    for _ in 0..width {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let [low, high, ..] = state.to_le_bytes();
        values.push(f32::from(u16::from_le_bytes([low, high])) / 32_768.0 - 1.0);
    }
    let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
    for value in &mut values {
        *value /= norm;
    }
    values
}

fn corpus_owner(row: u16) -> OwnerKey {
    let mut identity = [0; 16];
    let [low, high] = row.to_le_bytes();
    identity[0] = low;
    identity[1] = high;
    OwnerKey {
        producer: ProducerId([1; 16]),
        stable_id: StableOwnerId(identity),
    }
}

#[test]
fn native_ann_small_runs_carry_and_retain_independent_nearest_neighbor()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let measured = RegionLookupMap::new(&maps);
    let custody = TestCustody;
    let mut recipe = recipe();
    recipe.dimension = 64;
    let spec = instance(&recipe);
    let adapter = NativeSemantic::new(
        SourceVectorProvider {
            recipe: recipe.clone(),
            empty: false,
        },
        recipe.clone(),
    )?
    .with_ann();
    let corpus: Vec<_> = (0..576u16)
        .map(|row| direction(u64::from(row), 64))
        .collect();
    let query = direction(2_000_011, 64);
    // The fixed fresh-direction fixture's nearest row is independent of routing.
    let mut scores: Vec<_> = corpus
        .iter()
        .enumerate()
        .map(|(row, vector)| {
            let dot: f64 = query
                .iter()
                .zip(vector)
                .map(|(a, b)| f64::from(*a) * f64::from(*b))
                .sum();
            (row, dot)
        })
        .collect();
    scores.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let gold = scores.first().ok_or("empty ANN independent gold")?.0;
    check_eq!(gold, 268);
    let mut sources = BTreeMap::new();
    let mut owners = BTreeMap::new();
    let mut prior: Option<CoverageBinding> = None;
    let mut old_root = None;
    for row in 0..576u16 {
        let selected_owner = corpus_owner(row);
        let (source, payload) =
            semantic_source(&store, &format!("src/{row}.rs"), b"selected native vector")?;
        let vector = corpus
            .get(usize::from(row))
            .ok_or("ANN corpus row absent")?
            .clone();
        let (key, coverage_ref) = semantic_owner_vector_for_owner(
            &store,
            &spec,
            &recipe,
            source,
            payload,
            selected_owner,
            vector,
        )?;
        let mut work = ann_budget()?;
        let coverage_object = store.get(coverage_ref, &mut work)?;
        let coverage: OwnerCoverage = serde_json::from_slice(&coverage_object.payload)?;
        let member = coverage.searchable.ok_or("ANN fixture member absent")?;
        check_eq!(sources.insert(key.clone(), source), None);
        check_eq!(owners.insert(key, coverage_ref), None);
        let source_root = maps.seed(sources.clone())?;
        let owner_root = maps.seed(owners.clone())?;
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &measured,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        };
        let searchable = crate::run_blocking(
            &runtime,
            adapter.prepare_searchable(
                &spec,
                prior.as_ref(),
                owner_root,
                Some((selected_owner, member)),
                None,
                &mut context,
            ),
        )?
        .install(&mut context)?
        .ok_or("ANN fixture directory absent")?;
        prior = Some(maintenance::binding(
            &spec,
            &recipe,
            source_root,
            owner_root,
            searchable,
        ));
        if row == 63 {
            old_root = Some(searchable);
        }
    }
    let coverage = prior.ok_or("ANN final coverage absent")?;
    let root = coverage.searchable.ok_or("ANN final directory absent")?;
    // The 576-owner path exercises both the ordinary sweep and multi-run
    // carry. Each region page supplies the selected reference; no extra
    // region-map lookup should occur during materialization.
    check_eq!(measured.region_lookups(), 0);
    let mut work = ann_budget()?;
    let mut directory = ann_directory::AnnDirectory::load(
        root,
        recipe.fingerprint(),
        &mut ReadContext {
            objects: &store,
            maps: &measured,
            budget: &mut work,
        },
    )?;
    check_eq!(directory.region_count, 2);
    check_eq!(directory.next_region, 9);
    check_eq!(directory.delta_rows, 0);
    let memory = Arc::new(QueryMemory::default());
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(QueryMemoryScope(
        Arc::clone(&memory),
    )))));
    let result = crate::run_blocking(
        &runtime,
        ann_query::search(
            &SearchQuery {
                lexical_eligibility: None,
                lexical_owners: None,
                exact_path: None,
                coverage: &coverage,
                projection: None,
                source_owners: coverage.desired.root,
                source_paths: MapRoot(None),
                instance: spec.instance,
                format: spec.format,
                recipe: &recipe,
                vector: &query,
                top_k: 10,
                page_size: 64,
                max_owners: 576,
                overlay: None,
            },
            &mut ReadContext {
                objects: &store,
                maps: &measured,
                budget: &mut work,
            },
        ),
    )?;
    let best = result
        .hits
        .first()
        .ok_or("ANN fresh query returned no hits")?;
    check_eq!(best.owner, corpus_owner(u16::try_from(gold)?));
    check!(matches!(
        result.traversal,
        NativeSemanticTraversal::Approximate { .. }
    ));
    check_eq!(measured.region_lookups(), 0);
    drop(result);
    check_query_memory_released(&memory)?;
    // Old pinned roots still have their complete physical graph after merging.
    let old_root = old_root.ok_or("ANN first sealed root absent")?;
    let old = ann_directory::AnnDirectory::load(
        old_root,
        recipe.fingerprint(),
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut work,
        },
    )?;
    let first = old.region_at(
        0,
        &mut ReadContext {
            objects: &store,
            maps: &measured,
            budget: &mut work,
        },
    )?;
    check_eq!(measured.region_lookups(), 1);
    check!(graph_reaches(&store, old_root, first.descriptor.fileset)?);
    check!(!graph_reaches(&store, root, first.descriptor.fileset)?);
    // A pending owner at the final member page still keeps its old run live.
    let pending_owner = corpus_owner(511);
    let pending_key = owner_map_key(pending_owner).to_vec();
    let prior_ref = owners
        .get(&pending_key)
        .ok_or("ANN pending fixture absent")?;
    let prior_object = store.get(*prior_ref, &mut work)?;
    let mut pending: OwnerCoverage = serde_json::from_slice(&prior_object.payload)?;
    let (next_source, _) = semantic_source(&store, "src/511.rs", b"pending changed source")?;
    pending.desired.source = Some(next_source);
    check_ne!(pending.covered, Some(pending.desired));
    let children = ObjectChildren::from_references([
        pending.desired.source,
        pending.desired.projection,
        pending.searchable,
        None,
    ]);
    let pending_ref = store.insert(serde_json::to_vec(&pending)?, children.as_slice().to_vec())?;
    directory.owners = maps.seed(BTreeMap::from([(pending_key, pending_ref)]))?;
    for _ in 0..2 {
        directory.retire_obsolete(&mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        })?;
    }
    check_eq!(directory.region_count, 1);
    // Fully dead large regions retire after checking all bounded member pages.
    directory.owners = MapRoot(None);
    directory.retire_obsolete(&mut AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &maps,
        custody: &custody,
        scratch: &NoScratch,
        budget: &mut work,
    })?;
    check_eq!(directory.region_count, 0);
    Ok(())
}
