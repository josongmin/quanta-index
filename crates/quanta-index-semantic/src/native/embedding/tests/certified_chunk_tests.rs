use super::*;
use quanta_index_contract::native_protocol::{
    NativeSemanticManifestHeader, NativeSemanticManifestLimits, NativeSemanticManifestMember,
    encode_native_semantic_manifest_into, native_semantic_manifest_encoded_len,
};

const PROJECTION: ProjectionId = ProjectionId([31; 16]);
const PRODUCER: ProducerId = ProducerId([32; 16]);
const SCHEMA: SchemaId = SchemaId([33; 16]);
const ALPHA_DIGEST: Digest = Digest([
    0x8e, 0xd3, 0xf6, 0xad, 0x68, 0x5b, 0x95, 0x9e, 0xad, 0x70, 0x22, 0x51, 0x8e, 0x1a, 0xf7, 0x6c,
    0xd8, 0x16, 0xf8, 0xe8, 0xec, 0x7c, 0xcd, 0xda, 0x1e, 0xd4, 0x01, 0x8e, 0x8f, 0x22, 0x23, 0xf8,
]);
const BETA_DIGEST: Digest = Digest([
    0xe8, 0x88, 0xca, 0xe3, 0xc7, 0xb5, 0x40, 0x29, 0x4d, 0x62, 0xf6, 0x12, 0x85, 0xdd, 0x5f, 0xc6,
    0x62, 0x91, 0x4a, 0x32, 0xb1, 0x87, 0x9f, 0x04, 0x16, 0xd6, 0xeb, 0x5d, 0xca, 0xb0, 0x90, 0xff,
]);
const ALPH_DIGEST: Digest = Digest([
    0x24, 0x29, 0xa9, 0x9e, 0x01, 0xb3, 0x51, 0xde, 0x98, 0x7b, 0x50, 0x81, 0xcb, 0x49, 0xe4, 0xd8,
    0xf9, 0xaf, 0x4d, 0x1a, 0x72, 0x69, 0xcc, 0x9c, 0x0b, 0xe9, 0x36, 0x90, 0xf1, 0xb6, 0x86, 0xa3,
]);

#[derive(Clone, Copy)]
enum CertifiedBatchShape {
    Exact,
    Missing,
    Extra,
}

struct CertifiedTextProvider {
    calls: Arc<AtomicUsize>,
    inputs: Arc<Mutex<Vec<Vec<String>>>>,
    deadline: Instant,
    shape: CertifiedBatchShape,
    deterministic: bool,
}

