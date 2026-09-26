"""Immutable five-product lexical diagnostics using the existing scorer.

This executes the scorer over frozen external observations, not live searches.
Native timing layers remain in the raw report and never become a speed claim.
"""

from __future__ import annotations

import math
import os
import platform
import socket
import sys
import uuid
from datetime import datetime, timezone
from pathlib import Path

from custody import custody
from evidence import (
    EvidenceError,
    RunStore,
    _read_regular_file,
    _run_id,
    canonical_json,
    digest_bytes,
    parse_json,
    validate_payload,
)
from evidence_bridge import host_identity, promote_native_run, source_identity
from producer_execution import execute
from profile_capture import _directories, commit_capture, load_capture
from registry import registry_digest

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools.benchmark.retrieval import lexical_file_comparison as owner  # noqa: E402

PROFILE = "lexical-diagnostic"
FAMILY = "lexical-file-comparison"
PRODUCTS = (*owner.PRODUCTS, "quanta_lexical", "semble_lexical_only")
MODULE = "tools.benchmark.retrieval.lexical_file_comparison"


def require_registration(registry: dict) -> None:
    if registry["profiles"][PROFILE]["families"] != [FAMILY]:
        raise EvidenceError("lexical profile inventory differs from implemented owner")
    entry = registry["families"][FAMILY]
    producer = registry["producers"][entry["producer"]]
    if any(
        entry[key] != value
        for key, value in {
            "payload": "retrieval",
            "result_unit": "ratio",
            "validator": "lexical-file-diagnostic",
            "scorer": "lexical-file-diagnostic",
            "native_schema": "lexical-file-diagnostic:v1",
            "gate_tier": "diagnostic",
            "host_policy": "any",
            "baseline": "none",
        }.items()
    ) or producer != {
        "kind": "python-module",
        "module": "tools/benchmark/retrieval/lexical_file_comparison.py",
        "argv": ["--spec"],
        "requires_spec": True,
        "outputs": [],
    }:
        raise EvidenceError("lexical registration differs from implemented scorer contract")


def payloads(summary: dict, pack: dict) -> dict[str, dict]:
    tasks = [task["task_id"] for task in pack["tasks"]]
    if not tasks or len(tasks) != len(set(tasks)):
        raise EvidenceError("lexical query inventory is empty or duplicate")
    products = {**summary["products"], **summary["pair"]["routes"]}
    if set(products) != set(PRODUCTS):
        raise EvidenceError("lexical scorer product inventory is incomplete")
    result = {}
    for product in PRODUCTS:
        rows = products[product]["per_query"]
        by_id = {row["task_id"]: row for row in rows}
        if len(rows) != len(tasks) or len(by_id) != len(rows) or set(by_id) != set(tasks):
            raise EvidenceError("lexical per-product query inventory differs from pack")
        typed_rows = []
        for task in tasks:
            row = by_id[task]
            value = row.get("file_recall_at_10")
            if type(value) not in (float, int) or not math.isfinite(value) or not 0 <= value <= 1:
                raise EvidenceError("lexical row has malformed file recall")
            state = "judged"
            if product in {"quanta_lexical", "semble_lexical_only"}:
                status = row.get("status")
                if status in {"timeout", "unavailable"}:
                    state = "timeout" if status == "timeout" else "unsupported"
                    value = None
                elif status not in {"success", "capped", "abstained"}:
                    raise EvidenceError(
                        "lexical paired row lacks an admissible explicit terminal state"
                    )
            typed_rows.append(
                {
                    "query_id": task,
                    "metric": "file_recall_at_10",
                    "unit": "ratio",
                    "value": float(value) if value is not None else None,
                    "state": state,
                }
            )
        payload = {
            "kind": "retrieval",
            "lane": "controlled_mechanism",
            "metric_space": "file",
            "judgments": "mechanically_labeled",
            "unjudged": 0,
            "universe_attested": False,
            "corpus_digest": "sha256:" + summary["file_universe_digest"],
            "query_pack_digest": "sha256:" + summary["query_pack_sha256"],
            "rows": typed_rows,
        }
        validate_payload(payload)
        result[product] = payload
    return result


def frozen_inputs(raw: Path) -> dict[str, Path]:
    return {role: raw / f"input-{role}" for role in owner.INPUT_ROLES}


