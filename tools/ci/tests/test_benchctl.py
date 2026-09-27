"""Registry tests for the benchmark CLI authority mapping."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "benchmark" / "benchctl.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("benchctl", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["benchctl"] = module
    spec.loader.exec_module(module)
    return module


MODULE = _load_module()


def install_control_plane(repo_root: Path) -> None:
    """A fixture checkout owns the registry and the Justfile its producers name."""
    target = repo_root / "tools" / "benchmark"
    target.mkdir(parents=True, exist_ok=True)
    (target / "registry.toml").write_text(
        (REPO_ROOT / "tools" / "benchmark" / "registry.toml").read_text(encoding="utf-8"),
        encoding="utf-8",
    )
    (repo_root / "Justfile").write_text(
        (REPO_ROOT / "Justfile").read_text(encoding="utf-8"), encoding="utf-8"
    )


@pytest.mark.parametrize(
    "flag,value",
    [
        ("--criterion-samples", "10"),
        ("--producer-timeout", "1"),
        ("--criterion-measurement", "0.01"),
    ],
)
def test_criterion_controls_are_not_silently_ignored_on_native_profile(
    monkeypatch, capsys, flag, value
):
    def forbidden(*_args, **_kwargs):
        raise AssertionError("native producer ran with irrelevant Criterion controls")

    monkeypatch.setattr(MODULE, "require_clean_worktree", forbidden)
    assert MODULE.main(["run", "systems", flag, value]) == 2
    assert "apply only to micro/dsl-diagnostic" in capsys.readouterr().err


def test_criterion_compare_does_not_claim_a_missing_capture_adapter(capsys):
    assert MODULE.main(["compare", "micro"]) == 2
    error = capsys.readouterr().err
    assert "no registered baseline/comparator" in error
    assert "capture adapter" not in error


def test_manifest_maps_profiles_to_explicit_recipes_and_validator_profiles() -> None:
    profiles = MODULE.load_profiles()
    assert profiles["dsl-authority"]["recipes"] == ["rust-bench-dsl-warm", "rust-bench-dsl-cold"]
    assert profiles["dsl-authority"]["families"] == ["dsl-warm", "dsl-cold"]
    assert "rust-verify-quality-ann" in profiles["quality-full"]["recipes"]
    assert "rust-verify-quality-concurrency" in profiles["quality-full"]["recipes"]
    assert "rust-verify-quality-ambiguity" in profiles["quality-full"]["recipes"]
    assert "rust-verify-quality-snippet" in profiles["quality-full"]["recipes"]
    assert "rust-verify-quality-ops" in profiles["quality-full"]["recipes"]
    assert "rust-verify-quality-ui" in profiles["quality-full"]["recipes"]
    assert profiles["systems"]["families"] == ["freshness", "open-loop"]


def test_registry_refuses_a_profile_that_names_an_unknown_family(tmp_path: Path) -> None:
    text = (REPO_ROOT / "tools/benchmark/registry.toml").read_text(encoding="utf-8")
    mutated = text.replace(
        'families = ["relevance", "ambiguity", "snippet", "scale", "tail"]',
        'families = ["relevance", "ambiguity", "snippet", "scale", "tail", "not-a-family"]',
    )
    assert mutated != text
    path = tmp_path / "registry.toml"
    path.write_text(mutated, encoding="utf-8")

    try:
        MODULE.load_manifest(path)
    except MODULE.ManifestError as error:
        assert "unknown families" in str(error)
    else:
        raise AssertionError("registry with an unknown profile family was accepted")


def test_list_exposes_only_registered_profiles() -> None:
    result = subprocess.run(
        [sys.executable, str(SCRIPT_PATH), "list"],
        cwd=REPO_ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr
    assert "dsl-authority\tdsl-authority" in result.stdout
    assert "quality-full\tquality-full" in result.stdout


def test_summarize_reports_absent_evidence_as_unvalidated(tmp_path: Path, capsys) -> None:
    manifest = MODULE.load_manifest()
    profile = MODULE.load_profiles()["systems"]

    assert MODULE.summarize(tmp_path, profile, manifest) == 0
    payload = json.loads(capsys.readouterr().out)
    assert [family["status"] for family in payload["families"]] == ["absent", "absent"]


def test_compare_runs_only_declared_dsl_baseline_pairs(monkeypatch) -> None:
    calls: list[list[str]] = []

    def fake_run(command, **_kwargs):
        calls.append(command)
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(MODULE.subprocess, "run", fake_run)
    manifest = MODULE.load_manifest()
    profiles = MODULE.load_profiles()

    assert MODULE.compare(REPO_ROOT, profiles["dsl-authority"], manifest) == 0
    assert len(calls) == 2
    assert calls[0][-2:] == [
        str(REPO_ROOT / "tools/benchmark/baselines/warm-matrix.json"),
        str(REPO_ROOT / "artifacts/dsl-bench/warm-matrix.json"),
    ]
    calls.clear()
    assert MODULE.compare(REPO_ROOT, profiles["systems"], manifest) == 0
    assert calls == []


def test_preflight_requires_linux_before_a_canonical_profile_runs(
    monkeypatch, tmp_path: Path
) -> None:
    calls: list[list[str]] = []

    def fake_run(command, **_kwargs):
        calls.append(command)
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(MODULE.subprocess, "run", fake_run)
    manifest = MODULE.load_manifest()

    assert MODULE.preflight(REPO_ROOT, "dsl-authority", tmp_path / "receipt.json", manifest) == 0
    assert calls[0][-2:] == ["--expected-os", "linux"]
    calls.clear()
    assert MODULE.preflight(REPO_ROOT, "systems", tmp_path / "receipt.json", manifest) == 0
    assert "--expected-os" not in calls[0]


def test_run_refuses_a_dirty_worktree_before_any_producer(monkeypatch, capsys) -> None:
    def dirty(_repo_root: Path) -> None:
        raise RuntimeError(
            "worktree is dirty: benchmark producers require a clean checkout before capture"
        )

    monkeypatch.setattr(MODULE, "require_clean_worktree", dirty)
    monkeypatch.setattr(
        MODULE,
        "preflight",
        lambda *_args: (_ for _ in ()).throw(AssertionError("preflight must not run")),
    )

    assert MODULE.main(["run", "quality-full"]) == 2
    assert "worktree is dirty" in capsys.readouterr().err


def test_run_rejects_invalid_dsl_cold_sample_override_before_producers(monkeypatch, capsys) -> None:
    monkeypatch.setattr(
        MODULE,
        "require_clean_worktree",
        lambda *_args: (_ for _ in ()).throw(AssertionError("clean-worktree check must not run")),
    )
    monkeypatch.setattr(
        MODULE,
        "preflight",
        lambda *_args: (_ for _ in ()).throw(AssertionError("preflight must not run")),
    )
    monkeypatch.setattr(
        MODULE.subprocess,
        "run",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(AssertionError("producer must not run")),
    )

    assert MODULE.main(["run", "dsl-authority", "--cold-samples", "19"]) == 2
    assert "at least 20" in capsys.readouterr().err


def test_run_refuses_contended_override_before_producers(
    monkeypatch, tmp_path: Path, capsys
) -> None:
    install_control_plane(tmp_path)
    monkeypatch.setattr(MODULE, "require_clean_worktree", lambda _repo_root: None)
    monkeypatch.setattr(MODULE, "resolve_checkout_head", lambda _repo_root: "a" * 40)

    def contended(_repo_root, _profile, receipt, _manifest):
        receipt.parent.mkdir(parents=True, exist_ok=True)
        receipt.write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "kind": "quanta-index-timing-preflight",
                    "run_id": "benchctl:quality-full",
                    "status": "contended_override",
                    "foreign_rust_processes": [{"pid": 42}],
                }
            ),
            encoding="utf-8",
        )
        return 0

    monkeypatch.setattr(MODULE, "preflight", contended)
    monkeypatch.setattr(
        MODULE.subprocess,
        "run",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(AssertionError("producer must not run")),
    )

    assert MODULE.main(["--repo-root", str(tmp_path), "run", "quality-full"]) == 2
    assert "contended_override" in capsys.readouterr().err


def test_run_refuses_missing_declared_baseline_before_preflight(
    monkeypatch, tmp_path: Path, capsys
) -> None:
    install_control_plane(tmp_path)
    monkeypatch.setattr(MODULE, "require_clean_worktree", lambda _repo_root: None)
    monkeypatch.setattr(
        MODULE,
        "preflight",
        lambda *_args: (_ for _ in ()).throw(AssertionError("preflight must not run")),
    )

    assert MODULE.main(["--repo-root", str(tmp_path), "run", "dsl-authority"]) == 2
    assert "baseline" in capsys.readouterr().err


def test_run_refuses_invalid_declared_baseline_before_preflight(
    monkeypatch, tmp_path: Path, capsys
) -> None:
    install_control_plane(tmp_path)
    baseline = tmp_path / "tools" / "benchmark" / "baselines" / "warm-matrix.json"
    baseline.parent.mkdir(parents=True)
    baseline.write_text(json.dumps({"schema_version": 1}), encoding="utf-8")
    monkeypatch.setattr(MODULE, "require_clean_worktree", lambda _repo_root: None)
    monkeypatch.setattr(
        MODULE,
        "preflight",
        lambda *_args: (_ for _ in ()).throw(AssertionError("preflight must not run")),
    )

    assert MODULE.main(["--repo-root", str(tmp_path), "run", "dsl-authority"]) == 2
    assert "schema_version 1" in capsys.readouterr().err


def test_clean_preflight_runs_exact_profile_recipes(monkeypatch, tmp_path: Path) -> None:
    install_control_plane(tmp_path)
    monkeypatch.setattr(MODULE, "require_clean_worktree", lambda _repo_root: None)
    monkeypatch.setattr(MODULE, "resolve_checkout_head", lambda _repo_root: "a" * 40)

    def clean(_repo_root, _profile, receipt, _manifest):
        receipt.parent.mkdir(parents=True, exist_ok=True)
        receipt.write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "kind": "quanta-index-timing-preflight",
                    "run_id": "benchctl:systems",
                    "status": "clean",
                    "foreign_rust_processes": [],
                    "host": {"os": "darwin", "cpu_count": 8, "load_average": [1.0, 1.0, 1.0]},
                    "host_contention": {
                        "one_minute_load": 1.0,
                        "one_minute_load_limit": 4.0,
                        "over_limit": False,
                    },
                }
            ),
            encoding="utf-8",
        )
        return 0

    calls: list[list[str]] = []

    def fake_execute(command, **_kwargs):
        calls.append(command)
        return SimpleNamespace(command={"exit_code": 0})

    monkeypatch.setattr(MODULE, "preflight", clean)
    monkeypatch.setattr(MODULE, "validate", lambda *_args: 0)
    monkeypatch.setattr(MODULE, "execute", fake_execute)

    assert MODULE.main(["--repo-root", str(tmp_path), "run", "systems"]) == 0
    assert calls == [
        ["just", "rust-verify-quality-freshness"],
        ["just", "rust-verify-quality-open-loop"],
    ]


def test_command_only_native_run_uses_owned_execution_and_retains_nonzero_logs(
    monkeypatch, tmp_path: Path, capsys,
) -> None:
    args = _native_profile_fixture(monkeypatch, tmp_path, "systems")
    repo = args["repo_root"]
    raw_receipt = args["receipt"].read_bytes()

    def preflight(_repo, _profile, receipt, _manifest):
        receipt.parent.mkdir(parents=True, exist_ok=True)
        receipt.write_bytes(raw_receipt)
        return 0

    monkeypatch.setattr(MODULE, "require_declared_baselines", lambda *_: None)
    monkeypatch.setattr(MODULE, "require_clean_worktree", lambda *_: None)
    monkeypatch.setattr(MODULE, "resolve_checkout_head", lambda *_: args["initial_head"])
    monkeypatch.setattr(MODULE, "require_frozen_source", lambda *_: None)
    monkeypatch.setattr(MODULE, "preflight", preflight)
    monkeypatch.setattr(MODULE.tempfile, "gettempdir", lambda: str(tmp_path))
    tools = tmp_path / "fixture-tools"
    tools.mkdir()
    just = tools / "just"
    just.write_text("#!/bin/sh\nprintf legacy-stdout\nprintf legacy-stderr >&2\nexit 7\n")
    just.chmod(0o755)
    monkeypatch.setenv("PATH", str(tools) + os.pathsep + os.environ["PATH"])

    assert MODULE.main(["--repo-root", str(repo), "run", "systems"]) == 7
    logs = list(tmp_path.glob("quanta-native-command-*/recipe-00"))
    assert len(logs) == 1
    assert (logs[0] / "stdout").read_bytes() == b"legacy-stdout"
    assert (logs[0] / "stderr").read_bytes() == b"legacy-stderr"
    terminal = json.loads((logs[0] / "execution.json").read_text())
    assert terminal["status"] == "failed"
    assert terminal["command"]["exit_code"] == 7
    assert str(logs[0]) in capsys.readouterr().err
    assert not (args["evidence_root"] / "profiles").exists()


def test_clean_label_with_overloaded_host_is_refused(tmp_path: Path) -> None:
    receipt = tmp_path / "preflight.json"
    receipt.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "kind": "quanta-index-timing-preflight",
                "run_id": "benchctl:systems",
                "status": "clean",
                "foreign_rust_processes": [],
                "host": {"os": "darwin", "cpu_count": 8, "load_average": [20.0, 1.0, 1.0]},
                "host_contention": {
                    "one_minute_load": 20.0,
                    "one_minute_load_limit": 4.0,
                    "over_limit": False,
                },
            }
        ),
        encoding="utf-8",
    )

    with pytest.raises(RuntimeError, match="host load"):
        MODULE.require_clean_preflight_receipt(receipt, "systems")


def test_duplicate_preflight_status_is_refused(tmp_path: Path) -> None:
    receipt = tmp_path / "preflight.json"
    receipt.write_text(
        """{"schema_version":1,"kind":"quanta-index-timing-preflight",
        "run_id":"benchctl:systems","status":"contended","status":"clean",
        "foreign_rust_processes":[],
        "host":{"os":"darwin","cpu_count":8,"load_average":[1,1,1]},
        "host_contention":{"one_minute_load":1,"one_minute_load_limit":4,"over_limit":false}}
    """,
        encoding="utf-8",
    )
    with pytest.raises(RuntimeError, match="duplicate"):
        MODULE.require_clean_preflight_receipt(receipt, "systems")


def _prepare_source_drift_run(monkeypatch, tmp_path: Path) -> list[list[str]]:
    install_control_plane(tmp_path)
    monkeypatch.setattr(MODULE, "require_declared_baselines", lambda *_args: None)

    def preflight(_repo_root, _profile, receipt, _manifest):
        receipt.parent.mkdir(parents=True, exist_ok=True)
        receipt.write_text("{}", encoding="utf-8")
        return 0

    monkeypatch.setattr(MODULE, "preflight", preflight)
    monkeypatch.setattr(MODULE, "require_clean_preflight_receipt", lambda *_args: None)
    monkeypatch.setattr(MODULE, "validate", lambda *_args: 0)
    monkeypatch.setattr(MODULE, "compare", lambda *_args: 0)
    calls: list[list[str]] = []

    def fake_execute(command, **_kwargs):
        calls.append(command)
        return SimpleNamespace(command={"exit_code": 0})

    monkeypatch.setattr(MODULE, "execute", fake_execute)
    return calls


def test_run_refuses_head_drift_before_admitting_artifacts(monkeypatch, tmp_path: Path) -> None:
    calls = _prepare_source_drift_run(monkeypatch, tmp_path)
    monkeypatch.setattr(MODULE, "require_clean_worktree", lambda _repo_root: None)
    heads = iter(["a" * 40, "b" * 40])
    monkeypatch.setattr(
        MODULE, "resolve_checkout_head", lambda _repo_root: next(heads), raising=False
    )

    assert MODULE.main(["--repo-root", str(tmp_path), "run", "systems"]) == 2
    assert calls == []


def test_run_refuses_dirty_tree_after_first_producer(monkeypatch, tmp_path: Path) -> None:
    calls = _prepare_source_drift_run(monkeypatch, tmp_path)
    checks = 0

    def check_clean(_repo_root):
        nonlocal checks
        checks += 1
        if checks >= 3:
            raise RuntimeError("worktree changed during benchmark run")

    monkeypatch.setattr(MODULE, "require_clean_worktree", check_clean)
    monkeypatch.setattr(MODULE, "resolve_checkout_head", lambda _repo_root: "a" * 40, raising=False)

    assert MODULE.main(["--repo-root", str(tmp_path), "run", "systems"]) == 2
    assert calls == [["just", "rust-verify-quality-freshness"]]


def test_admit_baseline_is_dsl_only(monkeypatch, capsys) -> None:
    monkeypatch.setattr(
        MODULE, "preflight", lambda *_args: (_ for _ in ()).throw(AssertionError("no preflight"))
    )
    assert MODULE.main(["run", "systems", "--admit-baseline"]) == 2
    assert "only valid for dsl-authority" in capsys.readouterr().err


def test_dsl_admission_dispatches_both_producers_and_skips_old_baselines(
    monkeypatch, tmp_path: Path
) -> None:
    install_control_plane(tmp_path)
    monkeypatch.setattr(MODULE, "require_clean_worktree", lambda _repo_root: None)
    monkeypatch.setattr(MODULE, "resolve_checkout_head", lambda _repo_root: "a" * 40)
    monkeypatch.setattr(
        MODULE,
        "require_declared_baselines",
        lambda *_args: (_ for _ in ()).throw(AssertionError("old baseline is not required")),
    )

    def preflight(_root, _profile, receipt, _manifest):
        receipt.parent.mkdir(parents=True, exist_ok=True)
        receipt.write_text("fresh receipt", encoding="utf-8")
        return 0

    monkeypatch.setattr(MODULE, "preflight", preflight)
    monkeypatch.setattr(MODULE, "require_clean_preflight_receipt", lambda *_args: None)
    monkeypatch.setattr(MODULE, "validate", lambda *_args: 0)
    monkeypatch.setattr(
        MODULE, "compare", lambda *_args: (_ for _ in ()).throw(AssertionError("no compare"))
    )
    commands = []
    monkeypatch.setattr(
        MODULE, "execute",
        lambda command, **_kwargs: (commands.append(command), SimpleNamespace(command={"exit_code": 0}))[1],
    )
    admitted = []
    monkeypatch.setattr(MODULE, "admit_dsl_baselines", lambda *args: admitted.append(args))

    assert (
        MODULE.main(["--repo-root", str(tmp_path), "run", "dsl-authority", "--admit-baseline"]) == 0
    )
    assert commands == [["just", "rust-bench-dsl-warm"], ["just", "rust-bench-dsl-cold"]]
    assert len(admitted) == 1
    assert admitted[0][0] == tmp_path
    assert admitted[0][5] <= MODULE.time.time_ns()
    assert admitted[0][6] == hashlib.sha256(b"fresh receipt").hexdigest()


def test_dsl_admission_rejects_old_artifact_before_reading_it(monkeypatch, tmp_path: Path) -> None:
    manifest = MODULE.load_manifest()
    profile = MODULE.load_profiles()["dsl-authority"]
    receipt = tmp_path / "preflight.json"
    receipt.write_text("fresh receipt", encoding="utf-8")
    artifact = tmp_path / "artifacts/dsl-bench/warm-matrix.json"
    artifact.parent.mkdir(parents=True)
    artifact.write_text("old artifact", encoding="utf-8")
    old_ns = 1_000_000_000
    os.utime(artifact, ns=(old_ns, old_ns))
    monkeypatch.setattr(
        MODULE,
        "load_artifact",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(AssertionError("old artifact read")),
    )
    with pytest.raises(RuntimeError, match="not written by this run"):
        MODULE.admit_dsl_baselines(
            tmp_path,
            profile,
            manifest,
            receipt,
            "a" * 40,
            MODULE.time.time_ns(),
            hashlib.sha256(b"fresh receipt").hexdigest(),
        )


def test_dsl_admission_refuses_symlinked_candidate(monkeypatch, tmp_path: Path) -> None:
    manifest = MODULE.load_manifest()
    profile = MODULE.load_profiles()["dsl-authority"]
    receipt = tmp_path / "preflight.json"
    receipt.write_text("fresh receipt", encoding="utf-8")
    target = tmp_path / "outside.json"
    target.write_text("fabricated candidate", encoding="utf-8")
    artifact = tmp_path / "artifacts/dsl-bench/warm-matrix.json"
    artifact.parent.mkdir(parents=True)
    artifact.symlink_to(target)
    monkeypatch.setattr(
        MODULE,
        "load_artifact",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(AssertionError("symlink was parsed")),
    )
    with pytest.raises(RuntimeError, match="not a regular file"):
        MODULE.admit_dsl_baselines(
            tmp_path,
            profile,
            manifest,
            receipt,
            "a" * 40,
            0,
            hashlib.sha256(b"fresh receipt").hexdigest(),
        )


def test_dsl_admission_rejects_replaced_preflight_before_artifacts(
    monkeypatch, tmp_path: Path
) -> None:
    manifest = MODULE.load_manifest()
    profile = MODULE.load_profiles()["dsl-authority"]
    receipt = tmp_path / "preflight.json"
    receipt.write_text("replaced receipt", encoding="utf-8")
    monkeypatch.setattr(
        MODULE,
        "load_artifact",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(AssertionError("artifact read")),
    )
    with pytest.raises(RuntimeError, match="receipt changed during capture"):
        MODULE.admit_dsl_baselines(
            tmp_path,
            profile,
            manifest,
            receipt,
            "a" * 40,
            0,
            hashlib.sha256(b"original receipt").hexdigest(),
        )


def test_dsl_admission_does_not_write_warm_if_cold_is_invalid(monkeypatch, tmp_path: Path) -> None:
    manifest = MODULE.load_manifest()
    profile = MODULE.load_profiles()["dsl-authority"]
    receipt = tmp_path / "preflight.json"
    receipt.write_text("fresh receipt", encoding="utf-8")
    capture_started_ns = MODULE.time.time_ns()
    for mode in ("warm", "cold"):
        path = tmp_path / f"artifacts/dsl-bench/{mode}-matrix.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(mode, encoding="utf-8")

    def fake_load(path, *, role, content=None):
        mode = "warm" if path.name.startswith("warm") else "cold"
        return SimpleNamespace(
            git_head="a" * 40,
            mode=mode,
            rows={"scenario": SimpleNamespace(samples=200 if mode == "warm" else 0)},
        )

    monkeypatch.setattr(MODULE, "load_artifact", fake_load)
    monkeypatch.setattr(MODULE, "require_complete_baseline_candidate", lambda *_args: None)
    monkeypatch.setattr(MODULE, "require_clean_preflight", lambda *_args: None)
    monkeypatch.setattr(MODULE, "require_frozen_source", lambda *_args: None)

    with pytest.raises(RuntimeError, match="fewer than 20 samples"):
        MODULE.admit_dsl_baselines(
            tmp_path,
            profile,
            manifest,
            receipt,
            "a" * 40,
            capture_started_ns,
            hashlib.sha256(b"fresh receipt").hexdigest(),
        )
    assert not (tmp_path / "tools/benchmark/baselines/warm-matrix.json").exists()
    assert not (tmp_path / "tools/benchmark/baselines/cold-matrix.json").exists()


def test_dsl_admission_writes_both_only_after_guarded_capture(monkeypatch, tmp_path: Path) -> None:
    manifest = MODULE.load_manifest()
    profile = MODULE.load_profiles()["dsl-authority"]
    receipt = tmp_path / "preflight.json"
    receipt.write_text("fresh receipt", encoding="utf-8")
    capture_started_ns = MODULE.time.time_ns()
    for mode in ("warm", "cold"):
        path = tmp_path / f"artifacts/dsl-bench/{mode}-matrix.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(mode, encoding="utf-8")

    def fake_load(path, *, role, content=None):
        mode = "warm" if path.name.startswith("warm") else "cold"
        return SimpleNamespace(
            git_head="a" * 40,
            mode=mode,
            rows={"scenario": SimpleNamespace(samples=200 if mode == "warm" else 20)},
        )

    monkeypatch.setattr(MODULE, "load_artifact", fake_load)
    monkeypatch.setattr(MODULE, "require_complete_baseline_candidate", lambda *_args: None)
    monkeypatch.setattr(MODULE, "require_clean_preflight", lambda *_args: None)
    monkeypatch.setattr(MODULE, "require_frozen_source", lambda *_args: None)

    MODULE.admit_dsl_baselines(
        tmp_path,
        profile,
        manifest,
        receipt,
        "a" * 40,
        capture_started_ns,
        hashlib.sha256(b"fresh receipt").hexdigest(),
    )
    for mode in ("warm", "cold"):
        assert (tmp_path / f"tools/benchmark/baselines/{mode}-matrix.json").read_text() == mode
    assert not (tmp_path / "tools/benchmark/baselines/.dsl-admission-pending").exists()


def test_dsl_admission_refuses_receipt_changed_while_checking_candidates(
    monkeypatch, tmp_path: Path
) -> None:
    manifest = MODULE.load_manifest()
    profile = MODULE.load_profiles()["dsl-authority"]
    receipt = tmp_path / "preflight.json"
    receipt.write_text("original receipt", encoding="utf-8")
    capture_started_ns = MODULE.time.time_ns()
    for mode in ("warm", "cold"):
        artifact_path = tmp_path / f"artifacts/dsl-bench/{mode}-matrix.json"
        artifact_path.parent.mkdir(parents=True, exist_ok=True)
        artifact_path.write_text(mode, encoding="utf-8")

    def fake_load(path, *, role, content=None):
        if path.name.startswith("cold"):
            receipt.write_text("changed receipt", encoding="utf-8")
        mode = "warm" if path.name.startswith("warm") else "cold"
        return SimpleNamespace(
            git_head="a" * 40,
            mode=mode,
            rows={"scenario": SimpleNamespace(samples=200 if mode == "warm" else 20)},
        )

    monkeypatch.setattr(MODULE, "load_artifact", fake_load)
    monkeypatch.setattr(MODULE, "require_complete_baseline_candidate", lambda *_args: None)
    monkeypatch.setattr(MODULE, "require_clean_preflight", lambda *_args: None)
    monkeypatch.setattr(MODULE, "require_frozen_source", lambda *_args: None)
    with pytest.raises(RuntimeError, match="receipt changed during admission"):
        MODULE.admit_dsl_baselines(
            tmp_path,
            profile,
            manifest,
            receipt,
            "a" * 40,
            capture_started_ns,
            hashlib.sha256(b"original receipt").hexdigest(),
        )
    assert not (tmp_path / "tools/benchmark/baselines/warm-matrix.json").exists()
    assert not (tmp_path / "tools/benchmark/baselines/cold-matrix.json").exists()


def test_dsl_admission_refuses_artifact_changed_after_validation(
    monkeypatch, tmp_path: Path
) -> None:
    manifest = MODULE.load_manifest()
    profile = MODULE.load_profiles()["dsl-authority"]
    receipt = tmp_path / "preflight.json"
    receipt.write_text("fresh receipt", encoding="utf-8")
    capture_started_ns = MODULE.time.time_ns()
    for mode in ("warm", "cold"):
        artifact_path = tmp_path / f"artifacts/dsl-bench/{mode}-matrix.json"
        artifact_path.parent.mkdir(parents=True, exist_ok=True)
        artifact_path.write_text(mode, encoding="utf-8")

    def fake_load(path, *, role, content=None):
        mode = "warm" if path.name.startswith("warm") else "cold"
        assert content == mode
        if mode == "cold":
            path.write_text("replacement", encoding="utf-8")
        return SimpleNamespace(
            git_head="a" * 40,
            mode=mode,
            rows={"scenario": SimpleNamespace(samples=200 if mode == "warm" else 20)},
        )

    monkeypatch.setattr(MODULE, "load_artifact", fake_load)
    monkeypatch.setattr(MODULE, "require_complete_baseline_candidate", lambda *_args: None)
    monkeypatch.setattr(MODULE, "require_clean_preflight", lambda *_args: None)
    monkeypatch.setattr(MODULE, "require_frozen_source", lambda *_args: None)
    with pytest.raises(RuntimeError, match="artifact changed during admission"):
        MODULE.admit_dsl_baselines(
            tmp_path,
            profile,
            manifest,
            receipt,
            "a" * 40,
            capture_started_ns,
            hashlib.sha256(b"fresh receipt").hexdigest(),
        )
    assert not (tmp_path / "tools/benchmark/baselines/warm-matrix.json").exists()
    assert not (tmp_path / "tools/benchmark/baselines/cold-matrix.json").exists()


def test_dsl_baseline_pair_restores_prior_files_if_second_write_fails(
    monkeypatch, tmp_path: Path
) -> None:
    warm = tmp_path / "warm.json"
    cold = tmp_path / "cold.json"
    warm.write_text("prior warm", encoding="utf-8")
    cold.write_text("prior cold", encoding="utf-8")
    real_write = MODULE.atomically_write_baseline

    def fail_cold(destination: Path, content: str) -> None:
        if destination == cold:
            raise OSError("injected cold failure")
        real_write(destination, content)

    monkeypatch.setattr(MODULE, "atomically_write_baseline", fail_cold)
    with pytest.raises(RuntimeError, match="pair restored"):
        MODULE.publish_dsl_baseline_pair([(warm, "new warm"), (cold, "new cold")])
    assert warm.read_text(encoding="utf-8") == "prior warm"
    assert cold.read_text(encoding="utf-8") == "prior cold"
    assert not (tmp_path / ".dsl-admission-pending").exists()


def test_dsl_baseline_pair_removes_first_new_file_if_second_write_fails(
    monkeypatch, tmp_path: Path
) -> None:
    warm = tmp_path / "warm.json"
    cold = tmp_path / "cold.json"
    real_write = MODULE.atomically_write_baseline

    def fail_cold(destination: Path, content: str) -> None:
        if destination == cold:
            raise OSError("injected cold failure")
        real_write(destination, content)

    monkeypatch.setattr(MODULE, "atomically_write_baseline", fail_cold)
    with pytest.raises(RuntimeError, match="pair restored"):
        MODULE.publish_dsl_baseline_pair([(warm, "new warm"), (cold, "new cold")])
    assert not warm.exists()
    assert not cold.exists()
    assert not (tmp_path / ".dsl-admission-pending").exists()


def test_dsl_baseline_pair_refuses_symlink_target(tmp_path: Path) -> None:
    target = tmp_path / "outside.json"
    target.write_text("preserved", encoding="utf-8")
    warm = tmp_path / "warm.json"
    cold = tmp_path / "cold.json"
    warm.symlink_to(target)
    with pytest.raises(RuntimeError, match="not a regular file"):
        MODULE.publish_dsl_baseline_pair([(warm, "new warm"), (cold, "new cold")])
    assert target.read_text(encoding="utf-8") == "preserved"
    assert not cold.exists()


def test_dsl_baseline_pair_refuses_crash_marker(tmp_path: Path) -> None:
    marker = tmp_path / ".dsl-admission-pending"
    marker.write_text("interrupted", encoding="utf-8")
    warm = tmp_path / "warm.json"
    cold = tmp_path / "cold.json"
    with pytest.raises(RuntimeError, match="admission is incomplete"):
        MODULE.publish_dsl_baseline_pair([(warm, "new warm"), (cold, "new cold")])
    assert marker.read_text(encoding="utf-8") == "interrupted"
    assert not warm.exists()
    assert not cold.exists()


def test_dsl_baseline_pair_retains_marker_when_rollback_fails(monkeypatch, tmp_path: Path) -> None:
    baseline_dir = tmp_path / "tools/benchmark/baselines"
    baseline_dir.mkdir(parents=True)
    warm = baseline_dir / "warm-matrix.json"
    cold = baseline_dir / "cold-matrix.json"
    warm.write_text("prior warm", encoding="utf-8")
    real_write = MODULE.atomically_write_baseline

    def fail_cold_and_rollback(destination: Path, content: str) -> None:
        if destination == cold or content == "prior warm":
            raise OSError("injected write failure")
        real_write(destination, content)

    monkeypatch.setattr(MODULE, "atomically_write_baseline", fail_cold_and_rollback)
    with pytest.raises(RuntimeError, match="marker retained"):
        MODULE.publish_dsl_baseline_pair([(warm, "new warm"), (cold, "new cold")])
    assert warm.read_text(encoding="utf-8") == "new warm"
    assert (baseline_dir / ".dsl-admission-pending").exists()
    with pytest.raises(RuntimeError, match="admission is incomplete"):
        MODULE.require_declared_baselines(
            tmp_path,
            MODULE.load_profiles()["dsl-authority"],
            MODULE.load_manifest(),
        )


# --------------------------------------------------------------------------
# plan / replay / immutable evidence promotion
# --------------------------------------------------------------------------


def test_plan_is_deterministic_and_digest_bound(capsys) -> None:
    assert MODULE.main(["plan", "dsl-authority"]) == 0
    first = capsys.readouterr().out
    assert MODULE.main(["plan", "dsl-authority"]) == 0
    second = capsys.readouterr().out
    assert first == second
    plan = json.loads(first)
    assert plan["mutates"] is False
    assert plan["families"] == ["dsl-warm", "dsl-cold"]
    assert plan["registry_digest"].startswith("sha256:")
    warm = next(step for step in plan["steps"] if step["family"] == "dsl-warm")
    assert warm["command"] == ["just", "rust-bench-dsl-warm"]
    assert warm["host_policy"] == "canonical-linux"
    assert warm["baseline"] == "tools/benchmark/baselines/warm-matrix.json"
    assert warm["closure"] == "benchmark-control-plane"


def test_plan_resolves_a_cargo_producer_without_a_just_recipe(capsys) -> None:
    assert MODULE.main(["plan", "micro"]) == 0
    plan = json.loads(capsys.readouterr().out)
    step = next(item for item in plan["steps"] if item["family"] == "micro-lq-norm-pipeline")
    assert step["kind"] == "cargo-bench"
    assert step["command"][1:3] == ["--lane", "bench-lane"]
    assert "pipeline" in step["command"]


def test_plan_marks_recorded_families_as_not_runnable(capsys) -> None:
    assert MODULE.main(["plan", "recorded"]) == 0
    plan = json.loads(capsys.readouterr().out)
    agent = next(item for item in plan["steps"] if item["family"] == "agent-outcome")
    assert agent["runnable"] is False
    assert "recorded-only" in agent["reason"]


def _promote_sample_run(root: Path, run_id: str) -> Path:
    import evidence as evidence_module
    import evidence_bridge

    sealed = evidence_module.seal(evidence_module.sample_evidence())
    sealed["run_id"] = run_id
    sealed["digest"] = None
    sealed = evidence_module.seal(sealed)
    native = evidence_module.SAMPLE_RAW
    return evidence_bridge.promote_native_run(
        evidence_root=root,
        run_id=run_id,
        family="dsl-warm",
        profile="dsl-authority",
        created_utc="2026-09-26T12:00:00Z",
        raw_files={
            "warm-matrix.json": evidence_module.write_raw_file(
                root / "work" / run_id / "warm-matrix.json", [native]
            )
        },
        payload=sealed["payload"],
        source=sealed["source"],
        build=sealed["build"],
        inputs=sealed["inputs"],
        host=evidence_bridge.host_identity(
            policy="canonical-linux", os_name="linux", arch="x86_64",
            cpu_count=8, hostname="fixture", lease_mode="shared", lease_samples=1,
        ),
        command=sealed["command"],
        boundary=sealed["boundary"],
        verdict=sealed["verdict"],
    )["run_dir"]


def test_replay_refuses_contract_only_claim_for_registered_native_family(
    tmp_path: Path, capsys
) -> None:
    sys.path.insert(0, str(REPO_ROOT / "tools" / "benchmark"))
    run_id = "dsl-warm-20260926T120000Z-deadbeef"
    run_dir = _promote_sample_run(tmp_path / "root", run_id)
    assert MODULE.main(["replay", str(run_dir)]) == 2
    assert "native artifact oracle failed" in capsys.readouterr().err
    assert MODULE.main(["replay", "--evidence-root", str(tmp_path / "root"), run_id]) == 2


def test_replay_refuses_a_tampered_run(tmp_path: Path, capsys) -> None:
    sys.path.insert(0, str(REPO_ROOT / "tools" / "benchmark"))
    run_id = "dsl-warm-20260926T120000Z-deadbeef"
    run_dir = _promote_sample_run(tmp_path / "root", run_id)
    raw = run_dir / "raw" / "warm-matrix.json"
    raw.write_bytes(raw.read_bytes().replace(b"0.42", b"0.43"))
    assert MODULE.main(["replay", str(run_dir)]) == 2
    assert "digest mismatch" in capsys.readouterr().err


def test_replay_refuses_a_family_claimed_by_another_profile(tmp_path: Path, capsys) -> None:
    import evidence as evidence_module

    sys.path.insert(0, str(REPO_ROOT / "tools" / "benchmark"))
    run_dir = _promote_sample_run(tmp_path / "root", "dsl-warm-wrong-profile")
    evidence_path = run_dir / "evidence.json"
    forged = json.loads(evidence_path.read_text(encoding="utf-8"))
    forged["profile"] = "systems"
    forged["digest"] = None
    evidence_path.write_text(
        evidence_module.to_canonical_json(evidence_module.seal(forged)), encoding="utf-8"
    )
    assert MODULE.main(["replay", str(run_dir)]) == 2
    assert "not registered in its profile" in capsys.readouterr().err


def test_replay_requires_an_evidence_root_outside_a_run_directory(capsys) -> None:
    assert MODULE.main(["replay", "no-such-run"]) == 2
    assert "evidence-root" in capsys.readouterr().err


def test_evidence_root_inside_the_checkout_is_refused(monkeypatch, tmp_path: Path, capsys) -> None:
    install_control_plane(tmp_path)
    monkeypatch.setattr(MODULE, "require_clean_worktree", lambda _repo_root: None)
    monkeypatch.setattr(MODULE, "resolve_checkout_head", lambda _repo_root: "a" * 40)
    monkeypatch.setattr(MODULE, "require_declared_baselines", lambda *_args: None)
    monkeypatch.setattr(MODULE, "require_clean_preflight_receipt", lambda *_args: None)
    monkeypatch.setattr(MODULE, "require_frozen_source", lambda *_args: None)
    monkeypatch.setattr(MODULE, "validate", lambda *_args: 0)

    def clean(_repo_root, _profile, receipt, _manifest):
        receipt.parent.mkdir(parents=True, exist_ok=True)
        receipt.write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "kind": "quanta-index-timing-preflight",
                    "run_id": "benchctl:dsl-authority",
                    "status": "clean",
                    "foreign_rust_processes": [],
                }
            ),
            encoding="utf-8",
        )
        return 0

    monkeypatch.setattr(MODULE, "preflight", clean)
    monkeypatch.setattr(
        MODULE.subprocess, "run", lambda *_args, **_kwargs: SimpleNamespace(returncode=0)
    )
    assert (
        MODULE.main(
            [
                "--repo-root",
                str(tmp_path),
                "run",
                "dsl-authority",
                "--evidence-root",
                str(tmp_path / "evidence"),
            ]
        )
        == 2
    )
    assert "outside the checkout" in capsys.readouterr().err


def _promote_family_run(
    root: Path,
    family: str,
    run_id: str,
    *,
    revision: str = "a" * 40,
    verdict_status: str = "pass",
    preflight_digest: str = "sha256:" + "33" * 32,
    created_utc: str = "2026-09-26T12:00:00Z",
    wrong_payload: bool = False,
) -> Path:
    import evidence as evidence_module
    import evidence_bridge

    payload = (
        {
            "kind": "freshness",
            "phases": [{"name": "full_build", "ms": 10.0, "samples": 1}],
            "stale_hits": 0,
            "generation": "g1",
        }
        if (family == "freshness") != wrong_payload
        else {
            "kind": "load",
            "arrival": "open_loop",
            "generator_saturated": False,
            "points": [
                {
                    "label": "rate-200",
                    "offered_rate": 200.0,
                    "completed_rate": 180.0,
                    "dropped": 20,
                    "timeouts": 0,
                }
            ],
            "errors": 0,
        }
    )
    return evidence_bridge.promote_native_run(
        evidence_root=root,
        run_id=run_id,
        family=family,
        profile="systems",
        created_utc=created_utc,
        raw_files={
            f"{family}-summary.json": evidence_module.write_raw_file(
                root / "work" / run_id / "summary.json", [evidence_module.SAMPLE_RAW]
            )
        },
        payload=payload,
        source={
            "revision": revision,
            "dirty": False,
            "dirty_paths_digest": None,
            "closure_profile": "benchmark-control-plane",
            "closure_digest": "sha256:" + "11" * 32,
        },
        build={
            "toolchain": "rustc 1.92.0",
            "target_triple": "aarch64-apple-darwin",
            "lockfile_digest": "sha256:" + "22" * 32,
            "profile": "bench",
            "flags": ["--locked"],
            "binaries": [],
        },
        inputs=[
            {
                "id": "benchmark-preflight",
                "availability": "present",
                "digest": preflight_digest,
                "reason": None,
            },
            {
                "id": "workspace-fixture",
                "availability": "unavailable",
                "digest": None,
                "reason": "in-process deterministic fixture",
            },
        ],
        host=evidence_bridge.host_identity(
            policy="local-diagnostic",
            os_name="macos",
            arch="aarch64",
            cpu_count=10,
            hostname="host-a",
            lease_mode="shared",
            lease_samples=1,
        ),
        command={
            "argv": ["benchctl", "run", "systems"],
            "cwd": ".",
            "status": "completed",
            "exit_code": 0,
            "timeout_seconds": 1800,
            "wall_ms": 10,
        },
        boundary={
            "clock": "monotonic",
            "instrumentation": "none",
            "start_event": "producer_exec",
            "end_event": "artifact_written",
        },
        verdict={
            "scope": "diagnostic",
            "status": verdict_status,
            "reason": None if verdict_status == "pass" else "regression",
            "metrics": [],
        },
    )["run_dir"]


def _commit_native_test_capture(root, profile, capture_id="native-capture", run_ids=None):
    from profile_capture import commit_capture

    registry = MODULE.load_registry(REPO_ROOT / "tools/benchmark/registry.toml")
    return commit_capture(
        root,
        capture_id=capture_id,
        profile=profile,
        registry_digest=MODULE.registry_digest(registry),
        expected_cases={family: [None] for family in registry["profiles"][profile]["families"]},
        run_ids=run_ids
        if run_ids is not None
        else sorted(p.name for p in (root / "runs").iterdir()),
    )


def test_promoted_run_validation_refuses_unverifiable_payloads_and_other_profile(
    tmp_path: Path, capsys
) -> None:
    sys.path.insert(0, str(REPO_ROOT / "tools" / "benchmark"))
    manifest = MODULE.load_manifest()
    root = tmp_path / "root"
    _promote_family_run(root, "freshness", "freshness-20260926T120000Z-aaaaaaaa")
    _promote_family_run(root, "open-loop", "open-loop-20260926T120000Z-bbbbbbbb")
    _commit_native_test_capture(root, "systems")

    expected_source = {
        "revision": "a" * 40,
        "dirty": False,
        "dirty_paths_digest": None,
        "closure_profile": "benchmark-control-plane",
        "closure_digest": "sha256:" + "11" * 32,
    }
    expected_lock = "sha256:" + "22" * 32
    assert (
        MODULE.validate_promoted_runs(root, "systems", manifest, expected_source, expected_lock)
        == 2
    )
    assert "native artifact oracle failed" in capsys.readouterr().err

    # dsl-authority owns two different families; its runs are absent, so the
    # profile must refuse rather than accept the systems runs.
    assert (
        MODULE.validate_promoted_runs(
            root, "dsl-authority", manifest, expected_source, expected_lock
        )
        == 2
    )
    assert "complete profile 'dsl-authority'" in capsys.readouterr().err


def test_promoted_latency_profile_replays_both_native_artifacts(tmp_path: Path, capsys) -> None:
    import evidence as evidence_module
    import evidence_bridge

    from tools.ci.tests.test_check_bench_artifacts import HEAD, artifact

    manifest = MODULE.load_manifest()
    root = tmp_path / "runs"
    source = {
        "revision": HEAD,
        "dirty": False,
        "dirty_paths_digest": None,
        "closure_profile": "benchmark-control-plane",
        "closure_digest": "sha256:" + "11" * 32,
    }
    lock = "sha256:" + "22" * 32
    for family in ("dsl-warm", "dsl-cold"):
        native = artifact(family, head=HEAD)
        raw = (json.dumps(native) + "\n").encode()
        evidence_bridge.promote_native_run(
            evidence_root=root,
            run_id=f"{family}-native-proof",
            family=family,
            profile="dsl-authority",
            created_utc="2026-09-26T12:00:00Z",
            raw_files={
                f"{family}.json": evidence_module.write_raw_file(
                    root / "work" / family / "native.json", [raw]
                )
            },
            payload=evidence_bridge.latency_payload_from_artifact(native),
            source=source,
            build={
                "toolchain": "rustc 1.92.0",
                "target_triple": "x86_64-unknown-linux-gnu",
                "lockfile_digest": lock,
                "profile": "bench",
                "flags": ["--locked"],
                "binaries": [],
            },
            inputs=[
                {
                    "id": "benchmark-preflight",
                    "availability": "present",
                    "digest": "sha256:" + "33" * 32,
                    "reason": None,
                }
            ],
            host=evidence_bridge.host_identity(
                policy="canonical-linux",
                os_name="linux",
                arch="x86_64",
                cpu_count=8,
                hostname="test-host",
                lease_mode="shared",
                lease_samples=1,
            ),
            command={
                "argv": ["benchctl", "run", "dsl-authority"],
                "cwd": ".",
                "status": "completed",
                "exit_code": 0,
                "timeout_seconds": 3600,
                "wall_ms": 1,
            },
            boundary={
                "clock": "monotonic",
                "instrumentation": "none",
                "start_event": "producer_exec",
                "end_event": "artifact_written",
            },
            verdict={"scope": "diagnostic", "status": "pass", "reason": None, "metrics": []},
        )
    _commit_native_test_capture(root, "dsl-authority")
    assert MODULE.validate_promoted_runs(root, "dsl-authority", manifest, source, lock) == 0
    assert len(json.loads(capsys.readouterr().out)["runs"]) == 2
    evidence_path = root / "runs" / "dsl-warm-native-proof" / "evidence.json"
    forged = json.loads(evidence_path.read_text(encoding="utf-8"))
    forged["payload"]["rows"][0]["p50"] = 0.5
    forged["output_digest"] = evidence_module.digest_bytes(
        json.dumps(forged["payload"], sort_keys=True).encode()
    )
    forged["digest"] = None
    forged = evidence_module.seal(forged)
    evidence_path.write_text(evidence_module.to_canonical_json(forged), encoding="utf-8")
    # Rebind the forged envelope coherently: native replay, not a stale digest,
    # must catch the disagreement with immutable raw samples.
    _commit_native_test_capture(root, "dsl-authority", capture_id="forged-capture")
    assert MODULE.validate_promoted_runs(root, "dsl-authority", manifest, source, lock) == 2
    assert "typed payload differs" in capsys.readouterr().err


def test_promoted_run_validation_rejects_failed_wrong_source_and_tampered_runs(
    tmp_path: Path, capsys
) -> None:
    manifest = MODULE.load_manifest()
    expected_source = {
        "revision": "a" * 40,
        "dirty": False,
        "dirty_paths_digest": None,
        "closure_profile": "benchmark-control-plane",
        "closure_digest": "sha256:" + "11" * 32,
    }
    lock = "sha256:" + "22" * 32
    root = tmp_path / "failed"
    _promote_family_run(
        root, "freshness", "freshness-20260926T120000Z-failed01", verdict_status="fail"
    )
    _promote_family_run(root, "open-loop", "open-loop-20260926T120000Z-passed01")
    with pytest.raises(MODULE.EvidenceError, match="non-passing"):
        _commit_native_test_capture(root, "systems")
    assert not (root / "profiles/systems.json").exists()

    root = tmp_path / "wrong-source"
    _promote_family_run(root, "freshness", "freshness-20260926T120000Z-wrong001", revision="b" * 40)
    _promote_family_run(root, "open-loop", "open-loop-20260926T120000Z-passed02", revision="b" * 40)
    _commit_native_test_capture(root, "systems")
    assert MODULE.validate_promoted_runs(root, "systems", manifest, expected_source, lock) == 2
    assert "wrong profile or source" in capsys.readouterr().err

    root = tmp_path / "tampered"
    _promote_family_run(root, "freshness", "freshness-20260926T120000Z-old00001")
    new = _promote_family_run(root, "freshness", "freshness-20260926T130000Z-new00001")
    _promote_family_run(root, "open-loop", "open-loop-20260926T120000Z-passed03")
    _commit_native_test_capture(
        root, "systems", run_ids=[new.name, "open-loop-20260926T120000Z-passed03"]
    )
    (new / "raw" / "freshness-summary.json").write_bytes(b"tampered")
    assert MODULE.validate_promoted_runs(root, "systems", manifest, expected_source, lock) == 2
    assert "raw file" in capsys.readouterr().err

    root = tmp_path / "wrong-payload"
    _promote_family_run(root, "freshness", "freshness-wrong-payload", wrong_payload=True)
    _promote_family_run(root, "open-loop", "open-loop-correct-payload")
    _commit_native_test_capture(root, "systems")
    assert MODULE.validate_promoted_runs(root, "systems", manifest, expected_source, lock) == 2
    assert "wrong payload kind" in capsys.readouterr().err


def test_promoted_profile_refuses_mixed_capture_even_with_same_source(
    tmp_path: Path, capsys
) -> None:
    manifest = MODULE.load_manifest()
    root = tmp_path / "mixed"
    _promote_family_run(root, "freshness", "freshness-first")
    _promote_family_run(
        root,
        "open-loop",
        "open-loop-second",
        preflight_digest="sha256:" + "44" * 32,
        created_utc="2026-09-26T13:00:00Z",
    )
    source = {
        "revision": "a" * 40,
        "dirty": False,
        "dirty_paths_digest": None,
        "closure_profile": "benchmark-control-plane",
        "closure_digest": "sha256:" + "11" * 32,
    }
    _commit_native_test_capture(root, "systems")
    assert (
        MODULE.validate_promoted_runs(root, "systems", manifest, source, "sha256:" + "22" * 32) == 2
    )
    assert "mixes different benchmark captures" in capsys.readouterr().err


def _native_profile_fixture(monkeypatch, tmp_path: Path, profile_name: str) -> dict:
    """Real validator, bridge and store only publish the two selected families."""
    import evidence_bridge

    from tools.ci.tests.test_check_bench_artifacts import HEAD, artifact

    manifest = MODULE.load_manifest()
    repo = tmp_path / "repo"
    install_control_plane(repo)
    registry = MODULE.load_registry(REPO_ROOT / "tools/benchmark/registry.toml")
    for table in ("producers", "validators", "scorers"):
        for entry in registry[table].values():
            if "module" in entry:
                target = repo / entry["module"]
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes((REPO_ROOT / entry["module"]).read_bytes())
    selected = manifest["profiles"][profile_name]["families"]
    for family in [*selected, "scale"]:
        target = repo / manifest["families"][family]["artifact_glob"]
        target.parent.mkdir(parents=True, exist_ok=True)
        if profile_name == "systems" and family in selected:
            from tools.ci.tests.test_benchmark_evidence_bridge import system_artifacts

            native = system_artifacts(family)[0]
            if family == "open-loop":
                point = native["detail"]["points"][0]
                native["detail"].update(
                    {
                        "arrival_model": "seeded_poisson",
                        "duration_ms": 10_000,
                        "points": [{**point, "target_qps": rate} for rate in (50, 100, 200, 400)],
                    }
                )
        else:
            native = artifact(family)
        target.write_text(json.dumps(native), encoding="utf-8")
    source = {
        "revision": HEAD,
        "dirty": False,
        "dirty_paths_digest": None,
        "closure_profile": "benchmark-control-plane",
        "closure_digest": "sha256:" + "11" * 32,
    }
    # This is an artifact/store integration fixture; Git capture belongs to
    # source_closure tests. No artifact validator, bridge or store is mocked.
    monkeypatch.setattr(evidence_bridge, "source_identity", lambda *_args: source)
    monkeypatch.setattr(MODULE, "_host_os", lambda: "linux")
    checker = MODULE._load_lint_module(REPO_ROOT)
    monkeypatch.setattr(MODULE, "_load_lint_module", lambda *_args: checker)
    (repo / "Cargo.lock").write_text("# fixture\n", encoding="utf-8")
    (repo / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "1.92.0"\n')
    receipt = repo / "preflight.json"
    receipt.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "kind": "quanta-index-timing-preflight",
                "run_id": f"benchctl:{profile_name}",
                "status": "clean",
                "foreign_rust_processes": [],
                "host": {"os": "linux", "cpu_count": 8, "load_average": [1.0, 1.0, 1.0]},
                "host_contention": {
                    "one_minute_load": 1.0,
                    "one_minute_load_limit": 4.0,
                    "over_limit": False,
                },
            }
        ),
        encoding="utf-8",
    )
    return dict(
        repo_root=repo,
        profile_name=profile_name,
        manifest=manifest,
        evidence_root=tmp_path / "evidence",
        initial_head=HEAD,
        receipt=receipt,
        preflight_digest="sha256:" + hashlib.sha256(receipt.read_bytes()).hexdigest(),
        capture_started_ns=0,
        validated_artifacts=MODULE.snapshot_profile_artifacts(repo, profile_name, manifest),
    )


@pytest.mark.parametrize("profile_name", ["dsl-authority", "systems"])
def test_promotion_is_scoped_to_the_profile_families(monkeypatch, tmp_path, profile_name):
    import evidence_bridge

    args = _native_profile_fixture(monkeypatch, tmp_path, profile_name)
    repo, root = args["repo_root"], args["evidence_root"]
    manifest = args["manifest"]
    selected = manifest["profiles"][profile_name]["families"]
    source = evidence_bridge.source_identity(repo, "benchmark-control-plane")
    assert MODULE.promote_profile_runs(**args) == 0
    runs = [MODULE.RunStore(root).load(path.name) for path in (root / "runs").iterdir()]
    assert {run["family"] for run in runs} == set(selected)
    assert len(runs) == 2
    assert (root / "profiles" / f"{profile_name}.json").is_file()
    assert MODULE.RunStore(root).collect([]) == []
    for run in runs:
        replay = subprocess.run(
            [sys.executable, str(SCRIPT_PATH), "replay", run["run_id"],
             "--evidence-root", str(root)],
            cwd=REPO_ROOT, capture_output=True, text=True, timeout=30,
        )
        assert replay.returncode == 0, replay.stdout + replay.stderr
    assert MODULE.validate_promoted_runs(
        root, profile_name, manifest, source,
        "sha256:" + hashlib.sha256((repo / "Cargo.lock").read_bytes()).hexdigest(),
    ) == 0


def test_native_capture_keeps_real_failed_producer_logs_and_prior_pointer(monkeypatch, tmp_path):
    from types import SimpleNamespace

    args = _native_profile_fixture(monkeypatch, tmp_path, "systems")
    repo, root = args["repo_root"], args["evidence_root"]
    assert MODULE.promote_profile_runs(**args) == 0
    pointer = root / "profiles/systems.json"
    prior = pointer.read_bytes()
    tools = tmp_path / "fixture-tools"
    tools.mkdir()
    just = tools / "just"
    just.write_text("#!/bin/sh\nprintf native-failure-oracle\nexit 7\n")
    just.chmod(0o755)
    monkeypatch.setenv("PATH", str(tools) + os.pathsep + os.environ["PATH"])
    monkeypatch.setattr(MODULE, "require_declared_baselines", lambda *_: None)
    monkeypatch.setattr(MODULE, "require_clean_worktree", lambda *_: None)
    monkeypatch.setattr(MODULE, "resolve_checkout_head", lambda *_: args["initial_head"])
    monkeypatch.setattr(MODULE, "require_frozen_source", lambda *_: None)
    raw_receipt = args["receipt"].read_bytes()

    def preflight(_repo, _profile, receipt, _manifest):
        receipt.parent.mkdir(parents=True, exist_ok=True)
        receipt.write_bytes(raw_receipt)
        return 0

    monkeypatch.setattr(MODULE, "preflight", preflight)
    cli = SimpleNamespace(command="run", profile="systems", evidence_root=root,
                          admit_baseline=False, cold_samples=None)
    result = MODULE._capture_native_run(
        repo, root, "systems", args=cli, argv=["run", "systems"],
        profile={"recipes": ["fixture-producer"]}, manifest=args["manifest"], artifact_profile="systems",
    )
    assert result == 2
    assert pointer.read_bytes() == prior
    failures = list((root / "failures").glob("*.json"))
    assert len(failures) == 1
    failure = json.loads(failures[0].read_text())
    assert failure["phase"] == "execution"
    assert "exit 7" in failure["error"]["message"]
    observed = failure["observations"]["execution"]
    log_dir = Path(observed["log_dir"])
    assert log_dir.is_relative_to(Path(failure["work_root"]))
    assert (log_dir / "stdout").read_bytes() == b"native-failure-oracle"
    terminal = json.loads((log_dir / "execution.json").read_text())
    assert terminal["command"]["exit_code"] == 7
    assert observed["record"]["sha256"] == "sha256:" + hashlib.sha256((log_dir / "execution.json").read_bytes()).hexdigest()
    assert MODULE.RunStore(root).collect([]) == []
    assert failures[0].is_file()
    assert (log_dir / "execution.json").is_file()


def test_native_capture_binds_observed_host_through_promotion_and_replay(monkeypatch, tmp_path, capsys):
    import copy

    import host_monitor
    from evidence_bridge import verify_host_binding
    from profile_capture import load_capture

    args = _native_profile_fixture(monkeypatch, tmp_path, "systems")
    repo, root = args["repo_root"], args["evidence_root"]
    tools = tmp_path / "fixture-tools"
    tools.mkdir()
    just = tools / "just"
    just.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    just.chmod(0o755)
    monkeypatch.setenv("PATH", str(tools) + os.pathsep + os.environ["PATH"])
    monkeypatch.setattr(MODULE, "require_declared_baselines", lambda *_: None)
    monkeypatch.setattr(MODULE, "require_clean_worktree", lambda *_: None)
    monkeypatch.setattr(MODULE, "resolve_checkout_head", lambda *_: args["initial_head"])
    monkeypatch.setattr(MODULE, "require_frozen_source", lambda *_: None)
    monkeypatch.setattr(MODULE, "validate", lambda *_: 0)
    monkeypatch.setattr(MODULE, "compare", lambda *_: 0)
    receipt_bytes = args["receipt"].read_bytes()

    def preflight(_repo, _profile, receipt, _manifest):
        receipt.parent.mkdir(parents=True, exist_ok=True)
        receipt.write_bytes(receipt_bytes)
        return 0

    monkeypatch.setattr(MODULE, "preflight", preflight)
    original_execute = MODULE.execute

    def execute_and_refresh(*command_args, **command_kwargs):
        result = original_execute(*command_args, **command_kwargs)
        for path in args["validated_artifacts"]:
            os.utime(repo / path, None)
        return result

    monkeypatch.setattr(MODULE, "execute", execute_and_refresh)
    monkeypatch.setattr(host_monitor, "lock_path", lambda: tmp_path / "host-lock")
    host = {"os": "linux", "arch": "x86_64", "cpu_count": 8,
            "hostname_hash": "sha256:" + "a" * 64}
    facts = {"load_average": [0.1, 0.2, 0.3], "disk_available_bytes": 100,
             "process_count": 1, "process_snapshot_sha256": "sha256:" + "b" * 64,
             "foreign_rust": []}
    monkeypatch.setattr(host_monitor, "observe", lambda: (host, facts))
    cli = SimpleNamespace(command="run", profile="systems", evidence_root=root,
                          admit_baseline=False, cold_samples=None)
    assert MODULE._capture_native_run(
        repo, root, "systems", args=cli, argv=["run", "systems"],
        profile={"recipes": ["fixture-producer"], "families": ["freshness", "open-loop"]},
        manifest=args["manifest"], artifact_profile="systems",
    ) == 0
    store = MODULE.RunStore(root)
    runs = [store.load(path.name) for path in (root / "runs").iterdir()]
    assert len(runs) == 2
    assert {run["family"] for run in runs} == {"freshness", "open-loop"}
    capture_id = runs[0]["run_id"].removeprefix(runs[0]["family"] + "-").rsplit("-", 1)[0]
    assert load_capture(root, profile="systems", registry_digest=MODULE.registry_digest(
        MODULE.load_registry(repo / "tools/benchmark/registry.toml")
    ))["capture_id"] == capture_id
    for run in runs:
        assert run["host"]["lease"]["mode"] == "exclusive"
        assert run["host"]["lease"]["observed_samples"] >= 3
        verify_host_binding(store, run, capture_id=capture_id)
        assert MODULE.replay_command(repo, run["run_id"], root) == 0
    original_load = MODULE.RunStore.load

    def forged_policy(self, run_id):
        import evidence_bridge
        from evidence import RawFile

        record = copy.deepcopy(original_load(self, run_id))
        raw = RawFile.capture(self.run_dir(run_id) / "raw/host-observations.jsonl")
        record["host"] = evidence_bridge.host_from_observations(
            raw, policy="canonical-linux", capture_id=capture_id, profile="systems",
        )
        return record

    with monkeypatch.context() as changed:
        changed.setattr(MODULE.RunStore, "load", forged_policy)
        assert MODULE.replay_command(repo, runs[0]["run_id"], root) == 2
    assert "host policy differs" in capsys.readouterr().err


def test_native_preflight_nonzero_retains_actual_reason(monkeypatch, tmp_path):
    from types import SimpleNamespace

    args = _native_profile_fixture(monkeypatch, tmp_path, "systems")
    monkeypatch.setattr(MODULE, "require_declared_baselines", lambda *_: None)
    monkeypatch.setattr(MODULE, "require_clean_worktree", lambda *_: None)
    monkeypatch.setattr(MODULE, "resolve_checkout_head", lambda *_: args["initial_head"])
    monkeypatch.setattr(MODULE, "preflight", lambda *_: 7)
    root = args["evidence_root"]
    cli = SimpleNamespace(command="run", profile="systems", evidence_root=root,
                          admit_baseline=False, cold_samples=None)
    assert MODULE._capture_native_run(
        args["repo_root"], root, "systems", args=cli, argv=["run", "systems"],
        profile={"recipes": ["must-not-run"]}, manifest=args["manifest"], artifact_profile="systems",
    ) == 7
    failure = json.loads(next((root / "failures").glob("*.json")).read_text())
    assert failure["phase"] == "preflight"
    assert "local timing preflight returned exit 7; receipt path:" in failure["error"]["message"]
    assert "execution" not in failure["observations"]
    assert not (root / "profiles").exists()


def test_native_raw_filename_collision_refuses_before_promotion(monkeypatch, tmp_path, capsys):
    args = _native_profile_fixture(monkeypatch, tmp_path, "systems")
    original = MODULE._capture_native_family

    def duplicated(*values, **kwargs):
        raw, artifacts, payload = original(*values, **kwargs)
        return [*raw, raw[0]], artifacts, payload

    monkeypatch.setattr(MODULE, "_capture_native_family", duplicated)
    assert MODULE.promote_profile_runs(**args) == 2
    assert "repeats a destination filename" in capsys.readouterr().err
    assert not (args["evidence_root"] / "runs").exists()


def test_native_publication_does_not_reread_unbounded_artifact_bytes(monkeypatch, tmp_path):
    args = _native_profile_fixture(monkeypatch, tmp_path, "systems")
    original = Path.read_bytes

    def bounded_only(path):
        assert not path.is_relative_to(args["repo_root"] / "artifacts"), (
            "native artifact whole read"
        )
        return original(path)

    monkeypatch.setattr(Path, "read_bytes", bounded_only)
    assert MODULE.promote_profile_runs(**args) == 0


def test_native_preparation_keeps_commitments_not_payload_copies(monkeypatch, tmp_path):
    from evidence import RawFile

    args = _native_profile_fixture(monkeypatch, tmp_path, "systems")
    captures, artifacts, payload = MODULE._capture_native_family(
        args["repo_root"],
        "freshness",
        args["manifest"]["families"]["freshness"],
        args["initial_head"],
        args["manifest"],
        0,
        args["validated_artifacts"],
    )
    assert len(captures) == len(artifacts) == 1
    path, ref = captures[0]
    assert isinstance(ref, RawFile)
    assert ref.path == path
    assert ref.sha256 == "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()
    assert payload["kind"] == "freshness"


def test_native_prepared_source_mutation_cannot_publish(monkeypatch, tmp_path):
    import profile_capture

    args = _native_profile_fixture(monkeypatch, tmp_path, "systems")
    publish = profile_capture.publish_capture

    def mutate(*values, **kwargs):
        ref = next(iter(kwargs["runs"][0]["raw_files"].values()))
        ref.path.write_bytes(ref.path.read_bytes() + b" ")
        return publish(*values, **kwargs)

    monkeypatch.setattr(profile_capture, "publish_capture", mutate)
    assert MODULE.promote_profile_runs(**args) == 2
    assert not (args["evidence_root"] / "profiles/systems.json").exists()


def test_native_partial_publication_preserves_prior_complete_capture(monkeypatch, tmp_path):
    import evidence_bridge
    from profile_capture import load_capture

    args = _native_profile_fixture(monkeypatch, tmp_path, "systems")
    root = args["evidence_root"]
    assert MODULE.promote_profile_runs(**args) == 0
    pointer = root / "profiles/systems.json"
    previous = pointer.read_bytes()
    real = evidence_bridge.promote_native_run
    calls = []

    def fail_second(**kwargs):
        calls.append(kwargs["family"])
        if len(calls) == 2:
            raise MODULE.EvidenceError("injected second-family failure")
        return real(**kwargs)

    monkeypatch.setattr(evidence_bridge, "promote_native_run", fail_second)
    assert MODULE.promote_profile_runs(**args) == 2
    assert calls == ["freshness", "open-loop"]
    assert pointer.read_bytes() == previous
    registry = MODULE.load_registry(REPO_ROOT / "tools/benchmark/registry.toml")
    document = load_capture(
        root, profile="systems", registry_digest=MODULE.registry_digest(registry)
    )
    assert len(document["runs"]) == 2
    assert len(MODULE.RunStore(root).collect([])) == 1
    failure = json.loads(next((root / "failures").glob("*.json")).read_text())
    assert failure["phase"] == "promotion"
    assert failure["commit_state"] == "not_started"
    assert failure["error"]["message"] == "injected second-family failure"
    source = evidence_bridge.source_identity(args["repo_root"], "benchmark-control-plane")
    lock = "sha256:" + hashlib.sha256((args["repo_root"] / "Cargo.lock").read_bytes()).hexdigest()
    assert MODULE.validate_promoted_runs(root, "systems", args["manifest"], source, lock) == 0


def test_native_validator_ignores_newer_uncommitted_runs_and_requires_pointer(
    monkeypatch, tmp_path, capsys
):
    import evidence_bridge

    args = _native_profile_fixture(monkeypatch, tmp_path, "systems")
    assert MODULE.promote_profile_runs(**args) == 0
    root, repo = args["evidence_root"], args["repo_root"]
    _promote_family_run(root, "freshness", "newer-uncommitted", created_utc="2100-01-01T00:00:00Z")
    source = evidence_bridge.source_identity(repo, "benchmark-control-plane")
    lock = "sha256:" + hashlib.sha256((repo / "Cargo.lock").read_bytes()).hexdigest()
    assert MODULE.validate_promoted_runs(root, "systems", args["manifest"], source, lock) == 0
    (root / "profiles/systems.json").unlink()
    assert MODULE.validate_promoted_runs(root, "systems", args["manifest"], source, lock) == 2
    assert "cannot load complete profile" in capsys.readouterr().err


def test_native_profile_publication_excludes_real_gc_process(monkeypatch, tmp_path):
    import selectors

    import evidence_bridge

    args = _native_profile_fixture(monkeypatch, tmp_path, "systems")
    root = args["evidence_root"]
    real, children = evidence_bridge.promote_native_run, []
    script = f"""
