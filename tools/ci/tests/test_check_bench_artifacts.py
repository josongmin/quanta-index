"""Tests for tools/ci/lint/check-bench-artifacts.py (QI-BB-010, findings §9).

The gate refuses stale, unattributed and old-schema benchmark artifacts:

1. a complete schema-2 artifact at HEAD passes
2. a schema-1 artifact (short `git_rev`, no provenance) is refused as old-schema
3. a fresh artifact whose `git_head` is not HEAD is refused as stale
4. a `git_head` that is not 40 lowercase hex ("unknown", short SHA) is refused
5. a baseline is held to the shape and a full head, not HEAD equality
6. missing envelope / provenance / host / resource / phase / row fields are named
7. absence passes by default and fails under --require
8. the CLI exits 0 / 1 / 2 as documented
9. named profiles require only their explicit artifact families
10. malformed, duplicate and unmeasured rows cannot become attributed evidence
"""

from __future__ import annotations

import copy
import importlib.util
import json
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "ci" / "lint" / "check-bench-artifacts.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("check_bench_artifacts", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["check_bench_artifacts"] = module
    spec.loader.exec_module(module)
    return module


MODULE = _load_module()

HEAD = "0123456789abcdef0123456789abcdef01234567"
OTHER_HEAD = "fedcba9876543210fedcba9876543210fedcba98"
DIGEST = "sha256:" + "ab" * 32


def artifact(dimension: str = "dsl-warm", head: str = HEAD, *, clients: int = 1) -> dict:
    payload = {
        "schema_version": 2,
        "dimension": dimension,
        "mode": "warm",
        "concurrency": 1,
        "provenance": {
            "git_head": head,
            "corpus_digest": DIGEST,
            "config_digest": DIGEST,
            "model_revision": None,
        },
        "host": {
            "os": "linux",
            "arch": "x86_64",
            "cpu_count": 8,
            "mem_bytes": 1 << 34,
            "hostname_hash": DIGEST,
        },
        "resources": {"peak_rss_bytes": 12345},
        "phases": {"build_ms": None, "update_ms": None, "gc_ms": None},
        "disk_amplification": None,
        "rows": [
            {
                "scenario_id": "lexical.keyword.native",
                "route_family": "lexical",
                "syntax": "native",
                "result_shape": "candidates",
                "latency": {"p50_ms": 1.0, "p95_ms": 2.0, "p99_ms": 3.0, "samples": 500},
                "qps": None,
                "error_count": 0,
                "timeout_count": 0,
                "result_count": 3,
                "typed_error_code": None,
                "engine_touched": ["lexical"],
                "early_stop_reason": None,
            }
        ],
        "detail": {"passed": True},
    }

    if dimension == "concurrency":
        assert clients in (1, 8, 32)
        routes = ("lexical", "semantic", "hybrid", "symbol", "lexical_count")
        measurements = []
        for count in (1, 8, 32):

            def group(label: str, requests: int) -> dict:
                return {
                    "label": label,
                    "requests": requests,
                    "served": requests,
                    "error_count": 0,
                    "timeout_count": 0,
                    "qps": requests / 2.0,
                    "latency": {"p50_ms": 1.0, "p95_ms": 2.0, "p99_ms": 3.0, "samples": requests},
                    "error_codes": [],
                    "last_result_count": 3,
                }

            measurements.append(
                {
                    "clients": count,
                    "requests_per_client": 80,
                    "window_secs": 2.0,
                    "routes": [group(label, count * 16) for label in routes],
                    "fast": group("fast", count * 80),
                    "slow": group("slow", 16) if count > 1 else None,
                }
            )
        payload["concurrency"] = clients + int(clients > 1)
        payload["detail"].update(
            {
                "client_counts": [1, 8, 32],
                "mixed_routes": list(routes),
                "minimum_row_samples": 16,
                "maximum_samples_per_worker": 100_000,
                "measurement_timeout_secs": 600,
                "measurements": measurements,
            }
        )
        measurement = next(m for m in measurements if m["clients"] == clients)
        groups = measurement["routes"] + [measurement["fast"]]
        if measurement["slow"] is not None:
            groups.append(measurement["slow"])
        template = payload["rows"][0]
        payload["rows"] = [
            {
                **template,
                "scenario_id": f"concurrency.c{clients}.{g['label']}",
                "route_family": g["label"]
                if g["label"] in ("semantic", "hybrid", "symbol")
                else "lexical",
                "latency": copy.deepcopy(g["latency"]),
                "qps": g["qps"],
                "engine_touched": [],
            }
            for g in groups
        ]
    return payload