def replay_run(store: RunStore, evidence: dict) -> None:
    if (
        evidence["profile"] != PROFILE
        or evidence["family"] != FAMILY
        or evidence["case_id"] not in PRODUCTS
        or evidence["verdict"]["scope"] != "diagnostic"
    ):
        raise EvidenceError("lexical replay has wrong family/profile/product/scope")
    raw = store.run_dir(evidence["run_id"]) / "raw"
    origin = parse_json(_read_regular_file(raw / "capture-origin.json").decode())
    if not isinstance(origin, dict) or set(origin) != {
        "capture_id",
        "execution_root",
        "producer",
        "owner_digest",
        "python_digest",
        "toolchain",
    }:
        raise EvidenceError("lexical origin is malformed")
    _run_id(origin["capture_id"])
    if evidence["run_id"] != origin["capture_id"] + "-" + evidence["case_id"]:
        raise EvidenceError("lexical run mixes captures or product identities")
    native = Path(origin["execution_root"])
    if not native.is_absolute() or ".." in native.parts:
        raise EvidenceError("lexical origin has no canonical execution root")
    command = evidence["command"]
    if (
        origin["producer"] != command
        or command["argv"]
        != [
            command["argv"][0],
            "-m",
            MODULE,
            "--spec",
            str(native / "frozen-spec.json"),
            "--out",
            str(native / "report.json"),
        ]
        or not Path(command["argv"][0]).is_absolute()
    ):
        raise EvidenceError("lexical command differs from registered scorer")
    if origin["owner_digest"] != digest_bytes(_read_regular_file(Path(owner.__file__))):
        raise EvidenceError(
            "lexical scorer source changed; historical report needs its original owner"
        )
    spec = parse_json(_read_regular_file(raw / "frozen-spec.json").decode())
    if spec != {
        "schema_version": 1,
        **{role: str(native / f"input-{role}") for role in owner.INPUT_ROLES},
    }:
        raise EvidenceError("lexical frozen spec differs from captured role paths")
    original = owner.read_spec(raw / "original-spec.json")
    if set(original) != set(owner.INPUT_ROLES):
        raise EvidenceError("lexical original spec is incomplete")
    paths = frozen_inputs(raw)
    summary = owner.evaluate_capture(paths)
    if parse_json(_read_regular_file(raw / "report.json").decode()) != summary:
        raise EvidenceError("lexical raw report differs from owner recomputation")
    pack = owner._read(paths["query_pack"])
    if evidence["payload"] != payloads(summary, pack)[evidence["case_id"]]:
        raise EvidenceError("lexical typed rows differ from raw observations")
    inputs = [
        {
            "id": role,
            "availability": "present",
            "digest": digest_bytes(_read_regular_file(path)),
            "reason": None,
        }
        for role, path in paths.items()
    ]
    if evidence["inputs"] != inputs:
        raise EvidenceError("lexical frozen input inventory or digest differs")
    if (
        evidence["build"]["binaries"] != [{"name": "python", "sha256": origin["python_digest"]}]
        or evidence["build"]["toolchain"] != origin["toolchain"]
    ):
        raise EvidenceError("lexical scorer executable/toolchain inventory differs")


