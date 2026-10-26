use super::*;

fn selected_maps(
    source: ObjectRef,
    binding: &CoverageBinding,
    coverage: ObjectRef,
    spec: &InstanceSpec,
    rows: MapEntries,
) -> TestMap {
    TestMap {
        entries: Vec::new(),
        rooted_entries: vec![
            (
                MapRoot(Some(source)),
                rows.iter().map(|(key, _)| (key.clone(), source)).collect(),
            ),
            (binding.owners, rows),
            (
                MapRoot(Some(coverage)),
                vec![(member_map_key(spec.instance).to_vec(), coverage)],
            ),
        ],
        inline_rooted_entries: Vec::new(),
        fail: false,
    }
}

fn selected_inline_value<'a>(
    maps: &'a mut TestMap,
    key: &[u8],
) -> Result<&'a mut MapValue, Box<dyn std::error::Error>> {
    let (_, entries) = maps
        .inline_rooted_entries
        .first_mut()
        .ok_or("selected inline root absent")?;
    let (_, value) = entries
        .iter_mut()
        .find(|(entry, _)| entry.as_slice() == key)
        .ok_or("selected inline key absent")?;
    Ok(value)
}

#[test]
fn native_semantic_reader_binds_source_membership_to_each_selected_view()
-> Result<(), Box<dyn std::error::Error>> {
    let mut work = budget()?;
    let deadline = work.request().deadline();
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::new(AtomicUsize::new(0)),
        deadline,
        normalization: EmbeddingNormalization::L2Unit,
    })?);
    let recipe = bridge.recipe(1024)?;
    let spec = instance(&recipe);
    let reader = NativeSemanticReader::new(bridge, recipe.clone(), semantic_read_config(4))?;
    let store = MemoryStore::default();
    let (old_source, old_payload) = semantic_source(&store, "a.rs", b"old")?;
    let (new_source, new_payload) = semantic_source(&store, "a.rs", b"new")?;
    let old_row = semantic_owner_vector(
        &store,
        &spec,
        &recipe,
        old_source,
        old_payload,
        1,
        vec![1.0, 0.0],
    )?;
    let new_row = semantic_owner_vector(
        &store,
        &spec,
        &recipe,
        new_source,
        new_payload,
        1,
        vec![0.0, 1.0],
    )?;
    let (old_binding, old_coverage) =
        semantic_group_coverage(&store, &spec, old_source, CoverageStatus::Complete)?;
    let (new_binding, new_coverage) =
        semantic_group_coverage(&store, &spec, new_source, CoverageStatus::Complete)?;
    let old_ticket = semantic_ticket(MapRoot(Some(old_source)), MapRoot(Some(old_coverage)), 1)?;
    let new_ticket = semantic_ticket(MapRoot(Some(new_source)), MapRoot(Some(new_coverage)), 2)?;
    let old_maps = selected_maps(
        old_source,
        &old_binding,
        old_coverage,
        &spec,
        vec![old_row.clone()],
    );
    let new_maps = selected_maps(new_source, &new_binding, new_coverage, &spec, vec![new_row]);
    let stale_maps = selected_maps(new_source, &new_binding, new_coverage, &spec, vec![old_row]);
    let request: SemanticReadRequest = semantic_request("query", 1).into();
    let mut run = |ticket: &TicketRecord, maps: &TestMap| {
        reader.query(
            &request,
            &mut QueryReadContext {
                ticket,
                instance: &spec,
                objects: &store,
                maps,
                budget: &mut work,
            },
        )
    };
    check_eq!(
        run(&old_ticket, &old_maps)?
            .result()
            .hits
            .first()
            .ok_or("old hit")?
            .cosine
            .total_cmp(&1.0),
        Ordering::Equal
    );
    check_eq!(
        run(&new_ticket, &new_maps)?
            .result()
            .hits
            .first()
            .ok_or("new hit")?
            .cosine
            .total_cmp(&0.0),
        Ordering::Equal
    );
    // A self-consistent old artifact/member/coverage chain is not membership
    // in the new source cut, even if the outer coverage claims Complete.
    check!(matches!(
        run(&new_ticket, &stale_maps),
        Err(IndexError::CorruptObject(_))
    ));
    let mut deleted_maps = stale_maps;
    let (_, sources) = deleted_maps
        .rooted_entries
        .first_mut()
        .ok_or("source map")?;
    sources.clear();
    check!(matches!(
        run(&new_ticket, &deleted_maps),
        Err(IndexError::CorruptObject(_))
    ));
    // No global-head lookup may invalidate the still-selected old snapshot.
    check_eq!(
        run(&old_ticket, &old_maps)?
            .result()
            .hits
            .first()
            .ok_or("old hit again")?
            .cosine
            .total_cmp(&1.0),
        Ordering::Equal
    );
    Ok(())
}

