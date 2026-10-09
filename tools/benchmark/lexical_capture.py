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
from datetime import datetime, timezone
from pathlib import Path

from evidence import (
    EvidenceError,
    RawFile,
    RunStore,
    _read_control_file,
    _run_id,
    canonical_json,
    digest_bytes,
    file_digest,
    parse_json,
    validate_payload,
    write_raw_file,
)
from evidence_bridge import host_identity, source_identity
from producer_execution import execute
from profile_capture import (
    _directories,
    capture_entrypoint,
    current_capture,
    load_capture,
    publish_capture,
)
from registry import registry_digest

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

import corpus_binding  # noqa: E402

from tools.benchmark.retrieval import lexical_file_comparison as owner  # noqa: E402

PROFILE = "lexical-diagnostic"
FAMILY = "lexical-file-comparison"
PRODUCTS = (*owner.PRODUCTS, "quanta_lexical", "semble_lexical_only")
FILE_PRODUCTS = (*owner.PRODUCTS, "quanta_lexical", "semble_lexical_file")
MODULE = "tools.benchmark.retrieval.lexical_file_comparison"
RANK_UNITS = {
    **dict.fromkeys(owner.PRODUCTS, "distinct_file"),
    "quanta_lexical": "chunk",
    "semble_lexical_only": "chunk",
}
METRICS = {
    "distinct_file": "gold_file_recall_in_top_10_distinct_files",
    "chunk": "gold_file_recall_in_top_10_chunks",
}


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


def payloads(summary: dict, suite: dict, pack: dict) -> dict[str, dict]:
    current_file = suite.get("routes") == owner.FILE_ROUTES
    expected_equivalence = "equivalent_distinct_file_units" if current_file else "non_equivalent"
    if (
        summary.get("status") != "diagnostic_unqualified"
        or summary.get("rank_unit_equivalence") != expected_equivalence
        or summary.get("metric") != "gold_file_recall_in_native_top_10"
    ):
        raise EvidenceError("lexical capture claims an unsupported rank comparison")
    tasks = [task["task_id"] for task in pack["tasks"]]
    if not tasks or len(tasks) != len(set(tasks)):
        raise EvidenceError("lexical query inventory is empty or duplicate")
    judged = owner._tasks(suite, pack)
    products = {**summary["products"], **summary["pair"]["routes"]}
    inventory = FILE_PRODUCTS if current_file else PRODUCTS
    if set(products) != set(inventory):
        raise EvidenceError("lexical scorer product inventory is incomplete")
    result = {}
    for product in inventory:
        rank_unit = products[product].get("rank_unit")
        if rank_unit != ("distinct_file" if current_file else RANK_UNITS[product]):
            raise EvidenceError(f"{product}: lexical rank unit differs from native result unit")
        rows = products[product]["per_query"]
        by_id = {row["task_id"]: row for row in rows}
        if len(rows) != len(tasks) or len(by_id) != len(rows) or set(by_id) != set(tasks):
            raise EvidenceError("lexical per-product query inventory differs from pack")
        typed_rows = []
        for task in tasks:
            row = by_id[task]
            if row.get("status") == "unsupported" or (
                current_file and row.get("status") in {"error", "timeout", "unavailable"}
            ):
                typed_rows.append(
                    {
                        "query_id": task,
                        "metric": METRICS[rank_unit],
                        "unit": "ratio",
                        "value": None,
                        "state": "timeout" if row["status"] == "timeout" else "unsupported",
                    }
                )
                continue
            if current_file and row.get("eligible") is False:
                typed_rows.append(
                    {
                        "query_id": task,
                        "metric": METRICS[rank_unit],
                        "unit": "ratio",
                        "value": None,
                        "state": "unsupported",
                    }
                )
                continue
            value = row.get("file_recall_at_10")
            no_gold = not judged[task][1]
            if no_gold:
                if value != "not_applicable":
                    raise EvidenceError("no-gold lexical row must not carry recall")
                if product in {"quanta_lexical", "semble_lexical_only", "semble_lexical_file"}:
                    if (
                        row.get("answerable") is not False
                        or row.get("status") not in {"success", "capped", "abstained"}
                        or type(row.get("candidates")) is not int
                        or row["candidates"] < 0
                    ):
                        raise EvidenceError("no-gold paired row lacks valid candidate observation")
                    empty = row["candidates"] == 0
                else:
                    empty = row.get("no_gold_empty_at_10")
                    if type(empty) is not bool:
                        raise EvidenceError("no-gold external row lacks empty-result observation")
                value = float(empty)
                metric, state = "no_gold_empty_at_10", "no_answer"
            else:
                if (
                    type(value) not in (float, int)
                    or not math.isfinite(value)
                    or not 0 <= value <= 1
                ):
                    raise EvidenceError("lexical row has malformed file recall")
                metric, state = "file_recall_at_10", "judged"
            if not no_gold and product in {
                "quanta_lexical",
                "semble_lexical_only",
                "semble_lexical_file",
            }:
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
                    "metric": metric if no_gold else METRICS[rank_unit],
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
    spec = raw / "frozen-spec.json"
    roles = (
        owner.input_roles(parse_json(_read_control_file(spec).decode()))
        if spec.exists()
        else owner.INPUT_ROLES
    )
    return {role: raw / f"input-{role}" for role in roles}


