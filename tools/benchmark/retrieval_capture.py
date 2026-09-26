"""Common capture custody for existing retrieval proof/score owners.

Contract receipts are typed test proofs, not relevance. Raw execution roots
and command paths stay unchanged when copied into immutable run custody.
"""

from __future__ import annotations

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

PROFILE = "retrieval-contract"
RAILS = {"retrieval-sdk": "sdk", "retrieval-contract": "contract"}


def require_registered_owner(entry: dict, producer: dict, rail: str) -> None:
    if (
        producer.get("kind") != "just-recipe"
        or producer.get("recipe") != f"retrieval-{rail}-proof"
        or entry.get("payload") != "proof"
        or entry.get("validator") != "retrieval-proof"
        or entry.get("scorer") != "none"
        or entry.get("native_schema") != "retrieval-execution-context:v1"
        or entry.get("gate_tier") != "contract"
        or entry.get("host_policy") != "any"
        or entry.get("result_unit") != "count"
    ):
        raise EvidenceError("retrieval proof registration differs from implemented owner contract")


def target_identity(context: dict) -> str:
    version = context["tools"]["rustc"]["version"]
    hosts = [
        line.removeprefix("host: ") for line in version.splitlines() if line.startswith("host: ")
    ]
    if len(hosts) != 1 or not hosts[0]:
        raise EvidenceError("retrieval proof rustc identity has no unique host triple")
    targets = {
        command["inherited_environment"].get("CARGO_BUILD_TARGET", hosts[0])
        for command in context["commands"]
    }
    if len(targets) != 1 or not all(isinstance(value, str) and value for value in targets):
        raise EvidenceError("retrieval proof has inconsistent build target identity")
    return targets.pop()


def proof_payload(native: Path, context: dict, source: dict) -> dict:
    if not isinstance(context, dict) or context.get("rail") not in {"sdk", "contract"}:
        raise EvidenceError("retrieval proof context has an unknown or missing rail")
    names = (
        ["sdk_results.json"]
        if context["rail"] == "sdk"
        else ["contract_python_results.json", "contract_rust_results.json"]
    )
    summaries = [parse_json(_read_regular_file(native / name).decode()) for name in names]
    for summary in summaries:
        if not isinstance(summary, dict) or any(
            type(summary.get(key)) is not int
            for key in ("selected", "executed", "passed", "failed")
        ):
            raise EvidenceError("proof summary has missing or malformed terminal counts")
        if (
            summary["selected"] < 1
            or summary["selected"] != summary["executed"]
            or (
                summary["passed"] + summary["failed"] != summary["executed"]
                or summary["failed"] != 0
            )
        ):
            raise EvidenceError("proof summary is incomplete or failed")
    closure = parse_json(_read_regular_file(native / "source-closure.json").decode())
    if (
        not isinstance(closure, dict)
        or context.get("revision") != source["revision"]
        or closure.get("revision") != source["revision"]
    ):
        raise EvidenceError("retrieval proof does not bind the current frozen source")
    payload = {
        "kind": "proof",
        "rail": f"retrieval-{context['rail']}",
        **{
            key: sum(summary[key] for summary in summaries)
            for key in ("selected", "executed", "passed", "failed")
        },
        "source_digest": source["closure_digest"],
        "execution_context_digest": digest_bytes(
            _read_regular_file(native / "execution-context.json")
        ),
    }
    validate_payload(payload)
    return payload


