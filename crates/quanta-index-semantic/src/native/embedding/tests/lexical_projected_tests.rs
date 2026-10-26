//! Selected projection membership through the source reverse layout.

use super::*;
use quanta_index_core::index::NativeTextHitSource;

struct BoundedSelectionMemory {
    limit: u64,
    deny_reservation: Option<u64>,
    live: AtomicU64,
    peak: AtomicU64,
    denied: AtomicU64,
    invalid: AtomicBool,
}

impl quanta_index_core::index::MemoryLeaseOwner for BoundedSelectionMemory {
    fn release(&self, bytes: u64) {
        if self
            .live
            .fetch_update(AtomicOrdering::SeqCst, AtomicOrdering::SeqCst, |live| {
                live.checked_sub(bytes)
            })
            .is_err()
        {
            self.invalid.store(true, AtomicOrdering::SeqCst);
        }
    }
}

struct BoundedSelectionScope(Arc<BoundedSelectionMemory>);

impl quanta_index_core::index::MemoryScopePort for BoundedSelectionScope {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        if self.0.deny_reservation == Some(bytes) {
            let _previous_denied = self.0.denied.fetch_add(1, AtomicOrdering::SeqCst);
            return Err(IndexError::ResourceLimited);
        }
        let prior = self
            .0
            .live
            .fetch_update(AtomicOrdering::SeqCst, AtomicOrdering::SeqCst, |live| {
                live.checked_add(bytes).filter(|next| *next <= self.0.limit)
            })
            .map_err(|_live| IndexError::ResourceLimited)?;
        let next = prior
            .checked_add(bytes)
            .ok_or(IndexError::ResourceLimited)?;
        let _previous_peak = self.0.peak.fetch_max(next, AtomicOrdering::SeqCst);
        Ok(MemoryLease::new(bytes, self.0.clone()))
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "test fixture keeps independent projection, source and semantic identities explicit"
)]
fn projected_owner_row(
    store: &MemoryStore,
    spec: &InstanceSpec,
    recipe: &EmbeddingRecipe,
    projection: ProjectionId,
    producer: ProducerId,
    semantic_owner: OwnerKey,
    source_owner: OwnerKey,
    source: ObjectRef,
    values: Vec<f32>,
    empty_output: bool,
) -> Result<(ObjectRef, ObjectRef), Box<dyn std::error::Error>> {
    let output = store.insert(b"selected projection text".to_vec(), vec![])?;
    let contribution = ProjectionOwnerContribution {
        projection,
        producer,
        owner: semantic_owner,
        source_owner: Some(source_owner),
        source: Some(source),
        dependency_stamp: Digest([34; 32]),
        output: (!empty_output).then_some(output),
    };
    let contribution_ref = store.insert(
        serde_json::to_vec(&contribution)?,
        contribution.object_children(),
    )?;
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
            values,
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
        searchable: (!empty_output).then_some(member_ref),
    };
    let coverage_ref = store.insert(
        serde_json::to_vec(&owner_coverage)?,
        ObjectChildren::from_references([
            Some(source),
            Some(contribution_ref),
            (!empty_output).then_some(member_ref),
            None,
        ])
        .as_slice()
        .to_vec(),
    )?;
    Ok((contribution_ref, coverage_ref))
}

struct ProjectedFixture {
    store: MemoryStore,
    ticket: TicketRecord,
    maps: TestMap,
    spec: InstanceSpec,
    recipe: EmbeddingRecipe,
    source_a: ObjectRef,
    source_owner_a: OwnerKey,
    reverse_root: MapRoot,
    directory_ref: ObjectRef,
}

fn projected_fixture(
    bridge: &TextEmbeddingBridge<FixedTextProvider>,
    empty_second: bool,
) -> Result<ProjectedFixture, Box<dyn std::error::Error>> {
    projected_fixture_with_shared(bridge, empty_second, 2)
}

