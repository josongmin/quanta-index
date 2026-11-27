//! Old Tree policy is retained only as an exact selected Source predecessor.
//! The old builder is a fixture; the replacement is the real current Text index.

use super::{source_body_validation::scoped_budget, *};
use quanta_index_core::index::{
    BaseMember, DeltaInput, PlanContext, PlanDecision, SourceDelta, SourceTextAdmission,
    ports::{IndexBuildPort, IndexFuture},
};
use quanta_index_lexical::native::{CaseMode, NativeTextIndex, TextReadContext, TextRecipe};

const OLD_ARTIFACT: &[u8] = b"fixed previous Tree lifecycle artifact";

struct PreviousTreeFixture(Digest);

impl IndexBuildPort for PreviousTreeFixture {
    type Delta = SourceDelta;
    type Plan = PlanContext;
    type Candidate = MemberBinding;

    fn resource_demand(&self) -> ResourceNeed {
        NativeTextIndex::new(TextRecipe::new(Digest([31; 32]))).resource_demand()
    }

    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        if instance.policy != self.0
            || instance.input_domain != quanta_index_core::index::InputDomain::SourceLocal
        {
            return Err(IndexError::InvalidInput(
                "previous Tree fixture policy mismatch".into(),
            ));
        }
        Ok(())
    }

    fn plan(
        &self,
        _base: BaseMember,
        _input: DeltaInput<Self::Delta>,
        context: &PlanContext,
        budget: &mut WorkBudget,
    ) -> Result<PlanDecision<Self::Plan>, IndexError> {
        budget.checkpoint()?;
        self.validate_instance(&context.instance)?;
        Ok(PlanDecision::Incremental(context.clone()))
    }

    fn prepare<'a>(
        &'a self,
        plan: Self::Plan,
        context: &'a mut quanta_index_core::index::AttemptContext<'_>,
    ) -> IndexFuture<'a, Self::Candidate> {
        Box::pin(async move {
            let artifact = context.put(OLD_ARTIFACT, &[])?;
            Ok(MemberBinding {
                instance: plan.instance.instance,
                format: plan.instance.format,
                artifact,
                input: plan.input,
                logical_commitment: Digest(Sha256::digest(OLD_ARTIFACT).into()),
            })
        })
    }

    fn verify_and_seal<'a>(
        &'a self,
        candidate: Self::Candidate,
        context: &'a mut quanta_index_core::index::AttemptContext<'_>,
    ) -> IndexFuture<'a, Self::Candidate> {
        Box::pin(async move {
            let stored = context.objects.get(candidate.artifact, context.budget)?;
            if stored.payload != OLD_ARTIFACT || !stored.registration.children.is_empty() {
                return Err(IndexError::CorruptObject(
                    "previous Tree fixture artifact differs".into(),
                ));
            }
            Ok(candidate)
        })
    }
}

fn text_kernel(
    objects: Arc<FileObjectStore>,
    catalog: Arc<SqliteCatalog>,
    scratch: Arc<quanta_index_storage::FileScratchSpace>,
    recipe: TextRecipe,
    old: bool,
) -> TestResult<Kernel<SqliteCatalog>> {
    let mut registry = AdapterRegistry::new();
    let descriptor = AdapterDescriptor {
        kind: spec().kind,
        schema: spec().schema,
        format: spec().format,
    };
    if old {
        registry.register(
            descriptor,
            PreviousTreeFixture(recipe.output_policy()),
            |delta| Ok(delta.clone()),
        )?;
    } else {
        registry.register(descriptor, NativeTextIndex::new(recipe), |delta| {
            Ok(delta.clone())
        })?;
    }
    let services = KernelServices::new(
        objects.clone(),
        objects,
        catalog,
        scratch,
        Duration::from_secs(30),
    )?;
    Ok(Kernel::new(services, registry))
}

