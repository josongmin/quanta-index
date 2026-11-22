//! Owning selected-view ANN4K: native seal/map/member/query and original rerank.
//! This is local owner evidence; SDK/worker deadlines and on-disk Catalog reopen
//! remain separate product rails.
use super::*;
use crate::native::ann_lance;

#[path = "lance_cache.rs"]
mod lance_cache;

fn search_selected_lancedb(
    query: &SearchQuery<'_>,
    context: &mut ReadContext<'_>,
    runtime: &tokio::runtime::Runtime,
) -> Result<SearchResult, IndexError> {
    ann_directory::validate_coverage_layout(query.coverage)?;
    let directory =
        ann_directory::AnnDirectory::load_for_coverage(query.coverage, query.recipe, context)?;
    ann_query::search_loaded(query, directory.as_ref(), None, context, Some(runtime))
}

fn profile_work(seconds: u64) -> Result<WorkBudget, Box<dyn std::error::Error>> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 536_870_912,
            mutations: 1_000_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(seconds))
            .ok_or("ANN4K deadline")?,
    ))
}

fn profile_owner(index: usize) -> OwnerKey {
    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(&(index as u64).to_be_bytes());
    OwnerKey {
        producer: ProducerId([1; 16]),
        stable_id: StableOwnerId(bytes),
    }
}

struct ProfileMembers {
    sources: MapRoot,
    owners: MapRoot,
    expected_files: BTreeMap<OwnerKey, (ObjectRef, String, usize)>,
    entries: Vec<(ObjectRef, OwnerMemberBinding)>,
}

fn seed_profile_members(
    store: &MemoryStore,
    maps: &GraphMap<'_>,
    spec: &InstanceSpec,
    recipe: &EmbeddingRecipe,
    vectors: &[Vec<f32>],
    source_text: &[u8],
    work: &mut WorkBudget,
) -> Result<ProfileMembers, Box<dyn std::error::Error>> {
    let mut source_entries = BTreeMap::new();
    let mut owner_entries = BTreeMap::new();
    let mut expected_files = BTreeMap::new();
    let mut entries = Vec::new();
    for (index, vector) in vectors.iter().enumerate() {
        let owner = profile_owner(index);
        let path = format!("src/{index}.rs");
        let (source, payload) = semantic_source(store, &path, source_text)?;
        let (key, coverage) = semantic_owner_vector_for_owner(
            store,
            spec,
            recipe,
            source,
            payload,
            owner,
            vector.clone(),
        )?;
        check!(source_entries.insert(key.clone(), source).is_none());
        check!(owner_entries.insert(key, coverage).is_none());
        check!(
            expected_files
                .insert(owner, (source, path, index))
                .is_none()
        );
        let coverage_object = store.get(coverage, work).map_err(|error| {
            format!(
                "ANN profile coverage owner={index}: {error}; usage={:?}",
                work.usage()
            )
        })?;
        let binding: OwnerCoverage = serde_json::from_slice(&coverage_object.payload)?;
        let member_ref = binding.searchable.ok_or("ANN profile member absent")?;
        let member_object = store.get(member_ref, work).map_err(|error| {
            format!(
                "ANN profile member owner={index}: {error}; usage={:?}",
                work.usage()
            )
        })?;
        let member: OwnerMemberBinding = serde_json::from_slice(&member_object.payload)?;
        entries.push((member_ref, member));
    }
    Ok(ProfileMembers {
        sources: maps.seed(source_entries)?,
        owners: maps.seed(owner_entries)?,
        expected_files,
        entries,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "the shared fixture keeps the selected store, map, custody, and budget explicit"
)]
fn install_profile_lance_region(
    store: &MemoryStore,
    maps: &GraphMap<'_>,
    custody: &TestCustody,
    recipe: &EmbeddingRecipe,
    directory: &mut ann_directory::AnnDirectory,
    members: &[(ObjectRef, OwnerMemberBinding)],
    vectors: &[Vec<f32>],
    range: std::ops::Range<usize>,
    work: &mut WorkBudget,
    runtime: &tokio::runtime::Runtime,
) -> Result<(ObjectRef, Vec<f32>), Box<dyn std::error::Error>> {
    let member_root = maps.seed(
        range
            .clone()
            .map(|index| {
                (
                    owner_map_key(profile_owner(index)).to_vec(),
                    members[index].0,
                )
            })
            .collect(),
    )?;
    let rows: Vec<_> = range
        .clone()
        .map(|index| ann_lance::RegionRow {
            owner: profile_owner(index),
            member: members[index].1.logical_commitment,
            vector_id: b"whole-file",
            input: members[index].1.input,
            vector: &vectors[index],
        })
        .collect();
    let dimension = usize::try_from(recipe.dimension)?;
    let mut center = vec![0.0f32; dimension];
    for vector in &vectors[range.clone()] {
        for (sum, value) in center.iter_mut().zip(vector) {
            *sum += *value;
        }
    }
    let generation = directory.next_region + 1;
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: store,
        maps,
        custody,
        scratch: &NoScratch,
        budget: work,
    };
    let fileset = crate::run_blocking(
        runtime,
        ann_materialization::install_lance_region(
            &rows,
            recipe.fingerprint(),
            generation,
            dimension,
            &mut context,
        ),
    )?;
    let _descriptor = directory.append_region(
        &ann_directory::RegionDescriptor {
            fileset,
            members: member_root,
            center: center.clone(),
            rows: u32::try_from(range.len())?,
        },
        &mut context,
    )?;
    Ok((fileset, center))
}

