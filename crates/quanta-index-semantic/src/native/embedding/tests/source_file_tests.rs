//! Independent file oracle: multiple selected projections must occupy one slot.
use super::*;

#[test]
fn full_ranked_window_releases_evicted_id_before_replacement()
-> Result<(), Box<dyn std::error::Error>> {
    let store = MemoryStore::default();
    let maps = TestMap::default();
    let memory = Arc::new(BoundedSelectionMemory {
        limit: 1,
        deny_reservation: None,
        live: AtomicU64::new(0),
        peak: AtomicU64::new(0),
        denied: AtomicU64::new(0),
        invalid: AtomicBool::new(false),
    });
    let mut work = budget()?;
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        BoundedSelectionScope(Arc::clone(&memory)),
    ))));
    let mut context = ReadContext {
        objects: &store,
        maps: &maps,
        budget: &mut work,
    };
    // This window can fund one retained ID, which is also its result limit.
    let mut heap = BinaryHeap::with_capacity(1);
    for (key, score) in [(1, 0.25), (2, 0.75), (3, 0.5)] {
        source_file::admit(
            RankedHit(SearchHit::from_parts(owner(key), vec![key], score, None)),
            1,
            &mut heap,
            &mut context,
        )?;
        check_eq!(memory.live.load(AtomicOrdering::SeqCst), 1);
    }
    check_eq!(heap.len(), 1);
    check_eq!(
        heap.peek().ok_or("replacement hit absent")?.0.owner,
        owner(2)
    );
    check_eq!(context.budget.first_denial(), None);
    // The next candidate cannot fit even after the previous ID is released.
    let denied = source_file::admit(
        RankedHit(SearchHit::from_parts(owner(4), vec![4, 5], 1.0, None)),
        1,
        &mut heap,
        &mut context,
    );
    check!(matches!(denied, Err(IndexError::ResourceLimited)));
    check!(heap.is_empty());
    drop(heap);
    check_eq!(memory.live.load(AtomicOrdering::SeqCst), 0);
    check_eq!(
        work.first_denial(),
        Some(quanta_index_core::index::WorkBudgetDenial::Memory)
    );
    check!(!memory.invalid.load(AtomicOrdering::SeqCst));
    Ok(())
}

#[test]
fn cancelled_ranked_replacement_refunds_evicted_id() -> Result<(), Box<dyn std::error::Error>> {
    let store = MemoryStore::default();
    let maps = TestMap::default();
    let memory = Arc::new(BoundedSelectionMemory {
        limit: 1,
        deny_reservation: None,
        live: AtomicU64::new(0),
        peak: AtomicU64::new(0),
        denied: AtomicU64::new(0),
        invalid: AtomicBool::new(false),
    });
    let mut work = budget()?;
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        BoundedSelectionScope(Arc::clone(&memory)),
    ))));
    let mut heap = BinaryHeap::from([RankedHit(SearchHit::from_parts(
        owner(1),
        vec![1],
        0.25,
        work.reserve_memory(1, MemoryLifetime::Retained)?,
    ))]);
    work.request().cancel_handle().cancel();
    let result = source_file::admit(
        RankedHit(SearchHit::from_parts(owner(2), vec![2], 0.75, None)),
        1,
        &mut heap,
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut work,
        },
    );
    check!(matches!(result, Err(IndexError::Cancelled)));
    check!(heap.is_empty());
    drop(heap);
    check_eq!(memory.live.load(AtomicOrdering::SeqCst), 0);
    check_eq!(
        work.first_denial(),
        Some(quanta_index_core::index::WorkBudgetDenial::Cancelled)
    );
    check!(!memory.invalid.load(AtomicOrdering::SeqCst));
    Ok(())
}

