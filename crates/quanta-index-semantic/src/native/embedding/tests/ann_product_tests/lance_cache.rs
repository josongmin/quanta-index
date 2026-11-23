//! Real selected Lance region regressions for decoded-member cache custody.

use super::*;

#[test]
fn lance_selected_region_reuses_authenticated_multivector_artifact()
-> Result<(), Box<dyn std::error::Error>> {
    run_lance_cache_case(false)
}

#[test]
fn lance_selected_region_evicts_cached_members_before_required_decode()
-> Result<(), Box<dyn std::error::Error>> {
    run_lance_cache_case(true)
}

fn check_file_hits(
    result: &SearchResult,
    originals: &[(OwnerKey, ObjectRef, OwnerMemberBinding, OwnerArtifact)],
    query_vector: &[f32],
) -> Result<(), Box<dyn std::error::Error>> {
    check_eq!(result.hits.len(), 2);
    check!(result.hits[0].cosine > result.hits[1].cosine);
    for (index, (hit, (owner, source, _member, artifact))) in
        result.hits.iter().zip(originals).enumerate()
    {
        check_eq!(hit.owner, *owner);
        check_eq!(hit.vector_id.as_slice(), b"a");
        let file = hit.source_file().ok_or("missing selected file")?;
        check_eq!(file.source, *source);
        check_eq!(file.path(), format!("src/{index}.rs"));
        let original = &artifact.vectors[0].values;
        check!((hit.cosine - eligibility_reference_cosine(query_vector, original)).abs() < 1e-6);
    }
    Ok(())
}