#[derive(Clone, Copy)]
struct ObservedFileHit<'a> {
    owner: OwnerKey,
    file_owner: OwnerKey,
    source: ObjectRef,
    path: &'a str,
    vector_id: &'a [u8],
    cosine: f64,
}

fn verify_ranked_score(
    owner: OwnerKey,
    vector_id: &[u8],
    cosine: f64,
    expected_files: &BTreeMap<OwnerKey, (ObjectRef, String, usize)>,
    vectors: &[Vec<f32>],
    query: &[f32],
    previous: &mut Option<(f64, OwnerKey)>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (_, _, index) = expected_files
        .get(&owner)
        .ok_or("ANN4K returned an owner outside the selected fixture")?;
    check_eq!(vector_id, b"whole-file");
    let original = vectors.get(*index).ok_or("ANN4K expected vector absent")?;
    let expected_score = eligibility_reference_cosine(query, original);
    check!((cosine - expected_score).abs() < 1.0e-7);
    if let Some((prior_score, prior_owner)) = *previous {
        check!(
            prior_score.total_cmp(&cosine).is_gt()
                || (prior_score.total_cmp(&cosine).is_eq() && prior_owner < owner)
        );
    }
    *previous = Some((cosine, owner));
    Ok(())
}

fn verify_file_hits(
    hits: &[ObservedFileHit<'_>],
    expected_files: &BTreeMap<OwnerKey, (ObjectRef, String, usize)>,
    vectors: &[Vec<f32>],
    query: &[f32],
    top_k: usize,
) -> Result<BTreeSet<OwnerKey>, Box<dyn std::error::Error>> {
    check_eq!(hits.len(), top_k);
    let mut observed = BTreeSet::new();
    let mut previous: Option<(f64, OwnerKey)> = None;
    for hit in hits {
        let (source, path, _) = expected_files
            .get(&hit.owner)
            .ok_or("ANN4K returned an owner outside the selected fixture")?;
        check_eq!(hit.file_owner, hit.owner);
        check_eq!(hit.source, *source);
        check_eq!(hit.path, path.as_str());
        check!(observed.insert(hit.owner));
        verify_ranked_score(
            hit.owner,
            hit.vector_id,
            hit.cosine,
            expected_files,
            vectors,
            query,
            &mut previous,
        )?;
    }
    Ok(observed)
}

fn verify_selected_raw_hits(
    hits: &[SearchHit],
    expected_files: &BTreeMap<OwnerKey, (ObjectRef, String, usize)>,
    vectors: &[Vec<f32>],
    query: &[f32],
    top_k: usize,
) -> Result<BTreeSet<OwnerKey>, Box<dyn std::error::Error>> {
    check_eq!(hits.len(), top_k);
    let mut observed = BTreeSet::new();
    let mut previous = None;
    for hit in hits {
        check!(hit.source_file().is_none());
        check!(observed.insert(hit.owner));
        verify_ranked_score(
            hit.owner,
            &hit.vector_id,
            hit.cosine,
            expected_files,
            vectors,
            query,
            &mut previous,
        )?;
    }
    Ok(observed)
}

#[test]
fn ann4k_file_hit_oracle_rejects_duplicate_foreign_source_and_misordered_scores()
-> Result<(), Box<dyn std::error::Error>> {
    let first = profile_owner(0);
    let second = profile_owner(1);
    let first_source = ObjectRef(Digest([1; 32]));
    let second_source = ObjectRef(Digest([2; 32]));
    let expected_files = BTreeMap::from([
        (first, (first_source, "src/0.rs".to_owned(), 0)),
        (second, (second_source, "src/1.rs".to_owned(), 1)),
    ]);
    let vectors = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
    let query = [1.0, 0.0];
    let valid = [
        ObservedFileHit {
            owner: first,
            file_owner: first,
            source: first_source,
            path: "src/0.rs",
            vector_id: b"whole-file",
            cosine: 1.0,
        },
        ObservedFileHit {
            owner: second,
            file_owner: second,
            source: second_source,
            path: "src/1.rs",
            vector_id: b"whole-file",
            cosine: 0.0,
        },
    ];
    check!(verify_file_hits(&valid, &expected_files, &vectors, &query, 2).is_ok());
    let duplicate = [valid[0], valid[0]];
    check!(verify_file_hits(&duplicate, &expected_files, &vectors, &query, 2).is_err());
    let mut foreign_source = valid;
    foreign_source[1].source = first_source;
    check!(verify_file_hits(&foreign_source, &expected_files, &vectors, &query, 2).is_err());
    let misordered = [valid[1], valid[0]];
    check!(verify_file_hits(&misordered, &expected_files, &vectors, &query, 2).is_err());
    let mut wrong_score = valid;
    wrong_score[1].cosine = 0.5;
    check!(verify_file_hits(&wrong_score, &expected_files, &vectors, &query, 2).is_err());
    Ok(())
}

#[test]
fn ann4k_selected_file_strata_reject_wrong_score_and_reversed_page()
-> Result<(), Box<dyn std::error::Error>> {
    let first = profile_owner(0);
    let second = profile_owner(1);
    let first_source = ObjectRef(Digest([1; 32]));
    let second_source = ObjectRef(Digest([2; 32]));
    let expected_files = BTreeMap::from([
        (first, (first_source, "src/0.rs".to_owned(), 0)),
        (second, (second_source, "src/1.rs".to_owned(), 1)),
    ]);
    let vectors = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
    let query = [1.0, 0.0];
    let valid = [
        ObservedFileHit {
            owner: first,
            file_owner: first,
            source: first_source,
            path: "src/0.rs",
            vector_id: b"whole-file",
            cosine: 1.0,
        },
        ObservedFileHit {
            owner: second,
            file_owner: second,
            source: second_source,
            path: "src/1.rs",
            vector_id: b"whole-file",
            cosine: 0.0,
        },
    ];
    check!(verify_file_hits(&valid, &expected_files, &vectors, &query, 2).is_ok());
    let mut wrong_score = valid;
    wrong_score[1].cosine = 0.5;
    check!(verify_file_hits(&wrong_score, &expected_files, &vectors, &query, 2).is_err());
    check!(verify_file_hits(&[valid[1], valid[0]], &expected_files, &vectors, &query, 2).is_err());
    Ok(())
}

#[test]
fn ann4k_selected_raw_strata_reject_wrong_score_and_reversed_page()
-> Result<(), Box<dyn std::error::Error>> {
    let first = profile_owner(0);
    let second = profile_owner(1);
    let expected_files = BTreeMap::from([
        (
            first,
            (ObjectRef(Digest([1; 32])), "src/0.rs".to_owned(), 0),
        ),
        (
            second,
            (ObjectRef(Digest([2; 32])), "src/1.rs".to_owned(), 1),
        ),
    ]);
    let vectors = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
    let query = [1.0, 0.0];
    let valid = [
        SearchHit::from_parts(first, b"whole-file".to_vec(), 1.0, None),
        SearchHit::from_parts(second, b"whole-file".to_vec(), 0.0, None),
    ];
    check!(verify_selected_raw_hits(&valid, &expected_files, &vectors, &query, 2).is_ok());
    let wrong_score = [
        SearchHit::from_parts(first, b"whole-file".to_vec(), 1.0, None),
        SearchHit::from_parts(second, b"whole-file".to_vec(), 0.5, None),
    ];
    check!(verify_selected_raw_hits(&wrong_score, &expected_files, &vectors, &query, 2).is_err());
    let reversed = [
        SearchHit::from_parts(second, b"whole-file".to_vec(), 0.0, None),
        SearchHit::from_parts(first, b"whole-file".to_vec(), 1.0, None),
    ];
    check!(verify_selected_raw_hits(&reversed, &expected_files, &vectors, &query, 2).is_err());
    Ok(())
}

#[test]
fn native_ann4k_selected_file_recall_matches_independent_exact()
-> Result<(), Box<dyn std::error::Error>> {
    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let mut recipe = recipe();
    recipe.dimension = 64;
    let spec = instance(&recipe);
    let runtime = test_runtime()?;
    let mut seed = 42u64;
    let vectors: Vec<Vec<f32>> = (0..4096)
        .map(|_| {
            let mut vector: Vec<f32> = (0..64)
                .map(|_| {
                    seed = seed
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    ((seed >> 40) as f32 / 16_777_216.0) * 2.0 - 1.0
                })
                .collect();
            let norm = vector
                .iter()
                .map(|value| f64::from(*value).powi(2))
                .sum::<f64>()
                .sqrt();
            for value in &mut vector {
                *value = (f64::from(*value) / norm) as f32;
            }
            vector
        })
        .collect();
    let mut work = profile_work(30)?;
    let ProfileMembers {
        sources,
        owners,
        expected_files,
        entries: member_entries,
    } = seed_profile_members(
        &store,
        &maps,
        &spec,
        &recipe,
        &vectors,
        b"ANN4K original source",
        &mut work,
    )?;
    let mut directory = ann_directory::AnnDirectory::empty_with_backend(
        recipe.fingerprint(),
        owners,
        ann_directory::AnnBackend::LanceDb,
    );
    let mut sealed = Vec::new();
    for group in 0..8 {
        let start = group * 512;
        let end = start + 512;
        let (fileset, _center) = install_profile_lance_region(
            &store,
            &maps,
            &custody,
            &recipe,
            &mut directory,
            &member_entries,
            &vectors,
            start..end,
            &mut work,
            &runtime,
        )
        .map_err(|error| {
            format!(
                "ANN4K LanceDB install region={group}: {error}; usage={:?}; sealed={sealed:?}",
                work.usage()
            )
        })?;
        sealed.push(fileset);
    }
    let reference = directory
        .save(&mut AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        })
        .map_err(|error| {
            format!(
                "ANN4K directory save: {error}; usage={:?}; sealed={sealed:?}",
                work.usage()
            )
        })?;
    eprintln!(
        "ANN4K build complete: usage={:?}; sealed={sealed:?}",
        work.usage()
    );
    let coverage = maintenance::binding(&spec, &recipe, sources, owners, reference);
    let mut recalled = 0usize;
    for ordinal in 0..64 {
        let vector = &vectors[ordinal * 61];
        // Independent f64 brute-force cosine and owner tie break. No production
        // search/distance/rank helper supplies the expected identities.
        let norm = vector
            .iter()
            .map(|v| f64::from(*v).powi(2))
            .sum::<f64>()
            .sqrt();
        let score = |other: &[f32]| {
            let dot = vector
                .iter()
                .zip(other)
                .map(|(a, b)| f64::from(*a) * f64::from(*b))
                .sum::<f64>();
            dot / (norm
                * other
                    .iter()
                    .map(|v| f64::from(*v).powi(2))
                    .sum::<f64>()
                    .sqrt())
        };
        let mut exact: Vec<_> = vectors
            .iter()
            .enumerate()
            .map(|(i, v)| (profile_owner(i), score(v)))
            .collect();
        exact.sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let gold: BTreeSet<_> = exact.iter().take(10).map(|(owner, _)| *owner).collect();
        let mut query_work = profile_work(10)?;
        let result = search_selected_lancedb(
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
                vector,
                top_k: 10,
                page_size: 64,
                max_owners: 1024,
                overlay: None,
            },
            &mut ReadContext {
                objects: &store,
                maps: &maps,
                budget: &mut query_work,
            },
            &runtime,
        )
        .map_err(|error| {
            format!(
                "ANN4K query={ordinal}: {error}; usage={:?}; build_usage={:?}",
                query_work.usage(),
                work.usage()
            )
        })?;
        check!(!result.exhaustive);
        let mut hits = Vec::new();
        for hit in &result.hits {
            let file = hit.source_file().ok_or("ANN4K missing file witness")?;
            hits.push(ObservedFileHit {
                owner: hit.owner,
                file_owner: file.owner,
                source: file.source,
                path: file.path(),
                vector_id: &hit.vector_id,
                cosine: hit.cosine,
            });
        }
        let observed = verify_file_hits(&hits, &expected_files, &vectors, vector, 10)?;
        recalled += observed.intersection(&gold).count();
    }
    eprintln!("ANN4K independent recalled={recalled}/640");
    check!(recalled as f64 / 640.0 >= 0.95);
    verify_selected_eligibility_strata(
        &store, &maps, &custody, &spec, &recipe, &vectors, &coverage,
    )?;
    Ok(())
}