def schema_one_artifact() -> dict:
    return {
        "schema_version": 1,
        "mode": "warm",
        "git_rev": "3148c02",
        "rows": [],
    }


def write(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload) + "\n", encoding="utf-8")


def install_manifest(repo_root: Path) -> None:
    """A target checkout owns its benchmark control plane."""
    destination = repo_root / "tools" / "benchmark" / "registry.toml"
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(
        (REPO_ROOT / "tools" / "benchmark" / "registry.toml").read_text(encoding="utf-8"),
        encoding="utf-8",
    )
    (repo_root / "Justfile").write_text(
        (REPO_ROOT / "Justfile").read_text(encoding="utf-8"), encoding="utf-8"
    )


def test_a_complete_artifact_at_head_passes() -> None:
    assert MODULE.check_envelope(artifact(), dimension="dsl-warm", head=HEAD) == []


def test_a_schema_one_artifact_is_refused_as_old_schema() -> None:
    reasons = MODULE.check_envelope(schema_one_artifact(), dimension="dsl-warm", head=HEAD)
    assert any("schema_version 1 is not 2" in reason for reason in reasons), reasons


def test_a_fresh_artifact_not_at_head_is_stale() -> None:
    reasons = MODULE.check_envelope(artifact(head=OTHER_HEAD), dimension="dsl-warm", head=HEAD)
    assert reasons == [f"git_head {OTHER_HEAD} is not HEAD {HEAD}: stale artifact"]


def test_an_unknown_or_short_head_is_refused_even_for_a_baseline() -> None:
    for bad in ("unknown", "3148c02", HEAD.upper(), ""):
        reasons = MODULE.check_envelope(artifact(head=bad), dimension="dsl-warm", head=None)
        assert any("is not 40 lowercase hex" in reason for reason in reasons), (bad, reasons)


def test_a_baseline_is_held_to_the_shape_not_head_equality() -> None:
    assert MODULE.check_envelope(artifact(head=OTHER_HEAD), dimension="dsl-warm", head=None) == []


def test_canonical_linux_artifact_refuses_a_non_linux_host() -> None:
    payload = artifact()
    payload["host"]["os"] = "darwin"

    reasons = MODULE.check_envelope(
        payload,
        dimension="dsl-warm",
        head=HEAD,
        host_policy="canonical-linux",
    )

    assert "canonical-linux artifact was not measured on Linux" in reasons


def test_missing_fields_are_named() -> None:
    broken = artifact()
    del broken["provenance"]["corpus_digest"]
    del broken["host"]["mem_bytes"]
    del broken["resources"]
    del broken["phases"]["gc_ms"]
    del broken["rows"][0]["qps"]
    del broken["rows"][0]["latency"]["p99_ms"]
    reasons = MODULE.check_envelope(broken, dimension="dsl-warm", head=HEAD)
    for fragment in (
        "provenance is missing `corpus_digest`",
        "host is missing `mem_bytes`",
        "envelope is missing `resources`",
        "phases is missing `gc_ms`",
        "rows[0] is missing `qps`",
        "rows[0].latency is missing `p99_ms`",
    ):
        assert any(fragment in reason for reason in reasons), (fragment, reasons)


def test_a_wrong_dimension_a_bad_digest_and_empty_rows_are_refused() -> None:
    wrong = artifact(dimension="tail")
    reasons = MODULE.check_envelope(wrong, dimension="dsl-warm", head=HEAD)
    assert any("dimension 'tail' is not 'dsl-warm'" in reason for reason in reasons), reasons
    bad_digest = artifact()
    bad_digest["provenance"]["config_digest"] = "md5:abc"
    reasons = MODULE.check_envelope(bad_digest, dimension="dsl-warm", head=HEAD)
    assert any("config_digest" in reason and "sha256" in reason for reason in reasons), reasons
    empty = artifact()
    empty["rows"] = []
    reasons = MODULE.check_envelope(empty, dimension="dsl-warm", head=HEAD)
    assert "rows is empty: nothing was measured" in reasons


def test_exact_producer_shape_and_numeric_envelope_contract_are_enforced() -> None:
    broken = artifact()
    broken["unexpected"] = True
    broken["provenance"]["foreign"] = "ignored-by-old-gate"
    broken["host"]["hostname_hash"] = "anonymous"
    broken["phases"]["build_ms"] = float("inf")
    broken["detail"] = []
    broken["disk_amplification"] = {
        "bytes_written": 9,
        "changed_bytes": 4,
        "ratio": 2.0,
        "extra": True,
    }
    reasons = MODULE.check_envelope(broken, dimension="dsl-warm", head=HEAD)
    for fragment in (
        "envelope has unexpected `unexpected`",
        "provenance has unexpected `foreign`",
        "host.hostname_hash 'anonymous' is not a sha256: digest",
        "phases.build_ms is not a finite non-negative number or null",
        "detail is not an object",
        "disk_amplification has unexpected `extra`",
        "disk_amplification.ratio does not equal bytes_written / changed_bytes",
    ):
        assert fragment in reasons, (fragment, reasons)


