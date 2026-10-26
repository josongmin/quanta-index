use super::*;

#[derive(Clone, Copy)]
enum BatchShape {
    Exact,
    Missing,
    Extra,
}

struct WitnessTextProvider {
    calls: Arc<AtomicUsize>,
    inputs: Arc<Mutex<Vec<Vec<String>>>>,
    deadline: Instant,
    shape: BatchShape,
    deterministic: bool,
}

impl TextEmbeddingProvider for WitnessTextProvider {
    fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        let mut vectors: Vec<Vec<f32>> = texts
            .iter()
            .map(|text| match *text {
                "🙂" => vec![0.0, 1.0],
                "B" => vec![0.6, 0.8],
                _ => vec![1.0, 0.0],
            })
            .collect();
        match self.shape {
            BatchShape::Exact => {}
            BatchShape::Missing => {
                let _discarded = vectors.pop();
            }
            BatchShape::Extra => vectors.push(vec![1.0, 0.0]),
        }
        Ok(quanta_index_core::TextEmbeddingBatch::unscoped(vectors))
    }

    fn embed_batch_within(
        &self,
        texts: &[&str],
        budget: &RequestBudgetV1,
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        if budget.deadline() != self.deadline || budget.is_cancelled() {
            return Err(CoreError::InvalidContract(
                "chunk provider did not receive original request budget".into(),
            ));
        }
        let returned_count = match self.shape {
            BatchShape::Exact => texts.len(),
            BatchShape::Missing => texts.len().saturating_sub(1),
            BatchShape::Extra => texts.len().saturating_add(1),
        };
        let memory = quanta_index_core::TextEmbeddingBatch::reserve_output(
            returned_count,
            self.dimension(),
            budget,
        )?;
        self.inputs
            .lock()
            .map_err(|_error| CoreError::InvalidContract("test input lock".into()))?
            .push(texts.iter().map(|text| (*text).to_owned()).collect());
        let _previous = self.calls.fetch_add(1, AtomicOrdering::SeqCst);
        Ok(quanta_index_core::TextEmbeddingBatch::from_parts(
            self.embed_batch(texts)?.into_unscoped()?,
            memory,
        ))
    }

    fn model_id(&self) -> &'static str {
        "chunk-witness"
    }

    fn model_revision(&self) -> &'static str {
        "fixed-v1"
    }

    fn dimension(&self) -> usize {
        2
    }

    fn normalization(&self) -> EmbeddingNormalization {
        EmbeddingNormalization::L2Unit
    }

    fn supports_deterministic_payload_reuse(&self) -> bool {
        self.deterministic
    }
}

type Witness = (
    Arc<TextEmbeddingBridge<WitnessTextProvider>>,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<Vec<String>>>>,
);

fn witness(
    work: &WorkBudget,
    shape: BatchShape,
    deterministic: bool,
) -> Result<Witness, IndexError> {
    let calls = Arc::new(AtomicUsize::new(0));
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let bridge = TextEmbeddingBridge::with_utf8_chunks(
        WitnessTextProvider {
            calls: Arc::clone(&calls),
            inputs: Arc::clone(&inputs),
            deadline: work.request().deadline(),
            shape,
            deterministic,
        },
        Utf8ChunkPolicy { max_utf8_bytes: 4 },
    )?;
    Ok((Arc::new(bridge), calls, inputs))
}

fn chunk_input(source: ObjectRef, recipe: &EmbeddingRecipe) -> OwnerInputBinding {
    OwnerInputBinding {
        source: Some(source),
        projection: None,
        producer_context: Digest([0; 32]),
        derivation_policy: recipe.fingerprint(),
    }
}

fn hex(digest: Digest) -> String {
    use std::fmt::Write as _;
    let mut output = String::with_capacity(64);
    for byte in digest.0 {
        write!(&mut output, "{byte:02x}").expect("String formatting is infallible");
    }
    output
}

#[test]
fn duplicate_members_cannot_hide_an_unrelated_vector_at_equal_cardinality()
-> Result<(), Box<dyn std::error::Error>> {
    let policy = Utf8ChunkPolicy { max_utf8_bytes: 4 };
    let mut members = plan_chunks(b"AAAABBBB", policy, 2, &budget()?.request().clone())?.members;
    let duplicate = members.first().ok_or("first chunk member missing")?.id;
    members.get_mut(1).ok_or("second chunk member missing")?.id = duplicate;
    let mut vectors = vec![
        EmbeddingVector {
            id: duplicate.0.to_vec(),
            values: vec![1.0, 0.0],
        },
        EmbeddingVector {
            id: vec![0xff; 32],
            values: vec![0.0, 1.0],
        },
    ];
    vectors.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    check!(matches!(
        validate_chunk_bijection(&members, &vectors),
        Err(IndexError::BackendViolation(_))
    ));
    Ok(())
}

