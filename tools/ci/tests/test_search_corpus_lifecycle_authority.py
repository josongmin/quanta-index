"""Static architecture fence for the canonical search-corpus lifecycle owner."""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
PLANE = ROOT / "crates/quanta-index-search-plane/src"
LIFECYCLE = PLANE / "search_corpus_lifecycle.rs"
# The readiness module is a directory since the QI-BB-013 split; the fence
# reads every production file of it as one text.
READINESS_DIR = PLANE / "readiness"
RETENTION = PLANE / "search_corpus_retention.rs"
CONTROL = PLANE / "control_dispatcher.rs"
RUNTIME = ROOT / "crates/quanta-index-searchd-runtime/src/lib.rs"
APP_RUNTIME = ROOT / "crates/quanta-index-searchd/src/app/runtime.rs"
RESTART_SCENARIO = (
    ROOT / "crates/quanta-index-searchd-runtime/tests/composite_generation_authority_restart.rs"
)
PLANE_LIB = PLANE / "lib.rs"


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def read_readiness() -> str:
    """Every file of the readiness module, production and tests, as one text."""
    files = sorted(READINESS_DIR.rglob("*.rs"))
    assert files, f"no readiness sources under {READINESS_DIR}"
    return "\n".join(read(path) for path in files)


def test_one_pair_mutation_coordinator_owns_catalog_and_retention_v1() -> None:
    lifecycle = read(LIFECYCLE)
    plane_lib = read(PLANE_LIB)
    readiness = read_readiness()

    assert "struct SearchCorpusLifecycleOwner" in lifecycle
    assert "struct SearchCorpusPairMutationCoordinator" in lifecycle
    assert "SearchCorpusPairMutationGuard" in lifecycle
    assert "stripe: usize" in lifecycle
    assert "repo_id: RepoId" in lifecycle
    assert "revision_id: RevisionId" in lifecycle
    assert "require_pair_v1" in lifecycle
    assert "self.repo_id != *repo_id || self.revision_id != *revision_id" in lifecycle
    assert "pair_guard_rejects_a_different_pair_even_on_the_same_stripe_v1" in lifecycle
    assert "require_owner" not in lifecycle
    assert "mutation_locks" not in readiness
    assert "search_corpus_write_locks" not in readiness
    assert "lifecycle_coordinator" in readiness
    assert "pub(crate) struct SearchCorpusPairMutationCoordinator" in lifecycle
    assert "pub use search_corpus_lifecycle::SearchCorpusLifecycleOwner;" in plane_lib
    assert "SearchCorpusPairMutationCoordinator" not in plane_lib


def test_lifecycle_owner_derives_mutable_roots_from_one_state_root_v1() -> None:
    lifecycle = read(LIFECYCLE)
    runtime = read(RUNTIME)
    app_runtime = read(APP_RUNTIME)

    assert "state_root: impl AsRef<Path>" in lifecycle
    assert "state_root_identity_v1: PathBuf" in lifecycle
    canonicalize = lifecycle.index(
        "let state_root_identity_v1 = canonical_state_root_identity_v1(state_root.as_ref())?;"
    )
    activation_root = lifecycle.index(
        'let activation_root = state_root_identity_v1.join("activations");'
    )
    authority_root = lifecycle.index(
        'let authority_root = state_root_identity_v1.join("authorities");'
    )
    assert canonicalize < activation_root < authority_root
    assert "activation_root: impl AsRef<Path>" not in lifecycle
    assert "authority_root: impl AsRef<Path>" not in lifecycle
    assert "SearchCorpusLifecycleOwner::open(\n        &state_root," in runtime
    lease = runtime.index("let state_root_lease = StateRootLease::acquire")
    leased_root = runtime.index(
        "let state_root = state_root_lease.state_root_identity_v1().to_path_buf();"
    )
    lexical_open = runtime.index("LexicalAdapter::with_state_root_and_policies(")
    assert lease < leased_root < lexical_open
    assert 'state_root.join("activations")' not in runtime
    assert 'state_root.join("authorities")' not in runtime
    assert "state_root_identity_v1: PathBuf" in app_runtime
    assert "pub fn state_root_identity_v1(&self) -> &Path" in app_runtime
    assert app_runtime.count(".require_state_root_v1(config.state_root())") == 2
    last_root_validation = app_runtime.rindex(".require_state_root_v1(config.state_root())")
    # QI-BB-026 replaced the deep lexical bootstrap with the boot inventory.
    assert last_root_validation < app_runtime.index("boot_inventory::seed_track_readiness(")