#[test]
fn native_trained_ninth_region_quality_separates_routing_loss()
-> Result<(), Box<dyn std::error::Error>> {
    const REGIONS: usize = 9;
    const ROWS_PER_REGION: usize = 256;
    const DIMENSION: usize = 8;
    const TOP_K: usize = 10;

    let store = MemoryStore::default();
    let maps = GraphMap::new(&store);
    let custody = TestCustody;
    let mut recipe = recipe();
    recipe.dimension = u32::try_from(DIMENSION)?;
    let spec = instance(&recipe);
    let runtime = test_runtime()?;
    let query_vector = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    // The selected region has ten separated winners. The ninth centroid is
    // negative although it contains the global exact winner.
    let vectors: Vec<Vec<f32>> = (0..REGIONS)
        .flat_map(|region| {
            (0..ROWS_PER_REGION).map(move |row| {
                let x = if region == REGIONS - 1 {
                    if row == 0 {
                        1.0
                    } else {
                        -0.9 + row as f32 * 0.0001
                    }
                } else if region == REGIONS - 2 && row < TOP_K {
                    0.99 - row as f32 * 0.001
                } else {
                    0.55 + region as f32 * 0.045 + row as f32 * 0.0001
                };
                vec![x, (1.0 - x * x).sqrt(), 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
            })
        })
        .collect();
    let mut work = profile_work(180)?;
    let ProfileMembers {
        sources,
        owners,
        expected_files,
        entries: member_entries,
    } = seed_profile_members(
        &store,
        &maps,
        &spec,
        &recipe,
        &vectors,
        b"trained ANN source",
        &mut work,
    )?;
    let mut directory = ann_directory::AnnDirectory::empty_with_backend(
        recipe.fingerprint(),
        owners,
        ann_directory::AnnBackend::LanceDb,
    );
    let mut centers = Vec::new();
    for region in 0..REGIONS {
        let start = region * ROWS_PER_REGION;
        let end = start + ROWS_PER_REGION;
        let (fileset, center) = install_profile_lance_region(
            &store,
            &maps,
            &custody,
            &recipe,
            &mut directory,
            &member_entries,
            &vectors,
            start..end,
            &mut work,
            &runtime,
        )?;
        let inspected =
            crate::native::ann_files::inspect_fileset_manifest(&store, fileset, &mut work)?;
        check_eq!(inspected.seal.rows, u32::try_from(ROWS_PER_REGION)?);
        check_eq!(
            inspected.index_kind,
            quanta_index_core::DiagnosticAnnIndexKindV1::IvfHnswSq
        );
        centers.push(center);
    }

    let reference = directory.save(&mut AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &maps,
        custody: &custody,
        scratch: &NoScratch,
        budget: &mut work,
    })?;
    let coverage = maintenance::binding(&spec, &recipe, sources, owners, reference);
    let mut global_exact: Vec<_> = vectors
        .iter()
        .enumerate()
        .map(|(index, vector)| {
            (
                profile_owner(index),
                eligibility_reference_cosine(&query_vector, vector),
            )
        })
        .collect();
    global_exact.sort_unstable_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    check_eq!(global_exact[0].0, profile_owner(8 * ROWS_PER_REGION));
    let global_top: BTreeSet<_> = global_exact
        .iter()
        .take(TOP_K)
        .map(|(owner, _)| *owner)
        .collect();
    let mut center_scores: Vec<_> = centers
        .iter()
        .enumerate()
        .map(|(region, center)| (region, eligibility_reference_cosine(&query_vector, center)))
        .collect();
    center_scores.sort_unstable_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    let selected_regions: BTreeSet<_> = center_scores
        .iter()
        .take(REGIONS - 1)
        .map(|(region, _)| *region)
        .collect();
    check!(!selected_regions.contains(&(REGIONS - 1)));
    let routed_top: BTreeSet<_> = global_exact
        .iter()
        .filter(|(owner, _)| {
            selected_regions.contains(&(expected_files[owner].2 / ROWS_PER_REGION))
        })
        .take(TOP_K)
        .map(|(owner, _)| *owner)
        .collect();
    check_eq!(routed_top.len(), TOP_K);
    check_eq!(global_top.intersection(&routed_top).count(), TOP_K - 1);

    let mut query_work = profile_work(60)?;
    let result = search_selected_lancedb(
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
            vector: &query_vector,
            top_k: u32::try_from(TOP_K)?,
            page_size: 64,
            max_owners: 1024,
            overlay: None,
        },
        &mut ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut query_work,
        },
        &runtime,
    )?;
    check_eq!(
        result.traversal,
        NativeSemanticTraversal::Approximate { capped: true }
    );
    check!(!result.exhaustive);
    let mut hits = Vec::new();
    for hit in &result.hits {
        let file = hit.source_file().ok_or("trained ANN file witness absent")?;
        hits.push(ObservedFileHit {
            owner: hit.owner,
            file_owner: file.owner,
            source: file.source,
            path: file.path(),
            vector_id: &hit.vector_id,
            cosine: hit.cosine,
        });
    }
    let observed = verify_file_hits(&hits, &expected_files, &vectors, &query_vector, TOP_K)?;
    check!(
        observed
            .iter()
            .all(|owner| selected_regions.contains(&(expected_files[owner].2 / ROWS_PER_REGION)))
    );
    check_eq!(observed.intersection(&global_top).count(), TOP_K - 1);
    check_eq!(observed.intersection(&routed_top).count(), TOP_K);
    Ok(())
}