#[test]
fn full_overlay_window_releases_evicted_id_before_replacement()
-> Result<(), Box<dyn std::error::Error>> {
    let store = MemoryStore::default();
    let maps = TestMap::default();
    let mut work = budget()?;
    let bridge = TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::new(AtomicUsize::new(0)),
        deadline: work.request().deadline(),
        normalization: EmbeddingNormalization::L2Unit,
    })?;
    let recipe = bridge.recipe(1024)?;
    let spec = instance(&recipe);
    let (source, _) = semantic_source(&store, "rank.rs", b"fixed ranking fixture")?;
    let (coverage, _) = semantic_group_coverage(&store, &spec, source, CoverageStatus::Complete)?;
    let memory = Arc::new(BoundedSelectionMemory {
        limit: 64,
        deny_reservation: None,
        live: AtomicU64::new(0),
        peak: AtomicU64::new(0),
        denied: AtomicU64::new(0),
        invalid: AtomicBool::new(false),
    });
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        BoundedSelectionScope(Arc::clone(&memory)),
    ))));
    let mut heap = BinaryHeap::from([RankedHit(SearchHit::from_parts(
        owner(1),
        vec![b'a'],
        0.0,
        work.reserve_memory(1, MemoryLifetime::Retained)?,
    ))]);
    let documents = [SemanticOverlayDocument {
        owner: owner(2),
        revision: 1,
        path: None,
        text: Some(Arc::from("overlay")),
    }];
    let overlay = overlay::OverlaySearch {
        documents: &documents,
        bridge: &bridge,
    };
    let query = SearchQuery {
        source_files: false,
        lexical_eligibility: None,
        lexical_owners: None,
        exact_path: None,
        coverage: &coverage,
        projection: None,
        source_owners: MapRoot(None),
        source_paths: MapRoot(None),
        instance: spec.instance,
        format: spec.format,
        recipe: &recipe,
        vector: &[1.0, 0.0],
        top_k: 1,
        page_size: 1,
        max_owners: 1,
        overlay: Some(overlay),
    };
    overlay::rank_overlay(
        overlay,
        &query,
        1.0,
        1,
        &mut heap,
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut work,
        },
    )?;
    let winner = &heap.peek().ok_or("overlay replacement absent")?.0;
    check_eq!(winner.owner, owner(2));
    check!(winner.overlay.is_some());
    check_eq!(work.first_denial(), None);
    drop(heap);
    check_eq!(memory.live.load(AtomicOrdering::SeqCst), 0);
    check!(!memory.invalid.load(AtomicOrdering::SeqCst));

    let denied_memory = Arc::new(BoundedSelectionMemory {
        limit: 63,
        deny_reservation: None,
        live: AtomicU64::new(0),
        peak: AtomicU64::new(0),
        denied: AtomicU64::new(0),
        invalid: AtomicBool::new(false),
    });
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        BoundedSelectionScope(Arc::clone(&denied_memory)),
    ))));
    let mut heap = BinaryHeap::from([RankedHit(SearchHit::from_parts(
        owner(1),
        vec![b'a'],
        0.0,
        work.reserve_memory(1, MemoryLifetime::Retained)?,
    ))]);
    let denied = overlay::rank_overlay(
        overlay,
        &query,
        1.0,
        1,
        &mut heap,
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut work,
        },
    );
    check!(matches!(denied, Err(IndexError::ResourceLimited)));
    check!(heap.is_empty());
    drop(heap);
    check_eq!(denied_memory.live.load(AtomicOrdering::SeqCst), 0);
    check_eq!(
        work.first_denial(),
        Some(quanta_index_core::index::WorkBudgetDenial::Memory)
    );
    check!(!denied_memory.invalid.load(AtomicOrdering::SeqCst));
    Ok(())
}

