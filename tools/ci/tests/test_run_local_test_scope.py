"""Tests for the single-process local nextest scope runner."""

from __future__ import annotations

import hashlib
import importlib.util
import io
import json
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from tools.ci.daemon_execution_evidence import (
    P07_EVENT,
    P07_PROVIDER_INPUT_ENV,
    P07_PROVIDER_PIN_ENV,
    P07_PROVIDER_SHA_ENV,
    P07_SCOPE,
    P07_WITNESS_FILE,
    P10_SCOPE,
)

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "ci" / "run-local-test-scope.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("run_local_test_scope", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["run_local_test_scope"] = module
    spec.loader.exec_module(module)
    return module


MODULE = _load_module()


def _catalog(tmp_path: Path) -> dict:
    first = tmp_path / "crates" / "first" / "tests" / "alpha.rs"
    second = tmp_path / "crates" / "second" / "tests" / "beta.rs"
    first.parent.mkdir(parents=True)
    second.parent.mkdir(parents=True)
    first.write_text("", encoding="utf-8")
    second.write_text("", encoding="utf-8")
    return {
        "local_scopes": {
            "one": {"lane": "one-lane", "test_threads": 2, "targets": ["first-alpha"]},
            "all-second": {"lane": "two-lane", "test_threads": 4, "owners": ["second"]},
        },
        "integration_targets": [
            {
                "id": "first-alpha",
                "owner": "first",
                "path": str(first.relative_to(tmp_path)),
            },
            {
                "id": "second-beta",
                "owner": "second",
                "path": str(second.relative_to(tmp_path)),
            },
        ],
    }


def test_combined_scopes_build_one_nextest_command(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)
    lane, test_threads, include_lib, extra_packages, targets = MODULE.resolve_targets(
        _catalog(tmp_path), ["one", "all-second"]
    )

    assert lane == "local-validation-lane"
    assert test_threads == 2
    assert include_lib is False
    assert extra_packages == []
    assert [target["id"] for target in targets] == ["first-alpha", "second-beta"]
    command = MODULE.build_command(lane, test_threads, include_lib, extra_packages, targets)
    assert command.count("nextest") == 1
    assert command.count("run") == 1
    assert command.count("-p") == 2
    assert command.count("--test") == 2
    assert command[command.index("--status-level") + 1] == "leak"
    assert command[command.index("--final-status-level") + 1] == "fail"
    assert command[command.index("--no-tests") + 1] == "fail"


def test_scope_refuses_cross_product_target_leak(tmp_path: Path, monkeypatch) -> None:
    data = _catalog(tmp_path)
    shadow = tmp_path / "crates" / "second" / "tests" / "alpha.rs"
    shadow.write_text("", encoding="utf-8")
    data["integration_targets"].append(
        {"id": "second-alpha", "owner": "second", "path": str(shadow.relative_to(tmp_path))}
    )
    data["local_scopes"]["mixed"] = {
        "lane": "mixed-lane",
        "test_threads": 2,
        "targets": ["first-alpha", "second-beta"],
    }
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)

    try:
        MODULE.resolve_targets(data, ["mixed"])
    except ValueError as error:
        assert "undeclared package/target pair" in str(error)
    else:  # pragma: no cover - regression assertion
        raise AssertionError("cross-product target leak was accepted")


def test_scope_refuses_cross_product_declared_target_alias(tmp_path: Path, monkeypatch) -> None:
    data = _catalog(tmp_path)
    shadow = tmp_path / "crates" / "second" / "tests" / "gamma.rs"
    shadow.write_text("", encoding="utf-8")
    data["integration_targets"].append(
        {
            "id": "second-gamma",
            "owner": "second",
            "path": str(shadow.relative_to(tmp_path)),
            "target": "alpha",
        }
    )
    data["local_scopes"]["mixed"] = {
        "lane": "mixed-lane",
        "test_threads": 2,
        "targets": ["first-alpha", "second-beta"],
    }
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)

    with pytest.raises(ValueError, match="undeclared package/target pair: second::alpha"):
        MODULE.resolve_targets(data, ["mixed"])


def test_library_and_integration_selectors_are_rejected(tmp_path: Path, monkeypatch) -> None:
    data = _catalog(tmp_path)
    library = tmp_path / "crates" / "library"
    library.mkdir(parents=True)
    (library / "Cargo.toml").write_text(
        '[package]\nname = "library"\nversion = "0.1.0"\n', encoding="utf-8"
    )
    data["local_scopes"]["composite"] = {
        "lane": "composite-lane",
        "test_threads": 3,
        "includes": ["one"],
        "packages": ["library"],
        "lib": True,
    }
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)

    try:
        MODULE.resolve_targets(data, ["composite"])
    except ValueError as error:
        assert "library and integration selectors" in str(error)
    else:  # pragma: no cover - regression assertion
        raise AssertionError("mixed library/integration selectors were accepted")