def replay_run(store: RunStore, evidence: dict) -> None:
    if (
        evidence["profile"] != PROFILE
        or evidence["family"] != FAMILY
        or evidence["case_id"] not in {*PRODUCTS, *FILE_PRODUCTS}
        or evidence["verdict"]["scope"] != "diagnostic"
    ):
        raise EvidenceError("lexical replay has wrong family/profile/product/scope")
    raw = store.run_dir(evidence["run_id"]) / "raw"
    origin = parse_json(_read_control_file(raw / "capture-origin.json").decode())
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
    if origin["owner_digest"] != file_digest(Path(owner.__file__))[0]:
        raise EvidenceError(
            "lexical scorer source changed; historical report needs its original owner"
        )
    spec = parse_json(_read_control_file(raw / "frozen-spec.json").decode())
    roles = owner.input_roles(spec)
    if spec != {
        "schema_version": 1,
        **{role: str(native / f"input-{role}") for role in roles},
    }:
        raise EvidenceError("lexical frozen spec differs from captured role paths")
    selection, _original_paths = corpus_binding.read_spec(
        _read_control_file(raw / "original-spec.json"), roles
    )
    paths = frozen_inputs(raw)
    capsule = RawFile.capture(raw / "corpus-release.zip")
    binding_raw = _read_control_file(raw / "corpus-binding.json")
    binding = corpus_binding.replay(
        capsule,
        selection,
        _read_control_file(paths["suite"]),
        _read_control_file(paths["query_pack"]),
    )
    if parse_json(binding_raw.decode()) != binding:
        raise EvidenceError("lexical corpus/view/query binding differs from retained Git objects")
    summary = owner.evaluate_capture(paths)
    if parse_json(_read_control_file(raw / "report.json").decode()) != summary:
        raise EvidenceError("lexical raw report differs from owner recomputation")
    pack = owner._read(paths["query_pack"])
    if (
        evidence["payload"]
        != payloads(summary, owner._read(paths["suite"]), pack)[evidence["case_id"]]
    ):
        raise EvidenceError("lexical typed rows differ from raw observations")
    inputs = [
        {
            "id": role,
            "availability": "present",
            "digest": file_digest(path)[0],
            "reason": None,
        }
        for role, path in paths.items()
    ] + [
        {
            "id": "corpus-release",
            "availability": "present",
            "digest": capsule.sha256,
            "reason": None,
        },
        {
            "id": "corpus-view-query-binding",
            "availability": "present",
            "digest": digest_bytes(binding_raw),
            "reason": None,
        },
    ]
    if evidence["inputs"] != inputs:
        raise EvidenceError("lexical frozen input inventory or digest differs")
    if (
        evidence["build"]["binaries"] != [{"name": "python", "sha256": origin["python_digest"]}]
        or evidence["build"]["toolchain"] != origin["toolchain"]
    ):
        raise EvidenceError("lexical scorer executable/toolchain inventory differs")


def capture(repo: Path, root: Path, registry: dict, spec_path: Path, timeout: int) -> dict:
    require_registration(registry)
    if repo.resolve() != ROOT:
        raise EvidenceError("lexical driver must come from the requested checkout")
    if root.resolve().is_relative_to(repo.resolve()):
        raise EvidenceError("lexical evidence must stay outside the checkout")
    original = _read_control_file(spec_path)
    envelope = parse_json(original.decode())
    roles = owner.input_roles(envelope) if "inputs" in envelope else owner.INPUT_ROLES
    selection, paths = corpus_binding.read_spec(original, roles)
    release = Path(selection["release_path"])
    if root.resolve().is_relative_to(release.resolve()) or release.resolve().is_relative_to(
        root.resolve()
    ):
        raise EvidenceError("lexical evidence and corpus release roots overlap")
    if any(path.resolve().is_relative_to(repo.resolve()) for path in (spec_path, *paths.values())):
        raise EvidenceError("lexical spec and observations must stay outside the checkout")
    _directories(root)
    return _capture_admitted(
        repo, root, registry, spec_path, timeout, original, selection, paths, release
    )


