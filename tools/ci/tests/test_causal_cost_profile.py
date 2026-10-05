"""Independent refusal controls for the optional lifecycle cost profile."""

from __future__ import annotations

from copy import deepcopy

import pytest

from tools.benchmark.retrieval.causal_cost_profile import parse_trace, replay
from tools.benchmark.retrieval.conditional_proof import canonical

SOURCE = "a" * 40
PHASES = (
    "full_ingest", "full_seal", "full_activate", "delta_ingest_seal",
    "delta_activate", "noop_seal", "noop_activate", "delete_seal",
    "delete_activate", "same_process_reopen",
)
SINGLE_TRACE = (
    b"QI_CAUSAL_V1 kind=phase_start name=full_seal\n"
    b"QI_CAUSAL_V1 kind=sync label=atomic_file ok=1 elapsed_ns=400\n"
    b"QI_CAUSAL_V1 kind=exact_live_token_scan ok=1 elapsed_ns=800 terms=2 postings=4 live_postings=3 live_tokens=7\n"
    b"QI_CAUSAL_V1 kind=phase_end name=full_seal ok=1\n"
)
TRACE = b"".join(
    SINGLE_TRACE if name == "full_seal" else (
        f"QI_CAUSAL_V1 kind=phase_start name={name}\n"
        f"QI_CAUSAL_V1 kind=phase_end name={name} ok=1\n"
    ).encode()
    for name in PHASES
)


def _summary() -> dict:
    phase = {
        "cpu_process_user_ms": 1.0, "cpu_process_system_ms": 2.0,
        "observation_span_ms": 10.0,
        "process_write_io": {
            "write_bytes": 4096, "cancelled_write_bytes": 0, "syscw": 2, "wchar": 100,
        },
        "process_write_io_unavailable_reason": None,
    }
    return {
        "schema_version": 2,
        "provenance": {
            "git_head": SOURCE, "corpus_digest": "sha256:" + "c" * 64,
            "config_digest": "sha256:" + "d" * 64,
        },
        "detail": {
            "passed": True,
            "measured_tiers": [{
                "status": "measured", "tier": "large", "seed": 5,
                "file_count": 4096, "source_repo_count": 16,
                "client_request_timeout_ms": 30_000,
                "requested_client_request_timeout_ms": None,
                "history_max_generations": 2,
                "history_max_bytes": 16 * 1024 * 1024,
                "requested_history_max_bytes": None,
                "history_max_revision_pairs": 128,
                "history_max_total_bytes": 256 * 1024 * 1024,
                "phase_resources": {name: deepcopy(phase) for name in PHASES},
            }],
        },
    }


def _manifest() -> dict:
    return {
        "kind": "quanta-index-scale-tier-manifest",
        "manifest_schema_version": 2, "dimension": "scale",
        "query_token": "scale_needle_token",
        "tiers": [
            {
                "tier": name, "repo_count": repos,
                "files_per_repo": per_repo, "total_files": repos * per_repo,
                "source_repo_count": repos, "default_run": name == "small",
                "selectable": True,
            }
            for name, repos, per_repo in (
                ("small", 1, 16), ("medium", 4, 64),
                ("large", 16, 256), ("xlarge", 64, 512),
            )
        ],
    }


def _replay(
    summary: dict | None = None, manifest: dict | None = None, *,
    trace: bytes = TRACE, expected_tier: str = "large", expected_seed: int = 5,
    requested_client_timeout_ms: int | None = None,
    requested_history_max_bytes: int | None = None,
    source_revision: str = SOURCE,
) -> dict:
    return replay(
        canonical(_summary() if summary is None else summary), trace, b"binary",
        canonical(_manifest() if manifest is None else manifest),
        source_revision=source_revision, expected_tier=expected_tier,
        expected_seed=expected_seed,
        requested_client_timeout_ms=requested_client_timeout_ms,
        requested_history_max_bytes=requested_history_max_bytes,
    )


def test_profile_binds_full_lifecycle_and_cost_domains() -> None:
    result = _replay()
    assert result["status"] == "diagnostic_unqualified"
    assert set(result["phases"]) == set(PHASES)
    assert result["file_count"] == 4096
    assert result["runtime_config"]["client_request_timeout_ms"] == 30_000
    phase = result["phases"]["full_seal"]
    assert phase["sync"]["atomic_file"]["elapsed_ns"] == 400
    assert phase["exact_live_token_scan"]["live_tokens"] == 7
    assert phase["process_write_io"]["write_bytes"] == 4096
    assert phase["sync_call_wall_ratio"] == 0.00004


