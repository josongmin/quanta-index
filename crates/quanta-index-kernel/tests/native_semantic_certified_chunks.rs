//! Certified projection members through real Kernel publication and semantic reads.
use std::{
    error::Error,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{
    EmbeddingNormalization,
    native_protocol::{
        NativeSemanticChunkSpan, NativeSemanticManifestHeader, NativeSemanticManifestMember,
    },
};
use quanta_index_core::index::{
    CollectionId, Digest, GroupActivation, IndexInstanceId, IndexKind, IndexProfile, InputDomain,
    InstanceSpec, ObjectRef, OperationKey, OperationStatus, OwnerKey, OwnerMutation,
    ProducerCompleteness, ProducerId, ProjectionBinding, ProjectionCertificate, ProjectionId,
    ProjectionOwnerContribution, ProjectionTransition, PublicationGroupId, SchemaId, SlotId,
    SourceEvent, SourceTextAdmission, SourceTransition, StableOwnerId, StorageFormatId, StreamId,
    SubmitOutcome, TenantId, UpdateKind, UpdateRequest, ViewRef, ViewSelection, WorkBudget,
    WorkLimits, WorkspaceKey, WorktreeId, ports::CatalogReadPort,
};
use quanta_index_core::{CoreError, RequestBudgetV1, TextEmbeddingProvider};
use quanta_index_kernel::{AdapterDescriptor, AdapterRegistry, Kernel, KernelServices};
use quanta_index_semantic::native::{
    EmbeddingRecipe, NativeSemantic, ReadContext, SearchResult, TextEmbeddingBridge,
    TicketSearchQuery, encode_certified_manifest, search_exact_acquired,
};
use quanta_index_storage::FileObjectStore;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
type OpenedKernel = (
    Kernel<SqliteCatalog>,
    Arc<FileObjectStore>,
    Arc<SqliteCatalog>,
    EmbeddingRecipe,
);

macro_rules! check {
    ($condition:expr $(,)?) => {
        if !$condition {
            return Err(format!("assertion failed: {}", stringify!($condition)).into());
        }
    };
}

macro_rules! check_eq {
    ($left:expr, $right:expr $(,)?) => {
        if &$left != &$right {
            return Err(format!(
                "assertion failed: {} == {}",
                stringify!($left),
                stringify!($right)
            )
            .into());
        }
    };
}
const SOURCE: ProducerId = ProducerId([51; 16]);
const RUNTIME: ProducerId = ProducerId([52; 16]);
const PROJECTION: ProjectionId = ProjectionId([53; 16]);
const INSTANCE: IndexInstanceId = IndexInstanceId([54; 16]);
const SCHEMA: SchemaId = SchemaId([55; 16]);
const FORMAT: StorageFormatId = StorageFormatId([56; 16]);
const KIND: IndexKind = IndexKind([57; 16]);
const ALPHA_DIGEST: Digest = Digest([
    0x8e, 0xd3, 0xf6, 0xad, 0x68, 0x5b, 0x95, 0x9e, 0xad, 0x70, 0x22, 0x51, 0x8e, 0x1a, 0xf7, 0x6c,
    0xd8, 0x16, 0xf8, 0xe8, 0xec, 0x7c, 0xcd, 0xda, 0x1e, 0xd4, 0x01, 0x8e, 0x8f, 0x22, 0x23, 0xf8,
]);
const ALPH_DIGEST: Digest = Digest([
    0x24, 0x29, 0xa9, 0x9e, 0x01, 0xb3, 0x51, 0xde, 0x98, 0x7b, 0x50, 0x81, 0xcb, 0x49, 0xe4, 0xd8,
    0xf9, 0xaf, 0x4d, 0x1a, 0x72, 0x69, 0xcc, 0x9c, 0x0b, 0xe9, 0x36, 0x90, 0xf1, 0xb6, 0x86, 0xa3,
]);

fn budget() -> WorkBudget {
    WorkBudget::with_request(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 128 * 1024 * 1024,
            mutations: 1_000_000,
        },
        RequestBudgetV1::for_duration(Duration::from_secs(30)),
    )
}