def test_control_dispatcher_delegates_composite_mutations_v1() -> None:
    control = read(CONTROL)

    assert "SearchCorpusLifecycleService" in control
    assert "search_corpus_lifecycle.activate_prepared_v1" in control
    assert "search_corpus_lifecycle.rollback_v1" in control
    assert "activation_catalog.rollback(" not in control
    assert ".activate_prepared_under_guard_v1(" not in control


def test_retention_requires_active_pin_and_shared_guard_v1() -> None:
    lifecycle = read(LIFECYCLE)
    readiness = read_readiness()
    retention = read(RETENTION)

    assert "trait ActiveSearchCorpusPinReadPort" in lifecycle
    assert "active_search_corpus_under_guard_v1" in readiness
    assert "active: bool" in retention
    assert "required active/candidate set" in retention
    assert "active generation is absent from durable history" in readiness


def test_atomic_writes_stage_outside_canonical_roots_and_boot_reconciles_v1() -> None:
    readiness = read_readiness()
    runtime = read(RUNTIME)

    assert 'join(".staging")' in readiness
    assert "atomic_replace_file_from_staging_v1" in readiness
    assert "reconcile_search_corpus_startup_v1" in readiness
    assert "reconcile_search_corpus_staging_v1" in readiness
    assert "collect_abandoned_search_corpus_pair_directories_v1" in readiness
    assert "remove_abandoned_search_corpus_pair_directories_v1" in readiness
    assert "active pair directory is empty" in readiness
    assert "foreign staging entry" in readiness
    assert "SearchCorpusLifecycleOwner::open" in runtime
    assert runtime.index("StateRootLease::acquire") < runtime.index(
        "SearchCorpusLifecycleOwner::open"
    )


def test_startup_accepts_the_canonical_uppercase_pair_digest_v1() -> None:
    readiness = read_readiness()

    assert 'format!("{digest:X}")' in readiness
    assert "pair_name.bytes().all(|byte| byte.is_ascii_hexdigit())" in readiness


def test_restart_revalidates_active_composite_before_socket_bind_v1() -> None:
    lifecycle = read(LIFECYCLE)
    app_runtime = read(APP_RUNTIME)

    assert "validate_rehydrated_active_generations_v1" in lifecycle
    assert "all_active_search_corpora_for_bootstrap_v1" in lifecycle
    assert "lexical_generation_validator" in lifecycle
    assert "semantic_generation_validator" in lifecycle
    assert "restart_rehydrate_rejects_missing_active_lexical_generation_v1" in lifecycle
    assert "restart_rehydrate_rejects_missing_active_semantic_generation_v1" in lifecycle
    assert "separate_state_roots_rehydrate_same_repo_without_cross_root_aliasing_v1" in lifecycle
    restore = app_runtime.index(".restore_into(&mut guard)")
    revalidate = app_runtime.index(".validate_rehydrated_active_generations_v1(")
    query_bind = app_runtime.index("SearchPlaneQueryServer::bind(")
    control_bind = app_runtime.index("SearchPlaneControlServer::bind(")
    ingest_bind = app_runtime.index("SearchPlaneIngestServer::bind(")
    assert revalidate < restore < query_bind
    assert revalidate < control_bind
    assert revalidate < ingest_bind


