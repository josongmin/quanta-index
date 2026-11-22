"""Regressions for the Legacy lexical disposition gate after source deletion."""

import hashlib
import importlib.util
import json
import os
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

from tools.ci import retirement_nextest


@pytest.fixture
def project_checker(tmp_path, monkeypatch):
    path = Path(__file__).resolve().parents[1] / "lint/check-lexical-test-disposition.py"
    spec = importlib.util.spec_from_file_location("lexical_test_disposition", path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    roster = module.tomllib.loads(module.MANIFEST.read_text(encoding="utf-8"))
    monkeypatch.setattr(module, "ROOT", tmp_path)
    manifest = tmp_path / "tools/ci/inventory/lexical-test-disposition.toml"
    manifest.parent.mkdir(parents=True)
    monkeypatch.setattr(module, "MANIFEST", manifest)
    monkeypatch.setattr(module, "LEXICAL_CARGO_MANIFEST", tmp_path / "Cargo.toml")
    module.LEXICAL_CARGO_MANIFEST.write_text("[package]\n", encoding="utf-8")
    sections = ["version = 1\n"]
    for target in roster["targets"]:
        source = target["path"]
        feature = target.get("required_feature")
        tests = [name for group in target["dispositions"] for name in group["tests"]]
        lines = ["[[targets]]", f"path = {json.dumps(source)}"]
        if feature is not None:
            lines.append(f"required_feature = {json.dumps(feature)}")
        lines.extend([
            "[[targets.dispositions]]",
            'disposition = "retire"' if feature else 'disposition = "retain"',
            'proof = "NOT_APPLICABLE"' if feature else 'proof = "NOT_RUN"',
            'replacement = "fixture for checker behavior"',
            f"tests = {json.dumps(tests)}",
        ])
        sections.append("\n".join(lines))
        if feature is not None:
            continue
        retained = tmp_path / source
        retained.parent.mkdir(parents=True)
        retained.write_text(
            "".join(f"#[test]\nfn {name}() {{}}\n" for name in tests),
            encoding="utf-8",
        )
    module.MANIFEST.write_text("\n\n".join(sections) + "\n", encoding="utf-8")
    return module


@pytest.fixture
def checker(project_checker, monkeypatch):
    # Other cases isolate one gate at a time using a synthetic retired roster.
    targets = project_checker.tomllib.loads(project_checker.MANIFEST.read_text(encoding="utf-8"))["targets"]
    monkeypatch.setattr(project_checker, "ENGINE_ONLY_RETIREMENTS", frozenset(
        (target["path"], test)
        for target in targets if "required_feature" in target
        for group in target["dispositions"]
        for test in group["tests"]
    ))
    return project_checker


@pytest.mark.parametrize("inventory_only", [True, False])
def test_engine_removal_does_not_waive_public_and_data_contracts(project_checker, capsys, inventory_only):
    assert project_checker.main(inventory_only=inventory_only) == 1
    errors = capsys.readouterr().err
    assert "engine-only retirement is not recorded" in errors
    assert "point_inventory_reports_control_file_read_failures" in errors
    assert "tantivy_select_file_collapses_multiple_chunks_per_path" in errors
    assert "point_inventory_matches_full_inventory_for_digest_and_family_symlink" not in errors


def test_deleting_targets_and_their_manifest_rows_does_not_erase_obligations(checker, capsys):
    inventory = checker.MANIFEST.read_text(encoding="utf-8")
    retained = next(
        section
        for section in inventory.split("[[targets]]")[1:]
        if "sealed_commitment_cost.rs" in section
    )
    checker.MANIFEST.write_text(f"version = 1\n[[targets]]{retained}", encoding="utf-8")

    assert checker.main() == 1
    errors = capsys.readouterr().err
    for target in ("sealed_manifest.rs", "text_authority_shards.rs", "tantivy_smoke.rs"):
        assert f"{target}: missing frozen target" in errors


def test_removed_source_requires_each_archived_id_to_be_disposed(checker, capsys):
    inventory = checker.MANIFEST.read_text(encoding="utf-8")
    checker.MANIFEST.write_text(
        inventory.replace('disposition = "retire"', 'disposition = "migrate"').replace(
            'proof = "NOT_APPLICABLE"', 'proof = "NOT_RUN"'
        ),
        encoding="utf-8",
    )
    assert checker.main() == 1
    errors = capsys.readouterr().err
    for target, count in (
        ("sealed_manifest", 43),
        ("text_authority_shards", 7),
        ("tantivy_smoke", 41),
    ):
        assert f"{target}.rs: source was removed with {count} test IDs" in errors
    assert "[NOT_RUN]" in errors
    assert "shared_file_body_fault_fences_every_referencing_generation" in errors


def test_inventory_only_accepts_pending_bodies_but_requires_the_frozen_ids(checker, capsys):
    inventory = checker.MANIFEST.read_text(encoding="utf-8")
    pending = inventory.replace('disposition = "retire"', 'disposition = "migrate"').replace(
        'proof = "NOT_APPLICABLE"', 'proof = "NOT_RUN"'
    )
    checker.MANIFEST.write_text(pending, encoding="utf-8")
    assert checker.main(inventory_only=True) == 0
    assert "inventory only: 93 IDs across 4 targets" in capsys.readouterr().out
    assert checker.main() == 1
    assert "lacking VERIFIED migrate/retire proof" in capsys.readouterr().err

    checker.MANIFEST.write_text(
        pending.replace('"tantivy_applies_query_boost_to_scores"', '"unrelated_same_count_test"'),
        encoding="utf-8",
    )
    assert checker.main(inventory_only=True) == 1
    assert "mapped test IDs differ from the frozen census" in capsys.readouterr().err


def test_pending_candidate_source_is_checked_without_execution(checker, capsys):
    candidate = "crates/quanta-index-lexical/tests/replacement.rs"
    source = checker.ROOT / candidate
    source.write_text("#[test]\nfn replacement() {}\n", encoding="utf-8")
    inventory = checker.MANIFEST.read_text(encoding="utf-8")
    checker.MANIFEST.write_text(
        inventory.replace(
            'disposition = "retire"\nproof = "NOT_APPLICABLE"',
            'disposition = "migrate"\nproof = "NOT_RUN"\n'
            f'candidates = [{{path = "{candidate}", tests = ["replacement"]}}]',
            1,
        ),
        encoding="utf-8",
    )
    assert checker.main(inventory_only=True) == 0
    source.write_text("#[test]\nfn renamed() {}\n", encoding="utf-8")
    assert checker.main(inventory_only=True) == 1
    assert f"replacement test 'replacement' is absent from {candidate}" in capsys.readouterr().err


def test_missing_assertion_blocks_proof_promotion(checker, capsys):
    inventory = checker.MANIFEST.read_text(encoding="utf-8")
    pending = inventory.replace(
        'disposition = "retire"\nproof = "NOT_APPLICABLE"',
        'disposition = "migrate"\nproof = "NOT_RUN"\n'
        'remaining_assertions = ["Reopen the original exact view"]',
        1,
    )
    checker.MANIFEST.write_text(pending, encoding="utf-8")
    assert checker.main(inventory_only=True) == 0
    checker.MANIFEST.write_text(
        pending.replace(
            'proof = "NOT_RUN"\nremaining_assertions',
            'proof = "VERIFIED"\nremaining_assertions',
            1,
        ),
        encoding="utf-8",
    )
    assert checker.main(inventory_only=True) == 1
    assert "verified disposition still has missing assertions" in capsys.readouterr().err


def test_migration_cannot_claim_it_has_no_applicable_body(checker, capsys):
    inventory = checker.MANIFEST.read_text(encoding="utf-8")
    checker.MANIFEST.write_text(
        inventory.replace('disposition = "retire"', 'disposition = "migrate"'),
        encoding="utf-8",
    )
    assert checker.main(inventory_only=True) == 1
    assert "migration requires a body outcome" in capsys.readouterr().err


@pytest.mark.parametrize(
    ("evidence", "expected_error"),
    [
        ("missing", "requires at least one Nextest body observation"),
        ("stale", "observation source changed since"),
        ("wrong_case", "is absent from"),
        ("valid", None),
    ],
)
def test_verified_migration_requires_current_named_nextest_observation(
    checker, capsys, evidence, expected_error,
):
    source = "crates/quanta-index-lexical/tests/replacement.rs"
    path = checker.ROOT / source
    body = b"#[test]\nfn replacement() {}\n"
    path.write_bytes(body)
    inventory = checker.MANIFEST.read_text(encoding="utf-8")
    retired = 'disposition = "retire"\nproof = "NOT_APPLICABLE"'
    verified = (
        'disposition = "migrate"\nproof = "VERIFIED"\n'
        f'candidates = [{{path = "{source}", tests = ["replacement"]}}]'
    )
    if evidence != "missing":
        case = "different_test" if evidence == "wrong_case" else "replacement"
        digest = hashlib.sha256(body).hexdigest()
        verified += (
            f'\nobservations = [{{source = "{source}", case = "{case}", '
            f'source_sha256 = "{digest}", run_id = "3739620b-a0ff-451d-897f-e5a32fc146eb"}}]'
        )
    checker.MANIFEST.write_text(inventory.replace(retired, verified, 1), encoding="utf-8")

    if evidence == "stale":
        path.write_bytes(body.replace(b"{}", b"{ changed(); }"))
    assert checker.main(inventory_only=True) == (0 if expected_error is None else 1)
    if expected_error is not None:
        assert expected_error in capsys.readouterr().err


@pytest.mark.parametrize(
    ("evidence", "expected_error"),
    [
        ("uuid_only", "lacks an exact Nextest event"),
        ("events_only", "lacks the Nextest inventory SHA-256"),
        ("absent", "cannot read nextest evidence"),
        ("wrong_binary", "differs from"),
        ("same_leaf_different_target", "differs from"),
        ("same_leaf_different_module", "differs from"),
        ("same_leaf_different_library_module", "differs from"),
        ("valid_library_module", None),
        ("wrong_inventory", "inventory SHA-256 differs"),
        ("wrong_digest", "terminal SHA-256 differs"),
        ("different_pass", "has no PASS"),
        ("unrelated_valid_case", "terminal observations differ from declared replacement tests"),
        ("unobserved_required_case", "terminal observations differ from declared replacement tests"),
        ("no_declared_cases", "requires explicit replacement tests"),
        ("truncated", "incomplete suite events"),
        ("valid", None),
        ("release_missing_manifest", "lacks the frozen source manifest SHA-256"),
        ("release_stale_lock", "frozen source tree differs from the release candidate"),
        ("release_stale_implementation", "frozen source tree differs from the release candidate"),
        ("release_wrong_tree", "frozen source tree SHA-256 differs"),
        ("release_unbound_terminal", "source manifest events_sha256 differs from the observation"),
        ("release_unbound_binary", "lacks its revision and terminal binding"),
        ("release_swapped_binary", "Nextest collection differs from the built executable inventory"),
        ("release_changed_binary", "retained Nextest executable bytes differ"),
        ("release_wrong_feature_command", "retained Nextest commands differ from the recorded selection"),
        ("release_wrong_selection", "Nextest collection contains a target outside the recorded selection"),
        ("release_missing_test_target", "Nextest collection lacks recorded test targets"),
        ("release_missing_lib_target", "Nextest collection lacks the recorded library target"),
        ("release_wrong_workspace", "Nextest build metadata differs from the workspace or its Cargo target"),
        ("release_unbound_legacy_path", "old-Q execution path differs from the retained executable"),
        ("release_changed_legacy_binary", "old-Q executable SHA-256 differs"),
        ("release_valid", "retained old-Q tests require VERIFIED execution"),
    ],
)
def test_retirement_requires_complete_nextest_evidence(checker, capsys, monkeypatch, evidence, expected_error):
    crate = "quanta-index-lexical"
    test_source = f"crates/{crate}/tests/replacement.rs"
    library_event = evidence in {"same_leaf_different_library_module", "valid_library_module"}
    source = (
        f"crates/{crate}/src/required_contract.rs"
        if library_event or evidence == "same_leaf_different_target"
        else test_source
    )
    body = b"#[test]\nfn replacement() {}\n"
    if evidence in {"unrelated_valid_case", "unobserved_required_case"}:
        body += b"#[test]\nfn required_behavior() {}\n"
    if library_event:
        body = b"mod required {\n#[test]\nfn replacement() {}\n}\n"
    (checker.ROOT / source).parent.mkdir(parents=True, exist_ok=True)
    (checker.ROOT / source).write_bytes(body)
    if source != test_source:
        (checker.ROOT / test_source).write_bytes(body)
        (checker.ROOT / f"crates/{crate}/Cargo.toml").write_text("[package]\n", encoding="utf-8")
    catalog = checker.ROOT / "tools/ci/test-authority.toml"
    catalog.write_text(
        f'[[integration_targets]]\npath = "{test_source}"\nowner = "{crate}"\n',
        encoding="utf-8",
    )
    run_id = "3739620b-a0ff-451d-897f-e5a32fc146eb"
    target = crate.replace("-", "_") if library_event else "replacement"
    kind = "lib" if library_event else "test"
    case = {
        "same_leaf_different_module": "other_module::replacement",
        "same_leaf_different_library_module": "required_contract::unrelated::replacement",
        "valid_library_module": "required_contract::required::replacement",
    }.get(evidence, "replacement")
    event = f"{crate}::{target}${case}"
    recorded = event.replace("$replacement", "$other") if evidence == "different_pass" else event
    meta = {"crate": crate, "test_binary": target, "kind": kind}
    binary_id = f"{crate}::{target}"
    target_dir = checker.ROOT / f"{run_id}-target"
    binary = {
        "binary-id": binary_id, "binary-path": str(target_dir / "replacement"),
        "binary-name": target, "kind": kind,
    }
    inventory = {
        "test-count": 1,
        "rust-suites": {
            binary_id: {
                **binary,
                "package-name": crate,
                "status": "listed",
                "testcases": {
                    recorded.partition("$")[2]: {
                        "ignored": False,
                        "filter-match": {"status": "matches"},
                    }
                },
            }
        },
    }
    inventory_bytes = json.dumps(inventory).encode()
    (checker.ROOT / f"{run_id}-inventory.json").write_bytes(inventory_bytes)
    inventory_digest = (
        "e" * 64 if evidence == "wrong_inventory" else hashlib.sha256(inventory_bytes).hexdigest()
    )
    rows = [
        {"type": "suite", "event": "started", "test_count": 1, "nextest": meta},
        {"type": "test", "event": "started", "name": recorded},
        {"type": "test", "event": "ok", "name": recorded},
        {"type": "suite", "event": "ok", "passed": 1, "failed": 0, "ignored": 0, "nextest": meta},
    ]
    if evidence == "truncated":
        rows.pop()
    raw = "".join(json.dumps(row) + "\n" for row in rows).encode()
    if evidence != "absent":
        (checker.ROOT / f"{run_id}-nextest.jsonl").write_bytes(raw)
    sha256 = hashlib.sha256(raw).hexdigest() if evidence != "wrong_digest" else "f" * 64
    if evidence == "wrong_binary":
        event = event.replace("::replacement", "::other")
    receipt_fields = ""
    if evidence != "uuid_only":
        receipt_fields = f', event = "{event}", events_sha256 = "{sha256}"'
        if evidence != "events_only":
            receipt_fields += f', inventory_sha256 = "{inventory_digest}"'
    release = evidence.startswith("release_")
    if release:
        monkeypatch.setattr(retirement_nextest, "release_event_roster", lambda *_: {event: {}})
    candidate_lock = b'version = 4\n# candidate lock\n'
    stale_lock = b'version = 4\n# previous lock\n'
    lock = checker.ROOT / "Cargo.lock"
    implementation = checker.ROOT / "crates/quanta-index-lexical/src/lib.rs"
    manifest_path = str(checker.MANIFEST.relative_to(checker.ROOT))

    def git(*args):
        return subprocess.check_output(
            ["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", *args],
            cwd=checker.ROOT, text=True,
        ).strip()

    if release:
        lock.write_bytes(stale_lock if evidence == "release_stale_lock" else candidate_lock)
        implementation.parent.mkdir()
        implementation.write_text("fn before() {}\n", encoding="utf-8")
        git("init", "-q")
        (checker.ROOT / ".git/info/exclude").write_text("/*-inventory.json\n/*-nextest.jsonl\n", encoding="utf-8")
        git("add", manifest_path, "Cargo.toml", "Cargo.lock", "crates", "tools/ci/test-authority.toml")
        git("commit", "-qm", "executed source")
        run_revision = git("rev-parse", "HEAD")

    if release and evidence != "release_missing_manifest":
        source_binding = retirement_nextest.release_source_manifest(
            checker.ROOT,
            run_revision,
            checker.ROOT / f"{run_id}-inventory.json",
            checker.ROOT / f"{run_id}-nextest.jsonl",
        )
        if evidence == "release_wrong_tree":
            source_binding["source_tree_sha256"] = "f" * 64
        if evidence == "release_unbound_terminal":
            source_binding["events_sha256"] = "f" * 64
        if evidence != "release_unbound_binary":
            built = dict(binary)
            if evidence == "release_swapped_binary":
                built["binary-path"] = str(target_dir / "different-replacement")
            binaries_bytes = json.dumps({
                "rust-build-meta": {"target-directory": str(target_dir)},
                "rust-binaries": {binary_id: built},
            }).encode()
            cargo_bytes = json.dumps({
                "target_directory": str(target_dir),
                "workspace_root": str(
                    checker.ROOT / "unrelated-source" if evidence == "release_wrong_workspace" else checker.ROOT
                ),
            }).encode()
            (checker.ROOT / f"{run_id}-binaries.json").write_bytes(binaries_bytes)
            (checker.ROOT / f"{run_id}-cargo.json").write_bytes(cargo_bytes)
            target_dir.mkdir()
            executable = target_dir / "replacement"
            executable.write_bytes(b"changed binary" if evidence == "release_changed_binary" else b"test binary")
            selection = {
                "lane": "test-integration-lane", "package": crate,
                "include_lib": evidence == "release_missing_lib_target",
                "test_targets": ["unrelated" if evidence == "release_wrong_selection" else target],
                "filterset": "test(=replacement)", "features": [],
            }
            if evidence == "release_missing_test_target":
                selection["test_targets"].append("uncollected")
            commands = retirement_nextest._execution_commands(
                selection, checker.ROOT / f"{run_id}-binaries.json", checker.ROOT / f"{run_id}-cargo.json",
            )
            if evidence == "release_wrong_feature_command":
                commands["cargo.json"].extend(["--features", f"{crate}/different"])
            legacy = None
            if evidence in {"release_changed_legacy_binary", "release_unbound_legacy_path"}:
                retained = target_dir / "legacy-searchd"
                retained.write_bytes(
                    b"replacement old daemon" if evidence == "release_changed_legacy_binary" else b"old daemon"
                )
                executed = checker.ROOT / "old-searchd" if evidence == "release_unbound_legacy_path" else retained
                legacy = {"path": str(executed), "sha256": hashlib.sha256(b"old daemon").hexdigest()}
            source_binding["execution"] = {
                "workspace_root": str(checker.ROOT),
                "binaries_sha256": "sha256:" + hashlib.sha256(binaries_bytes).hexdigest(),
                "cargo_sha256": "sha256:" + hashlib.sha256(cargo_bytes).hexdigest(),
                "executables": {binary_id: hashlib.sha256(b"test binary").hexdigest()},
                "selection": selection,
                "commands": commands,
                "legacy_searchd": legacy,
            }
        source_manifest = json.dumps(source_binding).encode()
        (checker.ROOT / f"{run_id}-source-manifest.json").write_bytes(source_manifest)
        receipt_fields += f', source_manifest_sha256 = "{hashlib.sha256(source_manifest).hexdigest()}"'
    observation = (
        f'observations = [{{source = "{source}", case = "replacement", '
        f'source_sha256 = "{hashlib.sha256(body).hexdigest()}", run_id = "{run_id}"{receipt_fields}}}]'
    )
    cases = ["replacement"]
    if evidence == "unrelated_valid_case":
        cases = ["required_behavior"]
    elif evidence == "unobserved_required_case":
        cases.append("required_behavior")
    candidates = (
        f'candidates = [{{path = "{source}", tests = {json.dumps(cases)}}}]\n'
        if evidence != "no_declared_cases" else ""
    )
    roster = checker.MANIFEST.read_text(encoding="utf-8").replace(
        'disposition = "retire"\nproof = "NOT_APPLICABLE"',
        f'disposition = "migrate"\nproof = "VERIFIED"\n{candidates}{observation}',
        1,
    )
    checker.MANIFEST.write_text(roster, encoding="utf-8")

    revision = None
    if release:
        lock.write_bytes(candidate_lock)
        if evidence == "release_stale_implementation":
            implementation.write_text("fn after() {}\n", encoding="utf-8")
        git("add", manifest_path, "Cargo.lock", "crates", "tools/ci/test-authority.toml")
        git("commit", "-qm", "candidate")
        revision = git("rev-parse", "HEAD")
        lock.write_bytes(stale_lock)
    if evidence == "events_only":
        assert checker.main(inventory_only=True) == 0
    assert checker.main(revision=revision, evidence_dir=checker.ROOT) == (0 if expected_error is None else 1)
    if expected_error is not None:
        errors = capsys.readouterr().err
        assert expected_error in errors
        if evidence == "release_valid":
            assert "sealed_manifest.rs" not in errors
        else:
            assert "sealed_manifest.rs: source was removed with 43 test IDs still lacking VERIFIED" in errors
            assert "sealed_manifest.rs [BLOCKED]" in errors


@pytest.mark.parametrize(
    ("state", "message"),
    [
        ("dirty", "refusing dirty retirement checkout"),
        ("during-capture", "refusing dirty retirement checkout"),
        ("git-normalized", "differs from committed HEAD: Cargo.lock"),
        ("restored-during-capture", "release source changed during execution: Cargo.lock"),
        ("new-head", "differs from retirement checkout HEAD"),
        ("assume-unchanged", "refusing hidden or unresolved tracked retirement source: Cargo.lock"),
        ("skip-worktree", "refusing hidden or unresolved tracked retirement source: Cargo.lock"),
    ],
)
def test_release_source_manifest_rejects_checkout_drift(tmp_path, monkeypatch, state, message):
    checkout = tmp_path / "source"
    checkout.mkdir()

    def git(*args):
        return subprocess.check_output(
            ["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", *args],
            cwd=checkout, text=True,
        ).strip()

    git("init", "-q")
    source = checkout / "Cargo.lock"
    source.write_text("version = 4\n", encoding="utf-8")
    if state == "git-normalized":
        (checkout / ".gitattributes").write_text("Cargo.lock text eol=crlf\n", encoding="utf-8")
        git("add", ".gitattributes")
    git("add", "Cargo.lock")
    git("commit", "-qm", "executed source")
    revision = git("rev-parse", "HEAD")
    if state in {"assume-unchanged", "skip-worktree"}:
        git("update-index", f"--{state}", "Cargo.lock")
    if state == "git-normalized":
        source.unlink()
        git("checkout-index", "--force", "--index", "--", "Cargo.lock")
        assert source.read_bytes() == b"version = 4\r\n"
        assert git("status", "--porcelain") == ""
    elif state not in {"during-capture", "restored-during-capture"}:
        source.write_text("version = 4\n# drift\n", encoding="utf-8")
    if state in {"assume-unchanged", "skip-worktree"}:
        assert git("status", "--porcelain") == ""
    if state == "new-head":
        git("commit", "-qam", "new candidate")

    inventory, events = tmp_path / "inventory.json", tmp_path / "events.jsonl"
    inventory.write_bytes(b"fixture inventory")

    def parse_terminal(*_):
        if state == "during-capture":
            source.write_text("version = 4\n# drift\n", encoding="utf-8")
        if state == "restored-during-capture":
            original = source.read_bytes()
            source.write_bytes(original + b"# drift\n")
            source.write_bytes(original)
        return SimpleNamespace(sha256="a" * 64)

    monkeypatch.setattr(retirement_nextest, "parse_nextest_inventory", lambda _: {})
    monkeypatch.setattr(retirement_nextest, "parse_nextest", parse_terminal)
    with pytest.raises(ValueError, match=message):
        retirement_nextest.release_source_manifest(checkout, revision, inventory, events)


@pytest.mark.parametrize("changed", [
    None, "test", "native-daemon", "old-q", "supplier-removed", "test-restored",
    "old-q-restored", "metadata-restored", "retained-dylib", "extra-selection", "omitted-selection",
    "outside-roster", "retained-outside-roster", "outside-roster-without-expectation",
    "retained-outside-roster-without-expectation", "roster-resume", "roster-changed-terminal",
    "native-stale-old-q",
])
def test_capture_uses_one_build_and_publishes_only_stable_execution(tmp_path, monkeypatch, changed):
    checkout = tmp_path / "source"
    checkout.mkdir()
    evidence = tmp_path / "evidence"
    evidence.mkdir()
    binary_id = "quanta-index-lexical::replacement"
    extra_targets = {"quanta_index_lexical": "lib", "additional": "test"}
    event = binary_id + "$passes"
    meta = {"crate": "quanta-index-lexical", "test_binary": "replacement", "kind": "test"}
    calls = []
    monkeypatch.setenv("QUANTA_INDEX_RESOURCE_ADMISSION", "0")
    monkeypatch.setenv("QUANTA_INDEX_CACHE_ROOT", str(tmp_path / "foreign-cache"))
    monkeypatch.setenv("CI", "true")
    legacy_path = tmp_path / "old-searchd"
    legacy_path.write_bytes(b"old daemon")
    legacy_sha = hashlib.sha256(b"old daemon").hexdigest()
    monkeypatch.setenv(retirement_nextest.LEGACY_BINARY_ENV, str(legacy_path))
    monkeypatch.setenv(retirement_nextest.LEGACY_SHA_ENV, legacy_sha)
    uses_old_q = changed != "native-stale-old-q"
    if uses_old_q:
        monkeypatch.setattr(retirement_nextest, "OLD_Q_TARGETS", {"quanta-index-lexical": {"replacement"}})
        monkeypatch.setattr(retirement_nextest, "_approved_linux_old_q_sha256", lambda *_: legacy_sha)
        monkeypatch.setattr(retirement_nextest, "platform", SimpleNamespace(
            system=lambda: "Linux", machine=lambda: "x86_64",
        ))
    else:
        legacy_path.unlink()
    platforms = {"host": {"platform": {"triple": "x86_64-unknown-linux-gnu"}}, "targets": []}

    def run(argv, *, cwd, env, stdout, check):
        assert cwd == checkout and check is True
        assert argv[:3] == [str(checkout / "scripts/cargow"), "--lane", "test-integration-lane"]
        target = Path(env["CARGO_TARGET_DIR"])
        assert target.parent == evidence and env["QUANTA_INDEX_PRESERVE_CARGO_TARGET_DIR"] == "1"
        assert env["QUANTA_INDEX_RESOURCE_ADMISSION"] == "1"
        assert "QUANTA_INDEX_CACHE_ROOT" not in env
        retained = target / "legacy-searchd"
        if uses_old_q:
            assert env[retirement_nextest.LEGACY_BINARY_ENV] == str(retained)
            assert os.access(retained, os.X_OK)
            assert retained.read_bytes() == b"old daemon"
        else:
            assert retirement_nextest.LEGACY_BINARY_ENV not in env
            assert retirement_nextest.LEGACY_SHA_ENV not in env
            assert not retained.exists()
        executable = target / "replacement"
        daemon = target / "debug/quanta-index-searchd"
        dylib = target / "debug/deps/libnative.dylib"
        row = {"binary-id": binary_id, "binary-path": str(executable), "binary-name": "replacement", "kind": "test"}
        rows = {binary_id: row}
        for name, kind in extra_targets.items():
            extra_id = f"quanta-index-lexical::{name}"
            rows[extra_id] = {
                "binary-id": extra_id, "binary-path": str(target / name),
                "binary-name": name, "kind": kind,
            }
        args = argv[3:]
        calls.append(args)
        if "binaries-only" in args:
            if changed == "supplier-removed":
                legacy_path.unlink()
            for built in rows.values():
                Path(built["binary-path"]).write_bytes(b"test binary")
            dylib.parent.mkdir(parents=True)
            daemon.write_bytes(b"native daemon")
            dylib.write_bytes(b"native library")
            stdout.write(json.dumps({
                "rust-build-meta": {
                    "target-directory": str(target),
                    "platforms": platforms,
                    "non-test-binaries": {
                        "test-subject": [
                            {"name": "quanta-index-searchd", "kind": "bin-exe", "path": "debug/quanta-index-searchd"},
                            {"name": "native", "kind": "dylib", "path": "debug/deps/libnative.dylib"},
                        ],
                    },
                },
                "rust-binaries": rows,
            }).encode())
        elif args[0] == "metadata":
            stdout.write(json.dumps({"workspace_root": str(checkout), "target_directory": str(target)}).encode())
        elif args[1] == "list":
            cases = {"passes": {"ignored": False, "filter-match": {"status": "matches"}}}
            if changed in {"extra-selection", "outside-roster-without-expectation"}:
                cases["unexpected"] = dict(cases["passes"])
            stdout.write(json.dumps({
                "test-count": len(cases),
                "rust-build-meta": {"platforms": platforms},
                "rust-suites": {
                    name: {
                        **built, "package-name": "quanta-index-lexical", "status": "listed",
                        "testcases": cases if name == binary_id else {},
                    }
                    for name, built in rows.items()
                },
            }).encode())
        else:
            assert args[1] == "run" and env["NEXTEST_EXPERIMENTAL_LIBTEST_JSON"] == "1"
            if changed == "test":
                executable.write_bytes(b"other binary")
            if changed == "native-daemon":
                daemon.write_bytes(b"other daemon")
            if changed == "old-q":
                retained.write_bytes(b"other old daemon")
            restored = {
                "test-restored": executable,
                "old-q-restored": retained,
                "metadata-restored": Path(args[args.index("--binaries-metadata") + 1]),
            }.get(changed)
            if restored is not None:
                original = restored.read_bytes()
                file_stat = restored.stat()
                restored.write_bytes(original + b"changed")
                restored.write_bytes(original)
                os.utime(restored, ns=(file_stat.st_atime_ns, file_stat.st_mtime_ns))
            executed = [event]
            if changed == "outside-roster-without-expectation":
                executed.append(binary_id + "$unexpected")
            events = [{"type": "suite", "event": "started", "test_count": len(executed), "nextest": meta}]
            for name in executed:
                events.extend((
                    {"type": "test", "event": "started", "name": name},
                    {"type": "test", "event": "ok", "name": name},
                ))
            events.append({
                "type": "suite", "event": "ok", "passed": len(executed), "failed": 0, "ignored": 0, "nextest": meta,
            })
            for line in events:
                stdout.write((json.dumps(line) + "\n").encode())

    monkeypatch.setattr(retirement_nextest, "_require_frozen_checkout", lambda *_: None)
    monkeypatch.setattr(retirement_nextest, "_capture_frozen_checkout", lambda *_: {})
    monkeypatch.setattr(retirement_nextest, "release_tree_sha256", lambda *_: "a" * 64)
    committed_events = {event: {}, binary_id + "$missing": {}}
    monkeypatch.setattr(retirement_nextest, "release_event_roster", lambda *_: committed_events)
    monkeypatch.setattr(retirement_nextest, "subprocess", SimpleNamespace(run=run))
    args = dict(
        lane="test-integration-lane", package="quanta-index-lexical",
        include_lib=True, test_targets=["replacement", "additional"],
        filterset="test(=passes)", features=["native-one, native-two"],
        expected_events=[event],
    )
    if changed == "outside-roster":
        args["expected_events"].append(binary_id + "$unexpected")
        with pytest.raises(ValueError, match="outside the committed retirement roster"):
            retirement_nextest.capture_release_execution(checkout, "b" * 40, evidence, **args)
        assert not calls
        assert not list(evidence.iterdir())
        return
    if changed == "outside-roster-without-expectation":
        del args["expected_events"]
        with pytest.raises(ValueError, match="outside the committed retirement roster"):
            retirement_nextest.capture_release_execution(checkout, "b" * 40, evidence, **args)
        assert len(calls) == 3
        assert not list(evidence.glob("*-nextest.jsonl"))
        return
    if changed in {"extra-selection", "omitted-selection"}:
        if changed == "omitted-selection":
            args["expected_events"].append(binary_id + "$missing")
        message = r"extra=\[.*unexpected" if changed == "extra-selection" else r"missing=\[.*missing"
        with pytest.raises(ValueError, match=message):
            retirement_nextest.capture_release_execution(checkout, "b" * 40, evidence, **args)
        assert len(calls) == 3
        assert not list(evidence.glob("*-nextest.jsonl"))
        return
    if changed not in {
        None, "supplier-removed", "retained-dylib", "retained-outside-roster",
        "retained-outside-roster-without-expectation", "roster-resume", "roster-changed-terminal",
        "native-stale-old-q",
    }:
        with pytest.raises(ValueError, match="build inputs or evidence changed"):
            retirement_nextest.capture_release_execution(checkout, "b" * 40, evidence, **args)
        assert not list(evidence.glob("*-source-manifest.json"))
    else:
        path = retirement_nextest.capture_release_execution(checkout, "b" * 40, evidence, **args)
        manifest = json.loads(path.read_bytes())
        assert manifest["execution"]["workspace_root"] == str(checkout)
        assert manifest["execution"]["selection"]["expected_events"] == [event]
        expected_executables = {
            f"quanta-index-lexical::{name}": hashlib.sha256(b"test binary").hexdigest()
            for name in ("replacement", *extra_targets)
        }
        expected_executables.update({
            "runtime/debug/quanta-index-searchd": hashlib.sha256(b"native daemon").hexdigest(),
            "runtime/debug/deps/libnative.dylib": hashlib.sha256(b"native library").hexdigest(),
        })
        assert manifest["execution"]["executables"] == expected_executables
        run_id = path.name.removesuffix("-source-manifest.json")
        target = evidence / f"{run_id}-target"
        expected_legacy = {
            "path": str(target / "legacy-searchd"), "sha256": legacy_sha,
        } if uses_old_q else None
        assert manifest["execution"]["legacy_searchd"] == expected_legacy
        assert list(manifest["execution"]["commands"].values()) == [
            ["--lane", args["lane"], *cmd]
            for cmd in (calls[0], calls[1], calls[2], calls[3])
        ]
        relocated = evidence.rename(tmp_path / "relocated")
        legacy_path.unlink(missing_ok=True)
        retained_manifest = relocated / path.name
        if changed == "retained-outside-roster-without-expectation":
            del manifest["execution"]["selection"]["expected_events"]
            retained_manifest.write_text(json.dumps(manifest), encoding="utf-8")
        artifact = retirement_nextest.RawFile.capture(retained_manifest)
        observation = {
            "run_id": run_id,
            "source_manifest_sha256": artifact.sha256.removeprefix("sha256:"),
            "inventory_sha256": manifest["inventory_sha256"],
            "events_sha256": manifest["events_sha256"],
        }
        if changed == "retained-dylib":
            (relocated / target.name / "debug/deps/libnative.dylib").write_bytes(b"other library")
            with pytest.raises(ValueError, match="retained Nextest executable bytes differ"):
                retirement_nextest.check_frozen_inputs(artifact, observation, checkout, "b" * 40)
        elif changed in {"retained-outside-roster", "retained-outside-roster-without-expectation"}:
            del committed_events[event]
            with pytest.raises(ValueError, match="outside the committed retirement roster"):
                retirement_nextest.check_frozen_inputs(artifact, observation, checkout, "c" * 40)
        else:
            retirement_nextest.check_frozen_inputs(artifact, observation, checkout, "b" * 40)
        if changed in {"roster-resume", "roster-changed-terminal"}:
            selection = manifest["execution"]["selection"]
            monkeypatch.setattr(retirement_nextest, "release_capture_batches", lambda *_: [selection])
            captures = retirement_nextest.capture_release_roster(
                checkout, "b" * 40, relocated, lane=selection["lane"],
            )
            if changed == "roster-changed-terminal":
                terminal = relocated / f"{run_id}-nextest.jsonl"
                terminal.write_bytes(b" " + terminal.read_bytes())
                with pytest.raises(ValueError, match="differs from the planned passing batch"):
                    list(captures)
            else:
                assert list(captures) == [retained_manifest]
    assert len(calls) == 4
    resolution = ["--locked", "--offline", "--no-default-features", "--features",
                  "quanta-index-lexical/native-one,quanta-index-lexical/native-two"]
    assert calls[0][2:] == [
        *resolution, "-p", "quanta-index-lexical", "--lib", "--test", "replacement",
        "--test", "additional", "--list-type", "binaries-only", "--message-format", "json",
    ]
    assert calls[1] == ["metadata", "--format-version", "1", *resolution]
    assert calls[2][:2] == ["nextest", "list"] and calls[3][:2] == ["nextest", "run"]
    assert calls[2][2:8] == calls[3][2:8]


@pytest.mark.parametrize("runtime_path", ["../native", "/native", "debug/./native"])
def test_runtime_artifact_path_must_be_canonical_under_target(tmp_path, runtime_path):
    target = tmp_path / "target"
    binaries = tmp_path / "binaries.json"
    binaries.write_text(json.dumps({
        "rust-build-meta": {
            "target-directory": str(target),
            "non-test-binaries": {"test-subject": [{"path": runtime_path}]},
        },
        "rust-binaries": {"test-binary": {"binary-id": "test-binary", "binary-path": str(target / "test")}},
    }), encoding="utf-8")
    cargo = tmp_path / "cargo.json"
    cargo.write_text(json.dumps({
        "target_directory": str(target), "workspace_root": str(tmp_path),
    }), encoding="utf-8")

    with pytest.raises(ValueError, match="runtime artifact is outside its Cargo target or has a noncanonical path"):
        retirement_nextest._build_paths(
            retirement_nextest.RawFile.capture(binaries), retirement_nextest.RawFile.capture(cargo), root=tmp_path,
        )


def test_release_roster_resolves_committed_events_and_deduplicates_shared_witnesses(tmp_path):
    path = "crates/quanta-index-lexical/tests/retained.rs"
    source = tmp_path / path
    source.parent.mkdir(parents=True)
    source.write_text("#[test]\nfn keep() {}\n#[test]\nfn replacement() {}\n", encoding="utf-8")
    inventory = tmp_path / "tools/ci/inventory"
    inventory.mkdir(parents=True)
    (inventory.parent / "test-authority.toml").write_text(
        f'[[integration_targets]]\npath = "{path}"\nowner = "quanta-index-lexical"\n', encoding="utf-8",
    )
    lexical = inventory / "lexical-test-disposition.toml"
    lexical.write_text(
        f'[[targets]]\npath = "{path}"\n'
        '[[targets.dispositions]]\ndisposition = "retain"\ntests = ["keep"]\n'
        '[[targets.dispositions]]\ndisposition = "migrate"\n'
        f'candidates = [{{path = "{path}", tests = ["replacement"]}}]\n',
        encoding="utf-8",
    )
    (inventory / "runtime-legacy-migration.toml").write_text(
        'exact_oracles = []\n[[current_proofs]]\n'
        f'candidates = [{{path = "{path}", tests = ["replacement"]}}]\n',
        encoding="utf-8",
    )
    (source.parent.parent / "Cargo.toml").write_text(
        '[[test]]\nname = "retained"\nrequired-features = ["retirement"]\n',
        encoding="utf-8",
    )

    def git(*args):
        return subprocess.check_output(
            ["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", *args],
            cwd=tmp_path, text=True,
        ).strip()

    git("init", "-q")
    git("add", ".")
    git("commit", "-qm", "committed roster")
    revision = git("rev-parse", "HEAD")
    lexical.write_text("targets = []\n", encoding="utf-8")
    (inventory.parent / "test-authority.toml").write_text("integration_targets = []\n", encoding="utf-8")

    assert retirement_nextest.release_event_roster(tmp_path, revision) == {
        "quanta-index-lexical::retained$keep": {"source": path, "case": "keep"},
        "quanta-index-lexical::retained$replacement": {"source": path, "case": "replacement"},
    }
    assert retirement_nextest.release_capture_batches(tmp_path, revision) == [{
        "package": "quanta-index-lexical", "include_lib": False,
        "test_targets": ["retained"], "features": ["quanta-index-lexical/retirement"],
        "filterset": "(binary(=retained) & (test(=keep) | test(=replacement)))",
        "expected_events": [
            "quanta-index-lexical::retained$keep",
            "quanta-index-lexical::retained$replacement",
        ],
    }]


def test_capture_roster_admits_pending_batches_before_work_and_serializes_owners(tmp_path, monkeypatch):
    checkout = tmp_path / "source"
    checkout.mkdir()
    evidence = tmp_path / "evidence"
    evidence.mkdir()
    native = "quanta-index-core::quanta_index_core$local"
    legacy = "quanta-index-lexical::legacy_existing$retained"
    batches = [
        {"package": "quanta-index-core", "test_targets": [], "expected_events": [native]},
        {"package": "quanta-index-lexical", "test_targets": ["legacy_existing"], "expected_events": [legacy]},
    ]
    monkeypatch.setattr(retirement_nextest, "_require_frozen_checkout", lambda *_: None)
    monkeypatch.setattr(retirement_nextest, "release_capture_batches", lambda *_: batches)
    retained = {}
    monkeypatch.setattr(retirement_nextest, "_reusable_captures", lambda *_: retained)
    monkeypatch.setattr(retirement_nextest, "_require_capture_host", lambda *_: None)
    monkeypatch.delenv(retirement_nextest.LEGACY_SHA_ENV, raising=False)
    monkeypatch.delenv(retirement_nextest.LEGACY_BINARY_ENV, raising=False)
    launched = []

    def capture(*_, **selection):
        launched.append(selection["expected_events"][0])
        return evidence / "new-manifest.json"

    monkeypatch.setattr(retirement_nextest, "capture_release_execution", capture)

    def run():
        return retirement_nextest.capture_release_roster(checkout, "b" * 40, evidence, lane="proof-lane")

    with pytest.raises(ValueError, match="retained old-Q executable is absent"):
        list(run())
    assert launched == []

    retained[(legacy,)] = evidence / "verified-manifest.json"
    monkeypatch.setenv(retirement_nextest.LEGACY_BINARY_ENV, str(tmp_path / "expired-old-q"))
    monkeypatch.setenv(retirement_nextest.LEGACY_SHA_ENV, "a" * 64)
    active = run()
    assert next(active) == evidence / "new-manifest.json"
    with pytest.raises(ValueError, match="already holds the evidence directory"):
        list(run())
    assert list(active) == [retained[(legacy,)]]
    assert launched == [native]

    active = run()
    assert next(active) == evidence / "new-manifest.json"
    evidence.rename(tmp_path / "original-evidence")
    evidence.mkdir()
    with pytest.raises(ValueError, match="retirement evidence directory changed"):
        list(active)
    assert launched == [native, native]


def test_retained_executable_digest_reuses_only_unchanged_bytes(tmp_path, monkeypatch):
    archive = tmp_path / "retained-binary"
    archive.write_bytes(b"first executable")
    before = archive.stat()
    captured = []
    capture = retirement_nextest.RawFile.capture

    def hash_once(path):
        captured.append(path)
        return capture(path)

    monkeypatch.setattr(retirement_nextest, "RawFile", SimpleNamespace(capture=hash_once))
    digest = retirement_nextest.retained_executable_digests()
    assert digest(archive) == digest(archive) == hashlib.sha256(b"first executable").hexdigest()
    assert len(captured) == 1
    archive.write_bytes(b"other executable")
    os.utime(archive, ns=(before.st_atime_ns, before.st_mtime_ns))
    assert digest(archive) == hashlib.sha256(b"other executable").hexdigest()
    assert len(captured) == 2
    archive.rename(tmp_path / "original")
    archive.symlink_to("original")
    with pytest.raises(ValueError, match="invalid retained Nextest executable"):
        digest(archive)


def test_removed_source_with_dispositions_keeps_frozen_identities(checker, capsys):
    inventory = checker.MANIFEST.read_text(encoding="utf-8")
    assert checker.main() == 0
    assert "93 IDs across 4 targets" in capsys.readouterr().out

    checker.MANIFEST.write_text(
        inventory.replace('"tantivy_applies_query_boost_to_scores"', '"unrelated_same_count_test"'),
        encoding="utf-8",
    )
    assert checker.main() == 1
    assert (
        "tantivy_smoke.rs: mapped test IDs differ from the frozen census" in capsys.readouterr().err
    )


def test_shipment_checks_committed_roster_not_working_tree(checker, capsys):
    inventory = checker.MANIFEST.read_text(encoding="utf-8")
    unresolved = inventory.replace('disposition = "retire"', 'disposition = "migrate"').replace(
        'proof = "NOT_APPLICABLE"', 'proof = "NOT_RUN"'
    )
    checker.MANIFEST.write_text(unresolved, encoding="utf-8")
    for args in (
        ("init", "-q"),
        ("add", "."),
        (
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "-qm",
            "candidate",
        ),
    ):
        subprocess.run(["git", *args], cwd=checker.ROOT, check=True)
    revision = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=checker.ROOT, text=True
    ).strip()

    checker.MANIFEST.write_text(inventory, encoding="utf-8")
    assert checker.main() == 0
    assert checker.main(revision=revision, inventory_only=True) == 1
    assert "committed retirement checks require migration dispositions" in capsys.readouterr().err
    assert checker.main(revision=revision) == 1
    assert "source was removed with 43 test IDs" in capsys.readouterr().err

    subprocess.run(
        [
            "git",
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "-qam",
            "retire removed engine",
        ],
        cwd=checker.ROOT,
        check=True,
    )
    approved_revision = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=checker.ROOT, text=True
    ).strip()
    assert checker.main(revision=approved_revision) == 1
    assert "sealed_commitment_cost.rs: retained old-Q tests require VERIFIED execution" in capsys.readouterr().err


