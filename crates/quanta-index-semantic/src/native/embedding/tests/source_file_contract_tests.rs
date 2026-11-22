//! Selected-view source identity and file-window contracts on the real reader.
use super::*;

#[test]
fn source_file_candidates_preserve_dense_only_source_and_projection_provenance()
-> Result<(), Box<dyn std::error::Error>> {
    let mut work = budget()?;
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::new(AtomicUsize::new(0)),
        deadline: work.request().deadline(),
        normalization: EmbeddingNormalization::L2Unit,
    })?);
    let fixture = projected_fixture_with_shared(&bridge, false, 3)?;
    let reader =
        NativeSemanticReader::new(bridge, fixture.recipe.clone(), semantic_read_config(16))?
            .with_ann();
    let request = SemanticReadRequest::for_source_files(semantic_request("query", 2))?;
    let outcome = reader.query(
        &request,
        &mut QueryReadContext {
            ticket: &fixture.ticket,
            instance: &fixture.spec,
            objects: &fixture.store,
            maps: &fixture.maps,
            budget: &mut work,
        },
    )?;
    let hits = &outcome.result().hits;
    check_eq!(hits.len(), 2);
    let nearest = hits.first().ok_or("dense-only file")?;
    let second = hits.get(1).ok_or("shared projected file")?;
    check_eq!(
        nearest.source_file().ok_or("dense source witness")?.owner,
        owner(52)
    );
    check_eq!(
        nearest.source_file().ok_or("dense source witness")?.path(),
        "src/b.rs"
    );
    check_eq!(nearest.owner, owner(200));
    check_eq!(
        second.source_file().ok_or("shared source witness")?.owner,
        owner(51)
    );
    check_eq!(
        second.source_file().ok_or("shared source witness")?.source,
        fixture.source_a
    );
    check_eq!(second.owner, owner(41));
    check_eq!(second.cosine.total_cmp(&0.0), Ordering::Equal);
    // IDs remain projection/vector provenance when metadata is moved out.
    let (mut hits, _memory) = outcome.into_result().into_parts();
    let parts = hits.remove(0).into_source_file_parts()?;
    check_eq!(parts.owner, owner(200));
    check_eq!(parts.source_file.owner, owner(52));
    check_eq!(parts.source_file.path(), "src/b.rs");
    check_eq!(parts.vector_id.as_slice(), b"whole-file");
    Ok(())
}