fn workspace() -> WorkspaceKey {
    WorkspaceKey {
        tenant: TenantId([1; 16]),
        collection: CollectionId([2; 16]),
        worktree: WorktreeId([3; 16]),
        slot: SlotId([4; 16]),
    }
}

fn owner(producer: ProducerId) -> OwnerKey {
    OwnerKey {
        producer,
        stable_id: StableOwnerId([9; 16]),
    }
}

fn operation(producer: ProducerId, stream: u8, sequence: u64) -> OperationKey {
    OperationKey {
        workspace: workspace(),
        epoch: 1,
        producer,
        stream: StreamId([stream; 16]),
        stream_epoch: 1,
        sequence,
    }
}

fn published(outcome: SubmitOutcome) -> TestResult<ViewRef> {
    match outcome {
        SubmitOutcome::Terminal(OperationStatus::Published(view)) => Ok(view),
        other @ (SubmitOutcome::Accepted(_)
        | SubmitOutcome::Terminal(_)
        | SubmitOutcome::Unknown(_)) => Err(format!("publication failed: {other:?}").into()),
    }
}

fn current(kernel: &Kernel<SqliteCatalog>) -> TestResult<quanta_index_core::index::CatalogView> {
    Ok(kernel
        .pin_view(
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            },
            &budget(),
        )?
        .view()
        .clone())
}

fn binding(
    objects: &FileObjectStore,
    root: quanta_index_core::index::MapRoot,
) -> TestResult<ProjectionBinding> {
    use quanta_index_core::index::ports::{ImmutableObjectPort, PersistentMapPort};
    let mut key = b"projection\0".to_vec();
    key.extend_from_slice(&PROJECTION.0);
    let reference = objects
        .lookup_object(root, &key, &mut budget())?
        .ok_or("projection absent")?;
    Ok(serde_json::from_slice(
        &objects.get(reference, &mut budget())?.payload,
    )?)
}

struct FixedTextProvider {
    calls: Arc<AtomicUsize>,
    batches: Arc<Mutex<Vec<Vec<String>>>>,
}

impl TextEmbeddingProvider for FixedTextProvider {
    fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        let vectors: Result<Vec<Vec<f32>>, CoreError> = texts
            .iter()
            .map(|text| match *text {
                "alpha" | "alph" | "prefix" => Ok(vec![1.0, 0.0]),
                "βeta" => Ok(vec![0.0, 1.0]),
                _ => Err(CoreError::InvalidContract(
                    "unknown fixed semantic text".into(),
                )),
            })
            .collect();
        Ok(quanta_index_core::TextEmbeddingBatch::unscoped(vectors?))
    }

    fn embed_batch_within(
        &self,
        texts: &[&str],
        budget: &RequestBudgetV1,
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        if budget.is_cancelled() {
            return Err(CoreError::InvalidContract(
                "cancelled semantic fixture".into(),
            ));
        }
        let memory = quanta_index_core::TextEmbeddingBatch::reserve_output(
            texts.len(),
            self.dimension(),
            budget,
        )?;
        self.batches
            .lock()
            .map_err(|_error| CoreError::InvalidContract("certified batch lock".into()))?
            .push(texts.iter().map(|text| (*text).to_owned()).collect());
        let _previous = self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(quanta_index_core::TextEmbeddingBatch::from_parts(
            self.embed_batch(texts)?.into_unscoped()?,
            memory,
        ))
    }

    fn model_id(&self) -> &'static str {
        "kernel-certified-chunk-gold"
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
        true
    }
}

// Deliberately leaves the optional text-only reuse capability at its default
// false. A stateful provider must derive all selected texts for a new payload.
struct StatefulTextProvider(FixedTextProvider);

impl TextEmbeddingProvider for StatefulTextProvider {
    fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        self.0.embed_batch(texts)
    }

    fn embed_batch_within(
        &self,
        texts: &[&str],
        budget: &RequestBudgetV1,
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        self.0.embed_batch_within(texts, budget)
    }

    fn model_id(&self) -> &str {
        self.0.model_id()
    }

    fn model_revision(&self) -> &str {
        self.0.model_revision()
    }

    fn dimension(&self) -> usize {
        self.0.dimension()
    }

    fn normalization(&self) -> EmbeddingNormalization {
        self.0.normalization()
    }
}