fn run_lance_cache_case(memory_pressure: bool) -> Result<(), Box<dyn std::error::Error>> {
    struct RankingMemory {
        meter: Arc<QueryMemory>,
        ranking_started: Arc<AtomicBool>,
        cache_state_bytes: u64,
        deny_cache_state: bool,
        cache_state_denied: AtomicU64,
        ceiling: AtomicU64,
        peak: AtomicU64,
        refusals: AtomicU64,
    }

    impl MemoryScopePort for RankingMemory {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if self.deny_cache_state
                && !self.ranking_started.load(AtomicOrdering::SeqCst)
                && bytes == self.cache_state_bytes
                && self
                    .cache_state_denied
                    .compare_exchange(0, 1, AtomicOrdering::SeqCst, AtomicOrdering::SeqCst)
                    .is_ok()
            {
                return Err(IndexError::ResourceLimited);
            }
            let live = self.meter.live.load(AtomicOrdering::SeqCst);
            if self.ranking_started.load(AtomicOrdering::SeqCst)
                && live + bytes > self.ceiling.load(AtomicOrdering::SeqCst)
            {
                let _previous = self.refusals.fetch_add(1, AtomicOrdering::SeqCst);
                return Err(IndexError::ResourceLimited);
            }
            let lease = QueryMemoryScope(Arc::clone(&self.meter)).reserve(bytes, lifetime)?;
            if self.ranking_started.load(AtomicOrdering::SeqCst) {
                let _previous = self.peak.fetch_max(live + bytes, AtomicOrdering::SeqCst);
            }
            Ok(lease)
        }
    }

    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let mut recipe = recipe();
    recipe.dimension = 128;
    let spec = instance(&recipe);
    let runtime = test_runtime()?;
    let query_vector = {
        let mut vector = vec![0.0; 128];
        vector[0] = 1.0;
        vector
    };
    let mut source_entries = BTreeMap::new();
    let mut owner_entries = BTreeMap::new();
    let mut member_entries = BTreeMap::new();
    let mut originals = Vec::new();
    let mut setup = profile_work(60)?;
    for (index, a_y, b_y) in [(0usize, 0.0f32, 0.6f32), (1, 0.43589, 0.71414)] {
        let owner = profile_owner(index);
        let path = format!("src/{index}.rs");
        let (source, payload) = semantic_source(&store, &path, b"source")?;
        let (key, original_coverage) = semantic_owner_vector_for_owner(
            &store,
            &spec,
            &recipe,
            source,
            payload,
            owner,
            query_vector.clone(),
        )?;
        let coverage_object = store.get(original_coverage, &mut setup)?;
        let mut owner_coverage: OwnerCoverage = serde_json::from_slice(&coverage_object.payload)?;
        let member_ref = owner_coverage.searchable.ok_or("missing selected member")?;
        let member_object = store.get(member_ref, &mut setup)?;
        let mut member: OwnerMemberBinding = serde_json::from_slice(&member_object.payload)?;
        let artifact_object = store.get(member.artifact, &mut setup)?;
        let mut artifact: OwnerArtifact = serde_json::from_slice(&artifact_object.payload)?;
        let mut a = vec![0.0; 128];
        a[0] = if index == 0 { 1.0 } else { 0.9 };
        a[1] = a_y;
        let mut b = vec![0.0; 128];
        b[0] = if index == 0 { 0.8 } else { 0.7 };
        b[1] = b_y;
        artifact.vectors = vec![
            EmbeddingVector {
                id: b"a".to_vec(),
                values: a,
            },
            EmbeddingVector {
                id: b"b".to_vec(),
                values: b,
            },
        ];
        let encoded = serde_json::to_vec(&artifact)?;
        member.artifact = store.insert(
            encoded.clone(),
            artifact_children(&artifact).as_ref().to_vec(),
        )?;
        member.logical_commitment = commitment(&encoded);
        let children = ObjectChildren::from_references([
            Some(member.artifact),
            member.input.source,
            member.input.projection,
            None,
        ]);
        let member_ref =
            store.insert(serde_json::to_vec(&member)?, children.as_slice().to_vec())?;
        owner_coverage.searchable = Some(member_ref);
        let coverage_children =
            ObjectChildren::from_references([Some(source), Some(member_ref), None, None]);
        let coverage_ref = store.insert(
            serde_json::to_vec(&owner_coverage)?,
            coverage_children.as_slice().to_vec(),
        )?;
        let _previous = source_entries.insert(key.clone(), source);
        let _previous = owner_entries.insert(key.clone(), coverage_ref);
        let _previous = member_entries.insert(key, member_ref);
        originals.push((owner, source, member, artifact));
    }
    let sources = maps.seed(source_entries)?;
    let owners = maps.seed(owner_entries)?;
    let members = maps.seed(member_entries)?;
    let mut directory = ann_directory::AnnDirectory::empty_with_backend(
        recipe.fingerprint(),
        owners,
        ann_directory::AnnBackend::LanceDb,
    );
    // Seal in owner/vector-ID order. The four distinct cosine scores make
    // the subsequent flat search visit owners 0, 1, 0, 1.
    let mut rows = Vec::new();
    for (owner, _source, member, artifact) in &originals {
        for vector in &artifact.vectors {
            rows.push(ann_lance::RegionRow {
                owner: *owner,
                member: member.logical_commitment,
                vector_id: &vector.id,
                input: member.input,
                vector: &vector.values,
            });
        }
    }
    let mut write = AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &maps,
        custody: &custody,
        scratch: &NoScratch,
        budget: &mut setup,
    };
    let fileset = crate::run_blocking(
        &runtime,
        ann_materialization::install_lance_region(&rows, recipe.fingerprint(), 1, 128, &mut write),
    )?;
    let _descriptor = directory.append_region(
        &ann_directory::RegionDescriptor {
            fileset,
            members,
            center: query_vector.clone(),
            rows: 4,
        },
        &mut write,
    )?;
    let directory_ref = directory.save(&mut write)?;
    let coverage = maintenance::binding(&spec, &recipe, sources, owners, directory_ref);
    let query = SearchQuery {
        source_files: true,
        lexical_eligibility: None,
        exact_path: None,
        coverage: &coverage,
        projection: None,
        source_owners: sources,
        source_paths: MapRoot(None),
        instance: spec.instance,
        format: spec.format,
        recipe: &recipe,
        vector: &query_vector,
        top_k: 2,
        page_size: 2,
        max_owners: 2,
        overlay: None,
    };
    if !memory_pressure {
        let memory = Arc::new(QueryMemory::default());
        let mut budget = profile_work(60)?;
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(QueryMemoryScope(
            Arc::clone(&memory),
        )))));
        let objects = ReadOnceDescriptor::new(&store, originals[0].2.artifact);
        let key = owner_map_key(originals[0].0);
        let mut member_reads = RecordingMap::new(&maps, members, false);
        member_reads.observe_lookup_key(key);
        let mut coverage_reads = RecordingMap::new(&member_reads, owners, false);
        coverage_reads.observe_lookup_key(key);
        let result = search_selected_lancedb(
            &query,
            &mut ReadContext {
                objects: &objects,
                maps: &coverage_reads,
                budget: &mut budget,
            },
            &runtime,
        )?;
        check_eq!(objects.reads(), 1);
        check_eq!(member_reads.take_lookup_count(), 2);
        check_eq!(coverage_reads.take_lookup_count(), 1);
        check_file_hits(&result, &originals, &query_vector)?;
        drop(result);
        check_query_memory_released(&memory)?;

        let objects = ReadOnceDescriptor::new(&store, originals[0].2.artifact);
        check_ann_scratch_before_overlay(
            SearchQuery {
                source_files: false,
                max_owners: 3,
                ..query
            },
            profile_owner(2),
            &mut ReadContext {
                objects: &objects,
                maps: &coverage_reads,
                budget: &mut profile_work(60)?,
            },
            |query, context| search_selected_lancedb(query, context, &runtime),
        )?;
        check_eq!(objects.reads(), 1);
        check_eq!(member_reads.take_lookup_count(), 2);

        let one_owner = search_selected_lancedb(
            &SearchQuery {
                source_files: false,
                max_owners: 1,
                ..query
            },
            &mut ReadContext {
                objects: &store,
                maps: &maps,
                budget: &mut profile_work(60)?,
            },
            &runtime,
        )?;
        check_eq!(one_owner.scanned_owners, 1);
        let vector_ids: Vec<_> = one_owner
            .hits
            .iter()
            .map(|hit| (hit.owner, hit.vector_id.as_slice()))
            .collect();
        check_eq!(
            vector_ids,
            vec![
                (originals[0].0, b"a".as_slice()),
                (originals[0].0, b"b".as_slice())
            ]
        );
        check_eq!(
            one_owner.traversal,
            NativeSemanticTraversal::Approximate { capped: true }
        );
        return Ok(());
    }
    let cache_state_bytes = u64::try_from(ann_query::lance_cache_state_bytes())?;
    let mut measured_peak = None;
    for deny_cache_state in [true, false] {
        let memory = Arc::new(QueryMemory::default());
        let ranking_started = Arc::new(AtomicBool::new(false));
        let scope = Arc::new(RankingMemory {
            meter: Arc::clone(&memory),
            ranking_started: Arc::clone(&ranking_started),
            cache_state_bytes,
            deny_cache_state,
            cache_state_denied: AtomicU64::new(0),
            ceiling: AtomicU64::new(measured_peak.unwrap_or(u64::MAX)),
            peak: AtomicU64::new(0),
            refusals: AtomicU64::new(0),
        });
        let mut query_budget = profile_work(60)?;
        query_budget.bind_memory_scope(Some(MemoryScope::new(scope.clone())));
        let objects = ReadOnceDescriptor::new(&store, originals[0].2.artifact)
            .allow_reopen()
            .with_read_signal(ranking_started);
        let result = search_selected_lancedb(
            &query,
            &mut ReadContext {
                objects: &objects,
                maps: &maps,
                budget: &mut query_budget,
            },
            &runtime,
        )?;
        check_eq!(query_budget.first_denial(), None);
        check_eq!(objects.reads(), 2);
        check_file_hits(&result, &originals, &query_vector)?;
        drop(result);
        check_query_memory_released(&memory)?;
        if deny_cache_state {
            check_eq!(scope.cache_state_denied.load(AtomicOrdering::SeqCst), 1);
            let peak = scope.peak.load(AtomicOrdering::SeqCst);
            check!(peak > 0);
            measured_peak = Some(peak);
        } else {
            check_eq!(scope.cache_state_denied.load(AtomicOrdering::SeqCst), 0);
            check!(scope.refusals.load(AtomicOrdering::SeqCst) > 0);
        }
    }
    Ok(())
}
