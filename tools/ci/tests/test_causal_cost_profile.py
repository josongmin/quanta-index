"""Independent refusal controls for the optional lifecycle cost profile."""

from __future__ import annotations

from copy import deepcopy

import pytest

from tools.benchmark.retrieval.causal_cost_profile import parse_trace, replay
from tools.benchmark.retrieval.conditional_proof import canonical

SOURCE = "a" * 40
PHASES = (
    "full_ingest",
    "full_seal",
    "full_activate",
    "delta_ingest_seal",
    "delta_activate",
    "noop_seal",
    "noop_activate",
    "delete_seal",
    "delete_activate",
    "same_process_reopen",
)
BM25_FULL = (
    b"QI_CAUSAL_V1 kind=bm25_live_build ok=1 elapsed_ns=1000 "
    b"reused_segments=0 changed_segments=0 new_segments=1 mask_docs=0 "
    b"new_census_docs=2 newly_dead_docs=0 logical_census_bytes=30 "
    b"changed_ns=0 new_ns=700 death_ns=0 correction_keys=0 "
    b"segment_fanout=1 retained_valid=1 retained_estimate_bytes=4096\n"
)
BM25_DELTA = (
    b"QI_CAUSAL_V1 kind=bm25_live_build ok=1 elapsed_ns=1200 "
    b"reused_segments=0 changed_segments=1 new_segments=1 mask_docs=2 "
    b"new_census_docs=1 newly_dead_docs=1 logical_census_bytes=40 "
    b"changed_ns=500 new_ns=500 death_ns=200 correction_keys=2 "
    b"segment_fanout=2 retained_valid=1 retained_estimate_bytes=5000\n"
)
BM25_REUSE = (
    b"QI_CAUSAL_V1 kind=bm25_live_build ok=1 elapsed_ns=300 "
    b"reused_segments=1 changed_segments=0 new_segments=0 mask_docs=0 "
    b"new_census_docs=0 newly_dead_docs=0 logical_census_bytes=0 "
    b"changed_ns=0 new_ns=0 death_ns=0 correction_keys=0 "
    b"segment_fanout=1 retained_valid=1 retained_estimate_bytes=4096\n"
)
BM25_ENCODE = b"QI_CAUSAL_V1 kind=bm25_boundary label=encode ok=1 elapsed_ns=70 logical_bytes=128 index_files=1\n"
BM25_BASE = b"QI_CAUSAL_V1 kind=bm25_boundary label=base_read ok=1 elapsed_ns=90 logical_bytes=256 index_files=1\n"
BM25_CENSUS = b"QI_CAUSAL_V1 kind=bm25_census_build ok=1 elapsed_ns=80 docs=2 encoded_bytes=32\n"
SEAL_PHASES = {"full_seal", "delta_ingest_seal", "noop_seal", "delete_seal"}


def _trace_phase(name: str) -> bytes:
    rows = [f"QI_CAUSAL_V1 kind=phase_start name={name}\n".encode()]
    if name == "full_ingest":
        rows.append(BM25_CENSUS)
    if name == "full_seal":
        rows.extend((b"QI_CAUSAL_V1 kind=sync label=atomic_file ok=1 elapsed_ns=400\n", BM25_FULL))
        rows.append(
            b"QI_CAUSAL_V1 kind=exact_live_token_scan ok=1 elapsed_ns=800 terms=2 postings=4 live_postings=3 live_tokens=7\n"
        )
    elif name in SEAL_PHASES:
        rows.extend((BM25_BASE, BM25_REUSE if name == "noop_seal" else BM25_DELTA))
    if name in SEAL_PHASES:
        rows.append(BM25_ENCODE)
    rows.append(f"QI_CAUSAL_V1 kind=phase_end name={name} ok=1\n".encode())
    return b"".join(rows)


SINGLE_TRACE = _trace_phase("full_seal")
TRACE = b"".join(_trace_phase(name) for name in PHASES)