def test_library_only_scope_builds_one_nextest_command(tmp_path: Path, monkeypatch) -> None:
    data = _catalog(tmp_path)
    library = tmp_path / "crates" / "library"
    library.mkdir(parents=True)
    (library / "Cargo.toml").write_text(
        '[package]\nname = "library"\nversion = "0.1.0"\n', encoding="utf-8"
    )
    data["local_scopes"]["library"] = {
        "lane": "library-lane",
        "test_threads": 3,
        "packages": ["library"],
        "lib": True,
    }
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)

    lane, threads, include_lib, packages, targets = MODULE.resolve_targets(data, ["library"])
    command = MODULE.build_command(lane, threads, include_lib, packages, targets)

    assert targets == []
    assert include_lib is True
    assert packages == ["library"]
    assert "--lib" in command
    assert command.count("nextest") == 1
    assert command[command.index("--no-tests") + 1] == "fail"


def test_owner_library_scope_runs_ignored_tests_only_when_requested(
    tmp_path: Path, monkeypatch
) -> None:
    data = _catalog(tmp_path)
    library = tmp_path / "crates" / "library"
    library.mkdir(parents=True)
    (library / "Cargo.toml").write_text('[package]\nname = "library"\nversion = "0.1.0"\n')
    data["local_scopes"]["library"] = {
        "lane": "library-lane",
        "test_threads": 3,
        "packages": ["library"],
        "lib": True,
    }
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)
    lane, threads, include_lib, packages, targets = MODULE.resolve_targets(data, ["library"])

    default = MODULE.build_command(lane, threads, include_lib, packages, targets)
    all_tests = MODULE.build_command(lane, threads, include_lib, packages, targets, "all")
    assert "--run-ignored" not in default
    assert all_tests[all_tests.index("--run-ignored") + 1] == "all"
    with pytest.raises(ValueError, match="run_ignored"):
        MODULE.build_command(lane, threads, include_lib, packages, targets, "only")


def test_include_cycle_is_rejected(tmp_path: Path, monkeypatch) -> None:
    data = _catalog(tmp_path)
    data["local_scopes"]["one"]["includes"] = ["all-second"]
    data["local_scopes"]["all-second"]["includes"] = ["one"]
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)

    try:
        MODULE.resolve_targets(data, ["one"])
    except ValueError as error:
        assert "include cycle" in str(error)
    else:  # pragma: no cover - regression assertion
        raise AssertionError("scope include cycle was accepted")


def test_composed_scope_uses_smallest_included_thread_cap(tmp_path: Path, monkeypatch) -> None:
    data = _catalog(tmp_path)
    data["local_scopes"]["one"]["test_threads"] = 1
    data["local_scopes"]["composite"] = {
        "lane": "composite-lane",
        "test_threads": 4,
        "includes": ["one"],
    }
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)

    lane, threads, include_lib, packages, targets = MODULE.resolve_targets(data, ["composite"])

    assert lane == "composite-lane"
    assert threads == 1
    assert include_lib is False
    assert packages == []
    assert [target["id"] for target in targets] == ["first-alpha"]


def test_multiple_source_rows_can_share_one_cargo_test_target(tmp_path: Path, monkeypatch) -> None:
    data = _catalog(tmp_path)
    data["integration_targets"][0]["target"] = "grouped-suite"
    data["integration_targets"][1]["target"] = "grouped-suite"
    data["local_scopes"]["grouped"] = {
        "lane": "grouped-lane",
        "test_threads": 2,
        "targets": ["first-alpha", "second-beta"],
    }
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)

    lane, threads, include_lib, packages, targets = MODULE.resolve_targets(data, ["grouped"])
    command = MODULE.build_command(lane, threads, include_lib, packages, targets)

    assert command.count("--test") == 1
    assert command[command.index("--test") + 1] == "grouped-suite"