#[test]
fn source_file_window_replaces_earlier_projection_with_later_higher_score()
-> Result<(), Box<dyn std::error::Error>> {
    let mut work = budget()?;
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::new(AtomicUsize::new(0)),
        deadline: work.request().deadline(),
        normalization: EmbeddingNormalization::L2Unit,
    })?);
    // The initial shared-file projections both score zero. Owner 42 is later
    // in the selected owner map and receives an independently fixed unit vector.
    let mut fixture = projected_fixture(&bridge, false)?;
    let binding_ref = fixture
        .maps
        .rooted_entries
        .iter()
        .find(|(root, _)| *root == fixture.ticket.view.projections)
        .and_then(|(_, entries)| entries.first())
        .ok_or("projection binding")?
        .1;
    let object = fixture.store.get(binding_ref, &mut work)?;
    let binding: ProjectionBinding = serde_json::from_slice(&object.payload)?;
    let (contribution, coverage) = projected_owner_row(
        &fixture.store,
        &fixture.spec,
        &fixture.recipe,
        binding.projection,
        binding.producer,
        owner(42),
        fixture.source_owner_a,
        fixture.source_a,
        vec![1.0, 0.0],
        false,
    )?;
    let owner_key = owner_map_key(owner(42));
    let mut reverse_key = owner_map_key(fixture.source_owner_a).to_vec();
    reverse_key.extend_from_slice(&owner_key);
    let (_, projection_rows) = fixture
        .maps
        .rooted_entries
        .iter_mut()
        .find(|(root, _)| *root == binding.owners)
        .ok_or("projection owner map")?;
    projection_rows
        .iter_mut()
        .find(|(key, _)| key.as_slice() == owner_key)
        .ok_or("later projection owner")?
        .1 = contribution;
    let (_, reverse_rows) = fixture
        .maps
        .rooted_entries
        .iter_mut()
        .find(|(root, _)| *root == fixture.reverse_root)
        .ok_or("source reverse map")?;
    reverse_rows
        .iter_mut()
        .find(|(key, _)| *key == reverse_key)
        .ok_or("later source reverse row")?
        .1 = coverage;
    let (_, semantic_rows) = fixture
        .maps
        .rooted_entries
        .iter_mut()
        .find(|(root, rows)| {
            *root != binding.owners
                && *root != fixture.reverse_root
                && rows.iter().any(|(key, _)| key.as_slice() == owner_key)
        })
        .ok_or("semantic owner map")?;
    semantic_rows
        .iter_mut()
        .find(|(key, _)| key.as_slice() == owner_key)
        .ok_or("later semantic owner")?
        .1 = coverage;

    let reader =
        NativeSemanticReader::new(bridge, fixture.recipe.clone(), semantic_read_config(16))?
            .with_ann();
    let request = SemanticReadRequest::for_source_files(semantic_request("query", 1))?;
    let outcome = reader.query(
        &request,
        &mut QueryReadContext {
            ticket: &fixture.ticket,
            instance: &fixture.spec,
            objects: &fixture.store,
            maps: &fixture.maps,
            budget: &mut work,
        },
    )?;
    check_eq!(outcome.result().hits.len(), 1);
    let hit = outcome.result().hits.first().ok_or("ranked file absent")?;
    let file = hit.source_file().ok_or("ranked source identity absent")?;
    check_eq!(hit.owner, owner(42));
    check_eq!(hit.vector_id.as_slice(), b"whole-file");
    check_eq!(hit.cosine.total_cmp(&1.0), Ordering::Equal);
    check_eq!(file.owner, fixture.source_owner_a);
    check_eq!(file.source, fixture.source_a);
    check_eq!(file.path(), "src/a.rs");
    check!(outcome.exhaustive());
    Ok(())
}

#[test]
fn source_file_candidates_refuse_foreign_source_even_outside_the_top_window()
-> Result<(), Box<dyn std::error::Error>> {
    let mut work = budget()?;
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::new(AtomicUsize::new(0)),
        deadline: work.request().deadline(),
        normalization: EmbeddingNormalization::L2Unit,
    })?);
    let mut fixture = projected_fixture(&bridge, false)?;
    let reader =
        NativeSemanticReader::new(bridge, fixture.recipe.clone(), semantic_read_config(16))?
            .with_ann();
    let request = SemanticReadRequest::for_source_files(semantic_request("query", 1))?;
    let baseline = reader.query(
        &request,
        &mut QueryReadContext {
            ticket: &fixture.ticket,
            instance: &fixture.spec,
            objects: &fixture.store,
            maps: &fixture.maps,
            budget: &mut work,
        },
    )?;
    check_eq!(baseline.result().hits.len(), 1);
    check_eq!(baseline.result().hits[0].owner, owner(200));
    drop(baseline);
    let (_, sources) = fixture
        .maps
        .rooted_entries
        .iter_mut()
        .find(|(root, _)| *root == fixture.ticket.view.source.owners)
        .ok_or("source map")?;
    let (_, source) = sources
        .iter_mut()
        .find(|(key, _)| *key == owner_map_key(owner(51)))
        .ok_or("shared source")?;
    let (foreign, _) = semantic_source(&fixture.store, "src/a.rs", b"foreign current bytes")?;
    *source = foreign;
    let result = reader.query(
        &request,
        &mut QueryReadContext {
            ticket: &fixture.ticket,
            instance: &fixture.spec,
            objects: &fixture.store,
            maps: &fixture.maps,
            budget: &mut work,
        },
    );
    check!(matches!(result, Err(IndexError::CorruptObject(_))));
    Ok(())
}