fn projected_fixture_with_shared(
    bridge: &TextEmbeddingBridge<FixedTextProvider>,
    empty_second: bool,
    shared_count: u8,
) -> Result<ProjectedFixture, Box<dyn std::error::Error>> {
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
    let store = MemoryStore::default();
    let source_owner_a = owner(51);
    let source_owner_b = owner(52);
    let (source_a, _) = semantic_source(&store, "src/a.rs", b"selected text")?;
    let (source_b, _) = semantic_source(&store, "src/b.rs", b"nearest excluded")?;
    let mut rows = Vec::new();
    for index in 0..shared_count {
        let semantic_owner = owner(41_u8.checked_add(index).ok_or("shared owner overflow")?);
        rows.push((
            semantic_owner,
            source_owner_a,
            source_a,
            vec![0.0, 1.0],
            index == 1 && empty_second,
        ));
    }
    rows.push((owner(200), source_owner_b, source_b, vec![1.0, 0.0], false));
    let mut projection_rows = Vec::new();
    let mut semantic_rows = Vec::new();
    let mut reverse_rows = Vec::new();
    for (semantic_owner, source_owner, source, vector, empty_output) in rows {
        let (contribution, coverage) = projected_owner_row(
            &store,
            &spec,
            &recipe,
            projection,
            producer,
            semantic_owner,
            source_owner,
            source,
            vector,
            empty_output,
        )?;
        projection_rows.push((owner_map_key(semantic_owner).to_vec(), contribution));
        semantic_rows.push((owner_map_key(semantic_owner).to_vec(), coverage));
        let mut key = owner_map_key(source_owner).to_vec();
        key.extend_from_slice(&owner_map_key(semantic_owner));
        reverse_rows.push((key, coverage));
    }
    reverse_rows.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    let projection_owners = MapRoot(Some(
        store.insert(
            b"selected projection owners".to_vec(),
            projection_rows
                .iter()
                .map(|(_, reference)| *reference)
                .collect(),
        )?,
    ));
    let semantic_owners = MapRoot(Some(
        store.insert(
            b"selected semantic owners".to_vec(),
            semantic_rows
                .iter()
                .map(|(_, reference)| *reference)
                .collect(),
        )?,
    ));
    let reverse_root = MapRoot(Some(
        store.insert(
            b"selected source reverse".to_vec(),
            reverse_rows
                .iter()
                .map(|(_, reference)| *reference)
                .collect(),
        )?,
    ));
    let source_owners = MapRoot(Some(
        store.insert(b"selected source owners".to_vec(), vec![source_a, source_b])?,
    ));
    let source_paths = MapRoot(Some(
        store.insert(b"selected source paths".to_vec(), vec![source_a, source_b])?,
    ));
    let source_cut = SourceCut {
        owners: source_owners,
        paths: source_paths,
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
    let projection_root = MapRoot(Some(
        store.insert(b"selected projection binding".to_vec(), vec![binding_ref])?,
    ));
    let mut directory = ann_directory::AnnDirectory::empty(recipe.fingerprint(), semantic_owners);
    directory.source_reverse = reverse_root;
    let directory_ref = store.insert(
        serde_json::to_vec(&directory)?,
        ObjectChildren::from_references([
            semantic_owners.0,
            directory.regions.0,
            directory.delta.0,
            reverse_root.0,
        ])
        .as_slice()
        .to_vec(),
    )?;
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
        searchable: Some(directory_ref),
        status: CoverageStatus::Complete,
        owners: semantic_owners,
    };
    let coverage_ref = store.insert(
        serde_json::to_vec(&coverage)?,
        ObjectChildren::from_references([
            projection_owners.0,
            semantic_owners.0,
            None,
            Some(directory_ref),
        ])
        .as_slice()
        .to_vec(),
    )?;
    let mut ticket = semantic_ticket(source_owners, MapRoot(Some(coverage_ref)), 1)?;
    ticket.view.source = source_cut;
    ticket.view.projections = projection_root;
    ticket.view.identity.digest = ticket.view.identity_digest()?;
    let mut projection_key = b"projection\0".to_vec();
    projection_key.extend_from_slice(&projection.0);
    let maps = TestMap {
        entries: Vec::new(),
        rooted_entries: vec![
            (
                source_owners,
                vec![
                    (owner_map_key(source_owner_a).to_vec(), source_a),
                    (owner_map_key(source_owner_b).to_vec(), source_b),
                ],
            ),
            (projection_owners, projection_rows),
            (semantic_owners, semantic_rows),
            (reverse_root, reverse_rows),
            (projection_root, vec![(projection_key, binding_ref)]),
            (
                ticket.view.coverage,
                vec![(member_map_key(spec.instance).to_vec(), coverage_ref)],
            ),
        ],
        inline_rooted_entries: vec![(
            source_paths,
            vec![
                (
                    b"path\0src/a.rs".to_vec(),
                    quanta_index_core::index::path_owner_map_value(source_owner_a)?,
                ),
                (
                    b"path\0src/b.rs".to_vec(),
                    quanta_index_core::index::path_owner_map_value(source_owner_b)?,
                ),
            ],
        )],
        fail: false,
    };
    Ok(ProjectedFixture {
        store,
        ticket,
        maps,
        spec,
        recipe,
        source_a,
        source_owner_a,
        reverse_root,
        directory_ref,
    })
}