@pytest.mark.parametrize(
    ("evidence", "expected_error"),
    [
        ("complete", None),
        ("partial", "terminal observations differ from retained tests"),
        ("foreign", "Nextest event"),
        ("without_old_q", "retained old-Q executable is absent"),
        ("changed_old_q", "old-Q executable SHA-256 differs"),
        ("unapproved_old_q", "does not match the committed approved Linux old-Q SHA-256"),
        ("dirty_approval", "does not match the committed approved Linux old-Q SHA-256"),
        ("darwin_pass", "requires x86-64 Linux host and target"),
        ("cross_compiled", "requires x86-64 Linux host and target"),
        ("collection_platform_drift", "Nextest collection platform differs"),
        ("missing_platform", "Nextest host and target platform metadata is absent or invalid"),
        ("dirty_source", "retained test source differs from committed HEAD"),
        ("source_swap", "observation source changed"),
        ("source_swap_with_original_observation", "retained test source changed during proof verification"),
        ("head_advance", "HEAD changed during retained proof verification"),
        ("relabeled_pending", "frozen old-Q tests must retain their original disposition"),
    ],
)
def test_committed_retained_old_q_tests_require_same_candidate_exact_passes(
    checker, tmp_path, capsys, monkeypatch, evidence, expected_error,
):
    source = "crates/quanta-index-lexical/tests/sealed_commitment_cost.rs"
    cases = [
        "file_admission_counts_verified_changed_sources_separately",
        "a_delta_seal_rehashes_coverage_but_inherits_other_unmodified_files",
    ]
    body = (checker.ROOT / source).read_bytes()
    package, target = "quanta-index-lexical", "sealed_commitment_cost"
    binary_id = f"{package}::{target}"
    monkeypatch.setattr(retirement_nextest, "release_event_roster", lambda *_: {
        f"{binary_id}${case}": {} for case in cases
    })
    run_id = "3739620b-a0ff-451d-897f-e5a32fc146eb"
    evidence_dir = tmp_path.with_name(f"{tmp_path.name}-retained-evidence")
    evidence_dir.mkdir()
    target_dir = evidence_dir / f"{run_id}-target"
    target_dir.mkdir()
    binary = target_dir / target
    binary.write_bytes(b"retained test executable")
    old_q = target_dir / "legacy-searchd"
    approved_digest = hashlib.sha256(b"old daemon").hexdigest()
    old_q.write_bytes(b"changed old daemon" if evidence in {
        "changed_old_q", "unapproved_old_q", "dirty_approval",
    } else b"old daemon")
    legacy_digest = (
        hashlib.sha256(old_q.read_bytes()).hexdigest()
        if evidence in {"unapproved_old_q", "dirty_approval"}
        else approved_digest
    )
    circleci = checker.ROOT / ".circleci/config.yml"
    circleci.parent.mkdir(exist_ok=True)
    ci_config = (
        "jobs:\n  verify-rust-tests:\n    environment:\n"
        f'      QUANTA_INDEX_LEGACY_SEARCHD_SHA256: "{approved_digest}"\n'
    )
    circleci.write_text(ci_config, encoding="utf-8")
    row = {
        "binary-id": binary_id, "binary-path": str(binary),
        "binary-name": target, "kind": "test",
    }
    linux = "x86_64-unknown-linux-gnu"
    darwin = "aarch64-apple-darwin"

    def platform_metadata(host, target=None):
        return {
            "target-directory": str(target_dir),
            "platforms": {
                "host": {"platform": {"triple": host}},
                "targets": [] if target is None else [{"platform": {"triple": target}}],
            },
            "target-platforms": [{"triple": target or host}],
        }

    builds = platform_metadata(
        darwin if evidence == "darwin_pass" else linux,
        "aarch64-unknown-linux-gnu" if evidence == "cross_compiled" else None,
    )
    if evidence == "missing_platform":
        builds = {"target-directory": str(target_dir)}
    binaries_bytes = json.dumps({
        "rust-build-meta": builds,
        "rust-binaries": {binary_id: row},
    }).encode()
    cargo_bytes = json.dumps({
        "target_directory": str(target_dir), "workspace_root": str(checker.ROOT),
    }).encode()
    inventory_bytes = json.dumps({
        "rust-build-meta": platform_metadata(darwin) if evidence == "collection_platform_drift" else builds,
        "test-count": len(cases),
        "rust-suites": {binary_id: {
            **row, "package-name": package, "status": "listed",
            "testcases": {
                case: {"ignored": False, "filter-match": {"status": "matches"}}
                for case in cases
            },
        }},
    }).encode()
    meta = {"crate": package, "test_binary": target, "kind": "test"}
    events = [{"type": "suite", "event": "started", "test_count": len(cases), "nextest": meta}]
    for case in cases:
        event = f"{binary_id}${case}"
        events.extend((
            {"type": "test", "event": "started", "name": event},
            {"type": "test", "event": "ok", "name": event},
        ))
    events.append({
        "type": "suite", "event": "ok", "passed": len(cases),
        "failed": 0, "ignored": 0, "nextest": meta,
    })
    events_bytes = "".join(json.dumps(event) + "\n" for event in events).encode()
    for suffix, payload in (
        ("binaries.json", binaries_bytes), ("cargo.json", cargo_bytes),
        ("inventory.json", inventory_bytes), ("nextest.jsonl", events_bytes),
    ):
        (evidence_dir / f"{run_id}-{suffix}").write_bytes(payload)
    catalog = checker.ROOT / "tools/ci/test-authority.toml"
    catalog.write_text(
        f'[[integration_targets]]\npath = "{source}"\nowner = "{package}"\n',
        encoding="utf-8",
    )

    def git(*args):
        return subprocess.check_output(
            ["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", *args],
            cwd=checker.ROOT, text=True,
        ).strip()

    git("init", "-q")
    git("add", ".")
    git("commit", "-qm", "executed source")
    run_revision = git("rev-parse", "HEAD")
    selection = {
        "lane": "test-integration-lane", "package": package,
        "include_lib": False, "test_targets": [target],
        "filterset": "test(sealed_commitment_cost)", "features": [],
    }
    commands = retirement_nextest._execution_commands(
        selection,
        evidence_dir / f"{run_id}-binaries.json",
        evidence_dir / f"{run_id}-cargo.json",
    )
    manifest = {
        "head": run_revision,
        "source_tree_sha256": retirement_nextest.release_tree_sha256(checker.ROOT, run_revision),
        "inventory_sha256": hashlib.sha256(inventory_bytes).hexdigest(),
        "events_sha256": hashlib.sha256(events_bytes).hexdigest(),
        "execution": {
            "workspace_root": str(checker.ROOT),
            "binaries_sha256": "sha256:" + hashlib.sha256(binaries_bytes).hexdigest(),
            "cargo_sha256": "sha256:" + hashlib.sha256(cargo_bytes).hexdigest(),
            "executables": {binary_id: hashlib.sha256(binary.read_bytes()).hexdigest()},
            "selection": selection, "commands": commands,
            "legacy_searchd": None if evidence == "without_old_q" else {
                "path": str(old_q), "sha256": legacy_digest,
            },
        },
    }
    manifest_bytes = json.dumps(manifest).encode()
    (evidence_dir / f"{run_id}-source-manifest.json").write_bytes(manifest_bytes)
    test_path = checker.ROOT / source
    changed_body = body + b"// Uncommitted test change\n"
    if evidence == "dirty_source":
        test_path.write_bytes(changed_body)
    observed_body = changed_body if evidence in {"dirty_source", "source_swap"} else body
    observations = []
    for index, case in enumerate(cases):
        if evidence == "partial" and index == 1:
            continue
        observed_event = f"{binary_id}${case}"
        if evidence == "foreign" and index == 1:
            observed_event = f"{package}::other_target${case}"
        observations.append({
            "source": source, "case": case, "source_sha256": hashlib.sha256(observed_body).hexdigest(),
            "run_id": run_id, "event": observed_event,
            "events_sha256": manifest["events_sha256"],
            "inventory_sha256": manifest["inventory_sha256"],
            "source_manifest_sha256": hashlib.sha256(manifest_bytes).hexdigest(),
        })
    roster = checker.MANIFEST.read_text(encoding="utf-8")
    toml_observations = "[" + ", ".join(
        "{" + ", ".join(f"{key} = {json.dumps(value)}" for key, value in observation.items()) + "}"
        for observation in observations
    ) + "]"
    pending_retain = 'disposition = "retain"\nproof = "NOT_RUN"'
    if evidence == "relabeled_pending":
        roster = roster.replace(pending_retain, 'disposition = "migrate"\nproof = "NOT_RUN"', 1)
    else:
        roster = roster.replace(
            pending_retain,
            'disposition = "retain"\nproof = "VERIFIED"\n'
            f'candidates = [{{path = "{source}", tests = {json.dumps(cases)}}}]\n'
            f'observations = {toml_observations}',
            1,
        )
    checker.MANIFEST.write_text(roster, encoding="utf-8")
    git("add", str(checker.MANIFEST.relative_to(checker.ROOT)))
    git("commit", "-qm", "candidate roster")
    revision = git("rev-parse", "HEAD")
    if evidence == "dirty_approval":
        circleci.write_text(ci_config.replace(approved_digest, legacy_digest), encoding="utf-8")

    read_git = checker.git_text

    def swap_after_source_check(*args):
        result = read_git(*args)
        if args == ("show", f"{revision}:{source}"):
            test_path.write_bytes(changed_body)
        return result

    def advance_after_head_read(*args):
        result = read_git(*args)
        if args == ("rev-parse", "--verify", "HEAD") and result.strip() == revision:
            cargo = checker.LEXICAL_CARGO_MANIFEST
            cargo.write_bytes(cargo.read_bytes() + b"\n# Changed committed build input\n")
            git("add", str(cargo.relative_to(checker.ROOT)))
            git("commit", "-qm", "advance candidate during local check")
        return result

    swaps_source = evidence in {"source_swap", "source_swap_with_original_observation"}
    for candidate in (revision, None):
        if swaps_source and candidate is None:
            monkeypatch.setattr(checker, "git_text", swap_after_source_check)
        if evidence == "head_advance" and candidate is None:
            monkeypatch.setattr(checker, "git_text", advance_after_head_read)
        local_only = {"head_advance", "source_swap_with_original_observation"}
        check_error = None if evidence in local_only and candidate is not None else expected_error
        assert checker.main(revision=candidate, evidence_dir=evidence_dir) == (
            0 if check_error is None else 1
        ), candidate
        if check_error is not None:
            error = (
                "observation source changed"
                if evidence == "dirty_source" and candidate is not None
                else check_error
            )
            assert error in capsys.readouterr().err
    if swaps_source:
        assert test_path.read_bytes() == changed_body
    if evidence == "head_advance":
        assert git("rev-parse", "HEAD") != revision