fn open_kernel<P: TextEmbeddingProvider + 'static>(
    root: &Path,
    provider: P,
) -> TestResult<OpenedKernel> {
    let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
        root.join("scratch"),
        16 * 1024 * 1024 * 1024,
    )?);
    let objects = Arc::new(FileObjectStore::open_with_scratch(
        root.join("objects"),
        scratch.clone(),
    )?);
    let catalog = Arc::new(SqliteCatalog::open(root, Duration::from_millis(100))?);
    let bridge = TextEmbeddingBridge::with_certified_chunks(provider)?;
    let recipe = bridge.certified_chunk_recipe(PROJECTION, RUNTIME, SCHEMA, 1024, 4)?;
    let mut registry = AdapterRegistry::new();
    registry.register_owner(
        AdapterDescriptor {
            kind: KIND,
            schema: SCHEMA,
            format: FORMAT,
        },
        NativeSemantic::new(bridge, recipe.clone())?,
    )?;
    Ok((
        Kernel::new(
            KernelServices::new(
                objects.clone(),
                objects.clone(),
                catalog.clone(),
                scratch,
                Duration::from_secs(30),
            )?,
            registry,
        ),
        objects,
        catalog,
        recipe,
    ))
}

fn spec(recipe: &EmbeddingRecipe) -> InstanceSpec {
    InstanceSpec {
        instance: INSTANCE,
        kind: KIND,
        group: PublicationGroupId([72; 16]),
        activation: GroupActivation::AsyncCoverage,
        input_domain: InputDomain::Projection {
            projection: PROJECTION,
            producer: RUNTIME,
        },
        schema: SCHEMA,
        policy: recipe.fingerprint(),
        format: FORMAT,
    }
}

async fn configure(
    kernel: &Kernel<SqliteCatalog>,
    recipe: &EmbeddingRecipe,
) -> TestResult<ViewRef> {
    let genesis = kernel.create_workspace(workspace(), 1, &mut budget())?;
    let key = operation(SOURCE, 60, 1);
    let ingress = kernel.begin_input(key)?;
    let profile = kernel.stage_profile(
        ingress,
        &IndexProfile {
            instances: vec![spec(recipe)],
        },
        &mut budget(),
    )?;
    let mut submit_budget = budget();
    published(
        kernel
            .submit(
                UpdateRequest {
                    operation: key,
                    expected_view: genesis,
                    kind: UpdateKind::Reconfigure { profile },
                },
                &mut submit_budget,
            )
            .await?,
    )
}

async fn advance_source(
    kernel: &Kernel<SqliteCatalog>,
    view: ViewRef,
) -> TestResult<(ViewRef, ObjectRef)> {
    let key = operation(SOURCE, 60, 2);
    let ingress = kernel.begin_input(key)?;
    let record = kernel.stage_source_text(
        ingress,
        "symbols.rs",
        b"source",
        SourceTextAdmission::Utf8 {
            policy: Digest([8; 32]),
        },
        &mut budget(),
    )?;
    let owners = kernel.stage_mutations(
        ingress,
        std::iter::once(Ok(OwnerMutation {
            owner: owner(SOURCE),
            expected: None,
            next: Some(record),
        })),
        &mut budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: current(kernel)?.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: SOURCE,
        stream: StreamId([60; 16]),
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    let mut submit_budget = budget();
    let next = published(
        kernel
            .submit(
                UpdateRequest {
                    operation: key,
                    expected_view: view,
                    kind: UpdateKind::SourceAdvance(transition),
                },
                &mut submit_budget,
            )
            .await?,
    )?;
    Ok((next, record))
}

struct ProjectionUpdate<'a> {
    view: ViewRef,
    source_record: ObjectRef,
    prior: Option<ProjectionBinding>,
    sequence: u64,
    expected: Option<ObjectRef>,
    members: Option<&'a [NativeSemanticManifestMember<'a>]>,
}