#[test]
fn projected_lexical_reverse_selects_shared_source_members_and_refuses_forgery()
-> Result<(), Box<dyn std::error::Error>> {
    let mut work = budget()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::clone(&calls),
        deadline: work.request().deadline(),
        normalization: EmbeddingNormalization::L2Unit,
    })?);
    let fixture = projected_fixture(&bridge, false)?;
    let reader = NativeSemanticReader::new(
        Arc::clone(&bridge),
        fixture.recipe.clone(),
        semantic_read_config(2),
    )?
    .with_ann(Arc::new(test_runtime()?));
    let entry = SemanticLexicalEligibleSource {
        owner: fixture.source_owner_a,
        source: NativeTextHitSource::Durable(fixture.source_a),
    };
    let view = fixture.ticket.view.identity;
    let run = |request: &SemanticReadRequest, maps: &TestMap, work: &mut WorkBudget| {
        reader.query(
            request,
            &mut QueryReadContext {
                ticket: &fixture.ticket,
                instance: &fixture.spec,
                objects: &fixture.store,
                maps,
                budget: work,
            },
        )
    };
    let mut scoped = super::lexical_scope_tests::scoped_request(view, vec![entry], 1)?;
    scoped.query.top_k = 2;
    let result = run(&scoped, &fixture.maps, &mut work)?;
    check_eq!(result.result().hits.len(), 2);
    check_eq!(
        result
            .result()
            .hits
            .first()
            .ok_or("first lexical hit")?
            .owner,
        owner(41)
    );
    check_eq!(
        result
            .result()
            .hits
            .get(1)
            .ok_or("second lexical hit")?
            .owner,
        owner(42)
    );
    check_eq!(result.result().scanned_owners, 2);
    check!(result.exhaustive());
    drop(result);
    check_eq!(calls.load(AtomicOrdering::SeqCst), 1);

    // The direct path caller uses the same source-to-projection selection,
    // without a synthetic lexical eligibility carrier.
    let mut exact = SemanticReadRequest::from(semantic_request("query", 2));
    exact.query.constraints.repo_relative_path_exact = Some(
        quanta_index_contract::ExactRepoRelativePathV1::new("src/a.rs")?,
    );
    let direct = run(&exact, &fixture.maps, &mut work)?;
    check_eq!(direct.result().hits.len(), 2);
    check_eq!(
        direct
            .result()
            .hits
            .first()
            .ok_or("first direct hit")?
            .owner,
        owner(41)
    );
    check_eq!(
        direct
            .result()
            .hits
            .get(1)
            .ok_or("second direct hit")?
            .owner,
        owner(42)
    );
    drop(direct);
    check_eq!(calls.load(AtomicOrdering::SeqCst), 2);

    let capped_reader =
        NativeSemanticReader::new(bridge, fixture.recipe.clone(), semantic_read_config(1))?
            .with_ann(Arc::new(test_runtime()?));
    let capped = capped_reader.query(
        &scoped,
        &mut QueryReadContext {
            ticket: &fixture.ticket,
            instance: &fixture.spec,
            objects: &fixture.store,
            maps: &fixture.maps,
            budget: &mut work,
        },
    )?;
    check_eq!(capped.result().scanned_owners, 1);
    check_eq!(
        capped.result().traversal,
        NativeSemanticTraversal::ExactCapped
    );
    check!(!capped.exhaustive());
    drop(capped);
    check_eq!(calls.load(AtomicOrdering::SeqCst), 3);

    let mut forged = TestMap {
        entries: fixture.maps.entries.clone(),
        rooted_entries: fixture.maps.rooted_entries.clone(),
        inline_rooted_entries: fixture.maps.inline_rooted_entries.clone(),
        fail: false,
    };
    let original_reverse_value = fixture
        .maps
        .rooted_entries
        .iter()
        .find(|(root, _)| *root == fixture.reverse_root)
        .ok_or("original reverse map root")?
        .1
        .first()
        .ok_or("original reverse row")?
        .1;
    let (_, reverse_rows) = forged
        .rooted_entries
        .iter_mut()
        .find(|(root, _)| *root == fixture.reverse_root)
        .ok_or("reverse map root")?;
    reverse_rows.first_mut().ok_or("reverse row")?.1 = fixture.directory_ref;
    check!(matches!(
        run(&scoped, &forged, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(calls.load(AtomicOrdering::SeqCst), 3);
    let (_, reverse_rows) = forged
        .rooted_entries
        .iter_mut()
        .find(|(root, _)| *root == fixture.reverse_root)
        .ok_or("reverse map root")?;
    reverse_rows.first_mut().ok_or("reverse row")?.1 = original_reverse_value;
    let _removed = reverse_rows
        .first_mut()
        .ok_or("reverse row")?
        .0
        .pop()
        .ok_or("reverse key byte")?;
    check!(matches!(
        run(&scoped, &forged, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(calls.load(AtomicOrdering::SeqCst), 3);
    let original_reverse_key = fixture
        .maps
        .rooted_entries
        .iter()
        .find(|(root, _)| *root == fixture.reverse_root)
        .ok_or("original reverse map root")?
        .1
        .first()
        .ok_or("original reverse row")?
        .0
        .clone();
    let (_, reverse_rows) = forged
        .rooted_entries
        .iter_mut()
        .find(|(root, _)| *root == fixture.reverse_root)
        .ok_or("reverse map root")?;
    reverse_rows.first_mut().ok_or("reverse row")?.0 = original_reverse_key;
    let (_, source_rows) = forged
        .rooted_entries
        .iter_mut()
        .find(|(root, _)| *root == fixture.ticket.view.source.owners)
        .ok_or("source owner root")?;
    drop(source_rows.remove(0));
    check!(matches!(
        run(&scoped, &forged, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(calls.load(AtomicOrdering::SeqCst), 3);
    let (_, source_rows) = forged
        .rooted_entries
        .iter_mut()
        .find(|(root, _)| *root == fixture.ticket.view.source.owners)
        .ok_or("source owner root")?;
    source_rows.insert(
        0,
        (
            owner_map_key(fixture.source_owner_a).to_vec(),
            fixture.source_a,
        ),
    );
    source_rows.first_mut().ok_or("source owner row")?.1 = fixture.directory_ref;
    check!(matches!(
        run(&scoped, &forged, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(calls.load(AtomicOrdering::SeqCst), 3);
    {
        let mut objects = fixture.store.0.lock().map_err(|_error| "store lock")?;
        let object = objects
            .get(&fixture.directory_ref)
            .cloned()
            .ok_or("directory object")?;
        drop(objects.insert(
            fixture.directory_ref,
            StoredObject::new(
                RegisteredObject::unscoped(
                    fixture.directory_ref,
                    u64::try_from(object.payload.len())?,
                    vec![],
                ),
                object.payload.clone(),
                None,
            ),
        ));
    }
    check!(matches!(
        run(&scoped, &fixture.maps, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(calls.load(AtomicOrdering::SeqCst), 3);
    Ok(())
}

#[test]
fn projected_empty_coverage_authenticates_text_source_without_embedding()
-> Result<(), Box<dyn std::error::Error>> {
    let mut work = budget()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::clone(&calls),
        deadline: work.request().deadline(),
        normalization: EmbeddingNormalization::L2Unit,
    })?);
    let fixture = projected_fixture(&bridge, false)?;
    let projection = ProjectionId([31; 16]);
    let producer = ProducerId([32; 16]);
    let schema = SchemaId([33; 16]);
    let certificate = fixture
        .store
        .insert(b"empty producer certificate".to_vec(), vec![])?;
    let binding = ProjectionBinding {
        projection,
        producer,
        owners: MapRoot(None),
        source: fixture.ticket.view.source,
        schema,
        producer_policy: Digest([35; 32]),
        dependency_context: Digest([36; 32]),
        certificate,
    };
    let binding_ref = fixture
        .store
        .insert(serde_json::to_vec(&binding)?, binding.object_children())?;
    let projection_root = MapRoot(Some(
        fixture
            .store
            .insert(b"empty projection binding".to_vec(), vec![binding_ref])?,
    ));
    let desired = InputBinding {
        root: MapRoot(None),
        schema,
        policy: fixture.spec.policy,
        context: binding.materialization_context(),
    };
    let coverage = CoverageBinding {
        instance: fixture.spec.instance,
        desired,
        covered: Some(desired),
        searchable: None,
        status: CoverageStatus::Complete,
        owners: MapRoot(None),
    };
    let coverage_ref = fixture
        .store
        .insert(serde_json::to_vec(&coverage)?, vec![])?;
    let coverage_root = MapRoot(Some(
        fixture
            .store
            .insert(b"empty semantic coverage".to_vec(), vec![coverage_ref])?,
    ));
    let mut ticket = semantic_ticket(fixture.ticket.view.source.owners, coverage_root, 2)?;
    ticket.view.source = fixture.ticket.view.source;
    ticket.view.projections = projection_root;
    ticket.view.identity.digest = ticket.view.identity_digest()?;
    let mut projection_key = b"projection\0".to_vec();
    projection_key.extend_from_slice(&projection.0);
    let mut maps = TestMap {
        entries: fixture.maps.entries.clone(),
        rooted_entries: fixture.maps.rooted_entries.clone(),
        inline_rooted_entries: fixture.maps.inline_rooted_entries.clone(),
        fail: false,
    };
    maps.rooted_entries
        .push((projection_root, vec![(projection_key, binding_ref)]));
    maps.rooted_entries.push((
        coverage_root,
        vec![(member_map_key(fixture.spec.instance).to_vec(), coverage_ref)],
    ));
    let reader =
        NativeSemanticReader::new(bridge, fixture.recipe.clone(), semantic_read_config(2))?
            .with_ann(Arc::new(test_runtime()?));
    let entry = SemanticLexicalEligibleSource {
        owner: fixture.source_owner_a,
        source: NativeTextHitSource::Durable(fixture.source_a),
    };
    let request = super::lexical_scope_tests::scoped_request(ticket.view.identity, vec![entry], 1)?;
    let run = |maps: &TestMap, work: &mut WorkBudget| {
        reader.query(
            &request,
            &mut QueryReadContext {
                ticket: &ticket,
                instance: &fixture.spec,
                objects: &fixture.store,
                maps,
                budget: work,
            },
        )
    };
    let empty = run(&maps, &mut work)?;
    check!(empty.result().hits.is_empty());
    check!(empty.exhaustive());
    drop(empty);
    check_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    let (_, source_rows) = maps
        .rooted_entries
        .iter_mut()
        .find(|(root, _)| *root == ticket.view.source.owners)
        .ok_or("source owner root")?;
    source_rows.first_mut().ok_or("source owner row")?.1 = fixture.directory_ref;
    check!(matches!(
        run(&maps, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    Ok(())
}

#[test]
fn current_empty_projection_output_is_exhaustive_for_lexical_and_direct_path()
-> Result<(), Box<dyn std::error::Error>> {
    let mut work = budget()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::clone(&calls),
        deadline: work.request().deadline(),
        normalization: EmbeddingNormalization::L2Unit,
    })?);
    let fixture = projected_fixture(&bridge, true)?;
    let reader =
        NativeSemanticReader::new(bridge, fixture.recipe.clone(), semantic_read_config(3))?
            .with_ann(Arc::new(test_runtime()?));
    let entry = SemanticLexicalEligibleSource {
        owner: fixture.source_owner_a,
        source: NativeTextHitSource::Durable(fixture.source_a),
    };
    let mut scoped =
        super::lexical_scope_tests::scoped_request(fixture.ticket.view.identity, vec![entry], 1)?;
    scoped.query.top_k = 2;
    let run = |request: &SemanticReadRequest, work: &mut WorkBudget| {
        reader.query(
            request,
            &mut QueryReadContext {
                ticket: &fixture.ticket,
                instance: &fixture.spec,
                objects: &fixture.store,
                maps: &fixture.maps,
                budget: work,
            },
        )
    };
    let lexical = run(&scoped, &mut work)?;
    check_eq!(lexical.result().hits.len(), 1);
    check_eq!(
        lexical.result().hits.first().ok_or("lexical hit")?.owner,
        owner(41)
    );
    check_eq!(lexical.result().scanned_owners, 2);
    check_eq!(lexical.result().skipped_stale, 0);
    check!(lexical.exhaustive());
    drop(lexical);
    let mut direct = SemanticReadRequest::from(semantic_request("query", 2));
    direct.query.constraints.repo_relative_path_exact = Some(
        quanta_index_contract::ExactRepoRelativePathV1::new("src/a.rs")?,
    );
    let exact = run(&direct, &mut work)?;
    check_eq!(exact.result().hits.len(), 1);
    check_eq!(
        exact.result().hits.first().ok_or("direct hit")?.owner,
        owner(41)
    );
    check_eq!(exact.result().scanned_owners, 2);
    check_eq!(exact.result().skipped_stale, 0);
    check!(exact.exhaustive());
    drop(exact);
    check_eq!(calls.load(AtomicOrdering::SeqCst), 2);
    Ok(())
}

#[test]
fn projected_small_source_selection_fits_one_mib_with_large_owner_cap()
-> Result<(), Box<dyn std::error::Error>> {
    let mut work = budget()?;
    let memory = Arc::new(BoundedSelectionMemory {
        limit: 1_048_576,
        deny_reservation: None,
        live: AtomicU64::new(0),
        peak: AtomicU64::new(0),
        denied: AtomicU64::new(0),
        invalid: AtomicBool::new(false),
    });
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        BoundedSelectionScope(Arc::clone(&memory)),
    ))));
    let calls = Arc::new(AtomicUsize::new(0));
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::clone(&calls),
        deadline: work.request().deadline(),
        normalization: EmbeddingNormalization::L2Unit,
    })?);
    let fixture = projected_fixture(&bridge, false)?;
    let reader =
        NativeSemanticReader::new(bridge, fixture.recipe.clone(), semantic_read_config(10_000))?
            .with_ann(Arc::new(test_runtime()?));
    let entry = SemanticLexicalEligibleSource {
        owner: fixture.source_owner_a,
        source: NativeTextHitSource::Durable(fixture.source_a),
    };
    let mut request =
        super::lexical_scope_tests::scoped_request(fixture.ticket.view.identity, vec![entry], 1)?;
    request.query.top_k = 2;
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
    check_eq!(result.result().hits.len(), 2);
    check_eq!(result.result().scanned_owners, 2);
    check!(result.exhaustive());
    drop(result);
    check_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    check!(!memory.invalid.load(AtomicOrdering::SeqCst));
    check_eq!(memory.live.load(AtomicOrdering::SeqCst), 0);
    check!(memory.peak.load(AtomicOrdering::SeqCst) <= memory.limit);
    Ok(())
}

#[test]
fn projected_reverse_pages_past_sixty_four_members_with_exact_cap_truth()
-> Result<(), Box<dyn std::error::Error>> {
    let mut work = budget()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::clone(&calls),
        deadline: work.request().deadline(),
        normalization: EmbeddingNormalization::L2Unit,
    })?);
    let fixture = projected_fixture_with_shared(&bridge, false, 66)?;
    let entry = SemanticLexicalEligibleSource {
        owner: fixture.source_owner_a,
        source: NativeTextHitSource::Durable(fixture.source_a),
    };
    let request =
        super::lexical_scope_tests::scoped_request(fixture.ticket.view.identity, vec![entry], 1)?;
    let reader = NativeSemanticReader::new(
        Arc::clone(&bridge),
        fixture.recipe.clone(),
        semantic_read_config(66),
    )?
    .with_ann(Arc::new(test_runtime()?));
    let full = reader.query(
        &request,
        &mut QueryReadContext {
            ticket: &fixture.ticket,
            instance: &fixture.spec,
            objects: &fixture.store,
            maps: &fixture.maps,
            budget: &mut work,
        },
    )?;
    check_eq!(full.result().scanned_owners, 66);
    check_eq!(
        full.result().traversal,
        NativeSemanticTraversal::ExactExhausted
    );
    check!(full.exhaustive());
    check_eq!(full.result().hits.len(), 1);
    check_eq!(
        full.result().hits.first().ok_or("full hit")?.owner,
        owner(41)
    );
    drop(full);

    let mut capped_work = WorkBudget::new(
        WorkLimits {
            reads: 1000,
            writes: 1000,
            bytes: 100_000_000,
            mutations: 1000,
        },
        work.request().deadline(),
    );
    let capped_reader =
        NativeSemanticReader::new(bridge, fixture.recipe.clone(), semantic_read_config(64))?
            .with_ann(Arc::new(test_runtime()?));
    let capped = capped_reader.query(
        &request,
        &mut QueryReadContext {
            ticket: &fixture.ticket,
            instance: &fixture.spec,
            objects: &fixture.store,
            maps: &fixture.maps,
            budget: &mut capped_work,
        },
    )?;
    check_eq!(capped.result().scanned_owners, 64);
    check_eq!(
        capped.result().traversal,
        NativeSemanticTraversal::ExactCapped
    );
    check!(!capped.exhaustive());
    check_eq!(capped.result().hits.len(), 1);
    check_eq!(
        capped.result().hits.first().ok_or("capped hit")?.owner,
        owner(41)
    );
    drop(capped);
    check_eq!(calls.load(AtomicOrdering::SeqCst), 2);
    Ok(())
}

#[test]
fn projected_selection_denied_replacement_backing_refunds_memory()
-> Result<(), Box<dyn std::error::Error>> {
    let mut work = budget()?;
    let replacement_bytes = std::mem::size_of::<super::super::selected_owner::SelectedOwner>()
        .checked_mul(67)
        .ok_or("replacement byte overflow")?;
    let memory = Arc::new(BoundedSelectionMemory {
        limit: 1_048_576,
        deny_reservation: Some(u64::try_from(replacement_bytes)?),
        live: AtomicU64::new(0),
        peak: AtomicU64::new(0),
        denied: AtomicU64::new(0),
        invalid: AtomicBool::new(false),
    });
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        BoundedSelectionScope(Arc::clone(&memory)),
    ))));
    let calls = Arc::new(AtomicUsize::new(0));
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::clone(&calls),
        deadline: work.request().deadline(),
        normalization: EmbeddingNormalization::L2Unit,
    })?);
    let fixture = projected_fixture_with_shared(&bridge, false, 66)?;
    let reader =
        NativeSemanticReader::new(bridge, fixture.recipe.clone(), semantic_read_config(66))?
            .with_ann(Arc::new(test_runtime()?));
    let entry = SemanticLexicalEligibleSource {
        owner: fixture.source_owner_a,
        source: NativeTextHitSource::Durable(fixture.source_a),
    };
    let request =
        super::lexical_scope_tests::scoped_request(fixture.ticket.view.identity, vec![entry], 1)?;
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
    check!(matches!(result, Err(IndexError::ResourceLimited)));
    check_eq!(memory.denied.load(AtomicOrdering::SeqCst), 1);
    check_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    check!(!memory.invalid.load(AtomicOrdering::SeqCst));
    check_eq!(memory.live.load(AtomicOrdering::SeqCst), 0);
    let initial_bytes = std::mem::size_of::<super::super::selected_owner::SelectedOwner>()
        .checked_mul(64)
        .ok_or("initial byte overflow")?;
    check!(memory.peak.load(AtomicOrdering::SeqCst) >= u64::try_from(initial_bytes)?);
    Ok(())
}