struct ChunkMemoryScope(Arc<EncodingMemory>);

impl quanta_index_core::index::MemoryScopePort for ChunkMemoryScope {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _previous = self.0.reservations.fetch_add(1, AtomicOrdering::SeqCst);
        let _previous_live = self
            .0
            .live
            .fetch_update(AtomicOrdering::SeqCst, AtomicOrdering::SeqCst, |live| {
                live.checked_add(bytes)
                    .filter(|next| *next <= self.0.capacity)
            })
            .map_err(|_live| IndexError::ResourceLimited)?;
        Ok(MemoryLease::new(bytes, self.0.clone()))
    }
}

#[test]
fn chunk_membership_admission_and_mismatch_refund_all_memory()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = "Aé\n🙂B".as_bytes();
    let policy = Utf8ChunkPolicy { max_utf8_bytes: 4 };
    let reference = plan_chunks(payload, policy, 4, &budget()?.request().clone())?;
    let mut vectors: Vec<_> = reference
        .members
        .iter()
        .map(|member| EmbeddingVector {
            id: member.id.0.to_vec(),
            values: vec![1.0, 0.0],
        })
        .collect();
    vectors.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    let mut recipe = recipe();
    recipe.chunk_policy = Some(policy);
    recipe.output_policy = policy.output_policy();

    let backing = u64::try_from(
        3usize
            .checked_mul(std::mem::size_of::<ChunkMember>())
            .ok_or("chunk backing overflow")?,
    )?;
    let denied = encoding_memory(
        backing.checked_sub(1).ok_or("empty chunk backing")?,
        0,
        None,
    );
    let mut denied_work = budget()?;
    denied_work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        ChunkMemoryScope(Arc::clone(&denied)),
    ))));
    check!(matches!(
        prepare_chunk_members(payload, &vectors, &recipe, &mut denied_work),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(denied.live.load(AtomicOrdering::SeqCst), 0);

    let admitted = encoding_memory(4096, 0, None);
    let mut work = budget()?;
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        ChunkMemoryScope(Arc::clone(&admitted)),
    ))));
    let (members, lease) = prepare_chunk_members(payload, &vectors, &recipe, &mut work)?;
    check_eq!(members.len(), 3);
    check_eq!(admitted.live.load(AtomicOrdering::SeqCst), backing);
    drop(members);
    drop(lease);
    check_eq!(admitted.live.load(AtomicOrdering::SeqCst), 0);

    check!(matches!(
        prepare_chunk_members(
            payload,
            vectors.get(..2).ok_or("missing vector prefix")?,
            &recipe,
            &mut work,
        ),
        Err(IndexError::BackendViolation(_))
    ));
    check_eq!(admitted.live.load(AtomicOrdering::SeqCst), 0);
    check!(!admitted.invalid.load(AtomicOrdering::SeqCst));
    Ok(())
}