def test_zero_change_disk_amplification_requires_null_ratio() -> None:
    broken = artifact()
    broken["disk_amplification"] = {
        "bytes_written": 9,
        "changed_bytes": 0,
        "ratio": 0.0,
    }
    reasons = MODULE.check_envelope(broken, dimension="dsl-warm", head=HEAD)
    assert "disk_amplification.ratio must be null when changed_bytes is zero" in reasons


def test_row_contract_refuses_duplicate_invalid_percentile_and_bad_early_stop() -> None:
    broken = artifact()
    duplicate = copy.deepcopy(broken["rows"][0])
    broken["rows"].append(duplicate)
    broken["rows"][0]["latency"] = {
        "p50_ms": 3.0,
        "p95_ms": 2.0,
        "p99_ms": 1.0,
        "samples": 0,
    }
    broken["rows"][0]["early_stop_reason"] = "fixture_missing"
    reasons = MODULE.check_envelope(broken, dimension="dsl-warm", head=HEAD)
    assert any("scenario_id 'lexical.keyword.native' is duplicated" in reason for reason in reasons)
    assert any("latency must be null" in reason for reason in reasons)


def test_absence_passes_by_default_and_fails_under_require(tmp_path: Path) -> None:
    refusals, checked, absent = MODULE.check_families(
        tmp_path, MODULE.FRESH_FAMILIES, head=HEAD, require=False
    )
    assert refusals == [] and checked == []
    assert set(absent) == {name for name, _ in MODULE.FRESH_FAMILIES}
    refusals, _, _ = MODULE.check_families(tmp_path, MODULE.FRESH_FAMILIES, head=HEAD, require=True)
    assert len(refusals) == len(MODULE.FRESH_FAMILIES)


def test_named_profile_scopes_required_evidence(tmp_path: Path, capsys) -> None:
    install_manifest(tmp_path)
    write(tmp_path / "artifacts/dsl-bench/warm-matrix.json", artifact())
    write(tmp_path / "artifacts/dsl-bench/cold-matrix.json", artifact("dsl-cold"))
    assert (
        MODULE.main(
            [
                "--repo-root",
                str(tmp_path),
                "--head",
                HEAD,
                "--profile",
                "dsl-authority",
                "--require",
                "--skip-baselines",
            ]
        )
        == 0
    )
    out = capsys.readouterr().out
    assert "checked artifacts/dsl-bench/warm-matrix.json" in out
    assert "absent  scale" not in out


def test_relevance_rows_are_intentionally_untimed() -> None:
    value = artifact(dimension="relevance")
    value["rows"][0]["latency"] = None
    value["provenance"]["model_revision"] = MODULE.POTION_CODE_MODEL_REVISION
    value["detail"]["semantic_quality"] = {"case_count": 12, "passed": True}
    value["rows"].extend(
        {
            **value["rows"][0],
            "scenario_id": f"relevance.semantic.sem.case_{index}.paraphrase",
            "route_family": "semantic",
        }
        for index in range(12)
    )
    assert MODULE.check_envelope(value, dimension="relevance", head=HEAD) == []
    assert any(
        "latency is not an object" in reason
        for reason in MODULE.check_envelope(value, dimension="dsl-warm", head=HEAD)
    )


def test_relevance_cannot_admit_hash_dev_as_canonical_quality() -> None:
    value = artifact(dimension="relevance")
    value["rows"][0]["latency"] = None
    value["provenance"]["model_revision"] = "hash-dev:v1:d256"
    value["detail"]["semantic_quality"] = None
    reasons = MODULE.check_envelope(value, dimension="relevance", head=HEAD)
    assert any("requires potion-code" in reason for reason in reasons)
    assert any("requires 12 judged paraphrase" in reason for reason in reasons)
    assert any("requires 12 distinct paraphrase rows" in reason for reason in reasons)


def test_contract_quality_rails_are_intentionally_untimed_but_verdict_bound() -> None:
    for dimension in ("ambiguity", "snippet", "ops", "ui"):
        value = artifact(dimension=dimension)
        value["rows"][0]["latency"] = None
        assert MODULE.check_envelope(value, dimension=dimension, head=HEAD) == []


