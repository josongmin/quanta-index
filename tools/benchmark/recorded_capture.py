"""Explicit, unauthenticated recorded-input import; never a producer execution.

Existing agent-outcome validation owns A/B/C pairing and trajectory metrics.
Scan imports retain every native row percentile, not timings from a Markdown
report or an invented text-scan measurement. Both families are diagnostic only.
"""

from __future__ import annotations

import os
import platform
import socket
import sys
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path

from evidence import (
    EvidenceError,
    RawFile,
    RunStore,
    _read_control_file,
    canonical_json,
    digest_bytes,
    parse_json,
    validate_payload,
    write_raw_file,
)
from evidence_bridge import (
    _load_module,
    host_identity,
    latency_payload_from_artifact,
    source_identity,
)
from profile_capture import _directories, load_capture, publish_capture
from registry import registry_digest


def agent_payload(raw: RawFile) -> tuple[dict, dict]:
    owner = _load_module(
        "recorded_agent_owner", Path(__file__).parent / "agent_outcome/__main__.py"
    )
    pairs, digest = owner.load_file(raw), raw.sha256
    summary = owner.summarize(pairs, digest)
    metrics = []
    for arm, aggregate in summary["aggregate"].items():
        for name in ("fail_to_pass", "pass_to_pass"):
            score = aggregate[name]
            metrics.append(
                {
                    "name": f"{arm}.{name}",
                    "unit": "ratio",
                    "value": float(score["rate"]),
                    "numerator": score["passed"],
                    "denominator": score["total"],
                }
            )
        total = aggregate["total_pairs"]
        for name, numerator in (
            ("resolved", aggregate["resolved_pairs"]),
            ("useful_evidence_coverage", aggregate["useful_evidence_pairs"]),
        ):
            metrics.append(
                {
                    "name": f"{arm}.{name}",
                    "unit": "ratio",
                    "value": numerator / total,
                    "numerator": numerator,
                    "denominator": total,
                }
            )
        first = aggregate["mean_first_useful_evidence_ms_when_present"]
        if first is not None:
            metrics.append(
                {
                    "name": f"{arm}.first_useful_evidence_ms_when_present",
                    "unit": "ms",
                    "value": float(first),
                    "numerator": None,
                    "denominator": None,
                }
            )
        for name, unit in (("mean_elapsed_ms", "ms"), ("mean_tool_calls", "count")):
            metrics.append(
                {
                    "name": f"{arm}.{name}",
                    "unit": unit,
                    "value": float(aggregate[name]),
                    "numerator": None,
                    "denominator": None,
                }
            )
    payload = {
        "kind": "agent_outcome",
        "task_count": len({p["A"]["task_id"] for p in pairs}),
        "pair_count": len(pairs),
        "arms": ["A", "B", "C"],
        "excluded_pairs": 0,
        "unknown_pairs": 0,
        "metrics": metrics,
        "capture": "recorded_unauthenticated",
        "input_digest": digest,
    }
    validate_payload(payload)
    return payload, summary