#[test]
fn chunk_owner_prepare_verify_and_readback_preserve_ordered_gold_and_provider_inputs()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let (source, _payload) = semantic_source(&store, "utf8.rs", "Aé\n🙂B".as_bytes())?;
    let mut work = budget()?;
    let (bridge, calls, inputs) = witness(&work, BatchShape::Exact, false)?;
    let recipe = bridge.chunk_recipe(1024, 4)?;
    let spec = instance(&recipe);
    let adapter = NativeSemantic::new(bridge, recipe.clone())?;
    let maps = TestMap::default();
    let custody = TestCustody;
    let scratch = NoScratch;
    let input = chunk_input(source, &recipe);
    let member = {
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let candidate = crate::run_blocking(
            &runtime,
            adapter.prepare_owner(&spec, owner(1), input, None, &mut context),
        )?;
        check_eq!(candidate.artifact.chunks.len(), 3);
        check_eq!(candidate.artifact.vectors.len(), 3);
        let gold = [
            (
                (0, 4),
                "6da52b0aeb87c36b8b897f991a0c21570222565e3ac7f7708f8204353f1bb91f",
                "9213d9f4b6b52d840364898d2e7b32a97b966d91519028b9150cb8d460dd3fe7",
            ),
            (
                (4, 8),
                "d06f1525f791397809f9bc98682b5c13318eca4c3123433467fd4dffda44fd14",
                "717b86677c251e81a63c741a7028c84cc94e0fa47ad8dae18e61baa3a0ce9a25",
            ),
            (
                (8, 9),
                "df7e70e5021544f4834bbee64a9e3789febc4be81470df629cad6ddb03320a5c",
                "a545f6ce5a551fbe1e8440e4df186f27863492d22ed8cf890fc6cde3cb672e56",
            ),
        ];
        for (member, ((start, end), input_gold, id_gold)) in
            candidate.artifact.chunks.iter().zip(gold)
        {
            check_eq!(member.payload_span()?, (start, end));
            check_eq!(hex(member.input_digest), input_gold);
            check_eq!(hex(member.id), id_gold);
        }
        let expected = [vec![1.0, 0.0], vec![0.0, 1.0], vec![0.6, 0.8]];
        for (member, values) in candidate.artifact.chunks.iter().zip(expected) {
            let vector = candidate
                .artifact
                .vectors
                .iter()
                .find(|vector| vector.id.as_slice() == member.id.0)
                .ok_or("missing vector for ordered chunk member")?;
            check_eq!(&vector.values, &values);
        }
        crate::run_blocking(&runtime, adapter.verify_owner(candidate, &mut context))?
    };
    check_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    check_eq!(
        inputs
            .lock()
            .map_err(|_error| "test input lock")?
            .as_slice(),
        &[vec!["Aé\n".to_owned(), "🙂".to_owned(), "B".to_owned()]]
    );
    let loaded = load_owner_artifact(&store, &mut work, &member, &recipe)?;
    check_eq!(loaded.chunks.len(), 3);
    check_eq!(loaded.vectors.len(), 3);
    check_eq!(
        loaded
            .chunks
            .first()
            .ok_or("first loaded chunk missing")?
            .payload_span()?
            .0,
        0
    );
    check_eq!(
        loaded
            .chunks
            .get(2)
            .ok_or("third loaded chunk missing")?
            .payload_span()?
            .1,
        9
    );
    Ok(())
}

#[test]
fn chunk_provider_wrong_cardinality_refuses_before_artifact_put()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime = test_runtime()?;
    for shape in [BatchShape::Missing, BatchShape::Extra] {
        let store = MemoryStore::default();
        let (source, _payload) = semantic_source(&store, "utf8.rs", "Aé\n🙂B".as_bytes())?;
        let before = store.0.lock().map_err(|_error| "test store lock")?.len();
        let mut work = budget()?;
        let (bridge, calls, _inputs) = witness(&work, shape, false)?;
        let recipe = bridge.chunk_recipe(1024, 4)?;
        let adapter = NativeSemantic::new(bridge, recipe.clone())?;
        let maps = TestMap::default();
        let custody = TestCustody;
        let scratch = NoScratch;
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        check!(matches!(
            crate::run_blocking(
                &runtime,
                adapter.prepare_owner(
                    &instance(&recipe),
                    owner(1),
                    chunk_input(source, &recipe),
                    None,
                    &mut context,
                )
            ),
            Err(IndexError::BackendViolation(_))
        ));
        check_eq!(calls.load(AtomicOrdering::SeqCst), 1);
        check_eq!(context.budget.usage().writes, 0);
        let after = store.0.lock().map_err(|_error| "test store lock")?.len();
        check_eq!(after, before);
    }
    Ok(())
}