def test_required_concurrency_profile_needs_all_client_counts(tmp_path: Path) -> None:
    write(
        tmp_path / "artifacts/search-quality/concurrency/latest/summary-c8.json",
        artifact("concurrency", clients=8),
    )
    family_paths = dict(MODULE.FRESH_FAMILIES)
    refusals, _, _ = MODULE.check_families(
        tmp_path, (("concurrency", family_paths["concurrency"]),), head=HEAD, require=True
    )
    assert {refusal.path.name for refusal in refusals} == {"summary-c1.json", "summary-c32.json"}


def test_required_evidence_refuses_failed_verdict_and_early_stop(tmp_path: Path) -> None:
    value = artifact("tail")
    value["detail"]["passed"] = False
    value["rows"][0]["latency"] = None
    value["rows"][0]["early_stop_reason"] = "fixture_missing"
    write(tmp_path / "artifacts/search-quality/tail/latest/summary.json", value)
    family_paths = dict(MODULE.FRESH_FAMILIES)
    refusals, _, _ = MODULE.check_families(
        tmp_path, (("tail", family_paths["tail"]),), head=HEAD, require=True
    )
    assert any("required rail verdict is not true" in refusal.reason for refusal in refusals)
    assert any("contains an early stop" in refusal.reason for refusal in refusals)


def test_new_quality_family_requires_its_manifest_verdict(tmp_path: Path) -> None:
    manifest = copy.deepcopy(MODULE.MANIFEST)
    manifest["families"]["new-quality"] = {
        "dimension": "new-quality",
        "artifact_glob": "artifacts/search-quality/new-quality/latest/summary.json",
        "producer": "rust-verify-quality-new",
        "minimum_samples": None,
        "host_policy": "local-diagnostic",
        "baseline": None,
        "requires_verdict": True,
    }
    value = artifact("new-quality")
    value["detail"] = {}
    write(tmp_path / "artifacts/search-quality/new-quality/latest/summary.json", value)

    refusals, _, _ = MODULE.check_families(
        tmp_path,
        (("new-quality", manifest["families"]["new-quality"]["artifact_glob"]),),
        head=HEAD,
        require=True,
        manifest=manifest,
    )
    assert any("required rail verdict is not true" in refusal.reason for refusal in refusals)


def test_authority_sample_floor_and_open_loop_ladder(tmp_path: Path) -> None:
    cold = artifact("dsl-cold")
    cold["rows"][0]["latency"]["samples"] = 19
    write(tmp_path / "artifacts/dsl-bench/cold-matrix.json", cold)
    load = artifact("open-loop")
    load["detail"] = {
        "passed": True,
        "arrival_model": "deterministic_periodic",
        "duration_ms": 1000,
        "points": [{}, {}],
    }
    write(tmp_path / "artifacts/search-quality/open-loop/latest/summary.json", load)
    refusals, _, _ = MODULE.check_families(
        tmp_path,
        (
            ("dsl-cold", dict(MODULE.FRESH_FAMILIES)["dsl-cold"]),
            ("open-loop", dict(MODULE.FRESH_FAMILIES)["open-loop"]),
        ),
        head=HEAD,
        require=True,
    )
    assert any("needs at least 20 samples" in refusal.reason for refusal in refusals)
    assert any("authority needs seeded_poisson" in refusal.reason for refusal in refusals)


def test_duplicate_native_verdict_is_refused(tmp_path: Path) -> None:
    path = tmp_path / "artifact.json"
    raw = json.dumps(artifact("scale")).replace('"passed": true', '"passed": false, "passed": true')
    path.write_text(raw, encoding="utf-8")
    refusals, checked, _ = MODULE.check_families(
        tmp_path, (("scale", "artifact.json"),), head=HEAD, require=True
    )
    assert checked == [path]
    assert any("duplicate" in refusal.reason for refusal in refusals)


@pytest.mark.parametrize("dimension,count", [("scale", 2), ("concurrency", 4)])
def test_native_inventory_limit_refuses_before_parsing(tmp_path, monkeypatch, dimension, count):
    for index in range(count):
        (tmp_path / f"summary-{index}.json").write_bytes(b"{}")

    def forbidden(_raw):
        pytest.fail("out-of-contract inventory reached JSON decoding")

    monkeypatch.setattr(MODULE, "parse_artifact_bytes", forbidden)
    refusals, checked, absent = MODULE.check_families(
        tmp_path, ((dimension, "summary-*.json"),), head=HEAD, require=True
    )
    assert checked == absent == []
    assert len(refusals) == 1
    assert "inventory exceeds registered count" in refusals[0].reason