#[test]
fn source_file_window_follows_authentication_and_deduplication()
-> Result<(), Box<dyn std::error::Error>> {
    let mut work = budget()?;
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::new(AtomicUsize::new(0)),
        deadline: work.request().deadline(),
        normalization: EmbeddingNormalization::L2Unit,
    })?);
    let fixture = projected_fixture_with_shared(&bridge, false, 3)?;
    let reader = NativeSemanticReader::new(
        Arc::clone(&bridge),
        fixture.recipe.clone(),
        semantic_read_config(16),
    )?
    .with_ann();
    let mut query = semantic_request("query", 2);
    query.constraints.repo_relative_path_exact = Some(
        quanta_index_contract::ExactRepoRelativePathV1::new("src/a.rs")?,
    );
    let request = SemanticReadRequest::for_source_files(query)?;
    let result = reader.query(
        &request,
        &mut QueryReadContext {
            ticket: &fixture.ticket,
            instance: &fixture.spec,
            objects: &fixture.store,
            maps: &fixture.maps,
            budget: &mut work,
        },
    )?;
    // Three different projection owners share exactly one source-map identity.
    // The file oracle has one eligible file regardless of the vector window.
    check_eq!(result.result().hits.len(), 1);
    let hit = result.result().hits.first().ok_or("selected file absent")?;
    let file = hit.source_file().ok_or("source identity absent")?;
    check_eq!(hit.owner, owner(41));
    check_eq!(file.owner, fixture.source_owner_a);
    check_eq!(file.source, fixture.source_a);
    check_eq!(file.path(), "src/a.rs");
    check_eq!(hit.vector_id.as_slice(), b"whole-file");
    check_eq!(hit.cosine.total_cmp(&0.0), Ordering::Equal);
    check!(result.exhaustive());
    Ok(())
}

#[test]
fn final_ranking_preserves_large_score_owner_and_vector_id_ties()
-> Result<(), Box<dyn std::error::Error>> {
    // Already-ranked fixture inputs; this checks final order and outgoing
    // grant lifetime, not a global allocator or Source-authority witness.
    let store = MemoryStore::default();
    let maps = TestMap::default();
    let recipe = recipe();
    let spec = instance(&recipe);
    let (source, _) = semantic_source(&store, "rank.rs", b"fixed ranking fixture")?;
    let (coverage, _) = semantic_group_coverage(&store, &spec, source, CoverageStatus::Complete)?;
    let heap: BinaryHeap<_> = (0_u8..32)
        .rev()
        .flat_map(|id| {
            [b'b', b'a'].into_iter().map(move |vector_id| {
                RankedHit(SearchHit::from_parts(
                    owner(id),
                    vec![vector_id],
                    if id >= 16 { 0.75 } else { 0.25 },
                    None,
                ))
            })
        })
        .collect();
    let expected_backing = u64::try_from(
        heap.len()
            .checked_mul(std::mem::size_of::<SearchHit>())
            .ok_or("hit backing overflow")?,
    )?;
    let memory = Arc::new(BoundedSelectionMemory {
        limit: expected_backing,
        deny_reservation: None,
        live: AtomicU64::new(0),
        peak: AtomicU64::new(0),
        denied: AtomicU64::new(0),
        invalid: AtomicBool::new(false),
    });
    let mut work = budget()?;
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        BoundedSelectionScope(Arc::clone(&memory)),
    ))));
    let result = finish_ranked(
        &SearchQuery {
            source_files: false,
            lexical_eligibility: None,
            lexical_owners: None,
            exact_path: None,
            coverage: &coverage,
            projection: None,
            source_owners: MapRoot(Some(source)),
            source_paths: MapRoot(None),
            instance: spec.instance,
            format: spec.format,
            recipe: &recipe,
            vector: &[1.0, 0.0],
            top_k: 64,
            page_size: 64,
            max_owners: 64,
            overlay: None,
        },
        1.0,
        64,
        heap,
        32,
        0,
        NativeSemanticTraversal::ExactExhausted,
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut work,
        },
    )?;
    let expected: Vec<_> = (16_u8..32)
        .chain(0..16)
        .flat_map(|id| {
            [b'a', b'b']
                .into_iter()
                .map(move |vector_id| (owner(id), vec![vector_id]))
        })
        .collect();
    let actual: Vec<_> = result
        .hits
        .iter()
        .map(|hit| (hit.owner, hit.vector_id.clone()))
        .collect();
    check_eq!(actual, expected);
    check_eq!(memory.live.load(AtomicOrdering::SeqCst), expected_backing);
    drop(result);
    check_eq!(memory.live.load(AtomicOrdering::SeqCst), 0);
    check_eq!(memory.denied.load(AtomicOrdering::SeqCst), 0);
    check!(!memory.invalid.load(AtomicOrdering::SeqCst));
    Ok(())
}
