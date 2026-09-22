"""Registry tests for the benchmark CLI authority mapping."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from types import SimpleNamespace
from pathlib import Path

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


def test_manifest_refuses_profile_that_omits_a_runnable_family_producer(tmp_path: Path) -> None:
    manifest = json.loads((REPO_ROOT / "tools/benchmark/manifest.json").read_text(encoding="utf-8"))
    manifest["profiles"]["quality-core"]["recipes"].pop()
    path = tmp_path / "manifest.json"
    path.write_text(json.dumps(manifest), encoding="utf-8")

    try:
        MODULE.load_manifest(path)
    except MODULE.ManifestError as error:
        assert "must exactly name each runnable family producer" in str(error)
    else:
        raise AssertionError("manifest with a missing producer recipe was accepted")


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


def test_preflight_requires_linux_before_a_canonical_profile_runs(monkeypatch, tmp_path: Path) -> None:
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