def test_native_control_limit_refuses_before_json_decode(tmp_path, monkeypatch):
    path = tmp_path / "summary.json"
    with path.open("wb") as stream:
        stream.truncate(MODULE.CONTROL_DOCUMENT_BYTES + 1)

    def forbidden(_raw):
        pytest.fail("oversized control document reached JSON decoding")

    monkeypatch.setattr(MODULE, "parse_artifact_bytes", forbidden)
    refusals, checked, absent = MODULE.check_families(
        tmp_path, (("scale", "summary.json"),), head=HEAD, require=True
    )
    assert checked == [path] and absent == []
    assert len(refusals) == 1
    assert "control document exceeds" in refusals[0].reason


@pytest.mark.parametrize("alias", ["leaf", "parent"])
def test_native_control_reader_refuses_symlink_alias(tmp_path, alias):
    real = tmp_path / "real"
    real.mkdir()
    write(real / "summary.json", artifact("scale"))
    link = tmp_path / "link"
    link.symlink_to(real / "summary.json" if alias == "leaf" else real)
    pattern = "link" if alias == "leaf" else "link/summary.json"
    refusals, _, _ = MODULE.check_families(tmp_path, (("scale", pattern),), head=HEAD, require=True)
    assert len(refusals) == 1 and "symlink" in refusals[0].reason


def test_the_cli_walks_fresh_families_and_baselines(tmp_path: Path, capsys) -> None:
    install_manifest(tmp_path)
    write(tmp_path / "artifacts/dsl-bench/warm-matrix.json", artifact())
    write(
        tmp_path / "artifacts/search-quality/concurrency/latest/summary-c8.json",
        artifact(dimension="concurrency", clients=8),
    )
    write(tmp_path / "tools/benchmark/baselines/cold-matrix.json", artifact("dsl-cold", OTHER_HEAD))
    assert MODULE.main(["--repo-root", str(tmp_path), "--head", HEAD]) == 0
    out = capsys.readouterr().out
    assert "checked artifacts/dsl-bench/warm-matrix.json" in out
    assert "checked artifacts/search-quality/concurrency/latest/summary-c8.json" in out
    assert "checked tools/benchmark/baselines/cold-matrix.json" in out
    assert "absent  scale" in out

    # A stale fresh artifact refuses the gate; the baseline at another head does not.
    write(tmp_path / "artifacts/dsl-bench/warm-matrix.json", artifact(head=OTHER_HEAD))
    assert MODULE.main(["--repo-root", str(tmp_path), "--head", HEAD]) == 1
    err = capsys.readouterr().err
    assert "stale artifact" in err and "warm-matrix.json" in err

    # An old-schema baseline refuses the gate too.
    write(tmp_path / "artifacts/dsl-bench/warm-matrix.json", artifact())
    write(tmp_path / "tools/benchmark/baselines/cold-matrix.json", schema_one_artifact())
    assert MODULE.main(["--repo-root", str(tmp_path), "--head", HEAD]) == 1
    err = capsys.readouterr().err
    assert "schema_version 1 is not 2" in err
    assert MODULE.main(["--repo-root", str(tmp_path), "--head", HEAD, "--skip-baselines"]) == 0
    capsys.readouterr()


def test_the_cli_refuses_a_malformed_head(tmp_path: Path, capsys) -> None:
    install_manifest(tmp_path)
    assert MODULE.main(["--repo-root", str(tmp_path), "--head", "unknown"]) == 2
    assert "not 40 lowercase hex" in capsys.readouterr().err


def test_baseline_checker_refuses_interrupted_pair(tmp_path: Path, capsys) -> None:
    install_manifest(tmp_path)
    marker = tmp_path / "tools/benchmark/baselines/.dsl-admission-pending"
    marker.parent.mkdir(parents=True)
    marker.write_text("interrupted", encoding="utf-8")
    assert MODULE.main(["--repo-root", str(tmp_path), "--head", HEAD]) == 1
    assert "baseline admission is incomplete" in capsys.readouterr().err


def test_clean_worktree_requirement_refuses_any_git_status_output(
    monkeypatch, tmp_path: Path
) -> None:
    class Completed:
        returncode = 0
        stdout = " M crates/owner.rs\n"
        stderr = ""

    monkeypatch.setattr(MODULE.subprocess, "run", lambda *_args, **_kwargs: Completed())
    try:
        MODULE.require_clean_worktree(tmp_path)
    except RuntimeError as exc:
        assert "worktree is dirty" in str(exc)
    else:
        raise AssertionError("dirty worktree unexpectedly qualified")