#[test]
fn source_file_window_does_not_collapse_different_producers_by_the_same_path()
-> Result<(), Box<dyn std::error::Error>> {
    let mut work = budget()?;
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::new(AtomicUsize::new(0)),
        deadline: work.request().deadline(),
        normalization: EmbeddingNormalization::L2Unit,
    })?);
    let mut fixture = projected_fixture(&bridge, false)?;
    let projection = fixture.ticket.view.projections;
    let binding_ref = fixture
        .maps
        .rooted_entries
        .iter()
        .find(|(root, _)| *root == projection)
        .and_then(|(_, entries)| entries.first())
        .ok_or("projection binding")?
        .1;
    let object = fixture.store.get(binding_ref, &mut work)?;
    let binding: ProjectionBinding = serde_json::from_slice(&object.payload)?;
    let second_owner = OwnerKey {
        producer: ProducerId([0; 16]),
        stable_id: owner(52).stable_id,
    };
    let (same_path, _) = semantic_source(&fixture.store, "src/a.rs", b"another producer bytes")?;
    let (contribution, coverage) = projected_owner_row(
        &fixture.store,
        &fixture.spec,
        &fixture.recipe,
        binding.projection,
        binding.producer,
        owner(200),
        second_owner,
        same_path,
        vec![0.0, 1.0],
        false,
    )?;
    for (root, entries) in &mut fixture.maps.rooted_entries {
        if *root == fixture.ticket.view.source.owners {
            entries.retain(|(key, _)| *key != owner_map_key(owner(52)));
            entries.push((owner_map_key(second_owner).to_vec(), same_path));
            entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        } else if *root == binding.owners {
            let (_, reference) = entries
                .iter_mut()
                .find(|(key, _)| *key == owner_map_key(owner(200)))
                .ok_or("projection row")?;
            *reference = contribution;
        } else if entries
            .iter()
            .any(|(key, _)| *key == owner_map_key(owner(41)))
        {
            let (_, reference) = entries
                .iter_mut()
                .find(|(key, _)| *key == owner_map_key(owner(200)))
                .ok_or("semantic row")?;
            *reference = coverage;
        }
    }
    let reader =
        NativeSemanticReader::new(bridge, fixture.recipe.clone(), semantic_read_config(16))?
            .with_ann();
    let request = SemanticReadRequest::for_source_files(semantic_request("query", 2))?;
    let outcome = reader.query(
        &request,
        &mut QueryReadContext {
            ticket: &fixture.ticket,
            instance: &fixture.spec,
            objects: &fixture.store,
            maps: &fixture.maps,
            budget: &mut work,
        },
    )?;
    check_eq!(outcome.result().hits.len(), 2);
    let identities: Vec<_> = outcome
        .result()
        .hits
        .iter()
        .map(|hit| hit.source_file().map(|file| (file.owner, file.path())))
        .collect();
    check_eq!(
        identities,
        vec![
            Some((second_owner, "src/a.rs")),
            Some((owner(51), "src/a.rs"))
        ]
    );
    Ok(())
}