def scan_payload(raw: RawFile) -> dict:
    document = parse_json(raw.read_control().decode("utf-8"))
    if not isinstance(document, dict) or set(document) != {"artifacts"}:
        raise EvidenceError("scan input must contain exactly an artifacts array")
    artifacts = document["artifacts"]
    if not isinstance(artifacts, list) or not artifacts:
        raise EvidenceError("scan input has no native artifacts")
    points, heads, seen = [], set(), set()
    for artifact in artifacts:
        if not isinstance(artifact, dict) or artifact.get("dimension") != "scan-vs-index":
            raise EvidenceError("scan input has a wrong native dimension")
        provenance = artifact.get("provenance")
        if not isinstance(provenance, dict) or not isinstance(provenance.get("git_head"), str):
            raise EvidenceError("scan input has no native source identity")
        head = provenance["git_head"]
        from compare_dsl_bench import FULL_HEAD_RE

        if not FULL_HEAD_RE.fullmatch(head):
            raise EvidenceError("scan input native source revision is malformed")
        heads.add(head)
        detail = artifact.get("detail")
        if (
            not isinstance(detail, dict)
            or type(detail.get("chunks")) is not int
            or detail["chunks"] < 1
        ):
            raise EvidenceError("scan input has no positive native chunk scale")
        scale = detail["chunks"]
        if scale in seen:
            raise EvidenceError("scan input repeats a chunk scale")
        seen.add(scale)
        latency = latency_payload_from_artifact(artifact)
        for row in latency["rows"]:
            if row["samples"] < 1 or row["early_stop_reason"] is not None:
                raise EvidenceError("scan input contains an unmeasured native row")
            for metric in ("p50", "p95", "p99"):
                points.append(
                    {
                        "label": f"chunks-{scale}:{row['case_id']}",
                        "metric": metric,
                        "unit": "ms",
                        "value": row[metric],
                    }
                )
            for metric in ("error_count", "timeout_count"):
                points.append(
                    {
                        "label": f"chunks-{scale}:{row['case_id']}",
                        "metric": metric,
                        "unit": "count",
                        "value": row[metric],
                    }
                )
    if len(heads) != 1:
        raise EvidenceError("scan input mixes native source revisions")
    payload = {
        "kind": "recorded_experiment",
        "experiment_id": "scan-vs-index",
        "diagnostic_only": True,
        "points": points,
        "source_digest": raw.sha256,
    }
    validate_payload(payload)
    return payload


def replay_run(store: RunStore, evidence: dict) -> None:
    family = evidence["family"]
    if family not in {"agent-outcome", "scan-vs-index"} or evidence["profile"] != "recorded":
        raise EvidenceError("recorded replay family/profile is not supported")
    expected = "agent.jsonl" if family == "agent-outcome" else "scan.json"
    refs = [ref for ref in evidence["raw"] if Path(ref["path"]).name == expected]
    if len(refs) != 1 or evidence["verdict"]["scope"] != "diagnostic":
        raise EvidenceError(
            "recorded replay requires exactly one native input and diagnostic scope"
        )
    path = store.run_dir(evidence["run_id"]) / refs[0]["path"]
    raw = RawFile.capture(path)
    if family == "agent-outcome":
        derived, summary = agent_payload(raw)
        summary_refs = [
            ref for ref in evidence["raw"] if Path(ref["path"]).name == "agent-summary.json"
        ]
        if (
            len(summary_refs) != 1
            or parse_json(
                _read_control_file(
                    store.run_dir(evidence["run_id"]) / summary_refs[0]["path"]
                ).decode()
            )
            != summary
        ):
            raise EvidenceError("recorded agent summary differs from native recomputation")
    else:
        derived = scan_payload(raw)
    if derived != evidence["payload"]:
        raise EvidenceError("recorded typed payload differs from native recomputation")
    inputs = [item for item in evidence["inputs"] if item["id"] == family]
    if (
        len(inputs) != 1
        or inputs[0]["availability"] != "present"
        or inputs[0]["digest"] != raw.sha256
    ):
        raise EvidenceError("recorded input identity differs from native bytes")