def test_fast_scope_keeps_migrated_modules_without_running_the_readiness_suite():
    catalog = MODULE.load_catalog()
    fast = MODULE.build_command(*MODULE.resolve_targets(catalog, ["daemon-fast"]))
    assert fast.count("nextest") == 1
    assert {fast[i + 1] for i, arg in enumerate(fast) if arg == "--test"} == {
        "runtime_fast_suite", "process_readiness_owner_v1",
    }
    expr = fast[fast.index("-E") + 1]
    assert "binary(=runtime_fast_suite)" in expr
    assert "binary(=process_readiness_owner_v1)" in expr
    for module in (
        "legacy_hybrid_filters", "legacy_explain_score_trace", "legacy_structural_hellgate",
        "legacy_sdk_code_search_frontdoor", "legacy_text_route_hellgate",
    ):
        assert f"test(/^{module}::/)" in expr
    assert "legacy_reference_process" not in expr
    inventory = MODULE.build_inventory_command(fast)
    assert inventory[inventory.index("-E") + 1] == expr

    for scope in ("daemon", "daemon-all"):
        broad = MODULE.build_command(*MODULE.resolve_targets(catalog, [scope]))
        assert "process_readiness_owner_v1" in broad
        assert "legacy_crash_compat" in broad
        assert "-E" not in broad


def test_module_selection_uses_the_actual_rust_mount_and_rejects_a_missing_mount(
    tmp_path, monkeypatch
):
    data = _catalog(tmp_path)
    suite = tmp_path / "crates" / "first" / "tests" / "suite.rs"
    suite.write_text('#[path = "./alpha.rs"]\nmod renamed_alpha;\n', encoding="utf-8")
    data["integration_targets"][0]["target"] = "suite"
    data["integration_targets"].append(
        {
            "id": "first-suite",
            "owner": "first",
            "path": str(suite.relative_to(tmp_path)),
        }
    )
    scope = data["local_scopes"]["one"]
    scope["module_targets"] = scope.pop("targets")
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)
    command = MODULE.build_command(*MODULE.resolve_targets(data, ["one"]))
    assert "test(/^renamed_alpha::/)" in command[command.index("-E") + 1]

    for invalid in (
        '/*\n#[path = "alpha.rs"]\nmod renamed_alpha;\n*/\n',
        '#[path = "alpha.rs"]\nmod renamed_alpha;\n#[path = "other.rs"]\nmod renamed_alpha;\n',
        '#[cfg(debug_assertions)]\n#[path = "alpha.rs"]\nmod renamed_alpha;\n',
    ):
        suite.write_text(invalid)
        with pytest.raises(ValueError):
            MODULE.resolve_targets(data, ["one"])

    suite.write_text('#[path = "other.rs"]\nmod renamed_alpha;\n', encoding="utf-8")
    with pytest.raises(ValueError, match="not mounted exactly once"):
        MODULE.resolve_targets(data, ["one"])


def test_mount_resolution_replays_committed_relative_aliases(tmp_path):
    from tools.ci.rust_test_mount import committed_path_resolver, mounted_test_module

    entries = _catalog(tmp_path)["integration_targets"]
    member = tmp_path / entries[0]["path"]
    members = member.parent
    (members / "nested").mkdir()
    (members / "nested" / "tracked").touch()
    alias = members / "alias.rs"
    alias.symlink_to("alpha.rs")
    launcher = members / "suite.rs"
    launcher.write_text('#[path = "nested/../alias.rs"]\nmod original;\n')
    entries[0]["target"] = "suite"
    entries.append({"owner": "first", "path": launcher.relative_to(tmp_path).as_posix()})

    def git(root, *args):
        return subprocess.check_output(["git", "-C", str(root), *args])

    git(tmp_path, "init", "-q")
    git(tmp_path, "add", ".")
    git(
        tmp_path,
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "-qm",
        "Test mounts",
    )
    head = git(tmp_path, "rev-parse", "HEAD").decode().strip()
    args = (tmp_path, member, "first", "suite", entries)
    assert mounted_test_module(*args) == "original"
    launcher.write_text("/* retired */\n")
    alias.unlink()
    resolve = committed_path_resolver(git, tmp_path, head)
    assert (
        mounted_test_module(
            *args,
            read_source=lambda path: git(tmp_path, "show", f"{head}:{path}"),
            resolve_path=resolve,
        )
        == "original"
    )
    assert resolve(Path("crates/first/tests/alpha.rs/../alpha.rs")) is None


def test_local_thread_override_only_reduces_catalog_cap() -> None:
    assert MODULE.effective_test_threads(4, None) == 4
    assert MODULE.effective_test_threads(4, "1") == 1
    assert MODULE.effective_test_threads(4, "4") == 4
    for invalid in ("", "0", "5", "-1", "1.0", " 1", "1 ", "１"):
        with pytest.raises(ValueError, match="QUANTA_INDEX_TEST_THREADS"):
            MODULE.effective_test_threads(4, invalid)


