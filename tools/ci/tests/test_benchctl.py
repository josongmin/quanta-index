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

    def fake_run(command, **_kwargs):
        calls.append(command)
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(MODULE, "preflight", clean)
    monkeypatch.setattr(MODULE, "validate", lambda *_args: 0)
    monkeypatch.setattr(MODULE.subprocess, "run", fake_run)

    assert MODULE.main(["--repo-root", str(tmp_path), "run", "systems"]) == 0
    assert calls == [
        ["just", "rust-verify-quality-freshness"],
        ["just", "rust-verify-quality-open-loop"],
    ]


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

    def run(command, **_kwargs):
        calls.append(command)
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(MODULE.subprocess, "run", run)
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
        MODULE.subprocess,
        "run",
        lambda command, **_kwargs: (commands.append(command), SimpleNamespace(returncode=0))[1],
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
    with pytest.raises(RuntimeError, match="fresh DSL artifact unreadable"):
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
        native_path=Path("warm-matrix.json"),
        native_bytes=native,
        payload=sealed["payload"],
        source=sealed["source"],
        build=sealed["build"],
        inputs=sealed["inputs"],
        host=sealed["host"],
        command=sealed["command"],
        boundary=sealed["boundary"],
        verdict=sealed["verdict"],
    )["run_dir"]


def test_replay_validates_an_immutable_run_from_raw_evidence(tmp_path: Path, capsys) -> None:
    sys.path.insert(0, str(REPO_ROOT / "tools" / "benchmark"))
    run_id = "dsl-warm-20260926T120000Z-deadbeef"
    run_dir = _promote_sample_run(tmp_path / "root", run_id)
    assert MODULE.main(["replay", str(run_dir)]) == 0
    receipt = json.loads(capsys.readouterr().out)
    assert receipt["run_id"] == run_id
    assert receipt["raw_verified"] is True
    assert receipt["replay"] == "contract_only"
    assert MODULE.main(["replay", "--evidence-root", str(tmp_path / "root"), run_id]) == 0


def test_replay_refuses_a_tampered_run(tmp_path: Path, capsys) -> None:
    sys.path.insert(0, str(REPO_ROOT / "tools" / "benchmark"))
    run_id = "dsl-warm-20260926T120000Z-deadbeef"
    run_dir = _promote_sample_run(tmp_path / "root", run_id)
    raw = run_dir / "raw" / "warm-matrix.json"
    raw.write_bytes(raw.read_bytes().replace(b"0.42", b"0.43"))
    assert MODULE.main(["replay", str(run_dir)]) == 2
    assert "digest mismatch" in capsys.readouterr().err


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


def _promote_family_run(root: Path, family: str, run_id: str) -> Path:
    import evidence as evidence_module
    import evidence_bridge

    payload = (
        {
            "kind": "freshness",
            "phases": [{"name": "full_build", "ms": 10.0, "samples": 1}],
            "stale_hits": 0,
            "generation": "g1",
        }
        if family == "freshness"
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
        created_utc="2026-09-26T12:00:00Z",
        native_path=Path(f"{family}-summary.json"),
        native_bytes=evidence_module.SAMPLE_RAW,
        payload=payload,
        source={
            "revision": "a" * 40,
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
                "id": "workspace-fixture",
                "availability": "unavailable",
                "digest": None,
                "reason": "in-process deterministic fixture",
            }
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
        verdict={"scope": "diagnostic", "status": "pass", "reason": None, "metrics": []},
    )["run_dir"]


def test_promoted_run_validation_is_scoped_to_the_profile(tmp_path: Path, capsys) -> None:
    """A profile must not require evidence runs owned by a different profile."""
    sys.path.insert(0, str(REPO_ROOT / "tools" / "benchmark"))
    manifest = MODULE.load_manifest()
    root = tmp_path / "root"
    _promote_family_run(root, "freshness", "freshness-20260926T120000Z-aaaaaaaa")
    _promote_family_run(root, "open-loop", "open-loop-20260926T120000Z-bbbbbbbb")

    expected_source = {
        "revision": "a" * 40,
        "dirty": False,
        "dirty_paths_digest": None,
        "closure_profile": "benchmark-control-plane",
        "closure_digest": "sha256:" + "11" * 32,
    }
    expected_lock = "sha256:" + "22" * 32
    assert MODULE.validate_promoted_runs(root, "systems", manifest, expected_source, expected_lock) == 0
    receipts = json.loads(capsys.readouterr().out)
    assert [entry["family"] for entry in receipts["runs"]] == ["freshness", "open-loop"]

    # dsl-authority owns two different families; its runs are absent, so the
    # profile must refuse rather than accept the systems runs.
    assert MODULE.validate_promoted_runs(root, "dsl-authority", manifest, expected_source, expected_lock) == 2
    assert "dsl-warm" in capsys.readouterr().err


def test_promotion_is_scoped_to_the_profile_families(monkeypatch, tmp_path: Path) -> None:
    """`run --evidence-root` may only promote the families the profile selects."""
    sys.path.insert(0, str(REPO_ROOT / "tools" / "benchmark"))
    import evidence_bridge

    manifest = MODULE.load_manifest()
    repo_root = tmp_path / "repo"
    for family, path in (
        ("freshness", "artifacts/search-quality/freshness/latest/summary.json"),
        ("open-loop", "artifacts/search-quality/open-loop/latest/summary.json"),
    ):
        target = repo_root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(
            json.dumps(
                {
                    "schema_version": 2,
                    "dimension": family,
                    "provenance": {"git_head": "a" * 40},
                    "rows": [
                        {
                            "scenario_id": "s",
                            "latency": {"p50_ms": 1.0, "p95_ms": 2.0, "p99_ms": 3.0, "samples": 25},
                            "error_count": 0,
                            "timeout_count": 0,
                            "early_stop_reason": None,
                        }
                    ],
                    "detail": {},
                }
            ),
            encoding="utf-8",
        )

    calls: list[str] = []
    monkeypatch.setattr(
        evidence_bridge,
        "promote_native_run",
        lambda **kwargs: (
            calls.append(kwargs["family"]),
            {"run_dir": tmp_path, "run_id": kwargs["run_id"], "digest": "sha256:" + "0" * 64},
        )[1],
    )
    monkeypatch.setattr(
        evidence_bridge, "source_identity", lambda *_args, **_kwargs: {"revision": "a" * 40}
    )
    (repo_root / "Cargo.lock").write_text("# fixture\n", encoding="utf-8")
    receipt = repo_root / "preflight.json"
    receipt.write_text(json.dumps({"status": "clean"}), encoding="utf-8")

    MODULE.promote_profile_runs(
        repo_root, "systems", manifest, tmp_path / "evidence", "a" * 40, receipt
    )
    assert calls == ["freshness", "open-loop"], calls