@pytest.mark.parametrize(
    ("package", "target"),
    [
        ("quanta-index-lexical", "sealed_commitment_cost"),
        ("quanta-index-searchd-runtime", "process_readiness_owner_v1"),
        ("quanta-index-searchd-runtime", "runtime_fast_suite"),
    ],
)
@pytest.mark.parametrize(
    ("provide_binary", "error"),
    [(False, "retained old-Q executable is absent"),
     (True, "does not match the committed approved Linux old-Q SHA-256")],
)
def test_retained_old_q_capture_rejects_a_missing_or_self_pinned_binary_before_build(
    tmp_path, monkeypatch, package, target, provide_binary, error,
):
    checkout = tmp_path / "checkout"
    circleci = checkout / ".circleci/config.yml"
    circleci.parent.mkdir(parents=True)
    circleci.write_text(
        "jobs:\n  verify-rust-tests:\n    environment:\n"
        f'      QUANTA_INDEX_LEGACY_SEARCHD_SHA256: "{"a" * 64}"\n',
        encoding="utf-8",
    )
    subprocess.run(["git", "init", "-q", str(checkout)], check=True)
    subprocess.run(["git", "-C", str(checkout), "add", "."], check=True)
    subprocess.run([
        "git", "-C", str(checkout), "-c", "user.name=Fixture", "-c", "user.email=fixture@example.test",
        "commit", "-qm", "approved fixture",
    ], check=True)
    revision = subprocess.check_output(["git", "-C", str(checkout), "rev-parse", "HEAD"], text=True).strip()
    evidence = tmp_path / "evidence"
    evidence.mkdir()
    for key in (retirement_nextest.LEGACY_BINARY_ENV, retirement_nextest.LEGACY_SHA_ENV):
        monkeypatch.delenv(key, raising=False)
    if provide_binary:
        foreign = tmp_path / "foreign-searchd"
        foreign.write_bytes(b"unapproved executable with a valid self-reported digest")
        foreign.chmod(0o700)
        monkeypatch.setenv(retirement_nextest.LEGACY_BINARY_ENV, str(foreign))
        monkeypatch.setenv(retirement_nextest.LEGACY_SHA_ENV, hashlib.sha256(foreign.read_bytes()).hexdigest())
    monkeypatch.setattr(retirement_nextest, "_capture_frozen_checkout", lambda *_: pytest.fail("entered capture"))
    with pytest.raises(ValueError, match=error):
        retirement_nextest.capture_release_execution(
            checkout, revision, evidence, lane="test-integration-lane", package=package,
            include_lib=False, test_targets=[target], filterset="all()", features=[],
        )
    assert not list(evidence.iterdir())