@pytest.mark.parametrize("missing_module", [False, True])
def test_proof_mode_collects_and_runs_the_same_target(
    tmp_path: Path, monkeypatch, missing_module: bool,
) -> None:
    data = _catalog(tmp_path)
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)
    lane, threads, include_lib, packages, targets = MODULE.resolve_targets(data, ["one"])
    if missing_module:
        targets[0]["module"] = "required_module"
    command = MODULE.build_command(lane, threads, include_lib, packages, targets)
    calls = []
    metadata = {"crate": "first", "test_binary": "alpha", "kind": "test"}
    name = "first::alpha$passes"

    def fake_run(args, *, cwd, stdout, stderr, env=None):
        calls.append(args)
        assert cwd == tmp_path
        if args[args.index("nextest") + 1] == "list":
            stdout.write(
                json.dumps(
                    {
                        "test-count": 1,
                        "rust-suites": {
                            "first::alpha": {
                                "package-name": "first",
                                "binary-name": "alpha",
                                "kind": "test",
                                "status": "listed",
                                "testcases": {
                                    "passes": {
                                        "ignored": False,
                                        "filter-match": {"status": "matches"},
                                    }
                                },
                            }
                        },
                    }
                ).encode()
            )
        else:
            assert env["NEXTEST_EXPERIMENTAL_LIBTEST_JSON"] == "1"
            rows = [
                {"type": "suite", "event": "started", "test_count": 1, "nextest": metadata},
                {"type": "test", "event": "started", "name": name},
                {"type": "test", "event": "ok", "name": name},
                {
                    "type": "suite",
                    "event": "ok",
                    "passed": 1,
                    "failed": 0,
                    "ignored": 0,
                    "nextest": metadata,
                },
            ]
            stdout.write("".join(json.dumps(row) + "\n" for row in rows).encode())
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(MODULE.subprocess, "run", fake_run)
    raw = tmp_path / "proof-raw"
    if missing_module:
        with pytest.raises(ValueError, match="selected no tests in first:alpha:required_module"):
            MODULE.run_with_proof_evidence(
                command, scopes=["one"], lane=lane, raw_dir=raw, targets=targets,
            )
        assert len(calls) == 1
        return

    assert MODULE.run_with_proof_evidence(
        command, scopes=["one"], lane=lane, raw_dir=raw, targets=targets,
    ) == 0
    assert calls[0][calls[0].index("nextest") + 1] == "list"
    assert calls[1][calls[1].index("nextest") + 1] == "run"
    assert "--message-format-version" in calls[1]
    assert len(list(raw.glob("*-inventory.json"))) == 1
    assert len(list(raw.glob("*-nextest.jsonl"))) == 1
    with pytest.raises(ValueError, match="already exists"):
        MODULE.run_with_proof_evidence(
            command, scopes=["one"], lane=lane, raw_dir=raw, targets=targets,
        )


@pytest.fixture
def native_release_checkout(tmp_path, monkeypatch):
    from tools.ci.tests.test_write_proof_manifest import _build_fixture_root, _run

    root, _ = _build_fixture_root(tmp_path.resolve())
    target = root / "crates/quanta-index-searchd-runtime/tests/native_product_process.rs"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.touch()
    cargo_lock = root / "Cargo.lock"
    cargo_lock.write_text("version = 4\n")
    _run(root, "git", "add", "--", "Cargo.lock", target.relative_to(root).as_posix())
    _run(root, "git", "commit", "-qm", "Track native test and lockfile")
    catalog = root / "tools/ci/test-authority.toml"
    load_catalog = MODULE.load_catalog
    monkeypatch.setattr(MODULE, "ROOT", root)
    monkeypatch.setattr(MODULE, "load_catalog", lambda path=catalog: load_catalog(path))
    return root


@pytest.mark.parametrize(("scope", "case"), [
    ("candidate-activation-release", "activation_readers_see_complete_views_and_reopen_the_published_head"),
    ("query-truth-release", "retained_text_keysets_preserve_order_and_refuse_a_different_view_cursor"),
    ("sdk-binding-release", "sdk_query_reply_stays_bound_to_the_configured_view_and_instance"),
    ("provider-boundary-release", "native_provider_enforces_approved_budget_and_cancellation"),
    ("runtime-supervisor-release", "native_signals_drain_peer_fds_and_required_maintenance_loss_stops_serving"),
    ("control-readiness-release", "candidate_readiness_refuses_lost_root_with_bounded_diagnostics"),
])
def test_native_release_scope_selects_the_exact_product_case(scope, case, monkeypatch, capsys):
    monkeypatch.delenv(MODULE.PROOF_RAW_DIR, raising=False)
    assert MODULE.main([scope, "--dry-run"]) == 0
    invocation = capsys.readouterr().out
    assert "--test native_product_process" in invocation
    assert "--no-default-features" in invocation
    assert f"test(=native_release::{case})" in invocation
    assert MODULE.main([scope]) == 2
    assert MODULE.PROOF_RAW_DIR in capsys.readouterr().err