def test_the_gate_refuses_the_repository_when_a_stale_artifact_is_present(tmp_path: Path) -> None:
    install_manifest(tmp_path)
    # A copy of a good artifact with one byte of the head changed is stale:
    # the gate is not fooled by an otherwise complete envelope.
    stale = copy.deepcopy(artifact())
    stale["provenance"]["git_head"] = HEAD[:-1] + ("0" if HEAD[-1] != "0" else "1")
    write(
        tmp_path / "artifacts/search-quality/tail/latest/summary.json",
        {**stale, "dimension": "tail"},
    )
    assert MODULE.main(["--repo-root", str(tmp_path), "--head", HEAD, "--skip-baselines"]) == 1


def test_concurrency_complete_public_inventory_passes() -> None:
    for clients in (1, 8, 32):
        value = artifact("concurrency", clients=clients)
        assert (
            MODULE.check_artifact(
                value,
                dimension="concurrency",
                head=HEAD,
                require=True,
                artifact_path=Path(f"summary-c{clients}.json"),
            )
            == []
        )


def test_concurrency_partial_rows_cannot_hide_behind_passed_detail() -> None:
    value = artifact("concurrency", clients=8)
    value["rows"] = [value["rows"][5]]
    reasons = MODULE.check_artifact(value, dimension="concurrency", head=HEAD, require=True)
    assert any("row inventory mismatch" in reason for reason in reasons), reasons


def test_concurrency_rows_are_bound_to_actual_measurement() -> None:
    for key, bad in (
        ("qps", 123.0),
        ("error_count", 1),
        ("timeout_count", 1),
        ("result_count", 99),
        ("route_family", "foreign"),
    ):
        value = artifact("concurrency", clients=8)
        value["rows"][0][key] = bad
        reasons = MODULE.check_artifact(value, dimension="concurrency", head=HEAD, require=True)
        assert any(f"{key} disagrees with detail" in r for r in reasons), reasons
    value = artifact("concurrency", clients=8)
    value["rows"][0]["latency"]["samples"] += 1
    assert any(
        "latency disagrees with detail" in r
        for r in MODULE.check_artifact(value, dimension="concurrency", head=HEAD, require=True)
    )


def test_concurrency_detail_tallies_cannot_hide_behind_good_rows() -> None:
    for key, bad in (
        ("requests", 129),
        ("served", 127),
        ("qps", 1.0),
        ("error_count", 1),
        ("timeout_count", 1),
    ):
        value = artifact("concurrency", clients=8)
        value["detail"]["measurements"][1]["routes"][0][key] = bad
        assert MODULE.check_artifact(value, dimension="concurrency", head=HEAD, require=True)
    value = artifact("concurrency", clients=1)
    value["detail"]["measurements"][2]["fast"]["latency"]["samples"] += 1
    reasons = MODULE.check_artifact(value, dimension="concurrency", head=HEAD, require=True)
    assert any("answered samples mismatch" in r for r in reasons), reasons


def test_concurrency_inventory_envelope_filename_and_order_are_bound() -> None:
    value = artifact("concurrency", clients=8)
    assert any(
        "filename disagrees" in r
        for r in MODULE.check_artifact(
            value,
            dimension="concurrency",
            head=HEAD,
            require=True,
            artifact_path=Path("summary-c32.json"),
        )
    )
    value["concurrency"] = 8
    assert any(
        "envelope disagrees" in r
        for r in MODULE.check_artifact(value, dimension="concurrency", head=HEAD, require=True)
    )
    value = artifact("concurrency", clients=8)
    value["rows"][0], value["rows"][1] = value["rows"][1], value["rows"][0]
    assert any(
        "row inventory mismatch" in r
        for r in MODULE.check_artifact(value, dimension="concurrency", head=HEAD, require=True)
    )
    value = artifact("concurrency", clients=8)
    value["detail"]["measurements"].pop()
    assert any(
        "measurement client inventory" in r
        for r in MODULE.check_artifact(value, dimension="concurrency", head=HEAD, require=True)
    )


def test_concurrency_malformed_detail_is_refused_without_exception() -> None:
    for malformed in (None, False, "wrong", [], {}):
        value = artifact("concurrency", clients=8)
        value["detail"]["measurements"][1]["routes"][0] = malformed
        assert MODULE.check_artifact(value, dimension="concurrency", head=HEAD, require=True)
    value = artifact("concurrency", clients=8)
    value["detail"]["measurements"][1]["slow"]["label"] = {}
    assert MODULE.check_artifact(value, dimension="concurrency", head=HEAD, require=True)


