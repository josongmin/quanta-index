use super::*;
use quanta_index_core::index::ports::{MapDiffRange, MapDiffVisitor};

#[derive(Default)]
struct WindowProbe {
    factories: AtomicUsize,
    scoped_lookups: AtomicUsize,
    drops: AtomicUsize,
    path_streams: AtomicUsize,
}

/// One edit of a map stream: root ordinal, key, expected and next value.
type StreamEdit = (u32, Vec<u8>, Option<MapValue>, Option<MapValue>);

struct WindowMaps<'a> {
    inner: &'a TestMaps,
    probe: &'a WindowProbe,
    // The only nonempty stream this fixture may stage; empty streams keep
    // their bases.
    path_edits: &'a [StreamEdit],
    enabled: bool,
    refuse: bool,
    cancel: bool,
}

struct WindowObjects<'a> {
    inner: &'a TestObjects,
    probe: &'a WindowProbe,
    expected_drops: usize,
}

impl ImmutableObjectPort for WindowObjects<'_> {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.inner.identify(payload, children, budget)
    }

    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.inner.put(payload, children, budget)
    }

    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        check_eq!(self.probe.drops.load(Ordering::SeqCst), self.expected_drops);
        self.inner.install(payload, children, sink, budget)
    }

    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
        self.inner.install_batch(inputs, sink, budget)
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        self.inner.get(reference, budget)
    }
}

struct ScopedWindow<'a> {
    inner: &'a TestMaps,
    probe: &'a WindowProbe,
    path_edits: &'a [StreamEdit],
}

/// The staged path-posting root this fixture's map oracle returns.
fn staged_path_root() -> MapRoot {
    MapRoot(Some(reference(97)))
}

/// The Tree path postings of an excluded Source whose path is `a`.
///
/// Path postings cover every Source owner whatever its text admission. `a`
/// is one indexed run kept under both case surfaces (sensitive `0`, folded
/// `1`), keyed `be16 tagged-term length | surface | term | owner` and valued
/// by the Source object. The case-tagged term is the surface byte followed
/// by the term, so its length counts the tag: `2` for `a`.
fn excluded_path_edits(owner: OwnerKey, source: ObjectRef) -> Vec<StreamEdit> {
    [0_u8, 1]
        .into_iter()
        .map(|surface| {
            let mut key = vec![0, 2, surface, b'a'];
            key.extend_from_slice(&owner.producer.0);
            key.extend_from_slice(&owner.stable_id.0);
            (0, key, None, Some(MapValue::Object(source)))
        })
        .collect()
}

// The excluded-owner fixture's independent map oracle reads every stream to
// its real EOF. An empty stream keeps its bases; the only nonempty stream it
// can stage is exactly `path_edits` over one empty base, which it stages as
// `staged_path_root()`. It cannot implement or skip any other edit.
fn excluded_map_stream(
    bases: &[MapRoot],
    edits: &mut dyn MapEditCursor,
    path_edits: &[StreamEdit],
    probe: &WindowProbe,
    budget: &mut WorkBudget,
) -> Result<quanta_index_core::index::StagedMapRoots, IndexError> {
    budget.checkpoint()?;
    let mut observed = Vec::new();
    while let Some(edit) = edits.next_edit(budget)? {
        observed.push((
            edit.root(),
            edit.key().to_vec(),
            edit.expected()?,
            edit.next()?,
        ));
    }
    let mut roots = quanta_index_core::index::StagedMapRoots::prepare(bases.len(), budget)?;
    if observed.is_empty() {
        for base in bases {
            roots.push(*base)?;
        }
        return roots.finish();
    }
    check_eq!(observed.as_slice(), path_edits);
    check_eq!(bases, [MapRoot(None)].as_slice());
    let _previous = probe.path_streams.fetch_add(1, Ordering::SeqCst);
    roots.push(staged_path_root())?;
    roots.finish()
}

impl Drop for ScopedWindow<'_> {
    fn drop(&mut self) {
        let _previous = self.probe.drops.fetch_add(1, Ordering::SeqCst);
    }
}