#[derive(Clone, Copy)]
enum Forgery {
    OwnerReference,
    OwnerMissingSourceEdge,
    OwnerExtraEdge,
    SourceContext,
    MissingSource,
    MemberReference,
    MemberMissingSourceEdge,
    MemberExtraEdge,
}

#[test]
fn native_semantic_pending_reader_requires_previous_missing_gc_edge()
-> Result<(), Box<dyn std::error::Error>> {
    let mut work = budget()?;
    let deadline = work.request().deadline();
    let calls = Arc::new(AtomicUsize::new(0));
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::clone(&calls),
        deadline,
        normalization: EmbeddingNormalization::L2Unit,
    })?);
    let recipe = bridge.recipe(1024)?;
    let spec = instance(&recipe);
    let reader = NativeSemanticReader::new(bridge, recipe.clone(), semantic_read_config(4))?;
    let store = MemoryStore::default();
    let (source, payload) = semantic_source(&store, "a.rs", b"source")?;
    let row = semantic_owner_vector(&store, &spec, &recipe, source, payload, 1, vec![1.0, 0.0])?;
    let previous_missing = store.insert(b"prior missing root".to_vec(), vec![row.1])?;
    let (binding, coverage) = semantic_group_coverage(
        &store,
        &spec,
        source,
        CoverageStatus::Pending {
            previous_missing: MapRoot(Some(previous_missing)),
        },
    )?;
    let ticket = semantic_ticket(MapRoot(Some(source)), MapRoot(Some(coverage)), 1)?;
    let maps = selected_maps(source, &binding, coverage, &spec, vec![row]);
    let request: SemanticReadRequest = semantic_request("query", 1).into();
    let mut run = || {
        reader.query(
            &request,
            &mut QueryReadContext {
                ticket: &ticket,
                instance: &spec,
                objects: &store,
                maps: &maps,
                budget: &mut work,
            },
        )
    };
    let result = run()?;
    check_eq!(result.result().hits.len(), 1);
    check_eq!(
        result.result().traversal,
        NativeSemanticTraversal::ExactExhausted
    );
    check!(!result.exhaustive());
    check_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    {
        let mut objects = store.0.lock().map_err(|_error| "store lock")?;
        let object = objects.get(&coverage).cloned().ok_or("coverage object")?;
        let children = object
            .registration
            .children
            .iter()
            .copied()
            .filter(|child| *child != previous_missing)
            .collect();
        drop(objects.insert(
            coverage,
            StoredObject::new(
                RegisteredObject::unscoped(
                    coverage,
                    u64::try_from(object.payload.len())?,
                    children,
                ),
                object.payload.clone(),
                None,
            ),
        ));
    }
    check!(matches!(run(), Err(IndexError::CorruptObject(_))));
    check_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    Ok(())
}