// The reference never calls production distance, rank or graph helpers.
fn eligibility_reference_cosine(left: &[f32], right: &[f32]) -> f64 {
    let dot = left
        .iter()
        .zip(right)
        .map(|(a, b)| f64::from(*a) * f64::from(*b))
        .sum::<f64>();
    let left_norm = left
        .iter()
        .map(|x| f64::from(*x).powi(2))
        .sum::<f64>()
        .sqrt();
    let right_norm = right
        .iter()
        .map(|x| f64::from(*x).powi(2))
        .sum::<f64>()
        .sqrt();
    dot / (left_norm * right_norm)
}

fn verify_selected_eligibility_strata(
    store: &MemoryStore,
    maps: &GraphMap<'_>,
    custody: &TestCustody,
    spec: &InstanceSpec,
    recipe: &EmbeddingRecipe,
    vectors: &[Vec<f32>],
    prior: &CoverageBinding,
) -> Result<(), Box<dyn std::error::Error>> {
    check_eq!(vectors.len(), 4096);
    let source_rows = maps.state(prior.desired.root)?;
    let coverage_rows = maps.state(prior.owners)?;
    check_eq!(source_rows.len(), 4096);
    check_eq!(coverage_rows.len(), 4096);
    let adapter = NativeSemantic::new(
        SourceVectorProvider {
            recipe: recipe.clone(),
            empty: false,
        },
        recipe.clone(),
    )?
    .with_lancedb_ann();
    let runtime = test_runtime()?;
    // Membership is fixed independently before any ANN output is observed.
    let mut farthest: Vec<_> = (0..4096).collect();
    farthest.sort_unstable_by(|a, b| {
        eligibility_reference_cosine(&vectors[0], &vectors[*a])
            .total_cmp(&eligibility_reference_cosine(&vectors[0], &vectors[*b]))
            .then_with(|| a.cmp(b))
    });
    // Held-out directions are distinct, close to the predeclared basis, and
    // fixed for both uniform and distance-correlated current membership.
    let queries: Vec<Vec<f32>> = (0..64)
        .map(|ordinal| {
            let noise = &vectors[(ordinal * 61 + 17) % 4096];
            let mut query: Vec<_> = vectors[0]
                .iter()
                .zip(noise)
                .map(|(a, b)| 0.995 * f64::from(*a) + 0.005 * f64::from(*b))
                .collect();
            let norm = query.iter().map(|x| x * x).sum::<f64>().sqrt();
            for value in &mut query {
                *value /= norm;
            }
            query.into_iter().map(|x| x as f32).collect()
        })
        .collect();
    let unique_queries: BTreeSet<Vec<u32>> = queries
        .iter()
        .map(|query| query.iter().map(|value| value.to_bits()).collect())
        .collect();
    check_eq!(unique_queries.len(), 64);
    let mut failures = Vec::new();
    for (label, stride, count) in [
        ("empty", 0usize, 0usize),
        ("uniform_le1pct", 103, 40),
        ("uniform_1to10pct", 20, 205),
        ("uniform_gt10pct", 2, 2048),
        ("correlated_le1pct", 0, 40),
        ("correlated_1to10pct", 0, 205),
        ("correlated_gt10pct", 0, 2048),
    ] {
        let active: Vec<_> = if count == 0 {
            Vec::new()
        } else if stride != 0 {
            (0..4096).filter(|index| index % stride == 0).collect()
        } else {
            farthest.iter().copied().take(count).collect()
        };
        check_eq!(active.len(), count);
        let mut sources = BTreeMap::new();
        let mut owners = BTreeMap::new();
        let mut expected_files = BTreeMap::new();
        for &index in &active {
            let owner = profile_owner(index);
            let key = owner_map_key(owner).to_vec();
            let source = *source_rows.get(&key).ok_or("eligible source missing")?;
            let coverage = *coverage_rows.get(&key).ok_or("eligible coverage missing")?;
            check!(sources.insert(key.clone(), source).is_none());
            check!(owners.insert(key, coverage).is_none());
            check!(
                expected_files
                    .insert(owner, (source, format!("src/{index}.rs"), index))
                    .is_none()
            );
        }
        let sources = maps.seed(sources)?;
        let owners = maps.seed(owners)?;
        let mut prepare_work = profile_work(30)?;
        let root = {
            let mut context = AttemptContext {
                attempt: attempt(),
                objects: store,
                maps,
                custody,
                scratch: &NoScratch,
                budget: &mut prepare_work,
            };
            // Canonical current-owner maintenance constructs the legal empty
            // or retained immutable-region directory; no test-only override.
            crate::run_blocking(
                &runtime,
                adapter.prepare_searchable(spec, Some(prior), owners, None, None, &mut context),
            )?
            .install(&mut context)?
            .ok_or("eligible directory missing")?
        };
        let selected = maintenance::binding(spec, recipe, sources, owners, root);
        let directory = ann_directory::AnnDirectory::load(
            root,
            recipe.fingerprint(),
            &mut ReadContext {
                objects: store,
                maps,
                budget: &mut prepare_work,
            },
        )?;
        check_eq!(directory.owners, owners);
        if count == 0 {
            check_eq!(directory.region_count, 0);
        } else {
            // Each predeclared nonempty subset retains at least one current
            // member in every original region, so routing must keep all eight.
            check_eq!(directory.region_count, 8);
        }
        let mut recalled = 0usize;
        let mut peak_scanned_owners = 0;
        let mut queries_at_owner_limit = 0usize;
        let expected_count = count.min(10);
        for (ordinal, vector) in queries.iter().enumerate() {
            let mut gold: Vec<_> = active
                .iter()
                .map(|&i| {
                    (
                        profile_owner(i),
                        eligibility_reference_cosine(vector, &vectors[i]),
                    )
                })
                .collect();
            gold.sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let gold: BTreeSet<_> = gold
                .iter()
                .take(expected_count)
                .map(|(owner, _)| *owner)
                .collect();
            check_eq!(gold.len(), expected_count);
            let mut work = profile_work(10)?;
            let query = SearchQuery {
                source_files: true,
                lexical_eligibility: None,
                exact_path: None,
                coverage: &selected,
                projection: None,
                source_owners: sources,
                source_paths: MapRoot(None),
                instance: spec.instance,
                format: spec.format,
                recipe,
                vector,
                top_k: 10,
                page_size: 64,
                max_owners: 1024,
                overlay: None,
            };
            let result = search_selected_lancedb(
                &query,
                &mut ReadContext {
                    objects: store,
                    maps,
                    budget: &mut work,
                },
                &runtime,
            )
            .map_err(|error| {
                format!(
                    "ANN4K eligibility {label} query={ordinal}: {error}; usage={:?}",
                    work.usage()
                )
            })?;
            check!(result.scanned_owners <= query.max_owners);
            peak_scanned_owners = peak_scanned_owners.max(result.scanned_owners);
            if result.scanned_owners == query.max_owners {
                queries_at_owner_limit += 1;
            }
            if count == 0 {
                check!(result.exhaustive);
                check_eq!(result.traversal, NativeSemanticTraversal::ExactExhausted);
            } else {
                check!(!result.exhaustive);
                check!(matches!(
                    result.traversal,
                    NativeSemanticTraversal::Approximate { .. }
                ));
            }
            let mut hits = Vec::new();
            for hit in &result.hits {
                let file = hit.source_file().ok_or("eligible file witness missing")?;
                hits.push(ObservedFileHit {
                    owner: hit.owner,
                    file_owner: file.owner,
                    source: file.source,
                    path: file.path(),
                    vector_id: &hit.vector_id,
                    cosine: hit.cosine,
                });
            }
            let observed =
                verify_file_hits(&hits, &expected_files, vectors, vector, expected_count)?;
            recalled += observed.intersection(&gold).count();
            // The raw-vector API shares the same selected-member accounting.
            // One fixed query per stratum catches a file-only repair while
            // the full file Q64 oracle and its denominator stay unchanged.
            if ordinal == 0 {
                let mut raw_work = profile_work(10)?;
                let raw = search_selected_lancedb(
                    &SearchQuery {
                        source_files: false,
                        ..query
                    },
                    &mut ReadContext {
                        objects: store,
                        maps,
                        budget: &mut raw_work,
                    },
                    &runtime,
                )
                .map_err(|error| {
                    format!(
                        "ANN4K raw eligibility {label}: {error}; usage={:?}",
                        raw_work.usage()
                    )
                })?;
                check!(raw.scanned_owners <= 1024);
                if count == 0 {
                    check!(raw.exhaustive);
                    check_eq!(raw.traversal, NativeSemanticTraversal::ExactExhausted);
                } else {
                    check!(!raw.exhaustive);
                    check!(matches!(
                        raw.traversal,
                        NativeSemanticTraversal::Approximate { .. }
                    ));
                }
                let raw_owners = verify_selected_raw_hits(
                    &raw.hits,
                    &expected_files,
                    vectors,
                    vector,
                    expected_count,
                )?;
                let raw_recalled = raw_owners.intersection(&gold).count();
                eprintln!(
                    "ANN4K raw selected eligibility {label}: recalled={raw_recalled}/{expected_count}, scanned_owners={}",
                    raw.scanned_owners
                );
                if raw_owners != gold {
                    failures.push(format!("{label}/raw: {raw_recalled}/{expected_count}"));
                }
            }
        }
        let denominator = 64 * expected_count;
        eprintln!(
            "ANN4K selected eligibility {label}: active={count}/4096, recalled={recalled}/{denominator}, peak_scanned_owners={peak_scanned_owners}, queries_at_owner_limit={queries_at_owner_limit}/{}",
            queries.len()
        );
        if denominator != 0 && recalled as f64 / (denominator as f64) < 0.95 {
            failures.push(format!("{label}: {recalled}/{denominator}"));
        }
    }
    if !failures.is_empty() {
        return Err(format!(
            "ANN4K selected eligibility Recall < .95: {}",
            failures.join(", ")
        )
        .into());
    }
    Ok(())
}