@pytest.mark.parametrize("fault, message", [
    (None, None),
    ("missing-approval", "registered approved provider"),
    ("changed-config", "differ from their registered approval"),
    ("loopback", "remote HTTPS origin"),
    ("excess-cost", "exactly two paid attempts"),
    ("no-credential", "approved provider credential is unavailable"),
    ("wrong-witness", "daemon witness differs"),
])
def test_p07_pins_approval_before_execution_and_requires_its_witness(
    tmp_path, monkeypatch, capsys, fault, message, native_release_checkout,
):
    from tools.ci.tests.p11_test_issuer import nextest_bytes

    approval = tmp_path / "provider-config.json"
    provider = {
        "kind": "open_ai",
        "base_url": "https://localhost" if fault == "loopback" else "https://provider.example.invalid",
        "api_key_env": "P07_FIXTURE_CREDENTIAL",
        "max_retries": 0,
        "work_budget": {
            "openai_max_request_cost_micros": 11,
            "total_cost_micros_ceiling": 33 if fault == "excess-cost" else 22,
            "retry_attempts_cap": 0,
        },
    }
    approved = json.dumps(provider).encode()
    approved_sha = hashlib.sha256(approved).hexdigest()
    approval.write_bytes(approved + b" " if fault == "changed-config" else approved)
    if fault != "missing-approval":
        registry = native_release_checkout / "tools/ci/proof-authority.toml"
        contents = registry.read_text()
        label = f'id = "{MODULE.P07_PROOF}"\n'
        registry.write_text(contents.replace(label, label + f'provider_approval_sha256 = "{approved_sha}"\n'))
    daemon = tmp_path / "candidate-daemon"
    daemon.write_bytes(b"frozen candidate executable")
    daemon.chmod(0o500)
    daemon_sha = hashlib.sha256(daemon.read_bytes()).hexdigest()
    monkeypatch.setenv(MODULE.DAEMON_PATH_ENV, str(daemon))
    monkeypatch.setenv(MODULE.PROOF_RAW_DIR, str(tmp_path / "raw"))
    monkeypatch.setenv(P07_PROVIDER_INPUT_ENV, str(approval))
    monkeypatch.setenv(P07_PROVIDER_PIN_ENV, "/untrusted/inherited/pin")
    if fault == "no-credential":
        monkeypatch.delenv("P07_FIXTURE_CREDENTIAL", raising=False)
    else:
        monkeypatch.setenv("P07_FIXTURE_CREDENTIAL", "fixture-value")

    namespace, case = P07_EVENT.split("$")
    package, binary = namespace.split("::")
    inventory, events = nextest_bytes(package, binary, "test", case)
    real_run = MODULE.subprocess.run
    phases = []

    def nextest(command, **kwargs):
        if "nextest" not in command:
            return real_run(command, **kwargs)
        phase = command[command.index("nextest") + 1]
        phases.append(phase)
        env = kwargs["env"]
        if phase == "list":
            assert P07_PROVIDER_PIN_ENV not in env
            kwargs["stdout"].write(inventory)
            approval.write_bytes(b"replaced after approval was captured")
        else:
            witness_dir = Path(env[MODULE.WITNESS_DIR_ENV])
            pin = Path(env[P07_PROVIDER_PIN_ENV])
            assert pin.parent == witness_dir.resolve()
            assert pin.read_bytes() == approved
            assert env[P07_PROVIDER_SHA_ENV] == approved_sha
            kwargs["stdout"].write(events)
            (witness_dir / P07_WITNESS_FILE).write_text(json.dumps({
                "schema_version": 1,
                "test": P07_EVENT,
                "nonce": env[MODULE.WITNESS_NONCE_ENV],
                "executables": [daemon_sha, daemon_sha],
                "provider_approval_sha256": "b" * 64 if fault == "wrong-witness" else approved_sha,
            }))
        return subprocess.CompletedProcess(command, 0)

    monkeypatch.setattr(MODULE.subprocess, "run", nextest)
    assert MODULE.main([P07_SCOPE]) == (2 if fault else 0)
    raw = tmp_path / "raw"
    if message:
        assert message in capsys.readouterr().err
        assert list(raw.glob("*-run-binding.json")) == []
    else:
        assert len(list(raw.glob("*-run-binding.json"))) == 1
    assert phases == (["list", "run"] if fault in (None, "wrong-witness") else [])