def capture(
    repo: Path, root: Path, registry: dict, agent: Path, scan: Path, authenticity: str
) -> dict:
    from benchctl import require_clean_worktree, require_frozen_source, resolve_checkout_head

    started = time.monotonic_ns()
    if authenticity != "recorded_unauthenticated":
        raise EvidenceError(
            "authenticated imports are refused: no underlying receipt authenticator is implemented"
        )
    if root.resolve().is_relative_to(repo.resolve()):
        raise EvidenceError("recorded evidence root must stay outside the checkout")
    _directories(root)
    if any(path.resolve().is_relative_to(repo.resolve()) for path in (agent, scan)):
        raise EvidenceError("recorded inputs must stay outside the checkout")
    require_clean_worktree(repo)
    head = resolve_checkout_head(repo)
    source = source_identity(repo, "benchmark-control-plane")
    agent_raw, scan_raw = RawFile.capture(agent), RawFile.capture(scan)
    # Validate every family before promoting any run or replacing the pointer.
    agent_result, agent_summary = agent_payload(agent_raw)
    scan_result = scan_payload(scan_raw)
    cpu = os.cpu_count()
    if type(cpu) is not int or cpu < 1:
        raise EvidenceError("cannot establish importer CPU inventory")
    capture_id = f"recorded-{uuid.uuid4().hex}"
    prepared = (
        ("agent-outcome", "agent.jsonl", agent_raw, agent_result),
        ("scan-vs-index", "scan.json", scan_raw, scan_result),
    )
    if set(registry["profiles"]["recorded"]["families"]) != {p[0] for p in prepared}:
        raise EvidenceError("recorded profile inventory differs from implemented owners")
    host = host_identity(
        policy="any",
        os_name="macos" if sys.platform == "darwin" else "linux",
        arch=platform.machine(),
        cpu_count=cpu,
        hostname=socket.gethostname(),
        lease_mode="none",
        lease_samples=0,
    )

    def verify_source():
        if time.monotonic_ns() - started >= 1800 * 1_000_000_000:
            raise EvidenceError("recorded import exceeded its publication deadline")
        require_frozen_source(repo, head)

    runs = []
    for family, name, raw, payload in prepared:
        verify_source()
        promotion = dict(
            run_id=f"{capture_id}-{family}",
            family=family,
            profile="recorded",
            case_id=None,
            created_utc=datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
            raw_files={
                name: raw,
                **(
                    {
                        "agent-summary.json": write_raw_file(
                            root / "work" / capture_id / family / "agent-summary.json",
                            [canonical_json(agent_summary).encode()],
                        )
                    }
                    if family == "agent-outcome"
                    else {}
                ),
            },
            payload=payload,
            source=source,
            build={
                "toolchain": f"Python {platform.python_version()}",
                "target_triple": f"{sys.platform}-{platform.machine()}",
                "lockfile_digest": digest_bytes((repo / "uv.lock").read_bytes()),
                "profile": "recorded-import",
                "flags": [],
                "binaries": [],
            },
            inputs=[
                {
                    "id": family,
                    "availability": "present",
                    "digest": raw.sha256,
                    "reason": None,
                }
            ],
            host=host,
            command={
                "argv": ["benchctl", "import-recorded", family],
                "cwd": str(repo),
                "status": "completed",
                "exit_code": 0,
                "timeout_seconds": 1800,
                "wall_ms": (time.monotonic_ns() - started) // 1_000_000,
            },
            boundary={
                "clock": "recorded",
                "instrumentation": "none",
                "start_event": "recorded_import",
                "end_event": "recorded_validation",
            },
            verdict={"scope": "diagnostic", "status": "pass", "reason": None, "metrics": []},
        )
        runs.append(promotion)
    return publish_capture(
        root,
        capture_id=capture_id,
        profile="recorded",
        registry_digest=registry_digest(registry),
        expected_cases={p[0]: [None] for p in prepared},
        runs=runs,
        replay=replay_run,
        verify_source=verify_source,
    )


def validate(repo: Path, root: Path, registry: dict) -> dict:
    document = load_capture(root, profile="recorded", registry_digest=registry_digest(registry))
    if document["source"] != source_identity(repo, "benchmark-control-plane"):
        raise EvidenceError("recorded importer source is stale or dirty")
    if document["expected_cases"] != {
        family: [None] for family in registry["profiles"]["recorded"]["families"]
    }:
        raise EvidenceError("recorded capture inventory differs from registry")
    store = RunStore(root)
    for record in document["runs"]:
        if not record["run_id"].startswith(document["capture_id"] + "-"):
            raise EvidenceError("recorded profile mixes captures")
        evidence = store.load(record["run_id"])
        if evidence["build"]["lockfile_digest"] != digest_bytes((repo / "uv.lock").read_bytes()):
            raise EvidenceError("recorded importer lockfile is stale")
        replay_run(store, evidence)
    return document