async fn advance_projection(
    kernel: &Kernel<SqliteCatalog>,
    request: ProjectionUpdate<'_>,
) -> TestResult<(ViewRef, Option<ObjectRef>, Option<ObjectRef>)> {
    let key = operation(RUNTIME, 63, request.sequence);
    let ingress = kernel.begin_input(key)?;
    let stamp = Digest([u8::try_from(request.sequence)?; 32]);
    let (next, output) = if let Some(members) = request.members {
        let header = NativeSemanticManifestHeader {
            owner: owner(RUNTIME),
            projection: PROJECTION,
            producer: RUNTIME,
            schema: SCHEMA,
            source: Some(request.source_record),
            dependency_stamp: stamp,
        };
        let mut staging = budget();
        let payload = encode_certified_manifest(header, members, &mut staging)?;
        let output = kernel.stage_projection_output(ingress, payload.as_ref(), &mut staging)?;
        let contribution = ProjectionOwnerContribution {
            projection: PROJECTION,
            producer: RUNTIME,
            owner: owner(RUNTIME),
            source_owner: Some(owner(SOURCE)),
            source: Some(request.source_record),
            dependency_stamp: stamp,
            output: Some(output),
        };
        let next = kernel.stage_projection_owner(ingress, contribution, &mut staging)?;
        (Some(next), Some(output))
    } else {
        (None, None)
    };
    let affected = kernel.stage_mutations(
        ingress,
        std::iter::once(Ok(OwnerMutation {
            owner: owner(RUNTIME),
            expected: request.expected,
            next,
        })),
        &mut budget(),
    )?;
    let source = current(kernel)?.source;
    let mut transition = ProjectionTransition {
        projection: PROJECTION,
        producer: RUNTIME,
        expected: request.prior,
        target_source: source,
        schema: SCHEMA,
        producer_policy: Digest([61; 32]),
        dependency_context: Digest([62; 32]),
        completeness: ProducerCompleteness::Partial {
            missing: quanta_index_core::index::MapRoot(None),
        },
        owners: affected,
        events: Vec::new(),
    };
    let certificate = ProjectionCertificate {
        projection: PROJECTION,
        producer: RUNTIME,
        prior: request.prior,
        target_source: source,
        schema: SCHEMA,
        producer_policy: transition.producer_policy,
        dependency_context: transition.dependency_context,
        affected,
    };
    let certificate = kernel.stage_projection_certificate(ingress, &certificate, &mut budget())?;
    transition.completeness = ProducerCompleteness::CompleteAffectedDomain { certificate };
    transition.events.push(SourceEvent {
        producer: RUNTIME,
        stream: StreamId([63; 16]),
        stream_epoch: 1,
        predecessor: request
            .sequence
            .checked_sub(1)
            .ok_or("projection sequence starts at one")?,
        sequence: request.sequence,
        payload: transition.payload_digest()?,
    });
    let mut submit_budget = budget();
    let view = published(
        kernel
            .submit(
                UpdateRequest {
                    operation: key,
                    expected_view: request.view,
                    kind: UpdateKind::ProjectionAdvance(Box::new(transition)),
                },
                &mut submit_budget,
            )
            .await?,
    )?;
    Ok((view, next, output))
}

async fn materialize(
    kernel: &Kernel<SqliteCatalog>,
    view: ViewRef,
    sequence: u64,
) -> TestResult<ViewRef> {
    let ready = kernel.ready_work(workspace(), 1, 8, &mut budget())?;
    let [intent] = ready.as_slice() else {
        return Err("expected one certified owner job".into());
    };
    let claim = kernel.claim_work(intent, Duration::from_secs(30), &budget())?;
    let mut submit_budget = budget();
    published(
        kernel
            .submit(
                UpdateRequest {
                    operation: operation(SOURCE, 60, sequence),
                    expected_view: view,
                    kind: UpdateKind::Materialize { claim },
                },
                &mut submit_budget,
            )
            .await?,
    )
}

fn query(
    catalog: &SqliteCatalog,
    objects: &FileObjectStore,
    recipe: &EmbeddingRecipe,
    selection: ViewSelection,
) -> TestResult<SearchResult> {
    let session = catalog.open_read_session()?;
    let mut read_budget = budget();
    let mut read = ReadContext {
        objects,
        maps: objects,
        budget: &mut read_budget,
    };
    Ok(search_exact_acquired(
        catalog,
        session.as_ref(),
        Duration::from_secs(30),
        TicketSearchQuery {
            selection,
            instance: INSTANCE,
            format: FORMAT,
            recipe,
            vector: &[1.0, 0.0],
            top_k: 4,
            page_size: 8,
            max_owners: 8,
        },
        &mut read,
    )?)
}