@pytest.mark.parametrize("scenario", [
    "both-observed", "launch-alias", "missing-observation", "wrong-kind", "wrong-binary",
    "replaced-during-collection", "extra-selection", "source-changed-during-collection",
    "frozen-written-and-restored",
])
def test_p04_scope_captures_both_process_observations(
    tmp_path, monkeypatch, capsys, scenario, native_release_checkout,
):
    from tools.ci.tests.p11_test_issuer import nextest_bytes

    scope = "read-view-lifetime"
    specs = MODULE.daemon_witness_specs(MODULE.live_proof(MODULE.ROOT, MODULE.P04_PROOF))
    names = sorted(specs)
    package, binary = names[0].split("$")[0].split("::")
    kind = "lib" if scenario == "wrong-kind" else "test"
    (first, first_events), (second, second_events) = [
        nextest_bytes(package, binary, kind, name.split("$")[1]) for name in names
    ]
    suite_id = f"{package}::{binary}"
    inventory = json.loads(first)
    inventory["test-count"] = 2
    inventory["rust-suites"][suite_id]["testcases"].update(
        json.loads(second)["rust-suites"][suite_id]["testcases"]
    )
    first_rows = [json.loads(raw) for raw in first_events.splitlines()]
    second_rows = [json.loads(raw) for raw in second_events.splitlines()]
    other_rows = second_rows[1:-1]
    if scenario == "extra-selection":
        extra, extra_events = nextest_bytes(package, binary, kind, "unregistered_release_case")
        inventory["rust-suites"][suite_id]["testcases"].update(
            json.loads(extra)["rust-suites"][suite_id]["testcases"]
        )
        inventory["test-count"] += 1
        other_rows.extend(json.loads(raw) for raw in extra_events.splitlines()[1:-1])
    inventory_bytes = json.dumps(inventory).encode()
    first_rows[0]["test_count"] = inventory["test-count"]
    first_rows[-1]["passed"] = inventory["test-count"]
    event_bytes = b"".join(
        json.dumps(row).encode() + b"\n"
        for row in [*first_rows[:-1], *other_rows, first_rows[-1]]
    )
    binary_path = tmp_path / "candidate-searchd"
    binary_path.write_bytes(b"fixed Native release executable")
    binary_path.chmod(0o700)
    approved_sha = hashlib.sha256(binary_path.read_bytes()).hexdigest()
    supplied = binary_path
    if scenario == "launch-alias":
        supplied = tmp_path / "approved-alias"
        supplied.symlink_to(binary_path)

    original_run = subprocess.run

    def nextest(args, **kwargs):
        if "nextest" not in args:
            return original_run(args, **kwargs)
        stdout, env = kwargs["stdout"], kwargs["env"]
        checkout = kwargs["cwd"]
        assert checkout != native_release_checkout
        assert Path(args[0]) == checkout / "scripts/cargow"
        assert env["QUANTA_INDEX_CACHE_CHECKOUT_ROOT"] == str(native_release_checkout)
        assert env["PYTHONPATH"] == str(checkout)
        assert (checkout / "Cargo.lock").read_text() == "version = 4\n"
        assert "--no-default-features" in args
        assert "--all-features" not in args
        assert all(f"test(={name.split('$')[1]})" in args[args.index("-E") + 1] for name in names)
        pinned = Path(env[MODULE.DAEMON_PATH_ENV])
        assert pinned not in (supplied, binary_path)
        assert hashlib.sha256(pinned.read_bytes()).hexdigest() == approved_sha
        if args[args.index("nextest") + 1] == "list":
            assert MODULE.WITNESS_DIR_ENV not in env
            stdout.write(inventory_bytes)
            if scenario == "replaced-during-collection":
                binary_path.write_bytes(b"different executable")
            if scenario == "source-changed-during-collection":
                (native_release_checkout / "Cargo.lock").write_text("version = 3\n")
            if scenario == "frozen-written-and-restored":
                lock = checkout / "Cargo.lock"
                mode = lock.stat().st_mode
                assert mode & 0o222 == 0
                lock.chmod(mode | 0o200)
                lock.write_text("version = 3\n")
                lock.write_text("version = 4\n")
                lock.chmod(mode)
        else:
            assert scenario not in ("source-changed-during-collection", "frozen-written-and-restored")
            stdout.write(event_bytes)
            directory = Path(env[MODULE.WITNESS_DIR_ENV])
            assert pinned.parent == directory.resolve()
            nonce = env[MODULE.WITNESS_NONCE_ENV]
            for name in names[:1] if scenario == "missing-observation" else names:
                filename, launches = specs[name]
                observed_sha = "1" * 64 if scenario == "wrong-binary" else approved_sha
                (directory / filename).write_text(json.dumps({
                    "schema_version": 1, "test": name, "nonce": nonce, "executables": [observed_sha] * launches,
                }))
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(MODULE.subprocess, "run", nextest)
    monkeypatch.setenv(MODULE.PROOF_RAW_DIR, str(tmp_path))
    monkeypatch.setenv(MODULE.DAEMON_PATH_ENV, str(supplied))
    if scenario not in ("both-observed", "launch-alias"):
        assert MODULE.main([scope]) == 2
        assert list(tmp_path.glob("*-daemon-observations.json")) == []
        assert list(tmp_path.glob("*-run-binding.json")) == []
        if scenario == "source-changed-during-collection":
            assert "proof source changed during nextest collection" in capsys.readouterr().err
        if scenario == "frozen-written-and-restored":
            assert "frozen proof checkout changed during nextest collection" in capsys.readouterr().err
        return
    assert MODULE.main([scope]) == 0
    (capture,) = tmp_path.glob("*-daemon-observations.json")
    payload = json.loads(capture.read_bytes())
    assert payload["events_sha256"] == hashlib.sha256(event_bytes).hexdigest()
    assert payload["inventory_sha256"] == hashlib.sha256(inventory_bytes).hexdigest()
    assert set(payload["observations"]) == set(names)
    (binding,) = tmp_path.glob("*-run-binding.json")
    record = json.loads(binding.read_bytes())
    assert record["daemon_sha256"] == approved_sha
    assert record["cargo_lock_sha256"] == hashlib.sha256(b"version = 4\n").hexdigest()
    assert record["artifacts"] == {
        "events": payload["events_sha256"],
        "inventory": payload["inventory_sha256"],
        "daemon_observations": hashlib.sha256(capture.read_bytes()).hexdigest(),
    }


