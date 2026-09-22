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


def artifact(dimension: str = "dsl-warm", head: str = HEAD) -> dict:
    return {
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
    assert MODULE.check_envelope(value, dimension="relevance", head=HEAD) == []
    assert any(
        "latency is not an object" in reason
        for reason in MODULE.check_envelope(value, dimension="dsl-warm", head=HEAD)
    )


def test_required_concurrency_profile_needs_all_client_counts(tmp_path: Path) -> None:
    write(
        tmp_path / "artifacts/search-quality/concurrency/latest/summary-c8.json",
        artifact("concurrency"),
    )
    refusals, _, _ = MODULE.check_families(
        tmp_path, (("concurrency", MODULE.FRESH_FAMILIES[7][1]),), head=HEAD, require=True
    )
    assert {refusal.path.name for refusal in refusals} == {"summary-c1.json", "summary-c32.json"}


def test_required_evidence_refuses_failed_verdict_and_early_stop(tmp_path: Path) -> None:
    value = artifact("tail")
    value["detail"]["passed"] = False
    value["rows"][0]["latency"] = None
    value["rows"][0]["early_stop_reason"] = "fixture_missing"
    write(tmp_path / "artifacts/search-quality/tail/latest/summary.json", value)
    refusals, _, _ = MODULE.check_families(
        tmp_path, (("tail", MODULE.FRESH_FAMILIES[3][1]),), head=HEAD, require=True
    )
    assert any("required rail verdict is not true" in refusal.reason for refusal in refusals)
    assert any("contains an early stop" in refusal.reason for refusal in refusals)


def test_authority_sample_floor_and_open_loop_ladder(tmp_path: Path) -> None:
    cold = artifact("dsl-cold")
    cold["rows"][0]["latency"]["samples"] = 19
    write(tmp_path / "artifacts/dsl-bench/cold-matrix.json", cold)
    load = artifact("open-loop")
    load["detail"] = {"passed": True, "duration_ms": 1000, "points": [{}, {}]}
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
    assert any("authority needs >=10 s" in refusal.reason for refusal in refusals)


def test_the_cli_walks_fresh_families_and_baselines(tmp_path: Path, capsys) -> None:
    write(tmp_path / "artifacts/dsl-bench/warm-matrix.json", artifact())
    write(
        tmp_path / "artifacts/search-quality/concurrency/latest/summary-c8.json",
        artifact(dimension="concurrency"),
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
    assert MODULE.main(["--repo-root", str(tmp_path), "--head", "unknown"]) == 2
    assert "not 40 lowercase hex" in capsys.readouterr().err


def test_the_gate_refuses_the_repository_when_a_stale_artifact_is_present(tmp_path: Path) -> None:
    # A copy of a good artifact with one byte of the head changed is stale:
    # the gate is not fooled by an otherwise complete envelope.
    stale = copy.deepcopy(artifact())
    stale["provenance"]["git_head"] = HEAD[:-1] + ("0" if HEAD[-1] != "0" else "1")
    write(
        tmp_path / "artifacts/search-quality/tail/latest/summary.json",
        {**stale, "dimension": "tail"},
    )
    assert MODULE.main(["--repo-root", str(tmp_path), "--head", HEAD, "--skip-baselines"]) == 1