impl TextEmbeddingProvider for CertifiedTextProvider {
    fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        let mut vectors: Vec<Vec<f32>> = texts
            .iter()
            .map(|text| {
                if *text == "βeta" {
                    vec![0.0, 1.0]
                } else {
                    vec![1.0, 0.0]
                }
            })
            .collect();
        match self.shape {
            CertifiedBatchShape::Exact => {}
            CertifiedBatchShape::Missing => {
                let _discarded = vectors.pop();
            }
            CertifiedBatchShape::Extra => vectors.push(vec![1.0, 0.0]),
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
                "certified chunk provider lost original request budget".into(),
            ));
        }
        let returned_count = match self.shape {
            CertifiedBatchShape::Exact => texts.len(),
            CertifiedBatchShape::Missing => texts.len().saturating_sub(1),
            CertifiedBatchShape::Extra => texts.len().saturating_add(1),
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
        "certified-chunk-witness"
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

type CertifiedWitness = (
    Arc<TextEmbeddingBridge<CertifiedTextProvider>>,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<Vec<String>>>>,
);

struct ReuseHookWitness {
    bridge: Arc<TextEmbeddingBridge<CertifiedTextProvider>>,
    identity_calls: Arc<AtomicUsize>,
    embed_calls: Arc<AtomicUsize>,
}

impl EmbeddingProvider for ReuseHookWitness {
    fn model_identity(&self) -> Digest {
        self.bridge.model_identity()
    }

    fn dimension(&self) -> u32 {
        self.bridge.dimension()
    }

    fn normalization(&self) -> VectorNormalization {
        self.bridge.normalization()
    }

    fn output_policy(&self) -> Digest {
        self.bridge.output_policy()
    }

    fn payload_reuse_identity(
        &self,
        recipe: &EmbeddingRecipe,
        payload: ObjectRef,
    ) -> Result<Option<Digest>, IndexError> {
        let _previous = self.identity_calls.fetch_add(1, AtomicOrdering::SeqCst);
        self.bridge.payload_reuse_identity(recipe, payload)
    }

    fn embed<'a>(&'a self, input: EmbeddingInput<'a>) -> IndexFuture<'a, EmbeddingOutput> {
        let _previous = self.embed_calls.fetch_add(1, AtomicOrdering::SeqCst);
        self.bridge.embed(input)
    }
}

fn certified_witness(work: &WorkBudget) -> Result<CertifiedWitness, IndexError> {
    certified_witness_with_shape(work, CertifiedBatchShape::Exact)
}

fn certified_witness_with_shape(
    work: &WorkBudget,
    shape: CertifiedBatchShape,
) -> Result<CertifiedWitness, IndexError> {
    certified_witness_with_properties(work, shape, true)
}

fn certified_witness_with_properties(
    work: &WorkBudget,
    shape: CertifiedBatchShape,
    deterministic: bool,
) -> Result<CertifiedWitness, IndexError> {
    let calls = Arc::new(AtomicUsize::new(0));
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let bridge = TextEmbeddingBridge::with_certified_chunks(CertifiedTextProvider {
        calls: Arc::clone(&calls),
        inputs: Arc::clone(&inputs),
        deadline: work.request().deadline(),
        shape,
        deterministic,
    })?;
    Ok((Arc::new(bridge), calls, inputs))
}

fn certified_spec(recipe: &EmbeddingRecipe) -> InstanceSpec {
    let mut spec = instance(recipe);
    spec.input_domain = InputDomain::Projection {
        projection: PROJECTION,
        producer: PRODUCER,
    };
    spec.schema = SCHEMA;
    spec
}

fn certified_header(source: Option<ObjectRef>, stamp: u8) -> NativeSemanticManifestHeader {
    NativeSemanticManifestHeader {
        owner: owner(1),
        projection: PROJECTION,
        producer: PRODUCER,
        schema: SCHEMA,
        source,
        dependency_stamp: Digest([stamp; 32]),
    }
}

fn manifest_bytes(
    header: NativeSemanticManifestHeader,
    members: &[NativeSemanticManifestMember<'_>],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let limits = NativeSemanticManifestLimits::MAXIMUM;
    let mut bytes = vec![0; native_semantic_manifest_encoded_len(header, members, limits)?];
    encode_native_semantic_manifest_into(header, members, limits, &mut bytes)?;
    Ok(bytes)
}

fn certified_input(
    store: &MemoryStore,
    recipe: &EmbeddingRecipe,
    header: NativeSemanticManifestHeader,
    payload: Vec<u8>,
) -> Result<(OwnerInputBinding, ObjectRef), Box<dyn std::error::Error>> {
    let output = store.insert(payload, vec![])?;
    let contribution = ProjectionOwnerContribution {
        projection: header.projection,
        producer: header.producer,
        owner: header.owner,
        source_owner: header.source.map(|_| header.owner),
        source: header.source,
        dependency_stamp: header.dependency_stamp,
        output: Some(output),
    };
    let reference = store.insert(
        serde_json::to_vec(&contribution)?,
        contribution.object_children(),
    )?;
    Ok((
        OwnerInputBinding {
            source: header.source,
            projection: Some(reference),
            producer_context: contribution.materialization_context(),
            derivation_policy: recipe.fingerprint(),
        },
        output,
    ))
}

fn prepare_certified<P: EmbeddingProvider>(
    runtime: &tokio::runtime::Runtime,
    adapter: &NativeSemantic<P>,
    spec: &InstanceSpec,
    store: &MemoryStore,
    input: OwnerInputBinding,
    prior: Option<OwnerMemberBinding>,
    work: &mut WorkBudget,
) -> Result<SemanticCandidate, IndexError> {
    let maps = TestMap::default();
    let custody = TestCustody;
    let scratch = NoScratch;
    let mut context = AttemptContext {
        attempt: attempt(),
        objects: store,
        maps: &maps,
        custody: &custody,
        scratch: &scratch,
        budget: work,
    };
    crate::run_blocking(
        runtime,
        adapter.prepare_owner(spec, owner(1), input, prior, &mut context),
    )
}

#[test]
fn certified_manifest_materializes_stable_member_identity_through_reorder_and_readback()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let (source, _source_payload) = semantic_source(&store, "symbols.rs", b"source")?;
    let mut work = budget()?;
    let (bridge, calls, inputs) = certified_witness(&work)?;
    let recipe = bridge.certified_chunk_recipe(PROJECTION, PRODUCER, SCHEMA, 1024, 4)?;
    let spec = certified_spec(&recipe);
    let adapter = NativeSemantic::new(bridge, recipe.clone())?;
    adapter.validate_instance(&spec)?;
    let first_members = [
        NativeSemanticManifestMember {
            id: Digest([11; 32]),
            text: "alpha",
        },
        NativeSemanticManifestMember {
            id: Digest([12; 32]),
            text: "βeta",
        },
    ];
    let header = certified_header(Some(source), 1);
    let (first_input, first_payload) = certified_input(
        &store,
        &recipe,
        header,
        manifest_bytes(header, &first_members)?,
    )?;
    let candidate = prepare_certified(
        &runtime,
        &adapter,
        &spec,
        &store,
        first_input,
        None,
        &mut work,
    )?;
    check_eq!(candidate.artifact.payload_ref, first_payload);
    check_eq!(candidate.artifact.chunks.len(), 2);
    let first_member = candidate
        .artifact
        .chunks
        .first()
        .ok_or("first chunk missing")?;
    let second_member = candidate
        .artifact
        .chunks
        .get(1)
        .ok_or("second chunk missing")?;
    check_eq!(first_member.id, Digest([11; 32]));
    check_eq!(second_member.id, Digest([12; 32]));
    check_eq!(first_member.input_digest, ALPHA_DIGEST);
    check_eq!(second_member.input_digest, BETA_DIGEST);
    check_eq!(
        first_member.location,
        crate::native::ChunkLocation::RenderedMember { ordinal: 0 }
    );
    check_eq!(
        second_member.location,
        crate::native::ChunkLocation::RenderedMember { ordinal: 1 }
    );
    let first_id = first_member.id;
    let second_id = second_member.id;
    check_eq!(
        candidate
            .artifact
            .vectors
            .iter()
            .find(|vector| vector.id.as_slice() == first_id.0)
            .ok_or("first vector missing")?
            .values,
        vec![1.0, 0.0]
    );
    check_eq!(
        candidate
            .artifact
            .vectors
            .iter()
            .find(|vector| vector.id.as_slice() == second_id.0)
            .ok_or("second vector missing")?
            .values,
        vec![0.0, 1.0]
    );
    let first = {
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
        crate::run_blocking(&runtime, adapter.verify_owner(candidate, &mut context))?
    };
    let loaded = load_owner_artifact(&store, &mut work, &first, &recipe)?;
    check_eq!(loaded.chunks.len(), 2);
    check_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    let reusable = prepare_certified(
        &runtime,
        &adapter,
        &spec,
        &store,
        first_input,
        Some(first),
        &mut work,
    )?;
    check_eq!(reusable.artifact.payload_ref, first_payload);
    check_eq!(reusable.artifact.chunks, loaded.chunks);
    check_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    let reusable = {
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
        crate::run_blocking(&runtime, adapter.verify_owner(reusable, &mut context))?
    };

    let second_members = [
        NativeSemanticManifestMember {
            id: Digest([12; 32]),
            text: "βeta",
        },
        NativeSemanticManifestMember {
            id: Digest([11; 32]),
            text: "alpha",
        },
    ];
    let second_header = certified_header(Some(source), 2);
    let (second_input, second_payload) = certified_input(
        &store,
        &recipe,
        second_header,
        manifest_bytes(second_header, &second_members)?,
    )?;
    let second = prepare_certified(
        &runtime,
        &adapter,
        &spec,
        &store,
        second_input,
        Some(reusable),
        &mut work,
    )?;
    check_eq!(second.artifact.payload_ref, second_payload);
    check_eq!(second.artifact.chunks.len(), 2);
    let shifted_b = second.artifact.chunks.first().ok_or("shifted b missing")?;
    let shifted_a = second.artifact.chunks.get(1).ok_or("shifted a missing")?;
    check_eq!(shifted_a.id, first_id);
    check_eq!(shifted_b.id, second_id);
    check_eq!(shifted_a.input_digest, ALPHA_DIGEST);
    check_eq!(shifted_b.input_digest, BETA_DIGEST);
    check_eq!(
        shifted_a.location,
        crate::native::ChunkLocation::RenderedMember { ordinal: 1 }
    );
    check_eq!(
        shifted_b.location,
        crate::native::ChunkLocation::RenderedMember { ordinal: 0 }
    );
    check_eq!(
        second
            .artifact
            .vectors
            .iter()
            .find(|vector| vector.id.as_slice() == first_id.0)
            .ok_or("reordered first vector missing")?
            .values,
        vec![1.0, 0.0]
    );
    check_eq!(
        second
            .artifact
            .vectors
            .iter()
            .find(|vector| vector.id.as_slice() == second_id.0)
            .ok_or("reordered second vector missing")?
            .values,
        vec![0.0, 1.0]
    );
    check_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    let second_binding = {
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
        crate::run_blocking(&runtime, adapter.verify_owner(second, &mut context))?
    };
    let second_artifact = load_owner_artifact(&store, &mut work, &second_binding, &recipe)?;
    check_eq!(second_artifact.chunks.len(), 2);

    let third_members = [
        NativeSemanticManifestMember {
            id: Digest([13; 32]),
            text: "prefix",
        },
        NativeSemanticManifestMember {
            id: Digest([11; 32]),
            text: "alph",
        },
        NativeSemanticManifestMember {
            id: Digest([12; 32]),
            text: "βeta",
        },
    ];
    let third_header = certified_header(Some(source), 3);
    let (third_input, third_payload) = certified_input(
        &store,
        &recipe,
        third_header,
        manifest_bytes(third_header, &third_members)?,
    )?;
    let third = prepare_certified(
        &runtime,
        &adapter,
        &spec,
        &store,
        third_input,
        Some(second_binding),
        &mut work,
    )?;
    check_eq!(third.artifact.payload_ref, third_payload);
    check_eq!(third.artifact.chunks.len(), 3);
    let changed_a = third.artifact.chunks.get(1).ok_or("changed a missing")?;
    let retained_b = third.artifact.chunks.get(2).ok_or("retained b missing")?;
    check_eq!(changed_a.id, first_id);
    check_eq!(changed_a.input_digest, ALPH_DIGEST);
    check_eq!(retained_b.id, second_id);
    check_eq!(retained_b.input_digest, BETA_DIGEST);
    check_eq!(calls.load(AtomicOrdering::SeqCst), 2);
    check_eq!(
        third
            .artifact
            .vectors
            .iter()
            .find(|vector| vector.id.as_slice() == second_id.0)
            .ok_or("retained vector missing")?
            .values,
        vec![0.0, 1.0]
    );
    let final_binding = {
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
        crate::run_blocking(&runtime, adapter.verify_owner(third, &mut context))?
    };
    let final_artifact = load_owner_artifact(&store, &mut work, &final_binding, &recipe)?;
    check_eq!(final_artifact.chunks.len(), 3);
    check_eq!(
        final_artifact
            .chunks
            .get(1)
            .ok_or("persisted shifted a missing")?
            .id,
        first_id
    );
    check_eq!(
        inputs
            .lock()
            .map_err(|_error| "test input lock")?
            .as_slice(),
        &[
            vec!["alpha".to_owned(), "βeta".to_owned()],
            vec!["prefix".to_owned(), "alph".to_owned()]
        ]
    );
    Ok(())
}

#[test]
fn certified_manifest_reembeds_all_members_without_text_only_provider_capability()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let mut work = budget()?;
    let (bridge, calls, inputs) =
        certified_witness_with_properties(&work, CertifiedBatchShape::Exact, false)?;
    let recipe = bridge.certified_chunk_recipe(PROJECTION, PRODUCER, SCHEMA, 1024, 4)?;
    let spec = certified_spec(&recipe);
    let adapter = NativeSemantic::new(bridge, recipe.clone())?;
    let first_members = [
        NativeSemanticManifestMember {
            id: Digest([11; 32]),
            text: "alpha",
        },
        NativeSemanticManifestMember {
            id: Digest([12; 32]),
            text: "βeta",
        },
    ];
    let first_header = certified_header(None, 1);
    let (first_input, _) = certified_input(
        &store,
        &recipe,
        first_header,
        manifest_bytes(first_header, &first_members)?,
    )?;
    let first = prepare_certified(
        &runtime,
        &adapter,
        &spec,
        &store,
        first_input,
        None,
        &mut work,
    )?;
    let first_binding = {
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
        crate::run_blocking(&runtime, adapter.verify_owner(first, &mut context))?
    };
    let second_members = [
        NativeSemanticManifestMember {
            id: Digest([12; 32]),
            text: "βeta",
        },
        NativeSemanticManifestMember {
            id: Digest([11; 32]),
            text: "alpha",
        },
    ];
    let second_header = certified_header(None, 2);
    let (second_input, _) = certified_input(
        &store,
        &recipe,
        second_header,
        manifest_bytes(second_header, &second_members)?,
    )?;
    let second = prepare_certified(
        &runtime,
        &adapter,
        &spec,
        &store,
        second_input,
        Some(first_binding),
        &mut work,
    )?;
    check_eq!(second.artifact.chunks.len(), 2);
    check_eq!(
        second
            .artifact
            .chunks
            .first()
            .ok_or("reordered first chunk missing")?
            .id,
        Digest([12; 32])
    );
    check_eq!(
        second
            .artifact
            .chunks
            .get(1)
            .ok_or("reordered second chunk missing")?
            .id,
        Digest([11; 32])
    );
    check_eq!(calls.load(AtomicOrdering::SeqCst), 2);
    check_eq!(
        inputs
            .lock()
            .map_err(|_error| "test input lock")?
            .as_slice(),
        &[
            vec!["alpha".to_owned(), "βeta".to_owned()],
            vec!["βeta".to_owned(), "alpha".to_owned()]
        ]
    );
    Ok(())
}