#[test]
fn source_file_window_excludes_certified_source_free_projection_before_admission()
-> Result<(), Box<dyn std::error::Error>> {
    let mut work = budget()?;
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::new(AtomicUsize::new(0)),
        deadline: work.request().deadline(),
        normalization: EmbeddingNormalization::L2Unit,
    })?);
    let mut fixture = projected_fixture(&bridge, false)?;
    let binding_ref = fixture
        .maps
        .rooted_entries
        .iter()
        .find(|(root, _)| *root == fixture.ticket.view.projections)
        .and_then(|(_, rows)| rows.first())
        .ok_or("projection binding")?
        .1;
    let stored = fixture.store.get(binding_ref, &mut work)?;
    let binding: ProjectionBinding = serde_json::from_slice(&stored.payload)?;
    let free_owner = owner(201);
    let (contribution, coverage) = projected_owner_vectors(
        &fixture.store,
        &fixture.spec,
        &fixture.recipe,
        binding.projection,
        binding.producer,
        free_owner,
        None,
        vec![EmbeddingVector {
            id: b"whole-file".to_vec(),
            values: vec![1.0, 0.0],
        }],
        false,
    )?;
    for (root, rows) in &mut fixture.maps.rooted_entries {
        if *root == binding.owners {
            rows.push((owner_map_key(free_owner).to_vec(), contribution));
        } else if rows.iter().any(|(key, _)| *key == owner_map_key(owner(41))) {
            rows.push((owner_map_key(free_owner).to_vec(), coverage));
        }
    }
    let reader =
        NativeSemanticReader::new(bridge, fixture.recipe.clone(), semantic_read_config(16))?
            .with_ann();
    let request = SemanticReadRequest::for_source_files(semantic_request("query", 2))?;
    let outcome = reader.query(
        &request,
        &mut QueryReadContext {
            ticket: &fixture.ticket,
            instance: &fixture.spec,
            objects: &fixture.store,
            maps: &fixture.maps,
            budget: &mut work,
        },
    )?;
    check_eq!(outcome.result().hits.len(), 2);
    let files: Vec<_> = outcome
        .result()
        .hits
        .iter()
        .map(|hit| hit.source_file().map(|file| file.owner))
        .collect();
    check_eq!(files, vec![Some(owner(52)), Some(owner(51))]);
    check!(
        outcome
            .result()
            .hits
            .iter()
            .all(|hit| hit.owner != free_owner)
    );
    Ok(())
}

#[test]
fn source_file_window_uses_best_vector_and_id_tie_before_file_cap()
-> Result<(), Box<dyn std::error::Error>> {
    let store = MemoryStore::default();
    let recipe = recipe();
    let spec = instance(&recipe);
    let mut source_rows = BTreeMap::new();
    let mut owner_rows = BTreeMap::new();
    for (id, vectors) in [
        (
            1u8,
            vec![
                EmbeddingVector {
                    id: b"a".to_vec(),
                    values: vec![1.0, 0.0],
                },
                EmbeddingVector {
                    id: b"b".to_vec(),
                    values: vec![1.0, 0.0],
                },
                EmbeddingVector {
                    id: b"c".to_vec(),
                    values: vec![0.8, 0.6],
                },
            ],
        ),
        (
            2u8,
            vec![EmbeddingVector {
                id: b"whole-file".to_vec(),
                values: vec![0.6, 0.8],
            }],
        ),
    ] {
        let owner = owner(id);
        let (source, payload) = semantic_source(&store, &format!("src/{id}.rs"), b"file oracle")?;
        let input = OwnerInputBinding {
            source: Some(source),
            projection: None,
            producer_context: Digest([0; 32]),
            derivation_policy: recipe.fingerprint(),
        };
        let artifact = OwnerArtifact {
            owner,
            input,
            recipe: recipe.fingerprint(),
            payload_ref: payload,
            reuse_identity: None,
            chunks: Vec::new(),
            vectors,
        };
        let bytes = serde_json::to_vec(&artifact)?;
        let artifact_ref = store.insert(
            bytes.clone(),
            artifact_children(&artifact).as_ref().to_vec(),
        )?;
        let member = OwnerMemberBinding {
            instance: spec.instance,
            owner,
            input,
            format: spec.format,
            artifact: artifact_ref,
            logical_commitment: commitment(&bytes),
        };
        let member_ref = store.insert(
            serde_json::to_vec(&member)?,
            ObjectChildren::from_references([Some(artifact_ref), Some(source), None, None])
                .as_slice()
                .to_vec(),
        )?;
        let coverage = OwnerCoverage {
            owner,
            desired: input,
            covered: Some(input),
            searchable: Some(member_ref),
        };
        let coverage_ref = store.insert(
            serde_json::to_vec(&coverage)?,
            ObjectChildren::from_references([Some(member_ref), Some(source), None, None])
                .as_slice()
                .to_vec(),
        )?;
        check!(
            source_rows
                .insert(owner_map_key(owner).to_vec(), source)
                .is_none()
        );
        check!(
            owner_rows
                .insert(owner_map_key(owner).to_vec(), coverage_ref)
                .is_none()
        );
    }
    let sources = MapRoot(Some(store.insert(
        b"multi-vector source map".to_vec(),
        source_rows.values().copied().collect(),
    )?));
    let owners = MapRoot(Some(store.insert(
        b"multi-vector coverage map".to_vec(),
        owner_rows.values().copied().collect(),
    )?));
    let maps = TestMap {
        entries: Vec::new(),
        rooted_entries: vec![
            (sources, source_rows.into_iter().collect()),
            (owners, owner_rows.into_iter().collect()),
        ],
        inline_rooted_entries: Vec::new(),
        fail: false,
    };
    let desired = InputBinding {
        root: sources,
        schema: spec.schema,
        policy: recipe.fingerprint(),
        context: Digest([0; 32]),
    };
    let coverage = CoverageBinding {
        instance: spec.instance,
        desired,
        covered: Some(desired),
        searchable: owners.0,
        status: CoverageStatus::Complete,
        owners,
    };
    let mut work = budget()?;
    let result = search_exact(
        &SearchQuery {
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
            vector: &[1.0, 0.0],
            top_k: 2,
            page_size: 64,
            max_owners: 16,
            overlay: None,
        },
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut work,
        },
    )?;
    check_eq!(result.hits.len(), 2);
    check_eq!(
        result.hits[0].source_file().ok_or("first file")?.owner,
        owner(1)
    );
    check_eq!(result.hits[0].vector_id.as_slice(), b"a");
    check_eq!(
        result.hits[1].source_file().ok_or("second file")?.owner,
        owner(2)
    );
    check!(result.exhaustive);
    Ok(())
}