@pytest.mark.parametrize("mutant", [
    SINGLE_TRACE.replace(b"name=full_seal ok=1", b"name=noop_seal ok=1"),
    SINGLE_TRACE + b"QI_CAUSAL_V1 kind=phase_start name=full_seal\n",
    SINGLE_TRACE.replace(b"label=atomic_file", b"label=unknown"),
    SINGLE_TRACE.replace(b"QI_CAUSAL_V1 kind=sync label=atomic_file ok=1 elapsed_ns=400\n", b""),
    SINGLE_TRACE.replace(b"postings=4", b"postings=2"),
    SINGLE_TRACE.replace(b"ok=1 elapsed_ns=400", b"ok=0 elapsed_ns=400"),
])
def test_profile_rejects_unpaired_unknown_or_inconsistent_trace(mutant: bytes) -> None:
    with pytest.raises(ValueError):
        parse_trace(mutant, {"full_seal"})


@pytest.mark.parametrize("key,value", [
    ("tier", "medium"), ("seed", 6), ("seed", True), ("seed", 5.0),
    ("file_count", 4095), ("file_count", 4096.0),
    ("source_repo_count", False), ("source_repo_count", 15),
    ("client_request_timeout_ms", 300_000),
    ("client_request_timeout_ms", 30_000.0),
    ("requested_client_request_timeout_ms", 30_000),
    ("requested_history_max_bytes", 16 * 1024 * 1024),
    ("history_max_bytes", True), ("history_max_generations", 3),
])
def test_profile_refuses_wrong_measured_input_or_policy(key: str, value: object) -> None:
    summary = _summary()
    summary["detail"]["measured_tiers"][0][key] = value
    with pytest.raises(ValueError):
        _replay(summary)


def test_profile_refuses_command_mismatch_and_accepts_exact_override() -> None:
    for kwargs in (
        {"expected_tier": "medium"}, {"expected_seed": 6},
        {"requested_client_timeout_ms": 300_000},
        {"requested_history_max_bytes": 256 * 1024 * 1024},
    ):
        with pytest.raises(ValueError):
            _replay(**kwargs)
    summary = _summary()
    tier = summary["detail"]["measured_tiers"][0]
    tier["client_request_timeout_ms"] = 300_000
    tier["requested_client_request_timeout_ms"] = 300_000
    tier["history_max_bytes"] = 256 * 1024 * 1024
    tier["requested_history_max_bytes"] = 256 * 1024 * 1024
    profile = _replay(
        summary, requested_client_timeout_ms=300_000,
        requested_history_max_bytes=256 * 1024 * 1024,
    )
    assert profile["runtime_config"]["history_max_bytes"] == 256 * 1024 * 1024


def test_profile_refuses_missing_phase_trace_and_manifest_mutants() -> None:
    summary = _summary()
    del summary["detail"]["measured_tiers"][0]["phase_resources"]["delete_seal"]
    with pytest.raises(ValueError):
        _replay(summary)
    absent = TRACE.replace(
        b"QI_CAUSAL_V1 kind=phase_start name=delete_seal\nQI_CAUSAL_V1 kind=phase_end name=delete_seal ok=1\n",
        b"",
    )
    with pytest.raises(ValueError):
        _replay(trace=absent)
    for key, value in (("total_files", 4095), ("repo_count", 16.0), ("tier", "medium")):
        manifest = _manifest()
        manifest["tiers"][2][key] = value
        with pytest.raises(ValueError):
            _replay(manifest=manifest)


def test_profile_refuses_source_and_io_type_alias() -> None:
    with pytest.raises(ValueError):
        _replay(source_revision="b" * 40)
    for invalid in (2.0, True):
        summary = _summary()
        summary["schema_version"] = invalid
        with pytest.raises(ValueError):
            _replay(summary)
    summary = _summary()
    summary["detail"]["measured_tiers"][0]["phase_resources"]["full_seal"]["process_write_io"]["write_bytes"] = 4096.0
    with pytest.raises(ValueError):
        _replay(summary)