def _summary() -> dict:
    phase = {
        "cpu_process_user_ms": 1.0,
        "cpu_process_system_ms": 2.0,
        "observation_span_ms": 10.0,
        "process_write_io": {
            "write_bytes": 4096,
            "cancelled_write_bytes": 0,
            "syscw": 2,
            "wchar": 100,
        },
        "process_write_io_unavailable_reason": None,
    }
    return {
        "schema_version": 2,
        "provenance": {
            "git_head": SOURCE,
            "corpus_digest": "sha256:" + "c" * 64,
            "config_digest": "sha256:" + "d" * 64,
        },
        "detail": {
            "passed": True,
            "measured_tiers": [
                {
                    "status": "measured",
                    "tier": "large",
                    "seed": 5,
                    "file_count": 4096,
                    "source_repo_count": 16,
                    "client_request_timeout_ms": 30_000,
                    "requested_client_request_timeout_ms": None,
                    "history_max_generations": 2,
                    "history_max_bytes": 16 * 1024 * 1024,
                    "requested_history_max_bytes": None,
                    "requested_history_max_total_bytes": None,
                    "history_policy_id": "harness-default-v1",
                    "history_max_revision_pairs": 128,
                    "history_max_total_bytes": 256 * 1024 * 1024,
                    "phase_resources": {name: deepcopy(phase) for name in PHASES},
                }
            ],
        },
    }


def _manifest() -> dict:
    return {
        "kind": "quanta-index-scale-tier-manifest",
        "manifest_schema_version": 2,
        "dimension": "scale",
        "query_token": "scale_needle_token",
        "tiers": [
            {
                "tier": name,
                "repo_count": repos,
                "files_per_repo": per_repo,
                "total_files": repos * per_repo,
                "source_repo_count": repos,
                "default_run": name == "small",
                "selectable": True,
            }
            for name, repos, per_repo in (
                ("small", 1, 16),
                ("medium", 4, 64),
                ("large", 16, 256),
                ("xlarge", 64, 512),
            )
        ],
    }