import fcntl, json, sys
from pathlib import Path
sys.path.insert(0, {str(REPO_ROOT / "tools/benchmark")!r})
from evidence import RunStore
root = Path(sys.argv[1])
with (root / '.custody.lock').open('rb') as lock:
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        print('custody-held', flush=True)
    else:
        raise RuntimeError('profile publication did not exclude GC')
print(json.dumps(RunStore(root).collect([])), flush=True)
"""

    def competing_gc(**kwargs):
        result = real(**kwargs)
        assert (root / "captures").is_dir(), "Rust GC exclusion must precede first promotion"
        if not children:
            child = subprocess.Popen(
                [sys.executable, "-c", script, str(root)],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            children.append(child)
            with selectors.DefaultSelector() as watch:
                watch.register(child.stdout, selectors.EVENT_READ)
                assert watch.select(10), "collector never tested custody"
            assert child.stdout.readline().strip() == "custody-held"
        return result

    monkeypatch.setattr(evidence_bridge, "promote_native_run", competing_gc)
    try:
        assert MODULE.promote_profile_runs(**args) == 0
        stdout, stderr = children[0].communicate(timeout=20)
        assert children[0].returncode == 0, stderr
        assert json.loads(stdout) == []
        assert len(list((root / "runs").iterdir())) == 2
    finally:
        for child in children:
            if child.poll() is None:
                child.kill()
                child.communicate(timeout=10)


def test_promotion_refuses_missing_preflight_and_partial_multi_artifact_claim(
    monkeypatch, tmp_path: Path, capsys
) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    root = tmp_path / "runs"
    manifest = {
        "families": {"fanout": {"artifact_glob": "artifacts/fanout/*.json"}},
        "profiles": {"fanout": {"families": ["fanout"]}},
    }
    receipt = repo / "preflight.json"
    monkeypatch.setattr(MODULE, "require_clean_preflight_receipt", lambda *_args: None)
    checker = MODULE._load_lint_module(REPO_ROOT)
    monkeypatch.setattr(MODULE, "_load_lint_module", lambda *_args: checker)
    digest = "sha256:" + hashlib.sha256(b'{"status":"clean"}').hexdigest()
    assert (
        MODULE.promote_profile_runs(
            repo, "fanout", manifest, root, "a" * 40, receipt, digest, 0, {}
        )
        == 2
    )
    assert "preflight receipt" in capsys.readouterr().err
    receipt.write_text('{"status":"clean"}')
    output = repo / "artifacts" / "fanout"
    output.mkdir(parents=True)
    (output / "first.json").write_text("{}")
    (output / "second.json").write_text("{}")
    assert (
        MODULE.promote_profile_runs(
            repo, "fanout", manifest, root, "a" * 40, receipt, digest, 0, {}
        )
        == 2
    )
    assert "inventory exceeds registered count" in capsys.readouterr().err


@pytest.mark.parametrize(
    "profile_name",
    [
        "micro",
        "dsl-diagnostic",
        "recorded",
        "retrieval-contract",
        "retrieval-diagnostic",
        "lexical-diagnostic",
    ],
)
def test_non_native_profiles_require_adapter_or_explicit_root_before_execution(
    monkeypatch, capsys, profile_name: str
) -> None:
    def forbidden(*_args, **_kwargs):
        raise AssertionError("capture ran despite missing adapter")

    monkeypatch.setattr(MODULE, "require_clean_worktree", forbidden)
    assert MODULE.main(["run", profile_name]) == 2
    error = capsys.readouterr().err
    assert "no producer was executed" in error.lower()
    assert "requires --evidence-root" in error


def test_native_fanout_rejects_mixed_inputs_and_incomplete_inventory() -> None:
    from tools.ci.tests.test_benchmark_evidence_bridge import system_artifacts

    artifacts = system_artifacts("concurrency")
    entry = MODULE.load_manifest()["families"]["concurrency"]
    checker = MODULE._load_lint_module(REPO_ROOT)
    assert MODULE._check_native_inventory(artifacts, entry, checker.CONCURRENCY_COUNTS) is None
    assert MODULE._check_native_inventory(artifacts[:-1], entry, checker.CONCURRENCY_COUNTS)
    assert MODULE._check_native_inventory(
        artifacts + [artifacts[0]], entry, checker.CONCURRENCY_COUNTS
    )
    artifacts[-1]["provenance"]["corpus_digest"] = "sha256:" + "77" * 32
    with pytest.raises(MODULE.EvidenceError, match="mixes corpus_digest"):
        MODULE._native_inputs(artifacts, "sha256:" + "33" * 32)


@pytest.mark.parametrize(
    "profile_name",
    [
        "micro",
        "dsl-diagnostic",
        "retrieval-contract",
        "retrieval-diagnostic",
        "lexical-diagnostic",
        "recorded",
    ],
)
def test_non_native_read_commands_have_explicit_unmeasured_state(
    tmp_path, capsys, profile_name: str
) -> None:
    assert MODULE.main(["summarize", profile_name]) == 0
    observed = json.loads(capsys.readouterr().out)
    assert observed["status"] == "registered_not_captured" and observed["measurement_count"] is None
    assert (
        MODULE.main(["preflight", profile_name, "--receipt", str(tmp_path / "preflight.json")]) == 2
    )
    assert "no native timing preflight" in capsys.readouterr().err
    assert not (tmp_path / "preflight.json").exists()


def test_promotion_refuses_stale_artifact_and_replaced_preflight(
    monkeypatch, tmp_path: Path, capsys
) -> None:
    repo = tmp_path / "repo"
    artifact = repo / "artifacts" / "family" / "summary.json"
    artifact.parent.mkdir(parents=True)
    artifact.write_text("{}", encoding="utf-8")
    receipt = repo / "preflight.json"
    receipt.write_text('{"status":"clean"}', encoding="utf-8")
    manifest = {
        "families": {"family": {"artifact_glob": "artifacts/family/summary.json"}},
        "profiles": {"one": {"families": ["family"]}},
    }
    monkeypatch.setattr(MODULE, "require_clean_preflight_receipt", lambda *_args: None)
    checker = MODULE._load_lint_module(REPO_ROOT)
    monkeypatch.setattr(MODULE, "_load_lint_module", lambda *_args: checker)
    digest = "sha256:" + hashlib.sha256(receipt.read_bytes()).hexdigest()
    assert (
        MODULE.promote_profile_runs(
            repo,
            "one",
            manifest,
            tmp_path / "runs",
            "a" * 40,
            receipt,
            digest,
            artifact.stat().st_mtime_ns + 1,
            {},
        )
        == 2
    )
    assert "not fresh regular output" in capsys.readouterr().err
    receipt.write_text('{"status":"clean","replaced":true}', encoding="utf-8")
    assert (
        MODULE.promote_profile_runs(
            repo, "one", manifest, tmp_path / "runs", "a" * 40, receipt, digest, 0, {}
        )
        == 2
    )
    assert "changed during capture" in capsys.readouterr().err
    receipt.write_text('{"status":"clean"}', encoding="utf-8")
    assert (
        MODULE.promote_profile_runs(
            repo, "one", manifest, tmp_path / "runs", "a" * 40, receipt, digest, 0, {}
        )
        == 2
    )
    assert "artifact changed after validation" in capsys.readouterr().err


def test_promotion_rechecks_the_exact_native_bytes(monkeypatch, tmp_path: Path, capsys) -> None:
    repo = tmp_path / "repo"
    artifact = repo / "artifacts" / "family" / "summary.json"
    artifact.parent.mkdir(parents=True)
    artifact.write_text('{"schema_version":2,"rows":[{"scenario_id":"s"}]}')
    receipt = repo / "preflight.json"
    receipt.write_text('{"status":"clean"}')
    manifest = {
        "families": {
            "family": {
                "artifact_glob": "artifacts/family/summary.json",
                "host_policy": "any",
                "requires_verdict": False,
                "minimum_samples": None,
                "payload": "latency",
            }
        },
        "profiles": {"one": {"families": ["family"]}},
    }
    monkeypatch.setattr(MODULE, "require_clean_preflight_receipt", lambda *_args: None)
    load_lint = MODULE._load_lint_module
    monkeypatch.setattr(MODULE, "_load_lint_module", lambda *_args: load_lint(REPO_ROOT))
    digest = "sha256:" + hashlib.sha256(receipt.read_bytes()).hexdigest()
    frozen = MODULE.snapshot_profile_artifacts(repo, "one", manifest)
    assert (
        MODULE.promote_profile_runs(
            repo, "one", manifest, tmp_path / "runs", "a" * 40, receipt, digest, 0, frozen
        )
        == 2
    )
    assert "native artifact refused" in capsys.readouterr().err
