use super::*;

fn published_fixture() -> Result<
    (
        tempfile::TempDir,
        SqliteCatalog,
        ImmutableOperation,
        CatalogView,
    ),
    Box<dyn Error>,
> {
    let root = tempfile::tempdir()?;
    let (catalog, genesis) = setup(root.path())?;
    let immutable = operation(31_051, 201);
    let attempt = admit_new(&catalog, immutable)?;
    let published = view(1, genesis.profile, None);
    check_eq!(
        publish(&catalog, attempt, &genesis, published.clone())?,
        OperationStatus::Published(published.identity)
    );
    Ok((root, catalog, immutable, published))
}

#[test]
fn published_result_survives_exact_view_retirement_and_reopen() -> TestResult {
    let (root, catalog, immutable, published) = published_fixture()?;
    let _next = publish_plain_next(&catalog, &published, 2, 204)?;
    check_eq!(
        catalog.advance_exact_history_floor(workspace(), 7, 2, &mut admission_budget())?,
        2
    );
    let retired =
        catalog.retire_exact_history_page(workspace(), 7, None, 8, &mut admission_budget())?;
    check_eq!(retired.retired, 2);
    {
        let guard = catalog.lock()?;
        check!(matches!(
            read_view_row(&guard, published.identity),
            Err(IndexError::SnapshotUnavailable)
        ));
    }
    check_eq!(
        catalog.published_view_metadata(immutable.key, &mut admission_budget())?,
        published
    );
    drop(catalog);
    let reopened = SqliteCatalog::open(root.path(), Duration::ZERO)?;
    check_eq!(
        reopened.published_view_metadata(immutable.key, &mut admission_budget())?,
        published
    );
    Ok(())
}

#[test]
fn failed_cas_and_unpublished_operations_have_no_result() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, genesis) = setup(root.path())?;
    let immutable = operation(31_052, 202);
    let attempt = admit_new(&catalog, immutable)?;
    let wrong_base = view(99, genesis.profile, None);
    check!(matches!(
        publish(
            &catalog,
            attempt,
            &wrong_base,
            view(1, genesis.profile, None)
        ),
        Err(IndexError::Conflict)
    ));
    check!(
        catalog
            .published_view_metadata(immutable.key, &mut admission_budget())
            .is_err()
    );
    check!(
        catalog
            .published_view_metadata(operation(31_053, 203).key, &mut admission_budget())
            .is_err()
    );
    check_eq!(
        catalog.inspect(
            immutable.key,
            OperationInspectAccess::Request(&admission_budget())
        )?,
        OperationStatus::Pending(attempt)
    );
    Ok(())
}

fn rewrite_published_row(
    catalog: &SqliteCatalog,
    key: OperationKey,
    mutate: impl FnOnce(&mut serde_json::Value),
) -> TestResult {
    let guard = catalog.lock()?;
    let payload: Vec<u8> = guard.query_row(
        "SELECT payload FROM native_operation_v1 WHERE operation=?1",
        params![operation_key(key)?],
        |row| row.get(0),
    )?;
    let mut value: serde_json::Value = serde_json::from_slice(&payload)?;
    mutate(&mut value);
    let payload = serde_json::to_vec(&value)?;
    check_eq!(
        guard.execute(
            "UPDATE native_operation_v1 SET payload=?2,row_digest=?3 WHERE operation=?1",
            params![
                operation_key(key)?,
                &payload,
                digest(b"qidx.native.operation.v1", &payload).to_vec()
            ],
        )?,
        1
    );
    Ok(())
}

#[test]
fn published_result_rejects_authenticated_shape_forgery() -> TestResult {
    for case in 0..3 {
        let (_root, catalog, immutable, published) = published_fixture()?;
        let mut forged = published.clone();
        match case {
            0 => forged.identity.digest = Digest([77; 32]),
            1 => {
                forged.identity.workspace.slot.0[0] ^= 1;
                forged.identity.digest = forged.identity_digest()?;
            }
            _ => {}
        }
        rewrite_published_row(&catalog, immutable.key, |row| {
            if case == 2 {
                row["status"] = serde_json::to_value(OperationStatus::Refused(Refusal::Conflict))
                    .expect("fixed refusal serializes");
            } else {
                row["published_view"] =
                    serde_json::to_value(forged).expect("fixed catalog view serializes");
            }
        })?;
        check!(
            catalog
                .published_view_metadata(immutable.key, &mut admission_budget())
                .is_err()
        );
    }
    Ok(())
}