def replay_run(store: RunStore, evidence: dict) -> None:
    from tools.benchmark.retrieval import portable_proof

    family = evidence["family"]
    if (
        family not in RAILS
        or evidence["profile"] != PROFILE
        or evidence["payload"]["kind"] != "proof"
    ):
        raise EvidenceError("retrieval proof family/profile/payload mismatch")
    raw = store.run_dir(evidence["run_id"]) / "raw"
    origin = parse_json(_read_regular_file(raw / "capture-origin.json").decode())
    if not isinstance(origin, dict) or set(origin) != {"capture_id", "execution_root", "producer"}:
        raise EvidenceError("retrieval proof origin is malformed")
    _run_id(origin["capture_id"])
    if (
        not isinstance(origin["execution_root"], str)
        or not Path(origin["execution_root"]).is_absolute()
    ):
        raise EvidenceError("retrieval proof origin has no absolute execution root")
    if not evidence["run_id"].startswith(origin["capture_id"] + "-"):
        raise EvidenceError("retrieval proof mixes captures")
    context = parse_json(_read_regular_file(raw / "execution-context.json").decode())
    if not isinstance(context, dict) or not isinstance(context.get("binaries"), dict):
        raise EvidenceError("retrieval proof context is malformed")
    if context.get("rail") != RAILS[family] or origin["producer"] != evidence["command"]:
        raise EvidenceError("retrieval proof command/rail differs from captured execution")
    if evidence["command"]["argv"] != [
        "just",
        f"retrieval-{RAILS[family]}-proof",
        origin["execution_root"],
    ]:
        raise EvidenceError("retrieval proof command differs from the registered producer")
    expected_binaries = {"runner", "searchd"} if RAILS[family] == "sdk" else set()
    if set(context["binaries"]) != expected_binaries:
        raise EvidenceError("retrieval proof binary role inventory is malformed")
    binary_files = {name: raw / f"frozen-binary-{name}" for name in context["binaries"]}
    checked = portable_proof.validate(
        raw / "execution-context.json",
        execution_root=Path(origin["execution_root"]),
        binary_files=binary_files,
    )
    if proof_payload(raw, checked, evidence["source"]) != evidence["payload"]:
        raise EvidenceError("retrieval proof typed counts differ from raw evidence")
    closure = parse_json(_read_regular_file(raw / "source-closure.json").decode())
    expected_inputs = {
        "execution-context": evidence["payload"]["execution_context_digest"],
        "native-source": "sha256:" + closure["digest"],
    }
    if (
        len(evidence["inputs"]) != len(expected_inputs)
        or {
            item["id"]: item["digest"]
            for item in evidence["inputs"]
            if item["availability"] == "present"
        }
        != expected_inputs
    ):
        raise EvidenceError("retrieval proof input identity mismatch")
    binaries = [
        {"name": name, "sha256": "sha256:" + entry["sha256"]}
        for name, entry in sorted(context["binaries"].items())
    ]
    if (
        binaries != evidence["build"]["binaries"]
        or evidence["build"]["toolchain"] != context["tools"]["rustc"]["version"]
        or evidence["build"]["target_triple"] != target_identity(context)
    ):
        raise EvidenceError("retrieval proof binary/toolchain inventory mismatch")


