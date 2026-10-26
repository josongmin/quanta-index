//! Fixed selected-source and rank golds for within-request lexical eligibility.

use super::*;
use quanta_index_core::index::NativeTextHitSource;

pub(super) fn scoped_request(
    view: ViewRef,
    entries: Vec<SemanticLexicalEligibleSource>,
    scope_top_k: u32,
) -> Result<SemanticReadRequest, IndexError> {
    let mut query = semantic_request("query", 1);
    query.lexical_scope = Some(quanta_index_contract::TextQueryRequest {
        syntax: quanta_index_contract::TextQuerySyntax::Native,
        query_text: "needle".into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k: scope_top_k,
        cursor: None,
    });
    let mut request = SemanticReadRequest::from(query);
    request.lexical_eligibility = Some(SemanticLexicalEligibility::new(view, entries, None)?);
    Ok(request)
}

#[test]
fn lexical_eligibility_ranks_only_selected_sources_and_refuses_forgery_before_provider()
-> Result<(), Box<dyn std::error::Error>> {
    let mut work = budget()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: calls.clone(),
        deadline: work.request().deadline(),
        normalization: EmbeddingNormalization::L2Unit,
    })?);
    let recipe = bridge.recipe(1024)?;
    let spec = instance(&recipe);
    let reader = NativeSemanticReader::new(bridge, recipe.clone(), semantic_read_config(1))?;
    let store = MemoryStore::default();
    let (source_a, payload_a) = semantic_source(&store, "a.rs", b"nearest but excluded")?;
    let (source_b, payload_b) = semantic_source(&store, "b.rs", b"lexical member")?;
    let row_a = semantic_owner_vector(
        &store,
        &spec,
        &recipe,
        source_a,
        payload_a,
        1,
        vec![1.0, 0.0],
    )?;
    let row_b = semantic_owner_vector(
        &store,
        &spec,
        &recipe,
        source_b,
        payload_b,
        2,
        vec![0.0, 1.0],
    )?;
    let (binding, coverage) =
        semantic_group_coverage(&store, &spec, source_a, CoverageStatus::Complete)?;
    let ticket = semantic_ticket(MapRoot(Some(source_a)), MapRoot(Some(coverage)), 1)?;
    let maps = TestMap {
        entries: Vec::new(),
        rooted_entries: vec![
            (
                ticket.view.source.owners,
                vec![
                    (owner_map_key(owner(1)).to_vec(), source_a),
                    (owner_map_key(owner(2)).to_vec(), source_b),
                ],
            ),
            (binding.owners, vec![row_a, row_b]),
            (
                ticket.view.coverage,
                vec![(member_map_key(spec.instance).to_vec(), coverage)],
            ),
        ],
        inline_rooted_entries: Vec::new(),
        fail: false,
    };
    let view = ticket.view.identity;
    let a = SemanticLexicalEligibleSource {
        owner: owner(1),
        source: NativeTextHitSource::Durable(source_a),
    };
    let b = SemanticLexicalEligibleSource {
        owner: owner(2),
        source: NativeTextHitSource::Durable(source_b),
    };
    let mut run = |request: &SemanticReadRequest, maps: &TestMap| {
        reader.query(
            request,
            &mut QueryReadContext {
                ticket: &ticket,
                instance: &spec,
                objects: &store,
                maps,
                budget: &mut work,
            },
        )
    };
    let result = run(&scoped_request(view, vec![b], 1)?, &maps)?;
    check_eq!(result.result().hits.len(), 1);
    check_eq!(
        result.result().hits.first().ok_or("scoped hit")?.owner,
        owner(2)
    );
    check_eq!(
        result
            .result()
            .hits
            .first()
            .ok_or("scoped hit")?
            .cosine
            .total_cmp(&0.0),
        Ordering::Equal
    );
    check_eq!(result.result().scanned_owners, 1);
    check!(result.exhaustive());
    drop(result);
    check_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    // Canonical owner order is deterministic; the original max_owners remains1.
    let capped = run(&scoped_request(view, vec![b, a], 2)?, &maps)?;
    check_eq!(
        capped.result().traversal,
        NativeSemanticTraversal::ExactCapped
    );
    check_eq!(capped.result().scanned_owners, 1);
    check!(!capped.exhaustive());
    check_eq!(
        capped.result().hits.first().ok_or("capped hit")?.owner,
        owner(1)
    );
    drop(capped);
    check_eq!(calls.load(AtomicOrdering::SeqCst), 2);
    for entry in [
        SemanticLexicalEligibleSource {
            owner: owner(2),
            source: NativeTextHitSource::Durable(source_a),
        },
        SemanticLexicalEligibleSource {
            owner: owner(3),
            source: NativeTextHitSource::Durable(source_b),
        },
    ] {
        check!(matches!(
            run(&scoped_request(view, vec![entry], 1)?, &maps),
            Err(IndexError::CorruptObject(_))
        ));
    }
    let mut foreign = view;
    foreign.sequence = 2;
    check!(matches!(
        run(&scoped_request(foreign, vec![b], 1)?, &maps),
        Err(IndexError::SnapshotUnavailable)
    ));
    check_eq!(calls.load(AtomicOrdering::SeqCst), 2);
    let input = OwnerInputBinding {
        source: Some(source_b),
        projection: None,
        producer_context: Digest([0; 32]),
        derivation_policy: recipe.fingerprint(),
    };
    let stale = OwnerCoverage {
        owner: owner(2),
        desired: input,
        covered: None,
        searchable: None,
    };
    let stale_ref = store.insert(serde_json::to_vec(&stale)?, vec![source_b])?;
    let mut stale_maps = TestMap {
        entries: maps.entries.clone(),
        rooted_entries: maps.rooted_entries.clone(),
        inline_rooted_entries: maps.inline_rooted_entries.clone(),
        fail: false,
    };
    let (_, entries) = stale_maps
        .rooted_entries
        .iter_mut()
        .find(|(root, _)| *root == binding.owners)
        .ok_or("stale owner map")?;
    let (_, reference) = entries
        .iter_mut()
        .find(|(key, _)| key.as_slice() == owner_map_key(owner(2)).as_slice())
        .ok_or("stale owner row")?;
    *reference = stale_ref;
    let skipped = run(&scoped_request(view, vec![b], 1)?, &stale_maps)?;
    check!(skipped.result().hits.is_empty());
    check!(!skipped.exhaustive());
    check_eq!(skipped.result().skipped_stale, 1);
    drop(skipped);
    check_eq!(calls.load(AtomicOrdering::SeqCst), 3);
    let empty = run(&scoped_request(view, Vec::new(), 1)?, &maps)?;
    check!(empty.result().hits.is_empty());
    check!(empty.exhaustive());
    check_eq!(calls.load(AtomicOrdering::SeqCst), 3);
    let mut tokenless = scoped_request(view, Vec::new(), 1)?;
    tokenless.query.query_text = "...".into();
    check!(matches!(
        run(&tokenless, &maps),
        Err(IndexError::InvalidInput(_))
    ));
    check_eq!(calls.load(AtomicOrdering::SeqCst), 3);
    // The lexical hit's source reference is not enough: the selected source
    // record must still own its physical payload edge before embedding.
    {
        let mut objects = store.0.lock().map_err(|_error| "store lock")?;
        let object = objects.get(&source_b).cloned().ok_or("source record")?;
        drop(objects.insert(
            source_b,
            StoredObject::new(
                RegisteredObject::unscoped(source_b, u64::try_from(object.payload.len())?, vec![]),
                object.payload.clone(),
                None,
            ),
        ));
    }
    check!(matches!(
        run(&scoped_request(view, vec![b], 1)?, &maps),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(calls.load(AtomicOrdering::SeqCst), 3);
    Ok(())
}

#[test]
fn lexical_scope_rejects_duplicate_owners_and_mismatched_canonical_constraints()
-> Result<(), Box<dyn std::error::Error>> {
    let ticket = semantic_ticket(MapRoot(None), MapRoot(None), 1)?;
    let entry = SemanticLexicalEligibleSource {
        owner: owner(1),
        source: NativeTextHitSource::Durable(ObjectRef(Digest([3; 32]))),
    };
    check!(matches!(
        SemanticLexicalEligibility::new(ticket.view.identity, vec![entry, entry], None),
        Err(IndexError::InvalidInput(_))
    ));
    let mut request = scoped_request(ticket.view.identity, Vec::new(), 1)?;
    request.query.constraints.repo_relative_path_exact = Some(
        quanta_index_contract::ExactRepoRelativePathV1::new("different.rs")?,
    );
    check!(matches!(
        request.validate_lexical_scope(),
        Err(IndexError::InvalidInput(_))
    ));
    request.query.constraints = QueryConstraintSetV1::unconstrained();
    request.overlay.push(SemanticOverlayDocument {
        owner: owner(1),
        revision: 1,
        path: None,
        text: None,
    });
    // A tombstone without a lexical hit still masks its durable owner. The
    // eligible-overlay checks reject a tombstone only if it is forged as a hit.
    check!(request.validate_lexical_scope().is_ok());
    request.overlay.clear();
    request.query.lexical_scope = None;
    check!(matches!(
        request.validate_lexical_scope(),
        Err(IndexError::InvalidInput(_))
    ));
    Ok(())
}

#[test]
fn lexical_overlay_ranks_only_same_snapshot_hits_and_keeps_unselected_masks()
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
    let reader =
        NativeSemanticReader::new(Arc::clone(&bridge), recipe.clone(), semantic_read_config(5))?;
    let capped_reader = NativeSemanticReader::new(bridge, recipe.clone(), semantic_read_config(1))?;
    let store = MemoryStore::default();
    let (source, payload) = semantic_source(&store, "old.rs", b"old")?;
    let row = semantic_owner_vector(&store, &spec, &recipe, source, payload, 1, vec![1.0, 0.0])?;
    let (binding, coverage) =
        semantic_group_coverage(&store, &spec, source, CoverageStatus::Complete)?;
    let ticket = semantic_ticket(MapRoot(Some(source)), MapRoot(Some(coverage)), 1)?;
    let maps = TestMap {
        entries: Vec::new(),
        rooted_entries: vec![
            (
                ticket.view.source.owners,
                vec![(owner_map_key(owner(1)).to_vec(), source)],
            ),
            (binding.owners, vec![row]),
            (
                ticket.view.coverage,
                vec![(member_map_key(spec.instance).to_vec(), coverage)],
            ),
        ],
        inline_rooted_entries: Vec::new(),
        fail: false,
    };
    let overlay = vec![
        SemanticOverlayDocument {
            owner: owner(1),
            revision: 7,
            path: Some(Arc::<str>::from("replacement.rs")),
            text: Some(Arc::<str>::from("replacement")),
        },
        SemanticOverlayDocument {
            owner: owner(2),
            revision: 8,
            path: None,
            text: None,
        },
        SemanticOverlayDocument {
            owner: owner(3),
            revision: 9,
            path: Some(Arc::<str>::from("new.rs")),
            text: Some(Arc::<str>::from("new")),
        },
    ];
    let view = ticket.view.identity;
    let hit = SemanticLexicalEligibleSource {
        owner: owner(3),
        source: NativeTextHitSource::Overlay { revision: 9 },
    };
    {
        let mut run = |request: &SemanticReadRequest| {
            reader.query(
                request,
                &mut QueryReadContext {
                    ticket: &ticket,
                    instance: &spec,
                    objects: &store,
                    maps: &maps,
                    budget: &mut work,
                },
            )
        };
        let mut request = scoped_request(view, vec![hit], 1)?;
        request.overlay = overlay.clone();
        let result = run(&request)?;
        check_eq!(result.result().scanned_owners, 1);
        check_eq!(result.result().hits.len(), 1);
        check_eq!(
            result.result().hits.first().ok_or("overlay hit")?.owner,
            owner(3)
        );
        check_eq!(
            result.result().hits.first().ok_or("overlay hit")?.overlay,
            Some(SemanticOverlayHit {
                revision: 9,
                chunk: None,
            })
        );
        check!(result.exhaustive());
        drop(result);
        // Query embedding plus one eligible overlay embedding; owner 1 is masked
        // and its live editor body is not a lexical candidate.
        check_eq!(calls.load(AtomicOrdering::SeqCst), 2);
        let mut no_hits = scoped_request(view, Vec::new(), 1)?;
        no_hits.overlay = overlay.clone();
        let empty = run(&no_hits)?;
        check!(empty.result().hits.is_empty());
        check_eq!(empty.result().scanned_owners, 0);
        check!(empty.exhaustive());
        drop(empty);
        check_eq!(calls.load(AtomicOrdering::SeqCst), 2);
        for invalid in [
            SemanticLexicalEligibleSource {
                owner: owner(3),
                source: NativeTextHitSource::Overlay { revision: 10 },
            },
            SemanticLexicalEligibleSource {
                owner: owner(2),
                source: NativeTextHitSource::Overlay { revision: 8 },
            },
            SemanticLexicalEligibleSource {
                owner: owner(4),
                source: NativeTextHitSource::Overlay { revision: 9 },
            },
            SemanticLexicalEligibleSource {
                owner: owner(1),
                source: NativeTextHitSource::Durable(source),
            },
        ] {
            let mut invalid_request = scoped_request(view, vec![invalid], 1)?;
            invalid_request.overlay = overlay.clone();
            check!(matches!(
                run(&invalid_request),
                Err(IndexError::BackendViolation(_))
            ));
        }
        let mut wrong_path = scoped_request(view, vec![hit], 1)?;
        wrong_path.overlay = overlay.clone();
        let constraint = QueryConstraintSetV1::from_exact_repo_relative_path(
            quanta_index_contract::ExactRepoRelativePathV1::new("other.rs")?,
        );
        wrong_path.query.constraints = constraint.clone();
        wrong_path
            .query
            .lexical_scope
            .as_mut()
            .ok_or("lexical scope")?
            .constraints = constraint;
        check!(matches!(
            run(&wrong_path),
            Err(IndexError::BackendViolation(_))
        ));
        check_eq!(calls.load(AtomicOrdering::SeqCst), 2);
    }
    let mut capped = scoped_request(
        view,
        vec![
            SemanticLexicalEligibleSource {
                owner: owner(1),
                source: NativeTextHitSource::Durable(source),
            },
            hit,
        ],
        2,
    )?;
    capped.overlay = vec![overlay.get(2).ok_or("eligible overlay")?.clone()];
    let capped_result = capped_reader.query(
        &capped,
        &mut QueryReadContext {
            ticket: &ticket,
            instance: &spec,
            objects: &store,
            maps: &maps,
            budget: &mut work,
        },
    )?;
    check_eq!(capped_result.result().scanned_owners, 1);
    check_eq!(
        capped_result.result().traversal,
        NativeSemanticTraversal::ExactCapped
    );
    check_eq!(
        capped_result
            .result()
            .hits
            .first()
            .ok_or("capped hit")?
            .owner,
        owner(3)
    );
    check!(!capped_result.exhaustive());
    drop(capped_result);
    check_eq!(calls.load(AtomicOrdering::SeqCst), 4);
    Ok(())
}