def test_real_restart_proves_cross_repo_retention_rollback_and_exclusive_root_v1() -> None:
    scenario = read(RESTART_SCENARIO)
    test_name = (
        "fn real_child_process_cross_repo_restart_retains_and_rolls_back_each_composite_v1()"
    )
    body = scenario[
        scenario.index(test_name) : scenario.index(
            "fn real_child_process_state_root_lease_rejects_second_owner_and_releases_v1()"
        )
    ]

    assert 'const REPO_B: &str = "repo-composite-restart-b";' in body
    assert body.count("SearchdBinaryProcess::start_with_history_max_generations(") == 3
    assert body.count("publish_and_activate_for(") == 6
    assert body.count("SearchPlaneRollbackSearchCorpusGenerationCasRequest") == 2
    assert "repo A rollback ack did not bind the exact CAS transition" in body
    assert "repo B rollback ack did not bind the exact CAS transition" in body
    assert "searchd_lease_probe::require_start_failure(directory.path())" in body
    assert "ERR_STATE_ROOT_IN_USE" in body
    assert "restart aliased or lost one repo's active composite" in body
    assert "second restart did not preserve both independently rolled-back composites" in body


FAULT_MATRIX_TESTS = (
    "retention_missing_active_history_preserves_activation_and_empty_history_v1",
    "retention_active_digest_mismatch_preserves_activation_and_history_v1",
    "retention_required_set_exhaustion_preserves_activation_and_history_v1",
    "activation_staging_write_failure_preserves_complete_active_pointer_v1",
    "rollback_staging_write_failure_preserves_complete_active_pointer_v1",
    "rollback_parent_sync_failure_fences_serving_and_rehydrates_one_composite_v1",
    "sealed_search_corpus_retry_repairs_staging_parent_sync_failure_v1",
)


def require_fault_matrix_surface_v1(readiness: str) -> None:
    for test_name in FAULT_MATRIX_TESTS:
        assert f"fn {test_name}" in readiness
    assert "active generation is absent from durable history" in readiness
    assert "required active/candidate set" in read(RETENTION)
    assert "mark_durability_uncertain_v1" in readiness
    assert "sync_search_corpus_staging_after_exact_retry_v1" in readiness
    assert "assert_active_composite_v1" in readiness


def test_retention_and_composite_fault_matrix_is_owner_local_v1() -> None:
    readiness = read_readiness()
    lifecycle = read(LIFECYCLE)

    require_fault_matrix_surface_v1(readiness)
    record_body = readiness[
        readiness.index("pub fn record_sealed_search_corpus(") : readiness.index(
            "pub fn inspect_sealed_search_corpus("
        )
    ]
    assert record_body.index("lock_pair(repo_id, revision_id)") < record_body.index(
        "search_corpus_root_lock.lock()"
    )
    assert record_body.index("plan_search_corpus_pair_records_v1(") < record_body.index(
        "atomic_replace_file_from_staging_v1("
    )
    assert "reconcile_existing_search_corpus_record_v1" in readiness
    assert "persist_new_search_corpus_record_v1" in readiness
    rollback_body = lifecycle[
        lifecycle.index("pub(crate) fn rollback_v1(") : lifecycle.index(
            "fn validate_physical_pair_v1("
        )
    ]
    assert rollback_body.count(".lock_pair(") == 1
    assert rollback_body.index(".lock_pair(") < rollback_body.index(".rollback_under_guard_v1(")
    assert "search_corpus_root_lock" not in rollback_body


def test_fault_matrix_static_fence_self_breaks_v1() -> None:
    readiness = read_readiness()
    broken = readiness.replace(
        "fn rollback_staging_write_failure_preserves_complete_active_pointer_v1",
        "fn removed_rollback_staging_write_failure",
        1,
    )

    try:
        require_fault_matrix_surface_v1(broken)
    except AssertionError:
        pass
    else:
        raise AssertionError("fault-matrix fence accepted removal of rollback write-failure proof")