def capture(repo: Path, root: Path, registry: dict, timeout: int) -> dict:
    from benchctl import require_clean_worktree, require_frozen_source, resolve_checkout_head

    from tools.benchmark.retrieval import portable_proof

    if repo.resolve() != ROOT:
        raise EvidenceError("retrieval capture driver must come from the requested checkout")
    if root.resolve().is_relative_to(repo.resolve()):
        raise EvidenceError("retrieval evidence root must stay outside the checkout")
    _directories(root)
    require_clean_worktree(repo)
    head = resolve_checkout_head(repo)
    source = source_identity(repo, "benchmark-retrieval")
    native_source = source_identity(repo, "retrieval")
    cpu = os.cpu_count()
    if type(cpu) is not int or cpu < 1:
        raise EvidenceError("cannot establish proof host CPU inventory")
    capture_id = f"retrieval-contract-{uuid.uuid4().hex}"
    prepared = []
    selected = registry["profiles"][PROFILE]["families"]
    if set(selected) != set(RAILS):
        raise EvidenceError("retrieval contract inventory differs from implemented owners")
    for family in selected:
        entry = registry["families"][family]
        producer = registry["producers"][entry["producer"]]
        require_registered_owner(entry, producer, RAILS[family])
        native = root / "work" / capture_id / family
        native.parent.mkdir(parents=True, exist_ok=True)
        print(f"Retrieval contract capture: {family}", flush=True)
        stdout, stderr, command = execute(
            ["just", producer["recipe"], str(native)],
            cwd=repo,
            env=dict(os.environ),
            timeout=timeout,
        )
        context = portable_proof.validate(native / "execution-context.json")
        if context["rail"] != RAILS[family]:
            raise EvidenceError("retrieval producer emitted the wrong rail")
        require_frozen_source(repo, head)
        closure = parse_json(_read_regular_file(native / "source-closure.json").decode())
        if "sha256:" + closure["digest"] != native_source["closure_digest"]:
            raise EvidenceError(
                "native proof source closure differs from the frozen producer source"
            )
        derived = proof_payload(native, context, source)
        raw = {}
        for path in sorted(native.iterdir()):
            if not path.is_file() or path.is_symlink():
                raise EvidenceError("retrieval proof directory has an unexpected non-file entry")
            raw[path.name] = _read_regular_file(path)
        raw.update(
            {
                "producer.stdout": stdout,
                "producer.stderr": stderr,
                "capture-origin.json": canonical_json(
                    {"capture_id": capture_id, "execution_root": str(native), "producer": command}
                ).encode(),
            }
        )
        for name, binary in context["binaries"].items():
            content = _read_regular_file(Path(binary["path"]))
            if digest_bytes(content) != "sha256:" + binary["sha256"]:
                raise EvidenceError("retrieval proof binary changed during freeze")
            raw[f"frozen-binary-{name}"] = content
        prepared.append((family, context, derived, raw, command))
    require_frozen_source(repo, head)
    host = host_identity(
        policy="any",
        os_name="macos" if os.name != "nt" and platform.system() == "Darwin" else "linux",
        arch=platform.machine(),
        cpu_count=cpu,
        hostname=socket.gethostname(),
        lease_mode="none",
        lease_samples=0,
    )
    with custody(root):
        (root / "captures").mkdir(exist_ok=True)
        runs = []
        for family, context, derived, raw, command in prepared:
            items = list(raw.items())
            result = promote_native_run(
                evidence_root=root,
                run_id=f"{capture_id}-{family}",
                family=family,
                profile=PROFILE,
                case_id=None,
                created_utc=datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
                native_path=Path(items[0][0]),
                native_bytes=items[0][1],
                additional_native=[(Path(name), data) for name, data in items[1:]],
                payload=derived,
                source=source,
                build={
                    "toolchain": context["tools"]["rustc"]["version"],
                    "target_triple": target_identity(context),
                    "lockfile_digest": digest_bytes((repo / "Cargo.lock").read_bytes()),
                    "profile": "proof",
                    "flags": [],
                    "binaries": [
                        {"name": name, "sha256": "sha256:" + entry["sha256"]}
                        for name, entry in sorted(context["binaries"].items())
                    ],
                },
                inputs=[
                    {
                        "id": "execution-context",
                        "availability": "present",
                        "digest": derived["execution_context_digest"],
                        "reason": None,
                    },
                    {
                        "id": "native-source",
                        "availability": "present",
                        "digest": native_source["closure_digest"],
                        "reason": None,
                    },
                ],
                host=host,
                command=command,
                boundary={
                    "clock": "monotonic",
                    "instrumentation": "none",
                    "start_event": "proof_producer_start",
                    "end_event": "terminal_inventory_verified",
                },
                verdict={"scope": "contract", "status": "pass", "reason": None, "metrics": []},
            )
            runs.append(result["run_id"])
            replay_run(RunStore(root), RunStore(root).load(result["run_id"]))
        require_frozen_source(repo, head)
        return commit_capture(
            root,
            capture_id=capture_id,
            profile=PROFILE,
            registry_digest=registry_digest(registry),
            expected_cases={family: [None] for family in selected},
            run_ids=runs,
        )


def validate(repo: Path, root: Path, registry: dict) -> dict:
    document = load_capture(root, profile=PROFILE, registry_digest=registry_digest(registry))
    if document["source"] != source_identity(repo, "benchmark-retrieval"):
        raise EvidenceError("retrieval proof source is stale or dirty")
    if document["expected_cases"] != {
        family: [None] for family in registry["profiles"][PROFILE]["families"]
    }:
        raise EvidenceError("retrieval proof profile inventory differs from registry")
    store = RunStore(root)
    for record in document["runs"]:
        if not record["run_id"].startswith(document["capture_id"] + "-"):
            raise EvidenceError("retrieval proof profile mixes captures")
        evidence = store.load(record["run_id"])
        if evidence["build"]["lockfile_digest"] != digest_bytes((repo / "Cargo.lock").read_bytes()):
            raise EvidenceError("retrieval proof lockfile is stale")
        replay_run(store, evidence)
    return document