@pytest.mark.parametrize("daemon", [None, "relative-searchd", "missing-absolute", "non-executable"])
def test_p04_refuses_missing_daemon_before_collecting_or_building(
    tmp_path, monkeypatch, capsys, daemon
):
    raw = tmp_path / "unstarted"
    monkeypatch.setenv(MODULE.PROOF_RAW_DIR, str(raw))
    if daemon is None:
        monkeypatch.delenv("QUANTA_INDEX_SEARCHD_BIN", raising=False)
    else:
        if daemon == "non-executable":
            (tmp_path / daemon).touch(mode=0o600)
        path = daemon if daemon == "relative-searchd" else str(tmp_path / daemon)
        monkeypatch.setenv("QUANTA_INDEX_SEARCHD_BIN", path)

    def nextest(*_args, **_kwargs):
        pytest.fail("P04 entered nextest with no usable release daemon")

    monkeypatch.setattr(MODULE.subprocess, "run", nextest)
    assert MODULE.main(["read-view-lifetime"]) == 2
    assert "QUANTA_INDEX_SEARCHD_BIN" in capsys.readouterr().err
    assert not raw.exists()


@pytest.mark.parametrize(("fault", "expected"), [
    ("missing_launch", 2),
    ("replaced_daemon", 2),
    ("different_observation", 2),
    ("rewritten_inventory", 2),
    ("rewritten_events", 2),
    ("source_changed_during_execution", 2),
    (None, 0),
])
def test_p10_release_scope_captures_the_exact_native_rebuild(
    tmp_path, monkeypatch, capsys, fault, expected, native_release_checkout,
):
    from tools.ci.tests.p11_test_issuer import nextest_bytes

    scope = P10_SCOPE
    specs = MODULE.daemon_witness_specs(MODULE.live_proof(MODULE.ROOT, MODULE.P10_PROOF))
    ((name, (filename, required_launches)),) = specs.items()
    assert required_launches == 4
    identity, case = name.split("$")
    package, binary = identity.split("::")
    inventory, events = nextest_bytes(package, binary, "test", case)
    daemon = tmp_path / "approved-daemon"
    daemon.write_bytes(b"fixed release executable")
    daemon.chmod(0o500)
    approved_sha = hashlib.sha256(daemon.read_bytes()).hexdigest()
    raw = tmp_path / "raw"

    original_run = subprocess.run

    def nextest(args, **kwargs):
        if "nextest" not in args:
            return original_run(args, **kwargs)
        stdout, env = kwargs["stdout"], kwargs["env"]
        assert "--no-default-features" in args
        assert "--all-features" not in args
        assert args[args.index("-E") + 1] == (
            f"(package(={package}) & binary(={binary}) & (test(={case})))"
        )
        pin = Path(env[MODULE.DAEMON_PATH_ENV])
        assert pin != daemon
        assert hashlib.sha256(pin.read_bytes()).hexdigest() == approved_sha
        assert pin.stat().st_mode & 0o777 == 0o500
        if args[args.index("nextest") + 1] == "list":
            assert MODULE.WITNESS_DIR_ENV not in env
            stdout.write(inventory)
        else:
            stdout.write(events)
            directory = Path(env[MODULE.WITNESS_DIR_ENV])
            assert pin == directory.resolve() / "quanta-index-searchd"
            if fault == "replaced_daemon":
                daemon.unlink()
                daemon.write_bytes(b"replacement")
                daemon.chmod(0o500)
            if fault == "rewritten_inventory":
                (published_inventory,) = raw.glob("*-inventory.json")
                published_inventory.write_bytes(inventory + b"\n")
            if fault == "source_changed_during_execution":
                (native_release_checkout / "Cargo.lock").write_text("version = 3\n")
            launches = 3 if fault == "missing_launch" else 4
            observed = "1" * 64 if fault == "different_observation" else approved_sha
            (directory / filename).write_text(json.dumps({
                "schema_version": 1,
                "test": name,
                "nonce": env[MODULE.WITNESS_NONCE_ENV],
                "executables": [observed] * launches,
            }))
        return SimpleNamespace(returncode=0)

    if fault == "rewritten_events":
        read_witness = MODULE.read_witness

        def rewrite_after_event_validation(directory, filename):
            (published_events,) = raw.glob("*-nextest.jsonl")
            published_events.write_bytes(events.replace(b"\n", b" \n", 1))
            return read_witness(directory, filename)

        monkeypatch.setattr(MODULE, "read_witness", rewrite_after_event_validation)
    monkeypatch.setattr(MODULE.subprocess, "run", nextest)
    monkeypatch.setenv(MODULE.DAEMON_PATH_ENV, str(daemon))
    monkeypatch.setenv(MODULE.PROOF_RAW_DIR, str(raw))
    assert MODULE.main([scope]) == expected
    captures = list(raw.glob("*-daemon-observations.json"))
    bindings = list(raw.glob("*-run-binding.json"))
    if expected == 0:
        (capture,) = captures
        result = json.loads(capture.read_bytes())
        assert set(result["observations"]) == {name}
        assert result["inventory_sha256"] == hashlib.sha256(inventory).hexdigest()
        assert result["events_sha256"] == hashlib.sha256(events).hexdigest()
        (binding,) = bindings
        record = json.loads(binding.read_bytes())
        assert record["daemon_sha256"] == approved_sha
        assert record["cargo_lock_sha256"] == hashlib.sha256(b"version = 4\n").hexdigest()
    else:
        assert captures == []
        assert bindings == []
    if fault in {"rewritten_inventory", "rewritten_events"}:
        assert "Nextest evidence changed after validation" in capsys.readouterr().err
    if fault == "source_changed_during_execution":
        assert "proof source changed during nextest execution" in capsys.readouterr().err