#[test]
fn persisted_chunk_tampering_with_rebound_commitment_fails_readback()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let (source, _payload) = semantic_source(&store, "utf8.rs", "Aé\n🙂B".as_bytes())?;
    let mut work = budget()?;
    let (bridge, _calls, _inputs) = witness(&work, BatchShape::Exact, false)?;
    let recipe = bridge.chunk_recipe(1024, 4)?;
    let spec = instance(&recipe);
    let adapter = NativeSemantic::new(bridge, recipe.clone())?;
    let maps = TestMap::default();
    let custody = TestCustody;
    let scratch = NoScratch;
    let candidate = {
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        crate::run_blocking(
            &runtime,
            adapter.prepare_owner(
                &spec,
                owner(1),
                chunk_input(source, &recipe),
                None,
                &mut context,
            ),
        )?
    };
    let base = candidate.artifact;
    for case in 0..5 {
        let mut forged = base.clone();
        match case {
            0 => {
                let member = forged
                    .chunks
                    .first_mut()
                    .ok_or("first forged chunk missing")?;
                let (start, end) = member.payload_span()?;
                member.location = crate::native::ChunkLocation::PayloadBytes {
                    start,
                    end: end.checked_add(1).ok_or("chunk span overflow")?,
                };
            }
            1 => {
                let member = forged
                    .chunks
                    .first_mut()
                    .ok_or("first forged chunk missing")?;
                *member
                    .input_digest
                    .0
                    .first_mut()
                    .ok_or("empty input digest")? ^= 1;
            }
            2 => forged.chunks.swap(0, 1),
            3 => {
                let other = forged
                    .chunks
                    .get(1)
                    .ok_or("second forged chunk missing")?
                    .id;
                forged
                    .chunks
                    .first_mut()
                    .ok_or("first forged chunk missing")?
                    .id = other;
            }
            4 => {
                forged
                    .vectors
                    .first_mut()
                    .ok_or("first forged vector missing")?
                    .id = vec![0; 32];
            }
            _ => return Err("unknown chunk corruption case".into()),
        }
        let encoded = serde_json::to_vec(&forged)?;
        let reference = store.insert(
            encoded.clone(),
            artifact_children(&forged).as_ref().to_vec(),
        )?;
        let member = OwnerMemberBinding {
            instance: spec.instance,
            owner: forged.owner,
            input: forged.input,
            format: spec.format,
            artifact: reference,
            logical_commitment: commitment(&encoded),
        };
        check!(matches!(
            load_owner_artifact(&store, &mut work, &member, &recipe),
            Err(IndexError::CorruptObject(_) | IndexError::BackendViolation(_))
        ));
    }
    let mut changed = recipe.clone();
    changed.chunk_policy = Some(Utf8ChunkPolicy { max_utf8_bytes: 8 });
    changed.output_policy = changed
        .chunk_policy
        .ok_or("missing policy")?
        .output_policy();
    let encoded = serde_json::to_vec(&base)?;
    let reference = store.insert(encoded.clone(), artifact_children(&base).as_ref().to_vec())?;
    let member = OwnerMemberBinding {
        instance: spec.instance,
        owner: base.owner,
        input: base.input,
        format: spec.format,
        artifact: reference,
        logical_commitment: commitment(&encoded),
    };
    check!(matches!(
        load_owner_artifact(&store, &mut work, &member, &changed),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn certified_same_payload_reuses_chunk_vectors_but_changed_payload_reembeds()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let (source, payload) = semantic_source(&store, "utf8.rs", "Aé\n🙂B".as_bytes())?;
    let mut work = budget()?;
    let (bridge, calls, _inputs) = witness(&work, BatchShape::Exact, true)?;
    let recipe = bridge.chunk_recipe(1024, 4)?;
    let spec = instance(&recipe);
    let adapter = NativeSemantic::new(bridge, recipe.clone())?;
    let maps = TestMap::default();
    let custody = TestCustody;
    let scratch = NoScratch;
    let input = chunk_input(source, &recipe);
    let first = {
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let candidate = crate::run_blocking(
            &runtime,
            adapter.prepare_owner(&spec, owner(1), input, None, &mut context),
        )?;
        crate::run_blocking(&runtime, adapter.verify_owner(candidate, &mut context))?
    };
    check_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    let second = {
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let candidate = crate::run_blocking(
            &runtime,
            adapter.prepare_owner(&spec, owner(1), input, Some(first), &mut context),
        )?;
        check_eq!(candidate.artifact.payload_ref, payload);
        check_eq!(candidate.artifact.chunks.len(), 3);
        crate::run_blocking(&runtime, adapter.verify_owner(candidate, &mut context))?
    };
    check_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    let (changed_source, _changed_payload) = semantic_source(&store, "utf8.rs", b"AAAABBBB")?;
    let changed_input = chunk_input(changed_source, &recipe);
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: &store,
        maps: &maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let changed = crate::run_blocking(
        &runtime,
        adapter.prepare_owner(&spec, owner(1), changed_input, Some(second), &mut context),
    )?;
    check_eq!(changed.artifact.chunks.len(), 2);
    check_eq!(calls.load(AtomicOrdering::SeqCst), 2);
    Ok(())
}