#[test]
fn native_semantic_reader_rejects_forged_owner_member_authority_before_scoring()
-> Result<(), Box<dyn std::error::Error>> {
    for forgery in [
        Forgery::OwnerReference,
        Forgery::OwnerMissingSourceEdge,
        Forgery::OwnerExtraEdge,
        Forgery::SourceContext,
        Forgery::MissingSource,
        Forgery::MemberReference,
        Forgery::MemberMissingSourceEdge,
        Forgery::MemberExtraEdge,
    ] {
        let mut work = budget()?;
        let deadline = work.request().deadline();
        let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
            calls: Arc::new(AtomicUsize::new(0)),
            deadline,
            normalization: EmbeddingNormalization::L2Unit,
        })?);
        let recipe = bridge.recipe(1024)?;
        let spec = instance(&recipe);
        let reader = NativeSemanticReader::new(bridge, recipe.clone(), semantic_read_config(4))?;
        let store = MemoryStore::default();
        let (source, payload) = semantic_source(&store, "a.rs", b"source")?;
        let row =
            semantic_owner_vector(&store, &spec, &recipe, source, payload, 1, vec![1.0, 0.0])?;
        let (binding, coverage) =
            semantic_group_coverage(&store, &spec, source, CoverageStatus::Complete)?;
        let ticket = semantic_ticket(MapRoot(Some(source)), MapRoot(Some(coverage)), 1)?;
        let maps = selected_maps(source, &binding, coverage, &spec, vec![row.clone()]);
        let request: SemanticReadRequest = semantic_request("query", 1).into();
        // The independent positive control uses the same literal source and
        // edge fixtures, rather than deriving expected failure from a helper.
        let good = reader.query(
            &request,
            &mut QueryReadContext {
                ticket: &ticket,
                instance: &spec,
                objects: &store,
                maps: &maps,
                budget: &mut work,
            },
        )?;
        check_eq!(good.result().hits.len(), 1);
        check_eq!(
            good.result()
                .hits
                .first()
                .ok_or("control hit")?
                .cosine
                .total_cmp(&1.0),
            Ordering::Equal
        );
        let extra = store.insert(b"unrelated edge".to_vec(), vec![])?;
        {
            let mut objects = store.0.lock().map_err(|_error| "store lock")?;
            let owner_object = objects.get(&row.1).cloned().ok_or("owner coverage")?;
            let owner: OwnerCoverage = serde_json::from_slice(&owner_object.payload)?;
            let reference = if matches!(
                forgery,
                Forgery::MemberReference
                    | Forgery::MemberMissingSourceEdge
                    | Forgery::MemberExtraEdge
            ) {
                owner.searchable.ok_or("member")?
            } else {
                row.1
            };
            let object = objects.get(&reference).cloned().ok_or("target object")?;
            let mut returned_reference = reference;
            let mut children = object.registration.children.clone();
            let mut bytes = object.payload.clone();
            match forgery {
                Forgery::OwnerReference | Forgery::MemberReference => returned_reference = extra,
                Forgery::OwnerMissingSourceEdge | Forgery::MemberMissingSourceEdge => {
                    children.retain(|child| *child != source);
                }
                Forgery::OwnerExtraEdge | Forgery::MemberExtraEdge => {
                    children.push(extra);
                    children.sort_unstable();
                }
                Forgery::SourceContext | Forgery::MissingSource => {
                    let mut changed = owner;
                    if matches!(forgery, Forgery::SourceContext) {
                        changed.desired.producer_context = Digest([0x71; 32]);
                    } else {
                        changed.desired.source = None;
                        children.retain(|child| *child != source);
                    }
                    changed.covered = Some(changed.desired);
                    bytes = serde_json::to_vec(&changed)?;
                }
            }
            drop(objects.insert(
                reference,
                StoredObject::new(
                    RegisteredObject::unscoped(
                        returned_reference,
                        u64::try_from(bytes.len())?,
                        children,
                    ),
                    bytes,
                    None,
                ),
            ));
        }
        let mutations_before = work.usage().mutations;
        check!(matches!(
            reader.query(
                &request,
                &mut QueryReadContext {
                    ticket: &ticket,
                    instance: &spec,
                    objects: &store,
                    maps: &maps,
                    budget: &mut work,
                }
            ),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(work.usage().mutations, mutations_before);
    }
    Ok(())
}