#[tokio::test]
#[expect(
    clippy::disallowed_methods,
    reason = "the Tokio test harness drives Kernel futures in this integration test"
)]
async fn certified_members_survive_published_delta_delete_old_pin_and_reopen() -> TestResult {
    let directory = tempfile::tempdir()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let batches = Arc::new(Mutex::new(Vec::new()));
    let (kernel, objects, catalog, recipe) = open_kernel(
        directory.path(),
        FixedTextProvider {
            calls: Arc::clone(&calls),
            batches: Arc::clone(&batches),
        },
    )?;
    let configured = configure(&kernel, &recipe).await?;
    let (sourced, source_record) = advance_source(&kernel, configured).await?;
    let initial_members = [
        NativeSemanticManifestMember {
            id: Digest([11; 32]),
            text: "alpha",
        },
        NativeSemanticManifestMember {
            id: Digest([12; 32]),
            text: "βeta",
        },
    ];
    let (first_pending, first_contribution, first_output) = advance_projection(
        &kernel,
        ProjectionUpdate {
            view: sourced,
            source_record,
            prior: None,
            sequence: 1,
            expected: None,
            members: Some(&initial_members),
        },
    )
    .await?;
    let first_contribution = first_contribution.ok_or("first contribution absent")?;
    let first_output = first_output.ok_or("first output absent")?;
    check!(
        query(
            &catalog,
            &objects,
            &recipe,
            ViewSelection::Exact(first_pending)
        )?
        .hits
        .is_empty()
    );
    let first = materialize(&kernel, first_pending, 3).await?;
    let old_pin = kernel.pin_view(ViewSelection::Exact(first), &budget())?;
    let first_result = query(&catalog, &objects, &recipe, ViewSelection::Exact(first))?;
    check_eq!(first_result.hits.len(), 2);
    let alpha = first_result
        .hits
        .iter()
        .find(|hit| hit.vector_id.as_slice() == [11_u8; 32].as_slice())
        .ok_or("first alpha missing")?;
    let alpha_chunk = alpha.chunk.as_ref().ok_or("first alpha chunk missing")?;
    check_eq!(alpha.owner, owner(RUNTIME));
    check_eq!(alpha.cosine.total_cmp(&1.0), std::cmp::Ordering::Equal);
    check_eq!(alpha_chunk.id, Digest([11; 32]));
    check_eq!(alpha_chunk.input_digest, ALPHA_DIGEST);
    check_eq!(alpha_chunk.payload, first_output);
    check_eq!(alpha_chunk.input.source, Some(source_record));
    check_eq!(alpha_chunk.input.projection, Some(first_contribution));
    check_eq!(
        alpha_chunk.span,
        NativeSemanticChunkSpan::RenderedMember { ordinal: 0 }
    );
    alpha_chunk.validate_binding(&alpha.vector_id, recipe.fingerprint())?;
    check_eq!(calls.load(Ordering::SeqCst), 1);
    check_eq!(
        batches
            .lock()
            .map_err(|_error| "certified batch lock")?
            .as_slice(),
        &[vec!["alpha".to_owned(), "βeta".to_owned()]]
    );

    let first_binding = binding(&objects, current(&kernel)?.projections)?;
    let shifted_members = [
        NativeSemanticManifestMember {
            id: Digest([12; 32]),
            text: "βeta",
        },
        NativeSemanticManifestMember {
            id: Digest([11; 32]),
            text: "alpha",
        },
    ];
    let (second_pending, second_contribution, second_output) = advance_projection(
        &kernel,
        ProjectionUpdate {
            view: first,
            source_record,
            prior: Some(first_binding),
            sequence: 2,
            expected: Some(first_contribution),
            members: Some(&shifted_members),
        },
    )
    .await?;
    let second_contribution = second_contribution.ok_or("second contribution absent")?;
    let second_output = second_output.ok_or("second output absent")?;
    check!(
        query(
            &catalog,
            &objects,
            &recipe,
            ViewSelection::Exact(second_pending)
        )?
        .hits
        .is_empty()
    );
    check_eq!(
        query(&catalog, &objects, &recipe, ViewSelection::Exact(first))?
            .hits
            .len(),
        2
    );
    let second = materialize(&kernel, second_pending, 4).await?;
    let second_result = query(&catalog, &objects, &recipe, ViewSelection::Exact(second))?;
    check_eq!(second_result.hits.len(), 2);
    let shifted_alpha = second_result
        .hits
        .iter()
        .find(|hit| hit.vector_id.as_slice() == [11_u8; 32].as_slice())
        .ok_or("shifted alpha missing")?;
    let shifted_chunk = shifted_alpha
        .chunk
        .as_ref()
        .ok_or("shifted alpha chunk missing")?;
    check_eq!(shifted_chunk.id, alpha_chunk.id);
    check_eq!(shifted_chunk.input_digest, ALPHA_DIGEST);
    check_eq!(shifted_chunk.payload, second_output);
    check_eq!(shifted_chunk.input.projection, Some(second_contribution));
    check_eq!(
        shifted_chunk.span,
        NativeSemanticChunkSpan::RenderedMember { ordinal: 1 }
    );
    check_eq!(calls.load(Ordering::SeqCst), 1);
    check_eq!(
        batches
            .lock()
            .map_err(|_error| "certified batch lock")?
            .as_slice(),
        &[vec!["alpha".to_owned(), "βeta".to_owned()]]
    );

    let second_binding = binding(&objects, current(&kernel)?.projections)?;
    let changed_members = [
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
    let (third_pending, third_contribution, third_output) = advance_projection(
        &kernel,
        ProjectionUpdate {
            view: second,
            source_record,
            prior: Some(second_binding),
            sequence: 3,
            expected: Some(second_contribution),
            members: Some(&changed_members),
        },
    )
    .await?;
    let third_contribution = third_contribution.ok_or("third contribution absent")?;
    let third_output = third_output.ok_or("third output absent")?;
    check!(
        query(
            &catalog,
            &objects,
            &recipe,
            ViewSelection::Exact(third_pending)
        )?
        .hits
        .is_empty()
    );
    let third = materialize(&kernel, third_pending, 5).await?;
    let third_result = query(&catalog, &objects, &recipe, ViewSelection::Exact(third))?;
    check_eq!(third_result.hits.len(), 3);
    let changed_alpha = third_result
        .hits
        .iter()
        .find(|hit| hit.vector_id.as_slice() == [11_u8; 32].as_slice())
        .ok_or("changed alpha missing")?;
    let changed_chunk = changed_alpha
        .chunk
        .as_ref()
        .ok_or("changed alpha chunk missing")?;
    check_eq!(changed_chunk.id, alpha_chunk.id);
    check_eq!(changed_chunk.input_digest, ALPH_DIGEST);
    check_eq!(changed_chunk.payload, third_output);
    check_eq!(changed_chunk.input.projection, Some(third_contribution));
    check_eq!(
        changed_chunk.span,
        NativeSemanticChunkSpan::RenderedMember { ordinal: 1 }
    );
    let retained_beta = third_result
        .hits
        .iter()
        .find(|hit| hit.vector_id.as_slice() == [12_u8; 32].as_slice())
        .ok_or("retained beta missing")?;
    check_eq!(
        retained_beta.cosine.total_cmp(&0.0),
        std::cmp::Ordering::Equal
    );
    check_eq!(calls.load(Ordering::SeqCst), 2);
    check_eq!(
        batches
            .lock()
            .map_err(|_error| "certified batch lock")?
            .as_slice(),
        &[
            vec!["alpha".to_owned(), "βeta".to_owned()],
            vec!["prefix".to_owned(), "alph".to_owned()],
        ]
    );

    let third_binding = binding(&objects, current(&kernel)?.projections)?;
    let (deleted, none, no_output) = advance_projection(
        &kernel,
        ProjectionUpdate {
            view: third,
            source_record,
            prior: Some(third_binding),
            sequence: 4,
            expected: Some(third_contribution),
            members: None,
        },
    )
    .await?;
    check!(none.is_none());
    check!(no_output.is_none());
    check!(
        query(&catalog, &objects, &recipe, ViewSelection::Exact(deleted))?
            .hits
            .is_empty()
    );
    check_eq!(
        query(&catalog, &objects, &recipe, ViewSelection::Exact(first))?
            .hits
            .len(),
        2
    );
    check_eq!(old_pin.view().identity, first);
    check_eq!(calls.load(Ordering::SeqCst), 2);
    drop(first_result);
    drop(second_result);
    drop(third_result);
    drop(old_pin);
    drop(kernel);
    drop(catalog);
    drop(objects);

    let (reopened, objects, catalog, reopened_recipe) = open_kernel(
        directory.path(),
        FixedTextProvider {
            calls: Arc::clone(&calls),
            batches: Arc::clone(&batches),
        },
    )?;
    check_eq!(reopened_recipe.fingerprint(), recipe.fingerprint());
    check_eq!(current(&reopened)?.identity, deleted);
    check!(
        query(
            &catalog,
            &objects,
            &reopened_recipe,
            ViewSelection::Exact(deleted)
        )?
        .hits
        .is_empty()
    );
    let reopened_first = query(
        &catalog,
        &objects,
        &reopened_recipe,
        ViewSelection::Exact(first),
    )?;
    check_eq!(reopened_first.hits.len(), 2);
    check_eq!(
        reopened_first
            .hits
            .iter()
            .find(|hit| hit.vector_id.as_slice() == [11_u8; 32].as_slice())
            .and_then(|hit| hit.chunk.as_ref())
            .ok_or("reopened first alpha binding absent")?
            .input_digest,
        ALPHA_DIGEST
    );
    check_eq!(calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
#[expect(
    clippy::disallowed_methods,
    reason = "the Tokio test harness drives Kernel futures in this integration test"
)]
async fn certified_stateful_provider_derives_reordered_members_without_prior_reuse() -> TestResult {
    let directory = tempfile::tempdir()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let batches = Arc::new(Mutex::new(Vec::new()));
    let (kernel, objects, catalog, recipe) = open_kernel(
        directory.path(),
        StatefulTextProvider(FixedTextProvider {
            calls: Arc::clone(&calls),
            batches: Arc::clone(&batches),
        }),
    )?;
    let configured = configure(&kernel, &recipe).await?;
    let (sourced, source_record) = advance_source(&kernel, configured).await?;
    let original = [
        NativeSemanticManifestMember {
            id: Digest([11; 32]),
            text: "alpha",
        },
        NativeSemanticManifestMember {
            id: Digest([12; 32]),
            text: "βeta",
        },
    ];
    let (first_pending, first_contribution, _) = advance_projection(
        &kernel,
        ProjectionUpdate {
            view: sourced,
            source_record,
            prior: None,
            sequence: 1,
            expected: None,
            members: Some(&original),
        },
    )
    .await?;
    let first_contribution = first_contribution.ok_or("initial contribution absent")?;
    let first = materialize(&kernel, first_pending, 3).await?;
    check_eq!(calls.load(Ordering::SeqCst), 1);
    let prior = binding(&objects, current(&kernel)?.projections)?;
    let reordered = [
        NativeSemanticManifestMember {
            id: Digest([12; 32]),
            text: "βeta",
        },
        NativeSemanticManifestMember {
            id: Digest([11; 32]),
            text: "alpha",
        },
    ];
    let (second_pending, _, _) = advance_projection(
        &kernel,
        ProjectionUpdate {
            view: first,
            source_record,
            prior: Some(prior),
            sequence: 2,
            expected: Some(first_contribution),
            members: Some(&reordered),
        },
    )
    .await?;
    check!(
        query(
            &catalog,
            &objects,
            &recipe,
            ViewSelection::Exact(second_pending)
        )?
        .hits
        .is_empty()
    );
    let second = materialize(&kernel, second_pending, 4).await?;
    check_eq!(
        query(&catalog, &objects, &recipe, ViewSelection::Exact(second))?
            .hits
            .len(),
        2
    );
    check_eq!(calls.load(Ordering::SeqCst), 2);
    check_eq!(
        batches
            .lock()
            .map_err(|_error| "certified batch lock")?
            .as_slice(),
        &[
            vec!["alpha".to_owned(), "βeta".to_owned()],
            vec!["βeta".to_owned(), "alpha".to_owned()],
        ]
    );
    Ok(())
}