#[test]
fn certified_manifest_rejects_duplicate_id_and_foreign_binding_before_provider()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let mut work = budget()?;
    let (bridge, calls, _inputs) = certified_witness(&work)?;
    let recipe = bridge.certified_chunk_recipe(PROJECTION, PRODUCER, SCHEMA, 1024, 4)?;
    let spec = certified_spec(&recipe);
    let adapter = NativeSemantic::new(bridge, recipe.clone())?;
    let members = [
        NativeSemanticManifestMember {
            id: Digest([11; 32]),
            text: "alpha",
        },
        NativeSemanticManifestMember {
            id: Digest([11; 32]),
            text: "βeta",
        },
    ];
    let header = certified_header(None, 1);
    let (input, _) = certified_input(&store, &recipe, header, manifest_bytes(header, &members)?)?;
    check!(matches!(
        prepare_certified(&runtime, &adapter, &spec, &store, input, None, &mut work),
        Err(IndexError::InvalidInput(_) | IndexError::CorruptObject(_))
    ));

    let unique = [NativeSemanticManifestMember {
        id: Digest([11; 32]),
        text: "alpha",
    }];
    for variant in 0..5 {
        let mut forged = header;
        match variant {
            0 => forged.owner = owner(2),
            1 => forged.projection = ProjectionId([88; 16]),
            2 => forged.producer = ProducerId([88; 16]),
            3 => forged.schema = SchemaId([88; 16]),
            4 => forged.dependency_stamp = Digest([88; 32]),
            _ => return Err("unknown forged header case".into()),
        }
        let (input, _) =
            certified_input(&store, &recipe, header, manifest_bytes(forged, &unique)?)?;
        check!(matches!(
            prepare_certified(&runtime, &adapter, &spec, &store, input, None, &mut work),
            Err(IndexError::InvalidInput(_) | IndexError::CorruptObject(_))
        ));
    }
    check_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    Ok(())
}