def test_local_thread_override_reaches_dry_run(tmp_path: Path, monkeypatch, capsys) -> None:
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)
    monkeypatch.setattr(MODULE, "load_catalog", lambda: _catalog(tmp_path))
    monkeypatch.setenv("QUANTA_INDEX_TEST_THREADS", "1")

    assert MODULE.main(["one", "--dry-run"]) == 0
    output = capsys.readouterr().out
    assert "test_threads=1" in output
    assert "--test-threads 1" in output


def test_local_thread_override_rejects_raise_before_execution(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)
    monkeypatch.setattr(MODULE, "load_catalog", lambda: _catalog(tmp_path))
    monkeypatch.setenv("QUANTA_INDEX_TEST_THREADS", "3")

    assert MODULE.main(["one", "--dry-run"]) == 2
    assert "declared cap 2" in capsys.readouterr().err


def test_local_scope_flushes_selection_before_exec(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)
    monkeypatch.setattr(MODULE, "load_catalog", lambda: _catalog(tmp_path))
    monkeypatch.setenv("QUANTA_INDEX_TEST_THREADS", "1")
    monkeypatch.delenv(MODULE.PROOF_RAW_DIR, raising=False)
    monkeypatch.setattr(MODULE.os, "chdir", lambda _path: None)

    class ObservedStdout(io.StringIO):
        flushed = False

        def flush(self) -> None:
            self.flushed = True
            super().flush()

    output = ObservedStdout()
    monkeypatch.setattr(sys, "stdout", output)

    class ObservedExec(Exception):
        pass

    def observe_exec(_path: str, _command: list[str]) -> None:
        assert output.flushed
        assert "test_threads=1" in output.getvalue()
        raise ObservedExec

    monkeypatch.setattr(MODULE.os, "execv", observe_exec)
    with pytest.raises(ObservedExec):
        MODULE.main(["one"])