#[test]
fn source_file_metadata_parts_keep_original_grants_until_drop()
-> Result<(), Box<dyn std::error::Error>> {
    let mut work = budget()?;
    let memory = Arc::new(BoundedSelectionMemory {
        limit: 4_194_304,
        deny_reservation: None,
        live: AtomicU64::new(0),
        peak: AtomicU64::new(0),
        denied: AtomicU64::new(0),
        invalid: AtomicBool::new(false),
    });
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        BoundedSelectionScope(Arc::clone(&memory)),
    ))));
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::new(AtomicUsize::new(0)),
        deadline: work.request().deadline(),
        normalization: EmbeddingNormalization::L2Unit,
    })?);
    let fixture = projected_fixture(&bridge, false)?;
    let reader =
        NativeSemanticReader::new(bridge, fixture.recipe.clone(), semantic_read_config(16))?
            .with_ann();
    let request = SemanticReadRequest::for_source_files(semantic_request("query", 2))?;
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
    let (mut hits, backing) = result.into_result().into_parts();
    let parts = hits
        .pop()
        .ok_or("missing retained file")?
        .into_source_file_parts()?;
    drop(hits);
    drop(backing);
    check!(memory.live.load(AtomicOrdering::SeqCst) > 0);
    check_eq!(parts.source_file.owner, owner(51));
    check_eq!(parts.source_file.path(), "src/a.rs");
    check_eq!(parts.vector_id.as_slice(), b"whole-file");
    let SemanticSourceFileHitParts {
        vector_id,
        vector_id_memory,
        source_file,
        ..
    } = parts;
    let id_bytes = vector_id_memory
        .as_ref()
        .ok_or("vector ID lost its independent retained grant")?
        .bytes();
    check!(id_bytes >= u64::try_from(vector_id.capacity())?);
    // A record grant alone must not conceal a missing vector-ID grant.
    check!(memory.live.load(AtomicOrdering::SeqCst) > id_bytes);
    drop(source_file);
    check_eq!(memory.live.load(AtomicOrdering::SeqCst), id_bytes);
    check_eq!(vector_id.as_slice(), b"whole-file");
    drop(vector_id);
    drop(vector_id_memory);
    check_eq!(memory.live.load(AtomicOrdering::SeqCst), 0);
    check_eq!(memory.denied.load(AtomicOrdering::SeqCst), 0);
    check!(!memory.invalid.load(AtomicOrdering::SeqCst));
    Ok(())
}