#[test]
fn invalid_certified_manifest_does_not_call_provider_identity_or_embedding_hook()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime = test_runtime()?;
    for fault in 0..3 {
        let store = MemoryStore::default();
        let mut work = budget()?;
        let (bridge, text_calls, _inputs) = certified_witness(&work)?;
        let recipe = bridge.certified_chunk_recipe(PROJECTION, PRODUCER, SCHEMA, 1024, 4)?;
        let spec = certified_spec(&recipe);
        let identity_calls = Arc::new(AtomicUsize::new(0));
        let embed_calls = Arc::new(AtomicUsize::new(0));
        let adapter = NativeSemantic::new(
            ReuseHookWitness {
                bridge,
                identity_calls: Arc::clone(&identity_calls),
                embed_calls: Arc::clone(&embed_calls),
            },
            recipe.clone(),
        )?;
        let header = certified_header(None, 1);
        let mut wire_header = header;
        let mut members = vec![NativeSemanticManifestMember {
            id: Digest([11; 32]),
            text: "alpha",
        }];
        if fault == 0 {
            wire_header.owner = owner(2);
        } else if fault == 1 {
            members.push(NativeSemanticManifestMember {
                id: Digest([11; 32]),
                text: "βeta",
            });
        }
        let (mut input, _) = certified_input(
            &store,
            &recipe,
            header,
            manifest_bytes(wire_header, &members)?,
        )?;
        if fault == 2 {
            input.producer_context = Digest([77; 32]);
        }
        let before = store.0.lock().map_err(|_error| "test store lock")?.len();
        check!(matches!(
            prepare_certified(&runtime, &adapter, &spec, &store, input, None, &mut work),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(identity_calls.load(AtomicOrdering::SeqCst), 0);
        check_eq!(embed_calls.load(AtomicOrdering::SeqCst), 0);
        check_eq!(text_calls.load(AtomicOrdering::SeqCst), 0);
        check_eq!(work.usage().writes, 0);
        let after = store.0.lock().map_err(|_error| "test store lock")?.len();
        check_eq!(after, before);
    }
    Ok(())
}

#[test]
fn certified_chunk_provider_wrong_cardinality_refuses_before_artifact_put()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime = test_runtime()?;
    for shape in [CertifiedBatchShape::Missing, CertifiedBatchShape::Extra] {
        let store = MemoryStore::default();
        let mut work = budget()?;
        let (bridge, calls, _inputs) = certified_witness_with_shape(&work, shape)?;
        let recipe = bridge.certified_chunk_recipe(PROJECTION, PRODUCER, SCHEMA, 1024, 4)?;
        let spec = certified_spec(&recipe);
        let adapter = NativeSemantic::new(bridge, recipe.clone())?;
        let header = certified_header(None, 1);
        let members = [
            NativeSemanticManifestMember {
                id: Digest([11; 32]),
                text: "alpha",
            },
            NativeSemanticManifestMember {
                id: Digest([12; 32]),
                text: "βeta",
            },
        ];
        let (input, _) =
            certified_input(&store, &recipe, header, manifest_bytes(header, &members)?)?;
        let before = store.0.lock().map_err(|_error| "test store lock")?.len();
        check!(matches!(
            prepare_certified(&runtime, &adapter, &spec, &store, input, None, &mut work),
            Err(IndexError::BackendViolation(_))
        ));
        check_eq!(calls.load(AtomicOrdering::SeqCst), 1);
        check_eq!(work.usage().writes, 0);
        let after = store.0.lock().map_err(|_error| "test store lock")?.len();
        check_eq!(after, before);
    }
    Ok(())
}