def capture(repo: Path, root: Path, registry: dict, spec_path: Path, timeout: int) -> dict:
    from benchctl import require_clean_worktree, require_frozen_source, resolve_checkout_head

    require_registration(registry)
    if repo.resolve() != ROOT:
        raise EvidenceError("lexical driver must come from the requested checkout")
    if root.resolve().is_relative_to(repo.resolve()):
        raise EvidenceError("lexical evidence must stay outside the checkout")
    _directories(root)
    paths = owner.read_spec(spec_path)
    if any(path.resolve().is_relative_to(repo.resolve()) for path in (spec_path, *paths.values())):
        raise EvidenceError("lexical spec and observations must stay outside the checkout")
    require_clean_worktree(repo)
    head = resolve_checkout_head(repo)
    source = source_identity(repo, "benchmark-retrieval")
    original = _read_regular_file(spec_path)
    contents = {role: _read_regular_file(path) for role, path in paths.items()}
    if _read_regular_file(spec_path) != original or owner.read_spec(spec_path) != paths:
        raise EvidenceError("lexical spec changed during freeze")
    capture_id = f"lexical-{uuid.uuid4().hex}"
    native = root / "work" / capture_id
    native.mkdir(parents=True, exist_ok=False)
    for role, content in contents.items():
        (native / f"input-{role}").write_bytes(content)
    (native / "original-spec.json").write_bytes(original)
    (native / "frozen-spec.json").write_text(
        canonical_json(
            {
                "schema_version": 1,
                **{role: str(path) for role, path in frozen_inputs(native).items()},
            }
        ),
        encoding="utf-8",
    )
    python_digest = digest_bytes(_read_regular_file(Path(sys.executable).resolve()))
    stdout, stderr, command = execute(
        [
            sys.executable,
            "-m",
            MODULE,
            "--spec",
            str(native / "frozen-spec.json"),
            "--out",
            str(native / "report.json"),
        ],
        cwd=repo,
        env=dict(os.environ),
        timeout=timeout,
    )
    if python_digest != digest_bytes(_read_regular_file(Path(sys.executable).resolve())):
        raise EvidenceError("lexical Python executable changed during scoring")
    summary = owner.evaluate_capture(frozen_inputs(native))
    if parse_json(_read_regular_file(native / "report.json").decode()) != summary:
        raise EvidenceError("lexical producer report differs from independent raw replay")
    typed = payloads(summary, owner._read(native / "input-query_pack"))
    raw = {path.name: _read_regular_file(path) for path in native.iterdir()}
    toolchain = f"Python {platform.python_version()}"
    raw.update(
        {
            "producer.stdout": stdout,
            "producer.stderr": stderr,
            "capture-origin.json": canonical_json(
                {
                    "capture_id": capture_id,
                    "execution_root": str(native),
                    "producer": command,
                    "owner_digest": digest_bytes(_read_regular_file(Path(owner.__file__))),
                    "python_digest": python_digest,
                    "toolchain": toolchain,
                }
            ).encode(),
        }
    )
    require_frozen_source(repo, head)
    cpu = os.cpu_count()
    if type(cpu) is not int or cpu < 1:
        raise EvidenceError("lexical scorer host CPU inventory unavailable")
    host = host_identity(
        policy="any",
        os_name="macos" if sys.platform == "darwin" else "linux",
        arch=platform.machine(),
        cpu_count=cpu,
        hostname=socket.gethostname(),
        lease_mode="none",
        lease_samples=0,
    )
    with custody(root):
        (root / "captures").mkdir(exist_ok=True)
        runs = []
        for product in PRODUCTS:
            items = list(raw.items())
            result = promote_native_run(
                evidence_root=root,
                run_id=f"{capture_id}-{product}",
                family=FAMILY,
                profile=PROFILE,
                case_id=product,
                created_utc=datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
                native_path=Path(items[0][0]),
                native_bytes=items[0][1],
                additional_native=[(Path(name), content) for name, content in items[1:]],
                payload=typed[product],
                source=source,
                build={
                    "toolchain": toolchain,
                    "target_triple": f"{sys.platform}-{platform.machine()}",
                    "lockfile_digest": digest_bytes((repo / "uv.lock").read_bytes()),
                    "profile": "lexical-recorded-scoring",
                    "flags": [],
                    "binaries": [{"name": "python", "sha256": python_digest}],
                },
                inputs=[
                    {
                        "id": role,
                        "availability": "present",
                        "digest": digest_bytes(content),
                        "reason": None,
                    }
                    for role, content in contents.items()
                ],
                host=host,
                command=command,
                boundary={
                    "clock": "recorded",
                    "instrumentation": "none",
                    "start_event": "frozen_observation_scoring",
                    "end_event": "scorer_replay",
                },
                verdict={"scope": "diagnostic", "status": "pass", "reason": None, "metrics": []},
            )
            runs.append(result["run_id"])
            replay_run(RunStore(root), RunStore(root).load(result["run_id"]))
        require_frozen_source(repo, head)
        return commit_capture(
            root,
            capture_id=capture_id,
            profile=PROFILE,
            registry_digest=registry_digest(registry),
            expected_cases={FAMILY: list(PRODUCTS)},
            run_ids=runs,
        )


def validate(repo: Path, root: Path, registry: dict) -> dict:
    from benchctl import require_clean_worktree

    require_clean_worktree(repo)
    require_registration(registry)
    document = load_capture(root, profile=PROFILE, registry_digest=registry_digest(registry))
    if document["source"] != source_identity(repo, "benchmark-retrieval") or document[
        "expected_cases"
    ] != {FAMILY: list(PRODUCTS)}:
        raise EvidenceError("lexical capture source or complete product inventory differs")
    store = RunStore(root)
    for record in document["runs"]:
        evidence = store.load(record["run_id"])
        if not evidence["run_id"].startswith(document["capture_id"] + "-") or evidence["build"][
            "lockfile_digest"
        ] != digest_bytes((repo / "uv.lock").read_bytes()):
            raise EvidenceError("lexical profile mixes captures or stale lockfile")
        replay_run(store, evidence)
    return document