#[derive(Clone)]
struct PublishBetweenResultReads {
    live: Arc<AtomicU64>,
    grants: Arc<AtomicU64>,
    committed: Arc<AtomicBool>,
    writer: Arc<SqliteCatalog>,
    attempt: AttemptRef,
    genesis: CatalogView,
    next: CatalogView,
}
impl quanta_index_core::index::MemoryLeaseOwner for PublishBetweenResultReads {
    fn release(&self, bytes: u64) {
        let previous = self.live.fetch_sub(bytes, Ordering::AcqRel);
        assert!(previous >= bytes, "result read lease underflow");
    }
}
impl quanta_index_core::index::MemoryScopePort for PublishBetweenResultReads {
    fn reserve(
        &self,
        bytes: u64,
        lifetime: quanta_index_core::index::MemoryLifetime,
    ) -> Result<quanta_index_core::index::MemoryLease, IndexError> {
        if lifetime != quanta_index_core::index::MemoryLifetime::Temporary {
            return Err(IndexError::BackendViolation(
                "result read must use temporary custody".into(),
            ));
        }
        let grant = self.grants.fetch_add(1, Ordering::AcqRel) + 1;
        if grant == 2 {
            let result = publish(&self.writer, self.attempt, &self.genesis, self.next.clone())?;
            if result != OperationStatus::Published(self.next.identity) {
                return Err(IndexError::BackendViolation(
                    "concurrent publication returned the wrong terminal result".into(),
                ));
            }
            self.committed.store(true, Ordering::Release);
        }
        let _previous_live = self.live.fetch_add(bytes, Ordering::AcqRel);
        Ok(quanta_index_core::index::MemoryLease::new(
            bytes,
            Arc::new(self.clone()),
        ))
    }
}

#[test]
fn published_result_reads_one_sqlite_snapshot_across_length_and_decode() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, genesis) = setup(root.path())?;
    let immutable = operation(31_055, 206);
    let attempt = admit_new(&catalog, immutable)?;
    let next = view(1, genesis.profile, None);
    let writer = Arc::new(SqliteCatalog::open(root.path(), Duration::from_secs(2))?);
    let live = Arc::new(AtomicU64::new(0));
    let grants = Arc::new(AtomicU64::new(0));
    let committed = Arc::new(AtomicBool::new(false));
    let hook = PublishBetweenResultReads {
        live: Arc::clone(&live),
        grants: Arc::clone(&grants),
        committed: Arc::clone(&committed),
        writer,
        attempt,
        genesis,
        next: next.clone(),
    };
    let mut work = admission_budget();
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        hook,
    ))));
    // The second memory grant is after the length SELECT and before the row
    // decoder. A second WAL Catalog connection commits the real publication.
    check_eq!(
        catalog.published_view_metadata(immutable.key, &mut work),
        Err(IndexError::Conflict)
    );
    check_eq!(grants.load(Ordering::Acquire), 2);
    check!(committed.load(Ordering::Acquire));
    check_eq!(live.load(Ordering::Acquire), 0);
    check_eq!(
        catalog.inspect(
            immutable.key,
            OperationInspectAccess::Request(&admission_budget())
        )?,
        OperationStatus::Published(next.identity)
    );
    Ok(())
}

#[test]
fn published_result_reads_multi_event_row_larger_than_boot_cap() -> TestResult {
    let (_root, catalog, immutable, published) = published_fixture()?;
    // This fixture exercises the general operation-row codec and budget.
    // The source-authority transition itself is covered by its own tests.
    let events: Vec<SourceEvent> = (1_u64..=32)
        .map(|sequence| SourceEvent {
            producer: ProducerId(id(8)),
            stream: StreamId(id(9)),
            stream_epoch: 1,
            predecessor: sequence.saturating_sub(1),
            sequence,
            payload: digest_id(88),
        })
        .collect();
    rewrite_published_row(&catalog, immutable.key, |row| {
        row["events"] = serde_json::to_value(events).expect("fixed source events serialize");
    })?;
    let guard = catalog.lock()?;
    let length: i64 = guard.query_row(
        "SELECT length(payload) FROM native_operation_v1 WHERE operation=?1",
        params![operation_key(immutable.key)?],
        |row| row.get(0),
    )?;
    check!(length > 4096);
    drop(guard);
    check_eq!(
        catalog.published_view_metadata(immutable.key, &mut admission_budget())?,
        published
    );
    Ok(())
}

#[derive(Clone)]
struct ResultReadWindow {
    live: Arc<AtomicU64>,
    limit: u64,
}
impl quanta_index_core::index::MemoryLeaseOwner for ResultReadWindow {
    fn release(&self, bytes: u64) {
        let previous = self.live.fetch_sub(bytes, Ordering::AcqRel);
        assert!(previous >= bytes, "result read lease underflow");
    }
}
impl quanta_index_core::index::MemoryScopePort for ResultReadWindow {
    fn reserve(
        &self,
        bytes: u64,
        lifetime: quanta_index_core::index::MemoryLifetime,
    ) -> Result<quanta_index_core::index::MemoryLease, IndexError> {
        if lifetime != quanta_index_core::index::MemoryLifetime::Temporary {
            return Err(IndexError::BackendViolation(
                "result read must use temporary custody".into(),
            ));
        }
        let _previous_live = self
            .live
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                live.checked_add(bytes).filter(|next| *next <= self.limit)
            })
            .map_err(|_prior| IndexError::ResourceLimited)?;
        Ok(quanta_index_core::index::MemoryLease::new(
            bytes,
            Arc::new(self.clone()),
        ))
    }
}