impl PersistentMapPort for WindowMaps<'_> {
    fn scoped_reads<'a>(
        &'a self,
        budget: &WorkBudget,
    ) -> Result<Option<Box<dyn PersistentMapPort + 'a>>, IndexError> {
        let _previous = self.probe.factories.fetch_add(1, Ordering::SeqCst);
        if self.refuse {
            return Err(IndexError::ResourceLimited);
        }
        if self.cancel {
            budget.request().cancel_handle().cancel();
            budget.checkpoint()?;
        }
        Ok(if self.enabled {
            let window: Box<dyn PersistentMapPort + 'a> = Box::new(ScopedWindow {
                inner: self.inner,
                probe: self.probe,
                path_edits: self.path_edits,
            });
            Some(window)
        } else {
            None
        })
    }

    fn lookup(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<MapValue>, IndexError> {
        self.inner.lookup(root, key, budget)
    }

    fn scan(
        &self,
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        self.inner.scan(root, after, limit, budget)
    }

    fn visit_diff(
        &self,
        before: MapRoot,
        after: MapRoot,
        range: MapDiffRange<'_>,
        visitor: &mut dyn MapDiffVisitor,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        self.inner.visit_diff(before, after, range, visitor, budget)
    }

    fn apply(
        &self,
        base: MapRoot,
        edits: &[MapEdit],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapWrite, IndexError> {
        self.inner.apply(base, edits, sink, budget)
    }

    fn apply_batch(
        &self,
        requests: &[MapBatchRequest],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapBatchWrite, IndexError> {
        self.inner.apply_batch(requests, sink, budget)
    }

    fn apply_stream(
        &self,
        bases: &[MapRoot],
        edits: &mut dyn MapEditCursor,
        _sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::StagedMapRoots, IndexError> {
        excluded_map_stream(bases, edits, self.path_edits, self.probe, budget)
    }
}

impl PersistentMapPort for ScopedWindow<'_> {
    fn lookup(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<MapValue>, IndexError> {
        let _previous = self.probe.scoped_lookups.fetch_add(1, Ordering::SeqCst);
        self.inner.lookup(root, key, budget)
    }

    fn scan(
        &self,
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        self.inner.scan(root, after, limit, budget)
    }

    fn apply(
        &self,
        base: MapRoot,
        edits: &[MapEdit],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapWrite, IndexError> {
        self.inner.apply(base, edits, sink, budget)
    }

    fn apply_batch(
        &self,
        requests: &[MapBatchRequest],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapBatchWrite, IndexError> {
        self.inner.apply_batch(requests, sink, budget)
    }

    fn apply_stream(
        &self,
        bases: &[MapRoot],
        edits: &mut dyn MapEditCursor,
        _sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::StagedMapRoots, IndexError> {
        excluded_map_stream(bases, edits, self.path_edits, self.probe, budget)
    }
}

#[test]
fn changed_prepare_mounts_one_window_and_drops_it_on_object_error() -> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let plan = TextPlan {
        base: None,
        context: context(fixture.recipe, fixture.ticket.view.source.owners),
        input: DeltaInput::Bootstrap,
    };
    let index = NativeTextIndex::new(fixture.recipe);
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut results = Vec::new();
    for enabled in [false, true] {
        let probe = WindowProbe::default();
        let maps = WindowMaps {
            inner: &fixture.maps,
            probe: &probe,
            path_edits: &[],
            enabled,
            refuse: false,
            cancel: false,
        };
        let mut work = budget()?;
        let result = ready(index.prepare(
            plan.clone(),
            &mut AttemptContext {
                attempt: test_attempt(fixture.ticket.view.identity.workspace),
                objects: &fixture.objects,
                maps: &maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            },
        ));
        check!(matches!(
            result,
            Err(IndexError::BackendViolation(message)) if message == "changed-body occurrence build reached object batch"
        ));
        check_eq!(probe.factories.load(Ordering::SeqCst), 1);
        check_eq!(
            probe.scoped_lookups.load(Ordering::SeqCst),
            usize::from(enabled)
        );
        check_eq!(probe.drops.load(Ordering::SeqCst), usize::from(enabled));
        results.push(work.usage());
    }
    check_eq!(results.first(), results.get(1));
    Ok(())
}

#[test]
fn excluded_change_drops_window_before_root_install() -> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    let mut record = fixture_record(&fixture)?;
    record.text = SourceTextAdmission::Excluded;
    fixture
        .objects
        .add(fixture.source_ref, encode(&record)?, vec![record.bytes])?;
    let plan = TextPlan {
        base: None,
        context: context(fixture.recipe, fixture.ticket.view.source.owners),
        input: DeltaInput::Bootstrap,
    };
    let index = NativeTextIndex::new(fixture.recipe);
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let path_edits = excluded_path_edits(fixture.owner, fixture.source_ref);
    let mut results = Vec::new();
    for enabled in [false, true] {
        let probe = WindowProbe::default();
        let maps = WindowMaps {
            inner: &fixture.maps,
            probe: &probe,
            path_edits: &path_edits,
            enabled,
            refuse: false,
            cancel: false,
        };
        let objects = WindowObjects {
            inner: &fixture.objects,
            probe: &probe,
            expected_drops: usize::from(enabled),
        };
        let mut work = budget()?;
        let candidate = ready(index.prepare(
            plan.clone(),
            &mut AttemptContext {
                attempt: test_attempt(fixture.ticket.view.identity.workspace),
                objects: &objects,
                maps: &maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            },
        ))?;
        check_eq!(probe.factories.load(Ordering::SeqCst), 1);
        check_eq!(
            probe.scoped_lookups.load(Ordering::SeqCst),
            usize::from(enabled)
        );
        check_eq!(probe.drops.load(Ordering::SeqCst), usize::from(enabled));
        // The excluded Source's path is indexed exactly once.
        check_eq!(probe.path_streams.load(Ordering::SeqCst), 1);
        results.push((candidate.binding, work.usage()));
    }
    check_eq!(results.first(), results.get(1));
    Ok(())
}

#[test]
fn window_factory_refusal_and_cancellation_keep_typed_errors() -> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let plan = TextPlan {
        base: None,
        context: context(fixture.recipe, fixture.ticket.view.source.owners),
        input: DeltaInput::Bootstrap,
    };
    let index = NativeTextIndex::new(fixture.recipe);
    let scratch = TestScratch::default();
    let custody = TestCustody;
    for cancel in [false, true] {
        let probe = WindowProbe::default();
        let maps = WindowMaps {
            inner: &fixture.maps,
            probe: &probe,
            path_edits: &[],
            enabled: true,
            refuse: !cancel,
            cancel,
        };
        let mut work = budget()?;
        let result = ready(index.prepare(
            plan.clone(),
            &mut AttemptContext {
                attempt: test_attempt(fixture.ticket.view.identity.workspace),
                objects: &fixture.objects,
                maps: &maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            },
        ));
        if cancel {
            check!(matches!(result, Err(IndexError::Cancelled)));
        } else {
            check!(matches!(result, Err(IndexError::ResourceLimited)));
        }
        check_eq!(probe.factories.load(Ordering::SeqCst), 1);
        check_eq!(probe.drops.load(Ordering::SeqCst), 0);
        check_eq!(probe.scoped_lookups.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[test]
fn empty_bootstrap_and_noop_patch_skip_window_factory() -> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    let index = NativeTextIndex::new(fixture.recipe);
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let probe = WindowProbe::default();
    let maps = WindowMaps {
        inner: &fixture.maps,
        probe: &probe,
        path_edits: &[],
        enabled: true,
        refuse: false,
        cancel: false,
    };
    let mut work = budget()?;
    let empty = TextPlan {
        base: None,
        context: context(fixture.recipe, MapRoot(None)),
        input: DeltaInput::Bootstrap,
    };
    let candidate = ready(index.prepare(
        empty,
        &mut AttemptContext {
            attempt: test_attempt(fixture.ticket.view.identity.workspace),
            objects: &fixture.objects,
            maps: &maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        },
    ))?;
    check_eq!(candidate.binding.input.root, MapRoot(None));
    check_eq!(probe.factories.load(Ordering::SeqCst), 0);
    check_eq!(probe.drops.load(Ordering::SeqCst), 0);

    let inconsistent = MapRoot(Some(reference(201)));
    fixture.maps.add(
        inconsistent,
        owner_map_key(fixture.owner).to_vec(),
        fixture.source_ref,
    )?;
    fixture.maps.empty_diff = Some((fixture.ticket.view.source.owners, inconsistent));
    let patch = empty_patch_plan(
        &fixture,
        OwnerMutationSet {
            root: MapRoot(None),
            count: 0,
            digest: OwnerMutationDigest::default().finish(),
        },
    )?;
    let maps = WindowMaps {
        inner: &fixture.maps,
        probe: &probe,
        path_edits: &[],
        enabled: true,
        refuse: false,
        cancel: false,
    };
    let candidate = ready(index.prepare(
        patch,
        &mut AttemptContext {
            attempt: test_attempt(fixture.ticket.view.identity.workspace),
            objects: &fixture.objects,
            maps: &maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        },
    ))?;
    check_eq!(candidate.binding.artifact, fixture.member.artifact);
    check_eq!(probe.factories.load(Ordering::SeqCst), 0);
    Ok(())
}