#[test]
fn certified_manifest_rejects_source_context_truncation_and_invalid_utf8()
-> Result<(), Box<dyn std::error::Error>> {
    const COUNT_OFFSET_WITH_SOURCE: usize = 14 + 32 + 16 + 16 + 16 + 1 + 32 + 32;
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let (source, _payload) = semantic_source(&store, "symbols.rs", b"source")?;
    let mut work = budget()?;
    let (bridge, calls, _inputs) = certified_witness(&work)?;
    let recipe = bridge.certified_chunk_recipe(PROJECTION, PRODUCER, SCHEMA, 1024, 4)?;
    let spec = certified_spec(&recipe);
    let adapter = NativeSemantic::new(bridge, recipe.clone())?;
    let header = certified_header(Some(source), 1);
    let members = [NativeSemanticManifestMember {
        id: Digest([11; 32]),
        text: "z",
    }];
    let mut foreign_source = header;
    foreign_source.source = None;
    let (input, _) = certified_input(
        &store,
        &recipe,
        header,
        manifest_bytes(foreign_source, &members)?,
    )?;
    check!(matches!(
        prepare_certified(&runtime, &adapter, &spec, &store, input, None, &mut work),
        Err(IndexError::InvalidInput(_) | IndexError::CorruptObject(_))
    ));

    let valid = manifest_bytes(header, &members)?;
    let (mut input, _) = certified_input(&store, &recipe, header, valid.clone())?;
    input.producer_context = Digest([77; 32]);
    check!(matches!(
        prepare_certified(&runtime, &adapter, &spec, &store, input, None, &mut work),
        Err(IndexError::InvalidInput(_) | IndexError::CorruptObject(_))
    ));

    let mut truncated = valid.clone();
    let _discarded = truncated.pop().ok_or("empty manifest fixture")?;
    let mut invalid_utf8 = valid.clone();
    *invalid_utf8.last_mut().ok_or("empty manifest fixture")? = 0xff;
    for malformed in [truncated, invalid_utf8] {
        let (input, _) = certified_input(&store, &recipe, header, malformed)?;
        check!(matches!(
            prepare_certified(&runtime, &adapter, &spec, &store, input, None, &mut work),
            Err(IndexError::InvalidInput(_) | IndexError::CorruptObject(_))
        ));
    }
    // Fixed v1 wire: magic, owner, projection, producer, schema, source tag
    // and source ref, then dependency stamp before the four-byte member count.
    let count_end = COUNT_OFFSET_WITH_SOURCE
        .checked_add(4)
        .ok_or("manifest count offset overflow")?;
    check_eq!(
        valid
            .get(COUNT_OFFSET_WITH_SOURCE..count_end)
            .ok_or("manifest count bytes absent")?,
        [0_u8, 0, 0, 1].as_slice()
    );
    for count in [0_u32, 5_u32] {
        let mut malformed = valid.clone();
        malformed
            .get_mut(COUNT_OFFSET_WITH_SOURCE..count_end)
            .ok_or("manifest count bytes absent")?
            .copy_from_slice(&count.to_be_bytes());
        let (input, _) = certified_input(&store, &recipe, header, malformed)?;
        check!(matches!(
            prepare_certified(&runtime, &adapter, &spec, &store, input, None, &mut work),
            Err(IndexError::InvalidInput(_) | IndexError::CorruptObject(_))
        ));
    }
    check_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    Ok(())
}

#[test]
fn certified_manifest_cancelled_request_does_not_enter_provider_or_put_artifact()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime = test_runtime()?;
    let store = MemoryStore::default();
    let mut work = budget()?;
    let (bridge, calls, _inputs) = certified_witness(&work)?;
    let recipe = bridge.certified_chunk_recipe(PROJECTION, PRODUCER, SCHEMA, 1024, 4)?;
    let spec = certified_spec(&recipe);
    let adapter = NativeSemantic::new(bridge, recipe.clone())?;
    let header = certified_header(None, 1);
    let members = [NativeSemanticManifestMember {
        id: Digest([11; 32]),
        text: "alpha",
    }];
    let (input, _) = certified_input(&store, &recipe, header, manifest_bytes(header, &members)?)?;
    let before = store.0.lock().map_err(|_error| "test store lock")?.len();
    work.request().cancel_handle().cancel();
    check!(matches!(
        prepare_certified(&runtime, &adapter, &spec, &store, input, None, &mut work),
        Err(IndexError::Cancelled)
    ));
    check_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    check_eq!(work.usage().writes, 0);
    let after = store.0.lock().map_err(|_error| "test store lock")?.len();
    check_eq!(after, before);
    Ok(())
}