fn result_read_budget(bytes: u64, live: Arc<AtomicU64>, memory_limit: u64) -> WorkBudget {
    let mut work = WorkBudget::with_request(
        WorkLimits {
            reads: 100,
            writes: 0,
            bytes,
            mutations: 0,
        },
        quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30)),
    );
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        ResultReadWindow {
            live,
            limit: memory_limit,
        },
    ))));
    work
}

#[test]
fn published_result_refuses_unfunded_read_and_refunds_temporary_memory() -> TestResult {
    let (_root, catalog, immutable, published) = published_fixture()?;
    let live = Arc::new(AtomicU64::new(0));
    let mut low_bytes = result_read_budget(1024, Arc::clone(&live), 1 << 20);
    check_eq!(
        catalog.published_view_metadata(immutable.key, &mut low_bytes),
        Err(IndexError::ResourceLimited)
    );
    check_eq!(live.load(Ordering::Acquire), 0);
    let mut low_memory = result_read_budget(1 << 20, Arc::clone(&live), 2048);
    check_eq!(
        catalog.published_view_metadata(immutable.key, &mut low_memory),
        Err(IndexError::ResourceLimited)
    );
    check_eq!(live.load(Ordering::Acquire), 0);
    let mut funded = result_read_budget(1 << 20, Arc::clone(&live), 1 << 20);
    check_eq!(
        catalog.published_view_metadata(immutable.key, &mut funded)?,
        published
    );
    check_eq!(live.load(Ordering::Acquire), 0);
    Ok(())
}

#[test]
fn published_result_distinguishes_missing_and_known_nonpublished_operation() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, _genesis) = setup(root.path())?;
    let immutable = operation(31_054, 205);
    check_eq!(
        catalog.published_view_metadata(immutable.key, &mut admission_budget()),
        Err(IndexError::SnapshotUnavailable)
    );
    let attempt = admit_new(&catalog, immutable)?;
    check_eq!(
        catalog.published_view_metadata(immutable.key, &mut admission_budget()),
        Err(IndexError::Conflict)
    );
    check_eq!(
        catalog.refuse(attempt, Refusal::Conflict)?,
        OperationStatus::Refused(Refusal::Conflict)
    );
    check_eq!(
        catalog.published_view_metadata(immutable.key, &mut admission_budget()),
        Err(IndexError::Conflict)
    );
    Ok(())
}

#[test]
fn published_result_refuses_large_forged_workspace_before_materialization() -> TestResult {
    let (_root, catalog, immutable, _published) = published_fixture()?;
    let guard = catalog.lock()?;
    check_eq!(
        guard.execute(
            "UPDATE native_operation_v1 SET workspace=zeroblob(65536) WHERE operation=?1",
            params![operation_key(immutable.key)?],
        )?,
        1
    );
    drop(guard);
    let live = Arc::new(AtomicU64::new(0));
    let mut work = result_read_budget(1 << 20, Arc::clone(&live), 8192);
    check_eq!(
        catalog.published_view_metadata(immutable.key, &mut work),
        Err(IndexError::ResourceLimited)
    );
    check_eq!(live.load(Ordering::Acquire), 0);
    Ok(())
}

#[test]
fn initial_full_single_event_operation_row_syntactic_upper_bound_fits_boot_cap() -> TestResult {
    // Syntactic upper envelope for one-event FULL row serialization.
    // The placeholder identity digest maximizes JSON width; this case only
    // encodes, so digest validation is covered by separate reader tests.
    // A future codec expansion must update the bounded boot reader as well.
    let workspace = WorkspaceKey {
        tenant: TenantId(id(255)),
        collection: CollectionId(id(255)),
        worktree: WorktreeId(id(255)),
        slot: SlotId(id(255)),
    };
    let key = OperationKey {
        workspace,
        epoch: u64::MAX,
        producer: ProducerId(id(255)),
        stream: StreamId(id(255)),
        stream_epoch: u64::MAX,
        sequence: u64::MAX,
    };
    let object = ObjectRef(digest_id(255));
    let publication = CatalogView {
        identity: ViewRef {
            workspace,
            epoch: u64::MAX,
            sequence: u64::MAX,
            digest: digest_id(255),
        },
        profile: object,
        source: SourceCut {
            owners: MapRoot(Some(object)),
            paths: MapRoot(Some(object)),
            frontiers: MapRoot(Some(object)),
        },
        projections: MapRoot(Some(object)),
        members: MapRoot(Some(object)),
        coverage: MapRoot(Some(object)),
    };
    let row = OperationRow {
        immutable: ImmutableOperation {
            key,
            request: object,
            source_mode: SourceOperationMode::Advance,
        },
        events: vec![SourceEvent {
            producer: key.producer,
            stream: key.stream,
            stream_epoch: u64::MAX,
            predecessor: u64::MAX.saturating_sub(1),
            sequence: u64::MAX,
            payload: digest_id(255),
        }],
        fence: u64::MAX,
        status: OperationStatus::Published(publication.identity),
        published_view: Some(publication),
    };
    let payload = encode(&row)?;
    check!(
        payload.len() <= 4096,
        "single-event FULL operation row is {} bytes",
        payload.len()
    );
    Ok(())
}