def test_concurrency_missing_static_budget_and_fields_are_refused() -> None:
    for key in ("maximum_samples_per_worker", "measurement_timeout_secs"):
        value = artifact("concurrency")
        del value["detail"][key]
        assert MODULE.check_artifact(value, dimension="concurrency", head=HEAD, require=True)
    value = artifact("concurrency")
    del value["detail"]["measurements"][0]["slow"]
    assert any(
        "missing fields" in reason
        for reason in MODULE.check_artifact(value, dimension="concurrency", head=HEAD, require=True)
    )


def test_concurrency_forged_round_robin_and_slow_budget_are_refused() -> None:
    value = artifact("concurrency", clients=8)
    measurement = value["detail"]["measurements"][1]
    measurement["routes"][0]["requests"] -= 1
    measurement["routes"][1]["requests"] += 1
    assert any(
        "round-robin partition" in reason
        for reason in MODULE.check_artifact(value, dimension="concurrency", head=HEAD, require=True)
    )
    value = artifact("concurrency", clients=8)
    slow = value["detail"]["measurements"][1]["slow"]
    slow.update(requests=100_001, served=100_001, qps=50_000.5)
    slow["latency"]["samples"] = 100_001
    row = value["rows"][-1]
    row.update(latency=copy.deepcopy(slow["latency"]), qps=slow["qps"])
    assert any(
        "counters must be" in reason
        for reason in MODULE.check_artifact(value, dimension="concurrency", head=HEAD, require=True)
    )


def test_concurrency_conserved_but_undercovered_partition_is_refused() -> None:
    value = artifact("concurrency", clients=8)
    measurement = value["detail"]["measurements"][1]
    for group, row, requests in zip(
        measurement["routes"], value["rows"], (129, 128, 128, 128, 127)
    ):
        group.update(requests=requests, served=requests, qps=requests / 2.0)
        group["latency"]["samples"] = requests
        row.update(latency=copy.deepcopy(group["latency"]), qps=group["qps"])
    reasons = MODULE.check_artifact(value, dimension="concurrency", head=HEAD, require=True)
    assert any("planned round-robin share" in reason for reason in reasons), reasons


def test_concurrency_extension_counts_are_allowed_when_conserved() -> None:
    value = artifact("concurrency", clients=8)
    measurement = value["detail"]["measurements"][1]
    for group, row in zip(measurement["routes"], value["rows"]):
        group.update(requests=136, served=136, qps=68.0)
        group["latency"]["samples"] = 136
        row.update(latency=copy.deepcopy(group["latency"]), qps=68.0)
    measurement["fast"].update(requests=680, served=680, qps=340.0)
    measurement["fast"]["latency"]["samples"] = 680
    value["rows"][5].update(latency=copy.deepcopy(measurement["fast"]["latency"]), qps=340.0)
    assert MODULE.check_artifact(value, dimension="concurrency", head=HEAD, require=True) == []


def test_concurrency_error_codes_cannot_outnumber_typed_error_responses() -> None:
    # Each typed error response carries one code; summaries deduplicate those codes.
    for codes in (["INVALID_REQUEST"], ["INVALID_REQUEST", "SERVER_OVERLOADED"]):
        value = artifact("concurrency")
        measurement = value["detail"]["measurements"][0]
        for group, row in (
            (measurement["routes"][0], value["rows"][0]),
            (measurement["fast"], value["rows"][5]),
        ):
            group.update(
                error_count=1,
                served=group["requests"] - 1,
                qps=(group["requests"] - 1) / measurement["window_secs"],
                error_codes=codes,
            )
            row.update(error_count=1, qps=group["qps"], typed_error_code=codes[0])
        reasons = MODULE.check_artifact(value, dimension="concurrency", head=HEAD, require=True)
        if len(codes) == 1:
            assert reasons == []
        else:
            assert any("outnumber" in reason for reason in reasons), reasons


def test_native_rows_reject_unknown_public_enum_variants() -> None:
    for key in ("route_family", "syntax", "result_shape"):
        value = artifact()
        value["rows"][0]["latency"]["samples"] = 10_000
        assert MODULE.check_artifact(value, dimension="dsl-warm", head=HEAD, require=True) == []
        value["rows"][0][key] = "not_a_public_variant"
        reasons = MODULE.check_artifact(value, dimension="dsl-warm", head=HEAD, require=True)
        assert any(f"{key} is not a public variant" in reason for reason in reasons), reasons


def test_every_public_native_row_enum_tag_is_accepted() -> None:
    # Fixed public serialization oracle from artifact.rs, not imported validator domains.
    domains = {
        "route_family": (
            "lexical",
            "semantic",
            "hybrid",
            "symbol",
            "repomap",
            "history",
            "runtime_catalog",
            "structural",
            "adversarial",
        ),
        "syntax": ("native", "sourcegraph"),
        "result_shape": ("candidates", "commits", "diff_paths", "typed_error", "empty"),
    }
    for key, tags in domains.items():
        for tag in tags:
            value = artifact()
            value["rows"][0][key] = tag
            assert MODULE.check_envelope(value, dimension="dsl-warm", head=HEAD) == []