@capture_entrypoint(PROFILE)
def _capture_admitted(
    repo, root, registry, spec_path, timeout, original, selection, paths, release
):
    from benchctl import require_clean_worktree, require_frozen_source, resolve_checkout_head

    current_capture().step("source")
    require_clean_worktree(repo)
    head = resolve_checkout_head(repo)
    source = source_identity(repo, "benchmark-retrieval")
    current_capture().step("inputs", source=source)
    contents = {role: RawFile.capture(path) for role, path in paths.items()}
    current_capture().inputs(contents)
    if _read_control_file(spec_path) != original:
        raise EvidenceError("lexical spec changed during freeze")
    _directories(root)
    capture_id = current_capture().capture_id
    native = root / "work" / capture_id
    binding, capsule = corpus_binding.capture(
        release,
        selection,
        contents["suite"].read_control(),
        contents["query_pack"].read_control(),
        native / "corpus-release.zip",
    )
    binding_raw = canonical_json(binding).encode()
    frozen = {role: content.copy_to(native / f"input-{role}") for role, content in contents.items()}
    (native / "original-spec.json").write_bytes(original)
    (native / "corpus-binding.json").write_bytes(binding_raw)
    (native / "frozen-spec.json").write_text(
        canonical_json(
            {
                "schema_version": 1,
                **{role: str(content.path) for role, content in frozen.items()},
            }
        ),
        encoding="utf-8",
    )
    python_digest = RawFile.capture(Path(sys.executable).resolve()).sha256
    stdout, stderr, command = current_capture().execute(
        execute,
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
        log_dir=native / "execution",
    )
    if python_digest != RawFile.capture(Path(sys.executable).resolve()).sha256:
        raise EvidenceError("lexical Python executable changed during scoring")
    summary = owner.evaluate_capture(frozen_inputs(native))
    if parse_json(_read_control_file(native / "report.json").decode()) != summary:
        raise EvidenceError("lexical producer report differs from independent raw replay")
    typed = payloads(
        summary, owner._read(native / "input-suite"), owner._read(native / "input-query_pack")
    )
    if any(file_digest(raw.path) != (raw.sha256, raw.size) for raw in frozen.values()):
        raise EvidenceError("lexical frozen inputs changed during scoring")
    raw = {
        path.name: RawFile.capture(path)
        for path in native.iterdir()
        if path.name not in {"capture.json", "execution"}
    }
    spool = root / "work" / capture_id / "prepared"
    toolchain = f"Python {platform.python_version()}"
    raw.update(
        {
            "producer.stdout": stdout,
            "producer.stderr": stderr,
            "capture-origin.json": write_raw_file(
                spool / "capture-origin.json",
                [
                    canonical_json(
                        {
                            "capture_id": capture_id,
                            "execution_root": str(native),
                            "producer": command,
                            "owner_digest": file_digest(Path(owner.__file__))[0],
                            "python_digest": python_digest,
                            "toolchain": toolchain,
                        }
                    ).encode()
                ],
            ),
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
    runs = []
    for product in typed:
        result = dict(
            run_id=f"{capture_id}-{product}",
            family=FAMILY,
            profile=PROFILE,
            case_id=product,
            created_utc=datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
            raw_files=raw,
            payload=typed[product],
            source=source,
            build={
                "toolchain": toolchain,
                "target_triple": f"{sys.platform}-{platform.machine()}",
                "lockfile_digest": file_digest(repo / "uv.lock")[0],
                "profile": "lexical-recorded-scoring",
                "flags": [],
                "binaries": [{"name": "python", "sha256": python_digest}],
            },
            inputs=[
                {
                    "id": role,
                    "availability": "present",
                    "digest": content.sha256,
                    "reason": None,
                }
                for role, content in contents.items()
            ]
            + [
                {
                    "id": "corpus-release",
                    "availability": "present",
                    "digest": capsule.sha256,
                    "reason": None,
                },
                {
                    "id": "corpus-view-query-binding",
                    "availability": "present",
                    "digest": digest_bytes(binding_raw),
                    "reason": None,
                },
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
        runs.append(result)
    return publish_capture(
        root,
        capture_id=capture_id,
        profile=PROFILE,
        registry_digest=registry_digest(registry),
        expected_cases={FAMILY: list(typed)},
        runs=runs,
        replay=replay_run,
        verify_source=lambda: require_frozen_source(repo, head),
    )


def validate(repo: Path, root: Path, registry: dict) -> dict:
    from benchctl import require_clean_worktree

    require_clean_worktree(repo)
    require_registration(registry)
    document = load_capture(root, profile=PROFILE, registry_digest=registry_digest(registry))
    if document["source"] != source_identity(repo, "benchmark-retrieval") or document[
        "expected_cases"
    ] not in ({FAMILY: list(PRODUCTS)}, {FAMILY: list(FILE_PRODUCTS)}):
        raise EvidenceError("lexical capture source or complete product inventory differs")
    store = RunStore(root)
    for record in document["runs"]:
        evidence = store.load(record["run_id"])
        if (
            not evidence["run_id"].startswith(document["capture_id"] + "-")
            or evidence["build"]["lockfile_digest"] != file_digest(repo / "uv.lock")[0]
        ):
            raise EvidenceError("lexical profile mixes captures or stale lockfile")
        replay_run(store, evidence)
    return document