#[test]
fn previous_tree_reconfigure_keeps_old_view_and_enables_delta() -> TestResult {
    let root = tempfile::tempdir()?;
    let catalog = Arc::new(SqliteCatalog::open(
        root.path(),
        Duration::from_millis(100),
    )?);
    let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
        root.path().join("scratch"),
        16 * 1024 * 1024 * 1024,
    )?);
    let objects = Arc::new(FileObjectStore::open_with_scratch(
        root.path().join("native-objects"),
        scratch.clone(),
    )?);
    let admission = Digest([31; 32]);
    let previous = TextRecipe::previous_tree(admission);
    let current = TextRecipe::new(admission);
    let old_spec = InstanceSpec {
        policy: previous.output_policy(),
        ..spec()
    };
    let old = text_kernel(
        objects.clone(),
        catalog.clone(),
        scratch.clone(),
        previous,
        true,
    )?;
    let genesis = old.create_workspace(workspace(), 1, &mut scoped_budget())?;
    let old_profile = old.stage_profile(
        old.begin_input(operation(1))?,
        &IndexProfile {
            instances: vec![old_spec.clone()],
        },
        &mut scoped_budget(),
    )?;
    let configured = published(block_on(old.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure {
                profile: old_profile,
            },
        },
        &mut scoped_budget(),
    ))?)?;
    let first_source = old.stage_source_text(
        old.begin_input(operation(2))?,
        "old.rs",
        b"alpha stable",
        SourceTextAdmission::Utf8 { policy: admission },
        &mut scoped_budget(),
    )?;
    let first = published(block_on(old.submit(
        request(
            &old,
            &catalog,
            &objects,
            configured,
            source(&old, configured)?,
            2,
            1,
            vec![OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(first_source),
            }],
        )?,
        &mut scoped_budget(),
    ))?)?;
    let old_ticket = old.pin_view(ViewSelection::Exact(first), &scoped_budget())?;
    let old_member = member(&old, &objects, first)?;
    assert_eq!(old_member.input.policy, previous.output_policy());
    drop(old);

    let current_spec = InstanceSpec {
        policy: current.output_policy(),
        ..old_spec
    };
    let live = text_kernel(objects.clone(), catalog.clone(), scratch, current, false)?
        .with_retained_tree(Some(quanta_index_kernel::RetainedSourceTreePolicyV20 {
            descriptor: AdapterDescriptor {
                kind: current_spec.kind,
                schema: current_spec.schema,
                format: current_spec.format,
            },
            previous: previous.output_policy(),
            current: current.output_policy(),
        }));
    let refused = block_on(live.submit(
        request(
            &live,
            &catalog,
            &objects,
            first,
            old_ticket.view().source,
            3,
            2,
            vec![],
        )?,
        &mut scoped_budget(),
    ))?;
    assert!(
        matches!(
            refused,
            SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::RebuildRequired))
        ),
        "unexpected legacy Tree write result: {refused:?}"
    );
    assert_eq!(source(&live, first)?, old_ticket.view().source);

    let current_profile = live.stage_profile(
        live.begin_input(operation(4))?,
        &IndexProfile {
            instances: vec![current_spec],
        },
        &mut scoped_budget(),
    )?;
    let rebuilt = published(block_on(live.submit(
        UpdateRequest {
            operation: operation(4),
            expected_view: first,
            kind: UpdateKind::Reconfigure {
                profile: current_profile,
            },
        },
        &mut scoped_budget(),
    ))?)?;
    assert_eq!(source(&live, rebuilt)?, old_ticket.view().source);
    assert_eq!(old_ticket.view().identity, first);
    assert_eq!(
        objects
            .get(old_member.artifact, &mut scoped_budget())?
            .payload,
        OLD_ARTIFACT
    );
    let index = NativeTextIndex::new(current);
    let df = |view| -> TestResult<u64> {
        let ticket = live.pin_view(ViewSelection::Exact(view), &scoped_budget())?;
        Ok(index.inspect_term_df(
            &member(&live, &objects, view)?,
            CaseMode::Sensitive,
            "alpha",
            TextReadContext {
                ticket: ticket.record(),
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut scoped_budget(),
            },
        )?)
    };
    assert_eq!(df(rebuilt)?, 1);

    let next_source = live.stage_source_text(
        live.begin_input(operation(5))?,
        "old.rs",
        b"beta stable",
        SourceTextAdmission::Utf8 { policy: admission },
        &mut scoped_budget(),
    )?;
    let next = published(block_on(live.submit(
        request(
            &live,
            &catalog,
            &objects,
            rebuilt,
            source(&live, rebuilt)?,
            5,
            2,
            vec![OwnerMutation {
                owner: owner(1),
                expected: Some(first_source),
                next: Some(next_source),
            }],
        )?,
        &mut scoped_budget(),
    ))?)?;
    assert_eq!(df(next)?, 0);
    let ticket = live.pin_view(ViewSelection::Exact(next), &scoped_budget())?;
    assert_eq!(
        index.inspect_term_df(
            &member(&live, &objects, next)?,
            CaseMode::Sensitive,
            "beta",
            TextReadContext {
                ticket: ticket.record(),
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut scoped_budget()
            },
        )?,
        1
    );
    assert_eq!(old_ticket.view().identity, first);
    assert_eq!(
        objects
            .get(old_member.artifact, &mut scoped_budget())?
            .payload,
        OLD_ARTIFACT
    );
    Ok(())
}