def test_native_unsigned_scalar_widths_match_public_rust_dto() -> None:
    # Rust DTO types are the oracle; positive envelope fields retain their existing lower bound.
    fields = (
        (("concurrency",), 32, 1),
        (("host", "cpu_count"), 32, 1),
        (("host", "mem_bytes"), 64, 1),
        (("resources", "peak_rss_bytes"), 64, 1),
        (("rows", 0, "error_count"), 64, 0),
        (("rows", 0, "timeout_count"), 64, 0),
        (("rows", 0, "result_count"), 64, 0),
        (("rows", 0, "latency", "samples"), 32, 1),
        (("disk_amplification", "bytes_written"), 64, 0),
        (("disk_amplification", "changed_bytes"), 64, 0),
    )
    for path, bits, minimum in fields:
        for scalar, allowed in (
            (minimum, True),
            ((1 << bits) - 1, True),
            (minimum - 1, False),
            (1 << bits, False),
            (True, False),
            (1.0, False),
        ):
            value = artifact()
            if path[0] == "disk_amplification":
                value["disk_amplification"] = {"bytes_written": 3, "changed_bytes": 1, "ratio": 3.0}
            parent = value
            for component in path[:-1]:
                parent = parent[component]
            parent[path[-1]] = scalar
            if path[0] == "disk_amplification":
                disk = value["disk_amplification"]
                disk["ratio"] = (
                    float(disk["bytes_written"]) / float(disk["changed_bytes"])
                    if disk["changed_bytes"]
                    else None
                )
            reasons = MODULE.check_envelope(value, dimension="dsl-warm", head=HEAD)
            assert (reasons == []) == allowed, (path, scalar, reasons)


def test_native_disk_ratio_uses_public_rust_widening_before_division() -> None:
    value = artifact()
    # The Rust DTO casts each operand to f64 first; integer rational division differs here.
    value["disk_amplification"] = {
        "bytes_written": (1 << 53) + 1,
        "changed_bytes": 3,
        "ratio": float((1 << 53) + 1) / float(3),
    }
    assert MODULE.check_envelope(value, dimension="dsl-warm", head=HEAD) == []


def test_native_f64_values_reject_overflow_before_conversion() -> None:
    fields = (
        ("rows", 0, "latency", "p50_ms"),
        ("rows", 0, "latency", "p95_ms"),
        ("rows", 0, "latency", "p99_ms"),
        ("rows", 0, "qps"),
        ("phases", "build_ms"),
        ("phases", "update_ms"),
        ("phases", "gc_ms"),
    )
    for path in fields:
        for invalid in (10**400, float("inf"), float("nan"), True, -1):
            value = artifact()
            parent = value
            for component in path[:-1]:
                parent = parent[component]
            parent[path[-1]] = invalid
            reasons = MODULE.check_envelope(value, dimension="dsl-warm", head=HEAD)
            assert any("finite non-negative" in reason for reason in reasons), (path, reasons)
    value = artifact()
    for key in ("p50_ms", "p95_ms", "p99_ms"):
        value["rows"][0]["latency"][key] = sys.float_info.max
    value["rows"][0]["qps"] = sys.float_info.max
    for key in ("build_ms", "update_ms", "gc_ms"):
        value["phases"][key] = sys.float_info.max
    assert MODULE.check_envelope(value, dimension="dsl-warm", head=HEAD) == []


def test_unselected_concurrency_result_count_still_obeys_u64_domain() -> None:
    for scalar, allowed in (((1 << 64) - 1, True), (1 << 64, False)):
        value = artifact("concurrency")
        value["detail"]["measurements"][2]["routes"][0]["last_result_count"] = scalar
        reasons = MODULE.check_artifact(value, dimension="concurrency", head=HEAD, require=True)
        assert (reasons == []) == allowed, reasons


def test_native_source_and_host_digests_are_exact_without_trailing_newline() -> None:
    for path in (
        ("provenance", "git_head"),
        ("provenance", "corpus_digest"),
        ("provenance", "config_digest"),
        ("host", "hostname_hash"),
    ):
        value = artifact()
        assert MODULE.check_envelope(value, dimension="dsl-warm", head=None) == []
        value[path[0]][path[1]] += "\n"
        reasons = MODULE.check_envelope(value, dimension="dsl-warm", head=None)
        assert any(path[1] in reason for reason in reasons), reasons