@pytest.mark.parametrize(
    ("host", "build_target", "expected_error"),
    [
        ("aarch64-unknown-linux-gnu", None, None),
        ("aarch64-apple-darwin", None, "requires a Linux host and target"),
        ("aarch64-unknown-linux-gnu", "aarch64-apple-darwin", "requires a Linux host and target"),
    ],
)
def test_boot_inventory_platform_preserves_linux_inotify_coverage(tmp_path, host, build_target, expected_error):
    meta = {
        "platforms": {
            "host": {"platform": {"triple": host}},
            "targets": [] if build_target is None else [{"platform": {"triple": build_target}}],
        },
    }
    path = tmp_path / "nextest.json"
    path.write_text(json.dumps({"rust-build-meta": meta}), encoding="utf-8")
    artifact = retirement_nextest.RawFile.capture(path)
    selection = {"package": "quanta-index-lexical", "test_targets": ["boot_inventory_data"]}
    if expected_error is None:
        retirement_nextest._require_retirement_platforms(artifact, artifact, selection)
    else:
        with pytest.raises(ValueError, match=expected_error):
            retirement_nextest._require_retirement_platforms(artifact, artifact, selection)


@pytest.mark.parametrize("target", ["process_readiness_owner_v1", "runtime_fast_suite"])
@pytest.mark.parametrize(
    ("host", "build_target", "expected_error"),
    [
        ("x86_64-unknown-linux-gnu", None, None),
        ("aarch64-apple-darwin", None, "requires x86-64 Linux host and target"),
        ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "requires x86-64 Linux host and target"),
    ],
)
def test_runtime_old_q_platform_matches_the_approved_binary(tmp_path, target, host, build_target, expected_error):
    meta = {
        "platforms": {
            "host": {"platform": {"triple": host}},
            "targets": [] if build_target is None else [{"platform": {"triple": build_target}}],
        },
    }
    path = tmp_path / "nextest.json"
    path.write_text(json.dumps({"rust-build-meta": meta}), encoding="utf-8")
    artifact = retirement_nextest.RawFile.capture(path)
    selection = {"package": "quanta-index-searchd-runtime", "test_targets": [target]}
    if expected_error is None:
        retirement_nextest._require_retirement_platforms(artifact, artifact, selection)
    else:
        with pytest.raises(ValueError, match=expected_error):
            retirement_nextest._require_retirement_platforms(artifact, artifact, selection)