#[test]
fn native_semantic_exact_path_filters_before_top_k_and_authenticates_source()
-> Result<(), Box<dyn std::error::Error>> {
    let mut work = budget()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::clone(&calls),
        deadline: work.request().deadline(),
        normalization: EmbeddingNormalization::L2Unit,
    })?);
    let recipe = bridge.recipe(1024)?;
    let spec = instance(&recipe);
    let reader = NativeSemanticReader::new(bridge, recipe.clone(), semantic_read_config(4))?;
    let store = MemoryStore::default();
    let (other, other_payload) = semantic_source(&store, "other.rs", b"closest")?;
    let (wanted, wanted_payload) = semantic_source(&store, "wanted.rs", b"eligible")?;
    let rows = vec![
        semantic_owner_vector(
            &store,
            &spec,
            &recipe,
            other,
            other_payload,
            1,
            vec![1.0, 0.0],
        )?,
        semantic_owner_vector(
            &store,
            &spec,
            &recipe,
            wanted,
            wanted_payload,
            2,
            vec![0.0, 1.0],
        )?,
    ];
    let (binding, coverage) =
        semantic_group_coverage(&store, &spec, other, CoverageStatus::Complete)?;
    let mut ticket = semantic_ticket(MapRoot(Some(other)), MapRoot(Some(coverage)), 1)?;
    let path_root = MapRoot(Some(
        store.insert(b"selected source paths".to_vec(), vec![])?,
    ));
    ticket.view.source.paths = path_root;
    ticket.view.identity.digest = ticket.view.identity_digest()?;
    let mut maps = selected_maps(other, &binding, coverage, &spec, rows);
    maps.rooted_entries.first_mut().ok_or("source root")?.1 = vec![
        (owner_map_key(owner(1)).to_vec(), other),
        (owner_map_key(owner(2)).to_vec(), wanted),
    ];
    maps.inline_rooted_entries.push((
        path_root,
        vec![
            (
                b"path\0other.rs".to_vec(),
                quanta_index_core::index::path_owner_map_value(owner(1))?,
            ),
            (
                b"path\0wanted.rs".to_vec(),
                quanta_index_core::index::path_owner_map_value(owner(2))?,
            ),
        ],
    ));
    let wanted_path = quanta_index_contract::ExactRepoRelativePathV1::new("wanted.rs")?;
    let missing_path = quanta_index_contract::ExactRepoRelativePathV1::new("missing.rs")?;
    let mut run =
        |ticket: &TicketRecord,
         maps: &TestMap,
         path: Option<&quanta_index_contract::ExactRepoRelativePathV1>| {
            let mut query = semantic_request("query", 1);
            query.constraints.repo_relative_path_exact = path.cloned();
            reader.query(
                &query.into(),
                &mut QueryReadContext {
                    ticket,
                    instance: &spec,
                    objects: &store,
                    maps,
                    budget: &mut work,
                },
            )
        };
    check_eq!(
        run(&ticket, &maps, None)?
            .result()
            .hits
            .first()
            .ok_or("unconstrained hit")?
            .owner,
        owner(1)
    );
    let filtered = run(&ticket, &maps, Some(&wanted_path))?;
    check_eq!(filtered.result().hits.len(), 1);
    check_eq!(
        filtered.result().hits.first().ok_or("filtered hit")?.owner,
        owner(2)
    );
    check!(filtered.exhaustive());
    let missing = run(&ticket, &maps, Some(&missing_path))?;
    check!(missing.result().hits.is_empty());
    check_eq!(missing.result().scanned_owners, 0);
    let no_paths_ticket = semantic_ticket(MapRoot(Some(other)), MapRoot(Some(coverage)), 1)?;
    check!(matches!(
        run(&no_paths_ticket, &maps, Some(&wanted_path)),
        Err(IndexError::CorruptObject(_))
    ));
    *selected_inline_value(&mut maps, b"path\0wanted.rs")? =
        quanta_index_core::index::path_owner_map_value(owner(1))?;
    check!(matches!(
        run(&ticket, &maps, Some(&wanted_path)),
        Err(IndexError::CorruptObject(_))
    ));
    *selected_inline_value(&mut maps, b"path\0wanted.rs")? = MapValue::Object(wanted);
    check!(matches!(
        run(&ticket, &maps, Some(&wanted_path)),
        Err(IndexError::CorruptObject(_))
    ));
    let mut owner_bytes = [0u8; 32];
    owner_bytes
        .get_mut(..16)
        .ok_or("owner producer range")?
        .copy_from_slice(&owner(2).producer.0);
    owner_bytes
        .get_mut(16..)
        .ok_or("owner stable ID range")?
        .copy_from_slice(&owner(2).stable_id.0);
    *selected_inline_value(&mut maps, b"path\0wanted.rs")? =
        MapValue::inline(&owner_bytes, Some(other))?;
    check!(matches!(
        run(&ticket, &maps, Some(&wanted_path)),
        Err(IndexError::CorruptObject(_))
    ));
    *selected_inline_value(&mut maps, b"path\0wanted.rs")? =
        quanta_index_core::index::path_owner_map_value(owner(2))?;
    // A queried certified path must retain its physical SourceRecord edge.
    {
        let mut objects = store.0.lock().map_err(|_error| "store lock")?;
        let object = objects.get(&wanted).cloned().ok_or("source record")?;
        drop(objects.insert(
            wanted,
            StoredObject::new(
                RegisteredObject::unscoped(wanted, u64::try_from(object.payload.len())?, vec![]),
                object.payload.clone(),
                None,
            ),
        ));
    }
    check!(matches!(
        run(&ticket, &maps, Some(&wanted_path)),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(calls.load(AtomicOrdering::SeqCst), 7);
    Ok(())
}

#[test]
fn native_semantic_projected_exact_path_uses_selected_source_owner()
-> Result<(), Box<dyn std::error::Error>> {
    let mut work = budget()?;
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::new(AtomicUsize::new(0)),
        deadline: work.request().deadline(),
        normalization: EmbeddingNormalization::L2Unit,
    })?);
    let projection = ProjectionId([31; 16]);
    let producer = ProducerId([32; 16]);
    let schema = SchemaId([33; 16]);
    let recipe = bridge.projection_recipe(projection, producer, schema, 1024)?;
    let mut spec = instance(&recipe);
    spec.input_domain = InputDomain::Projection {
        projection,
        producer,
    };
    spec.schema = schema;
    let reader = NativeSemanticReader::new(bridge, recipe.clone(), semantic_read_config(4))?;
    let store = MemoryStore::default();
    let semantic_owner = owner(41);
    let source_owner = owner(51);
    check_ne!(semantic_owner, source_owner);
    let (source, _) = semantic_source(&store, "src/wanted.rs", b"source text")?;
    let (forged_source, _) = semantic_source(&store, "src/forged.rs", b"other text")?;
    let output = store.insert(b"projection text".to_vec(), vec![])?;
    let contribution = ProjectionOwnerContribution {
        projection,
        producer,
        owner: semantic_owner,
        source_owner: Some(source_owner),
        source: Some(source),
        dependency_stamp: Digest([34; 32]),
        output: Some(output),
    };
    let contribution_ref = store.insert(
        serde_json::to_vec(&contribution)?,
        contribution.object_children(),
    )?;
    let projection_owners = MapRoot(Some(store.insert(
        b"selected projection owner root".to_vec(),
        vec![contribution_ref],
    )?));
    let source_cut = SourceCut {
        owners: MapRoot(Some(source)),
        paths: MapRoot(None),
        frontiers: MapRoot(None),
    };
    let certificate = store.insert(b"selected producer certificate".to_vec(), vec![])?;
    let binding = ProjectionBinding {
        projection,
        producer,
        owners: projection_owners,
        source: source_cut,
        schema,
        producer_policy: Digest([35; 32]),
        dependency_context: Digest([36; 32]),
        certificate,
    };
    let binding_ref = store.insert(serde_json::to_vec(&binding)?, binding.object_children())?;
    let projection_root = MapRoot(Some(store.insert(
        b"selected projection binding root".to_vec(),
        vec![binding_ref],
    )?));
    let input = OwnerInputBinding {
        source: Some(source),
        projection: Some(contribution_ref),
        producer_context: contribution.materialization_context(),
        derivation_policy: recipe.fingerprint(),
    };
    let artifact = OwnerArtifact {
        owner: semantic_owner,
        input,
        recipe: recipe.fingerprint(),
        payload_ref: output,
        reuse_identity: None,
        chunks: Vec::new(),
        vectors: vec![EmbeddingVector {
            id: b"whole-file".to_vec(),
            values: vec![1.0, 0.0],
        }],
    };
    let artifact_bytes = serde_json::to_vec(&artifact)?;
    let artifact_ref = store.insert(
        artifact_bytes.clone(),
        artifact_children(&artifact).as_ref().to_vec(),
    )?;
    let member = OwnerMemberBinding {
        instance: spec.instance,
        owner: semantic_owner,
        input,
        format: spec.format,
        artifact: artifact_ref,
        logical_commitment: commitment(&artifact_bytes),
    };
    let member_ref = store.insert(
        serde_json::to_vec(&member)?,
        ObjectChildren::from_references([
            Some(artifact_ref),
            Some(source),
            Some(contribution_ref),
            None,
        ])
        .as_slice()
        .to_vec(),
    )?;
    let owner_coverage = OwnerCoverage {
        owner: semantic_owner,
        desired: input,
        covered: Some(input),
        searchable: Some(member_ref),
    };
    let owner_coverage_ref = store.insert(
        serde_json::to_vec(&owner_coverage)?,
        ObjectChildren::from_references([
            Some(source),
            Some(contribution_ref),
            Some(member_ref),
            None,
        ])
        .as_slice()
        .to_vec(),
    )?;
    let semantic_owners = MapRoot(Some(store.insert(
        b"selected semantic owner root".to_vec(),
        vec![owner_coverage_ref],
    )?));
    let desired = InputBinding {
        root: projection_owners,
        schema,
        policy: spec.policy,
        context: binding.materialization_context(),
    };
    let coverage = CoverageBinding {
        instance: spec.instance,
        desired,
        covered: Some(desired),
        searchable: semantic_owners.0,
        status: CoverageStatus::Complete,
        owners: semantic_owners,
    };
    let coverage_ref = store.insert(
        serde_json::to_vec(&coverage)?,
        ObjectChildren::from_references([
            projection_owners.0,
            semantic_owners.0,
            coverage.searchable,
            None,
        ])
        .as_slice()
        .to_vec(),
    )?;
    let mut ticket = semantic_ticket(source_cut.owners, MapRoot(Some(coverage_ref)), 1)?;
    ticket.view.projections = projection_root;
    ticket.view.identity.digest = ticket.view.identity_digest()?;
    let semantic_key = owner_map_key(semantic_owner).to_vec();
    let mut maps = selected_maps(
        source,
        &coverage,
        coverage_ref,
        &spec,
        vec![(semantic_key.clone(), owner_coverage_ref)],
    );
    maps.rooted_entries.first_mut().ok_or("source owner map")?.1 =
        vec![(owner_map_key(source_owner).to_vec(), source)];
    maps.rooted_entries
        .push((projection_owners, vec![(semantic_key, contribution_ref)]));
    let mut projection_key = b"projection\0".to_vec();
    projection_key.extend_from_slice(&projection.0);
    maps.rooted_entries
        .push((projection_root, vec![(projection_key, binding_ref)]));

    let wanted = quanta_index_contract::ExactRepoRelativePathV1::new("src/wanted.rs")?;
    let missing = quanta_index_contract::ExactRepoRelativePathV1::new("src/missing.rs")?;
    let mut run = |maps: &TestMap, path: &quanta_index_contract::ExactRepoRelativePathV1| {
        let mut query = semantic_request("query", 1);
        query.constraints.repo_relative_path_exact = Some(path.clone());
        reader.query(
            &query.into(),
            &mut QueryReadContext {
                ticket: &ticket,
                instance: &spec,
                objects: &store,
                maps,
                budget: &mut work,
            },
        )
    };
    let matched = run(&maps, &wanted)?;
    check_eq!(matched.result().hits.len(), 1);
    check_eq!(
        matched
            .result()
            .hits
            .first()
            .ok_or("projected path hit")?
            .owner,
        semantic_owner
    );
    drop(matched);
    check!(run(&maps, &missing)?.result().hits.is_empty());
    // Keep this exact ticket while removing and forging its canonical source
    // owner membership. Neither case may silently become an empty path result.
    maps.rooted_entries
        .first_mut()
        .ok_or("source owner map")?
        .1
        .clear();
    check!(matches!(
        run(&maps, &missing),
        Err(IndexError::CorruptObject(_))
    ));
    maps.rooted_entries.first_mut().ok_or("source owner map")?.1 =
        vec![(owner_map_key(source_owner).to_vec(), forged_source)];
    check!(matches!(
        run(&maps, &wanted),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn native_semantic_exact_path_refuses_unlocated_live_overlay_before_provider()
-> Result<(), Box<dyn std::error::Error>> {
    let work = budget()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::clone(&calls),
        deadline: work.request().deadline(),
        normalization: EmbeddingNormalization::L2Unit,
    })?);
    let recipe = bridge.recipe(1024)?;
    let reader = NativeSemanticReader::new(bridge, recipe, semantic_read_config(4))?;
    let mut query = semantic_request("query", 1);
    query.constraints = QueryConstraintSetV1::from_exact_repo_relative_path(
        quanta_index_contract::ExactRepoRelativePathV1::new("wanted.rs")?,
    );
    let mut request = SemanticReadRequest {
        lexical_eligibility: None,
        query,
        overlay: vec![SemanticOverlayDocument {
            owner: owner(1),
            revision: 1,
            path: None,
            text: Some(Arc::<str>::from("new")),
        }],
        overlay_memory: None,
    };
    check!(matches!(
        reader.reservation(&request),
        Err(IndexError::SnapshotUnavailable)
    ));
    request.overlay.first_mut().ok_or("overlay row")?.path = Some(Arc::from("wanted.rs"));
    check!(reader.reservation(&request).is_ok());
    // A tombstone only masks a durable owner and needs no candidate path.
    let row = request.overlay.first_mut().ok_or("overlay row")?;
    row.text = None;
    row.path = None;
    check!(reader.reservation(&request).is_ok());
    check_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    Ok(())
}