def _replay(
    summary: dict | None = None,
    manifest: dict | None = None,
    *,
    trace: bytes = TRACE,
    expected_tier: str = "large",
    expected_seed: int = 5,
    requested_client_timeout_ms: int | None = None,
    requested_history_max_bytes: int | None = None,
    requested_history_max_total_bytes: int | None = None,
    source_revision: str = SOURCE,
) -> dict:
    return replay(
        canonical(_summary() if summary is None else summary),
        trace,
        b"binary",
        canonical(_manifest() if manifest is None else manifest),
        source_revision=source_revision,
        expected_tier=expected_tier,
        expected_seed=expected_seed,
        requested_client_timeout_ms=requested_client_timeout_ms,
        requested_history_max_bytes=requested_history_max_bytes,
        requested_history_max_total_bytes=requested_history_max_total_bytes,
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
    assert phase["bm25_live_build"][0]["new_census_docs"] == 2
    assert phase["bm25_boundary"]["encode"]["logical_bytes"] == 128
    assert result["phases"]["full_ingest"]["bm25_census_build"]["docs"] == 2
    assert result["phases"]["delta_ingest_seal"]["bm25_live_build"][0]["newly_dead_docs"] == 1
    assert phase["process_write_io"]["write_bytes"] == 4096
    assert phase["sync_call_wall_ratio"] == 0.00004


@pytest.mark.parametrize(
    "mutant",
    [
        SINGLE_TRACE.replace(b"name=full_seal ok=1", b"name=noop_seal ok=1"),
        SINGLE_TRACE + b"QI_CAUSAL_V1 kind=phase_start name=full_seal\n",
        SINGLE_TRACE.replace(b"label=atomic_file", b"label=unknown"),
        SINGLE_TRACE.replace(
            b"QI_CAUSAL_V1 kind=sync label=atomic_file ok=1 elapsed_ns=400\n", b""
        ),
        SINGLE_TRACE.replace(b"postings=4", b"postings=2"),
        SINGLE_TRACE.replace(b"ok=1 elapsed_ns=400", b"ok=0 elapsed_ns=400"),
    ],
)
def test_profile_rejects_unpaired_unknown_or_inconsistent_trace(mutant: bytes) -> None:
    with pytest.raises(ValueError):
        parse_trace(mutant, {"full_seal"})


@pytest.mark.parametrize(
    "key,value",
    [
        ("tier", "medium"),
        ("seed", 6),
        ("seed", True),
        ("seed", 5.0),
        ("file_count", 4095),
        ("file_count", 4096.0),
        ("source_repo_count", False),
        ("source_repo_count", 15),
        ("client_request_timeout_ms", 300_000),
        ("client_request_timeout_ms", 30_000.0),
        ("requested_client_request_timeout_ms", 30_000),
        ("requested_history_max_bytes", 16 * 1024 * 1024),
        ("requested_history_max_total_bytes", 256 * 1024 * 1024),
        ("history_policy_id", "explicit-pair-total-diagnostic-v1"),
        ("history_max_bytes", True),
        ("history_max_generations", 3),
    ],
)
def test_profile_refuses_wrong_measured_input_or_policy(key: str, value: object) -> None:
    summary = _summary()
    summary["detail"]["measured_tiers"][0][key] = value
    with pytest.raises(ValueError):
        _replay(summary)


def test_profile_refuses_command_mismatch_and_accepts_exact_override() -> None:
    for kwargs in (
        {"expected_tier": "medium"},
        {"expected_seed": 6},
        {"requested_client_timeout_ms": 300_000},
        {"requested_history_max_bytes": 256 * 1024 * 1024},
        {"requested_history_max_total_bytes": 512 * 1024 * 1024},
    ):
        with pytest.raises(ValueError):
            _replay(**kwargs)
    summary = _summary()
    tier = summary["detail"]["measured_tiers"][0]
    tier["client_request_timeout_ms"] = 300_000
    tier["requested_client_request_timeout_ms"] = 300_000
    tier["history_max_bytes"] = 256 * 1024 * 1024
    tier["requested_history_max_bytes"] = 256 * 1024 * 1024
    tier["history_policy_id"] = "explicit-pair-default-total-v1"
    profile = _replay(
        summary,
        requested_client_timeout_ms=300_000,
        requested_history_max_bytes=256 * 1024 * 1024,
    )
    assert profile["runtime_config"]["history_max_bytes"] == 256 * 1024 * 1024


def test_profile_binds_explicit_pair_and_total_with_independent_mutants() -> None:
    summary = _summary()
    tier = summary["detail"]["measured_tiers"][0]
    tier.update(
        {
            "history_max_bytes": 300_000_000,
            "requested_history_max_bytes": 300_000_000,
            "history_max_total_bytes": 600_000_000,
            "requested_history_max_total_bytes": 600_000_000,
            "history_policy_id": "explicit-pair-total-diagnostic-v1",
        }
    )
    profile = _replay(
        summary,
        requested_history_max_bytes=300_000_000,
        requested_history_max_total_bytes=600_000_000,
    )
    assert profile["runtime_config"]["history_max_total_bytes"] == 600_000_000
    assert profile["runtime_config"]["history_policy_id"] == "explicit-pair-total-diagnostic-v1"
    for key, value in (
        ("history_max_total_bytes", 600_000_001),
        ("requested_history_max_total_bytes", 600_000_000.0),
        ("history_policy_id", "explicit-pair-default-total-v1"),
    ):
        mutant = deepcopy(summary)
        mutant["detail"]["measured_tiers"][0][key] = value
        with pytest.raises(ValueError, match=f"{key} does not match declared scale input"):
            _replay(
                mutant,
                requested_history_max_bytes=300_000_000,
                requested_history_max_total_bytes=600_000_000,
            )
    for pair, total in (
        (None, 600_000_000),
        (600_000_001, 600_000_000),
        (300_000_000, 0),
        (True, 600_000_000),
    ):
        with pytest.raises(ValueError):
            _replay(
                summary, requested_history_max_bytes=pair, requested_history_max_total_bytes=total
            )


def test_profile_refuses_missing_phase_trace_and_manifest_mutants() -> None:
    summary = _summary()
    del summary["detail"]["measured_tiers"][0]["phase_resources"]["delete_seal"]
    with pytest.raises(ValueError):
        _replay(summary)
    absent = TRACE.replace(_trace_phase("delete_seal"), b"")
    assert absent != TRACE
    with pytest.raises(ValueError):
        _replay(trace=absent)
    for key, value in (("total_files", 4095), ("repo_count", 16.0), ("tier", "medium")):
        manifest = _manifest()
        manifest["tiers"][2][key] = value
        with pytest.raises(ValueError):
            _replay(manifest=manifest)


@pytest.mark.parametrize(
    "mutant",
    [
        SINGLE_TRACE.replace(BM25_FULL, b""),
        SINGLE_TRACE.replace(BM25_ENCODE, b""),
        SINGLE_TRACE.replace(b"segment_fanout=1", b"segment_fanout=2"),
        SINGLE_TRACE.replace(b"retained_valid=1", b"retained_valid=0"),
        SINGLE_TRACE.replace(b"new_census_docs=2", b"new_census_docs=True"),
        SINGLE_TRACE.replace(b"logical_census_bytes=30", b"logical_census_bytes=30.0"),
        SINGLE_TRACE.replace(b"correction_keys=0", b"correction_keys=-1"),
        SINGLE_TRACE.replace(b"label=encode", b"label=unknown"),
        SINGLE_TRACE.replace(b"logical_bytes=128", b"logical_bytes=128 extra=1"),
        SINGLE_TRACE.replace(
            BM25_FULL, b"QI_CAUSAL_V1 kind=bm25_live_build ok=0 reason=counter_overflow\n"
        ),
        SINGLE_TRACE.replace(
            BM25_FULL, b"QI_CAUSAL_V1 kind=bm25_live_build ok=0 reason=retained_estimate_failed\n"
        ),
        SINGLE_TRACE.replace(
            BM25_ENCODE,
            b"QI_CAUSAL_V1 kind=bm25_boundary label=encode ok=0 reason=counter_overflow\n",
        ),
        SINGLE_TRACE.replace(b"index_files=1", b"index_files=1.0"),
        SINGLE_TRACE.replace(
            BM25_FULL,
            BM25_FULL + b"QI_CAUSAL_V1 kind=bm25_census_build ok=0 reason=counter_overflow\n",
        ),
        SINGLE_TRACE.replace(
            BM25_FULL, BM25_FULL + b"QI_CAUSAL_V1 kind=bm25_census_build ok=0 reason=unknown\n"
        ),
    ],
)
def test_profile_refuses_missing_or_malformed_bm25_observation(mutant: bytes) -> None:
    with pytest.raises(ValueError):
        parse_trace(mutant, {"full_seal"})


@pytest.mark.parametrize(
    ("changed_ns", "new_ns"),
    [
        (0, 1001),  # One child alone exceeds the 1000 ns parent.
        (400, 700),  # Both children fit separately, but their sum does not.
    ],
)
def test_profile_refuses_bm25_disjoint_child_clock_overrun(
    changed_ns: int,
    new_ns: int,
) -> None:
    parent_ns = 1000
    assert changed_ns + new_ns > parent_ns
    if changed_ns and new_ns:
        assert changed_ns <= parent_ns and new_ns <= parent_ns
    mutated = BM25_FULL.replace(b"changed_ns=0", f"changed_ns={changed_ns}".encode()).replace(
        b"new_ns=700", f"new_ns={new_ns}".encode()
    )
    assert mutated != BM25_FULL
    trace = SINGLE_TRACE.replace(BM25_FULL, mutated)
    assert trace != SINGLE_TRACE
    with pytest.raises(ValueError, match="BM25 child call clocks exceed parent"):
        parse_trace(trace, {"full_seal"})


def test_profile_refuses_aggregate_bm25_counter_overflow() -> None:
    maximum = str((1 << 64) - 1).encode()
    huge = BM25_CENSUS.replace(b"docs=2", b"docs=" + maximum)
    trace = SINGLE_TRACE.replace(BM25_FULL, huge + BM25_CENSUS + BM25_FULL)
    with pytest.raises(ValueError):
        parse_trace(trace, {"full_seal"})


def test_profile_refuses_missing_delta_bm25_build() -> None:
    with pytest.raises(ValueError):
        _replay(trace=TRACE.replace(BM25_DELTA, b"", 1))


def test_profile_refuses_source_and_io_type_alias() -> None:
    with pytest.raises(ValueError):
        _replay(source_revision="b" * 40)
    for invalid in (2.0, True):
        summary = _summary()
        summary["schema_version"] = invalid
        with pytest.raises(ValueError):
            _replay(summary)
    summary = _summary()
    summary["detail"]["measured_tiers"][0]["phase_resources"]["full_seal"]["process_write_io"][
        "write_bytes"
    ] = 4096.0
    with pytest.raises(ValueError):
        _replay(summary)