def test_boot_inventory_capture_refuses_darwin_before_build(tmp_path, monkeypatch):
    checkout = tmp_path / "checkout"
    checkout.mkdir()
    evidence = tmp_path / "evidence"
    evidence.mkdir()
    monkeypatch.setattr(retirement_nextest.platform, "system", lambda: "Darwin")
    monkeypatch.setattr(retirement_nextest, "_capture_frozen_checkout", lambda *_: pytest.fail("entered capture"))
    with pytest.raises(ValueError, match="retirement capture requires a Linux host"):
        retirement_nextest.capture_release_execution(
            checkout, "a" * 40, evidence, lane="test-integration-lane", package="quanta-index-lexical",
            include_lib=False, test_targets=["boot_inventory_data"], filterset="all()", features=[],
        )
    assert not list(evidence.iterdir())


def test_retirement_without_body_proof_requires_the_feature_to_be_gone(checker, capsys):
    checker.LEXICAL_CARGO_MANIFEST.write_text("[features]\nlegacy-tantivy = []\n", encoding="utf-8")
    assert checker.main() == 1
    assert (
        "retirement requires VERIFIED proof or a removed frozen engine target"
        in capsys.readouterr().err
    )


def test_retirement_without_body_proof_requires_the_source_to_be_gone(checker, capsys):
    target = checker.tomllib.loads(checker.MANIFEST.read_text(encoding="utf-8"))["targets"][1]
    source = checker.ROOT / target["path"]
    source.write_text(
        "".join(
            f"#[test]\nfn {name}() {{}}\n"
            for group in target["dispositions"]
            for name in group["tests"]
        ),
        encoding="utf-8",
    )
    assert checker.main() == 1
    assert (
        "retirement requires VERIFIED proof or a removed frozen engine target"
        in capsys.readouterr().err
    )
