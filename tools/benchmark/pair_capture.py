"""Common paired-capture custody; native RB owns execution and every score.

The diagnostic bridge keeps the original native tree and a self-contained Git
snapshot. It never upgrades native verdicts to quality/performance admission.
"""

from __future__ import annotations

import io
import math
import os
import platform
import socket
import sys
import tempfile
import uuid
import zipfile
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath

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

from tools.benchmark.retrieval import evaluator  # noqa: E402
from tools.benchmark.retrieval import run as owner  # noqa: E402

PROFILE = "retrieval-diagnostic"
FAMILY = "retrieval-pair"
MODULE = "tools.benchmark.retrieval.run"
INPUT_ROLES = ("manifest", "suite", "query_pack", "host_profile", "semble_lockfile")
METRICS = {
    "file": "file_recall_at_10",
    "context": "chunk_recall_at_10",
    "span": "exact_index_span_recall_at_10",
}
GIT_ENV = {
    "GIT_OPTIONAL_LOCKS": "0",
    "GIT_TERMINAL_PROMPT": "0",
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_CONFIG_NOSYSTEM": "1",
}
BINARY_NAMES = {"driver_python", "runner", "searchd", "semble_python"}
INPUT_ARTIFACTS = {
    "manifest": "corpus_manifest",
    "suite": "suite",
    "query_pack": "query_pack",
    "host_profile": "host_profile",
    "semble_lockfile": "semble_lockfile",
}


def pack_native(root: Path) -> bytes:
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", compression=zipfile.ZIP_STORED) as archive:
        for name, data in tree_bytes(root).items():
            archive.writestr(name, data)
    return buffer.getvalue()


def unpack_native(data: bytes, destination: Path) -> None:
    """Restore only sorted unique regular entries; never extractall or links."""
    try:
        _unpack_native(data, destination)
    except (zipfile.BadZipFile, zipfile.LargeZipFile, RuntimeError, NotImplementedError) as exc:
        raise EvidenceError("native tree archive is malformed or unreadable") from exc


def _unpack_native(data: bytes, destination: Path) -> None:
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        names = archive.namelist()
        if not names or names != sorted(set(names)):
            raise EvidenceError("native tree inventory is empty, duplicate or reordered")
        for entry in archive.infolist():
            name = entry.filename
            path = PurePosixPath(name)
            mode = (entry.external_attr >> 16) & 0o170000
            if (
                path.is_absolute()
                or path.as_posix() != name
                or "\\" in name
                or any(part in {"", ".", ".."} for part in name.split("/"))
                or mode not in {0, 0o100000}
                or entry.is_dir()
                or entry.compress_type != zipfile.ZIP_STORED
                or entry.flag_bits & 1
                or entry.file_size > len(data)
            ):
                raise EvidenceError("native tree has unsafe/non-regular/compressed entry")
            target = destination / name
            target.parent.mkdir(parents=True, exist_ok=True)
            with target.open("xb") as handle:
                handle.write(archive.read(entry))


def require_disjoint_paths(root: Path, output: Path, corpus: Path) -> None:
    """Refuse overlapping mutable producer/custody and original corpus roots."""
    paths = [path.resolve() for path in (root, output, corpus)]
    for index, first in enumerate(paths):
        for second in paths[index + 1 :]:
            if first.is_relative_to(second) or second.is_relative_to(first):
                raise EvidenceError("pair evidence, native output and corpus roots overlap")


def bound_inputs(native: Path, manifest: dict, raw: Path) -> list[dict]:
    inputs = []
    for role in INPUT_ROLES:
        data = _read_regular_file(raw / f"input-{role}")
        actual = owner._resolve_artifact(native, manifest["artifacts"][INPUT_ARTIFACTS[role]], role)
        if _read_regular_file(actual) != data:
            raise EvidenceError("native frozen inputs differ from common capture inputs")
        inputs.append(
            {"id": role, "availability": "present", "digest": digest_bytes(data), "reason": None}
        )
    return inputs


def bind_runtime(native: Path, manifest: dict, binaries: list[dict], revision: str) -> None:
    hashes = {entry["name"]: entry["sha256"].removeprefix("sha256:") for entry in binaries}
    protocol = owner.read_json(
        owner._resolve_artifact(native, manifest["artifacts"]["protocol_lock"], "protocol")
    )
    if (
        manifest["provenance"]["quanta"]["source_sha"] != revision
        or manifest["provenance"]["quanta"]["binary_digest"] != hashes["runner"]
        or manifest["provenance"]["semble"]["interpreter_digest"] != hashes["semble_python"]
        or protocol["searchd_expected_sha256"] != hashes["searchd"]
    ):
        raise EvidenceError("native pair source or executable differs from the captured identity")


def require_registration(registry: dict) -> None:
    if registry["profiles"][PROFILE]["families"] != [FAMILY]:
        raise EvidenceError("pair profile inventory differs from the implemented owner")
    entry = registry["families"][FAMILY]
    if any(
        entry[key] != value
        for key, value in {
            "payload": "retrieval",
            "result_unit": "ratio",
            "producer": "retrieval-pair",
            "validator": "retrieval-pair",
            "scorer": "retrieval-relevance",
            "native_schema": f"retrieval-run-manifest:v{owner.MANIFEST_VERSION}",
            "host_policy": "any",
            "gate_tier": "diagnostic",
            "baseline": "none",
        }.items()
    ) or registry["producers"][entry["producer"]] != {
        "kind": "python-module",
        "module": "tools/benchmark/retrieval/run.py",
        "argv": ["pair", "--spec"],
        "requires_spec": True,
        "outputs": [],
    }:
        raise EvidenceError("pair registration differs from the native execution contract")


def tree_bytes(root: Path) -> dict[str, bytes]:
    """No followed links, special files, path aliases or partial inventories."""
    if root.is_symlink() or not root.is_dir():
        raise EvidenceError("pair tree must be a regular directory")
    result = {}
    for directory, directories, files in os.walk(root, followlinks=False):
        base = Path(directory)
        if any((base / name).is_symlink() for name in directories):
            raise EvidenceError("pair tree contains a symlinked directory")
        for name in files:
            path = base / name
            relative = path.relative_to(root).as_posix()
            result[relative] = _read_regular_file(path)
    if not result:
        raise EvidenceError("pair tree is empty")
    return dict(sorted(result.items()))


def clone_corpus(source: Path, destination: Path, commit: str, timeout: int) -> None:
    """A self-contained Git clone, not forged HEAD metadata or shared objects."""
    evaluator.verify_repo(source, commit)
    execute(
        [
            "git",
            "clone",
            "--no-local",
            "--single-branch",
            "--",
            str(source.resolve()),
            str(destination),
        ],
        cwd=ROOT,
        env={**os.environ, **GIT_ENV},
        timeout=timeout,
    )
    evaluator.verify_repo(destination, commit)
    if (destination / ".git" / "objects" / "info" / "alternates").exists():
        raise EvidenceError("corpus clone depends on external Git objects")


def restore_corpus(bundle: Path, destination: Path, timeout: int = 300) -> None:
    """Restore Git object/tree identity and executable modes outside raw custody."""
    environment = {**os.environ, **GIT_ENV}
    for argv in (
        ["git", "init", "--", str(destination)],
        [
            "git",
            "-C",
            str(destination),
            "-c",
            "protocol.file.allow=always",
            "fetch",
            "--no-tags",
            str(bundle),
            "HEAD",
        ],
        ["git", "-C", str(destination), "checkout", "--detach", "FETCH_HEAD"],
    ):
        execute(argv, cwd=ROOT, env=environment, timeout=timeout)


def owner_digests() -> dict[str, str]:
    files = [Path(__file__), *sorted(Path(owner.__file__).parent.glob("*.py"))]
    return {
        path.relative_to(ROOT).as_posix(): digest_bytes(_read_regular_file(path)) for path in files
    }


def derive(native: Path, corpus: Path, timeout: int = 300) -> tuple[dict, dict]:
    """Re-run the existing native verdict, including its report re-scoring."""
    manifest = owner._validate_manifest_shape(owner.read_json(native / "run-manifest.json"))
    suite = owner._resolve_artifact(native, manifest["artifacts"]["suite"], "pair suite")
    with tempfile.TemporaryDirectory(prefix="quanta-pair-verdict-") as scratch:
        out = Path(scratch).resolve() / "verdict.json"
        execute(
            [
                sys.executable,
                "-m",
                MODULE,
                "verdict",
                "--repo",
                str(corpus),
                "--suite",
                str(suite),
                "--run-manifest",
                str(native / "run-manifest.json"),
                "--out",
                str(out),
            ],
            cwd=ROOT,
            env={**os.environ, **GIT_ENV},
            timeout=timeout,
        )
        verdict = parse_json(_read_regular_file(out).decode())
    recorded = parse_json(_read_regular_file(native / "verdict.json").decode())
    if verdict != recorded:
        raise EvidenceError("native pair verdict differs from raw owner recomputation")
    # A failed pair cannot lend authority to numeric reports. Preserve its raw
    # failure tree in work, but do not publish a scored complete profile.
    if verdict["states"]["PAIR_VALID"] != "pass":
        raise EvidenceError("native PAIR_VALID is not pass; no scored profile published")
    return manifest, verdict


def typed_payloads(native: Path, manifest: dict) -> dict[str, dict]:
    artifacts = manifest["artifacts"]
    protocol = owner.read_json(
        owner._resolve_artifact(native, artifacts["protocol_lock"], "protocol")
    )
    pack = owner.read_json(owner._resolve_artifact(native, artifacts["query_pack"], "pack"))
    tasks = [task["task_id"] for task in pack["tasks"]]
    if not tasks or len(tasks) != len(set(tasks)):
        raise EvidenceError("pair task inventory is empty or duplicate")
    strategies = protocol["strategies"]
    routes = [*protocol["quanta_routes"], protocol["semble_route"]]
    if len(strategies) != len(set(strategies)) or len(routes) != len(set(routes)):
        raise EvidenceError("pair strategy/route inventory is duplicate")
    reports = {}
    for ref in artifacts["reports"]:
        report = owner.read_json(owner._resolve_artifact(native, ref, "report"))
        record_digest = report["runner_record_sha256"]
        reports.setdefault(record_digest, []).append(report)
    # The native verdict has already matched each report to its unique strategy.
    verdict = owner.read_json(native / "verdict.json")
    by_strategy = {}
    for comparison in verdict["comparisons"]:
        strategy = comparison["strategy"]
        candidates = reports.get(comparison["record_digest"], [])
        if not candidates:
            raise EvidenceError("pair comparison has no raw scored report")
        for report in candidates:
            if (
                strategy in by_strategy
                and by_strategy[strategy]["per_query"] != report["per_query"]
            ):
                raise EvidenceError("pair reports disagree on per-query observations")
            by_strategy[strategy] = report
    if set(by_strategy) != set(strategies):
        raise EvidenceError("pair scored strategy inventory is incomplete")
    result = {}
    for strategy in strategies:
        report = by_strategy[strategy]
        rows = report["per_query"]
        indexed = {(row["task_id"], row["route"]): row for row in rows}
        if len(indexed) != len(rows) or set(indexed) != {
            (task, route) for task in tasks for route in routes
        }:
            raise EvidenceError("pair scored task/route inventory is incomplete or duplicate")
        span_rows = report.get("span_accounting", {}).get("per_query", [])
        spans = {(row["task_id"], row["route"]): row for row in span_rows}
        if len(spans) != len(span_rows):
            raise EvidenceError("pair indexed-span inventory has duplicates")
        for route in routes:
            profile = protocol["execution_profiles"]
            native_default = (
                profile["semble"]["mode"] == "native-default"
                if route == protocol["semble_route"]
                else profile["quanta"]["policy"] == "native"
            )
            for space, metric in METRICS.items():
                typed = []
                for task in tasks:
                    row = indexed[(task, route)]
                    state, value = (
                        "judged",
                        row.get(metric)
                        if space != "span"
                        else spans.get((task, route), {}).get(metric),
                    )
                    if row["status"] == "timeout":
                        state, value = "timeout", None
                    elif row["status"] == "unavailable":
                        state, value = "unsupported", None
                    elif row["status"] not in evaluator.SCORED_STATUSES:
                        raise EvidenceError("pair row lacks an admissible terminal scored state")
                    elif not row["answerable"]:
                        state, value = "no_answer", float(row["status"] == "abstained")
                    elif space == "span" and (task, route) not in spans:
                        state, value = "unsupported", None
                    if state in {"judged", "no_answer"}:
                        if (
                            type(value) not in (int, float)
                            or not math.isfinite(value)
                            or not 0 <= value <= 1
                        ):
                            raise EvidenceError(
                                "pair metric is missing, malformed or outside [0,1]"
                            )
                        value = float(value)
                    typed.append(
                        {
                            "query_id": task,
                            "metric": metric if state != "no_answer" else "no_answer_abstention",
                            "unit": "ratio",
                            "value": value,
                            "state": state,
                        }
                    )
                payload = {
                    "kind": "retrieval",
                    "lane": "native_default" if native_default else "controlled_mechanism",
                    "metric_space": space,
                    "judgments": "judged",
                    "unjudged": 0,
                    "universe_attested": False,
                    "corpus_digest": "sha256:" + manifest["provenance"]["corpus"]["digest"],
                    "query_pack_digest": "sha256:"
                    + manifest["provenance"]["suite"]["query_pack_digest"],
                    "rows": typed,
                }
                validate_payload(payload)
                result[f"{strategy}.{route}.{space}"] = payload
    return result


def replay_run(store: RunStore, evidence: dict) -> list[str]:
    with custody(store.root):
        raw = store.run_dir(evidence["run_id"]) / "raw"
        origin = parse_json(_read_regular_file(raw / "capture-origin.json").decode())
        if (
            not isinstance(origin, dict)
            or set(origin)
            != {
                "capture_id",
                "producer",
                "owner_digests",
                "binaries",
                "toolchain",
                "inputs",
                "native_root",
            }
            or origin["owner_digests"] != owner_digests()
        ):
            raise EvidenceError("pair origin is malformed or native owner changed")
        _run_id(origin["capture_id"])
        execution_root = Path(origin["native_root"])
        if not execution_root.is_absolute() or ".." in execution_root.parts:
            raise EvidenceError("pair execution root is not canonical")
        if (
            not isinstance(origin["binaries"], list)
            or len(origin["binaries"]) != len(BINARY_NAMES)
            or any(
                not isinstance(entry, dict)
                or set(entry) != {"name", "sha256"}
                or entry["name"] not in BINARY_NAMES
                for entry in origin["binaries"]
            )
            or {entry["name"] for entry in origin["binaries"]} != BINARY_NAMES
        ):
            raise EvidenceError("pair binary inventory is incomplete or malformed")
        if (
            evidence["profile"] != PROFILE
            or evidence["family"] != FAMILY
            or evidence["run_id"] != origin["capture_id"] + "-" + evidence["case_id"]
        ):
            raise EvidenceError("pair replay mixes profile, case or capture identities")
        if evidence["command"] != origin["producer"] or evidence["command"]["argv"] != [
            evidence["command"]["argv"][0],
            "-m",
            MODULE,
            "pair",
            "--spec",
            str(Path(origin["native_root"]) / "frozen-spec.json"),
        ]:
            raise EvidenceError("pair command differs from the recorded native producer")
        build = evidence["build"]
        binaries = [
            {
                "name": entry["name"],
                "sha256": digest_bytes(_read_regular_file(raw / f"binary-{entry['name']}")),
            }
            for entry in origin["binaries"]
        ]
        if (
            build["binaries"] != binaries
            or binaries != origin["binaries"]
            or build["toolchain"] != origin["toolchain"]
        ):
            raise EvidenceError("pair executable/toolchain identity mismatch")
        original = owner.load_spec(raw / "original-spec.json")
        frozen = parse_json(_read_regular_file(raw / "frozen-spec.json").decode())
        if frozen != {
            **original,
            **{role: str(execution_root / f"input-{role}") for role in INPUT_ROLES},
            "repo": str(execution_root / "corpus"),
        }:
            raise EvidenceError("pair frozen spec differs from the original capture inputs")
        with tempfile.TemporaryDirectory(prefix="quanta-pair-corpus-") as scratch:
            corpus = Path(scratch).resolve() / "corpus"
            native = Path(scratch).resolve() / "native"
            restore_corpus(raw / "corpus.bundle", corpus)
            unpack_native(_read_regular_file(raw / "native-tree.zip"), native)
            manifest, _verdict = derive(native, corpus)
            bind_runtime(native, manifest, binaries, evidence["source"]["revision"])
            inputs = bound_inputs(native, manifest, raw)
            if evidence["inputs"] != inputs or inputs != origin["inputs"]:
                raise EvidenceError("pair input inventory differs from capture origin")
            payloads = typed_payloads(native, manifest)
        if (
            evidence["case_id"] not in payloads
            or evidence["payload"] != payloads[evidence["case_id"]]
        ):
            raise EvidenceError("pair typed metrics differ from the raw owner scores")
        return list(payloads)


def capture(repo: Path, root: Path, registry: dict, spec_path: Path, timeout: int) -> dict:
    from benchctl import require_clean_worktree, require_frozen_source, resolve_checkout_head

    require_registration(registry)
    if repo.resolve() != ROOT or root.resolve().is_relative_to(repo.resolve()):
        raise EvidenceError("pair requires its own checkout and an external evidence root")
    require_clean_worktree(repo)
    original = _read_regular_file(spec_path)
    with tempfile.TemporaryDirectory(prefix="quanta-pair-spec-") as scratch:
        frozen_input = Path(scratch).resolve() / "spec.json"
        frozen_input.write_bytes(original)
        spec = owner.load_spec(frozen_input)
    if any(key not in spec for key in (*INPUT_ROLES, "semble_python")):
        raise EvidenceError(
            "pair requires explicit corpus/suite/pack/host/lockfile/interpreter inputs"
        )
    if spec.get("scope", "exploratory") != "exploratory":
        raise EvidenceError(
            "diagnostic pair profile refuses qualified admission; use the native qualified rail"
        )
    external = [
        spec_path,
        *(Path(spec[role]) for role in INPUT_ROLES),
        Path(spec["repo"]),
        Path(spec["output_root"]),
    ]
    if any(path.resolve().is_relative_to(repo.resolve()) for path in external):
        raise EvidenceError("pair spec, corpus, inputs and output must stay outside the checkout")
    output = Path(spec["output_root"]).resolve()
    require_disjoint_paths(root, output, Path(spec["repo"]))
    _directories(root)
    head = resolve_checkout_head(repo)
    source = source_identity(repo, "benchmark-retrieval")
    capture_id = f"pair-{uuid.uuid4().hex}"
    work = root / "work" / capture_id
    work.mkdir(parents=True, exist_ok=False)
    contents = {role: _read_regular_file(Path(spec[role])) for role in INPUT_ROLES}
    for role, content in contents.items():
        (work / f"input-{role}").write_bytes(content)
    commit, _files = owner._manifest_rows(work / "input-manifest")
    clone_corpus(Path(spec["repo"]), work / "corpus", commit, timeout)
    binaries = {}
    paths = {
        "driver_python": Path(sys.executable).resolve(),
        "runner": Path(spec["runner_binary"]).resolve(),
        "searchd": Path(spec["searchd_binary"]).resolve(),
        "semble_python": Path(spec["semble_python"]).resolve(),
    }
    for name, path in paths.items():
        data = _read_regular_file(path)
        binaries[name] = digest_bytes(data)
        (work / f"binary-{name}").write_bytes(data)
    frozen = {
        **spec,
        **{role: str(work / f"input-{role}") for role in INPUT_ROLES},
        "repo": str(work / "corpus"),
    }
    (work / "original-spec.json").write_bytes(original)
    (work / "frozen-spec.json").write_text(canonical_json(frozen), encoding="utf-8")
    if _read_regular_file(spec_path) != original:
        raise EvidenceError("pair spec changed while freezing inputs")
    _stdout, _stderr, command = execute(
        [sys.executable, "-m", MODULE, "pair", "--spec", str(work / "frozen-spec.json")],
        cwd=repo,
        env={**os.environ, **GIT_ENV},
        timeout=timeout,
    )
    if any(
        digest_bytes(_read_regular_file(path)) != binaries[name] for name, path in paths.items()
    ):
        raise EvidenceError("pair executed binary changed during capture")
    manifest, _verdict = derive(output, work / "corpus")
    if manifest["provenance"]["quanta"]["source_sha"] != head:
        raise EvidenceError("pair producer belongs to a different source revision")
    payloads = typed_payloads(output, manifest)
    inventory = bound_inputs(output, manifest, work)
    execute(
        [
            "git",
            "-C",
            str(work / "corpus"),
            "bundle",
            "create",
            str(work / "corpus.bundle"),
            "HEAD",
        ],
        cwd=repo,
        env={**os.environ, **GIT_ENV},
        timeout=timeout,
    )
    raw = {"native-tree.zip": pack_native(output)}
    raw.update({path.name: _read_regular_file(path) for path in work.iterdir() if path.is_file()})
    binary_inventory = [{"name": name, "sha256": sha} for name, sha in sorted(binaries.items())]
    bind_runtime(output, manifest, binary_inventory, head)
    toolchain = f"Python {platform.python_version()}"
    raw.update(
        {
            "producer.stdout": _stdout,
            "producer.stderr": _stderr,
            "capture-origin.json": canonical_json(
                {
                    "capture_id": capture_id,
                    "producer": command,
                    "owner_digests": owner_digests(),
                    "binaries": binary_inventory,
                    "toolchain": toolchain,
                    "inputs": inventory,
                    "native_root": str(work),
                }
            ).encode(),
        }
    )
    cpu = os.cpu_count()
    if type(cpu) is not int or cpu < 1:
        raise EvidenceError("pair host CPU inventory unavailable")
    host = host_identity(
        policy="any",
        os_name="macos" if sys.platform == "darwin" else "linux",
        arch=platform.machine(),
        cpu_count=cpu,
        hostname=socket.gethostname(),
        lease_mode="none",
        lease_samples=0,
    )
    require_frozen_source(repo, head)
    with custody(root):
        runs = []
        for case, payload in payloads.items():
            items = list(raw.items())
            result = promote_native_run(
                evidence_root=root,
                run_id=capture_id + "-" + case,
                family=FAMILY,
                profile=PROFILE,
                case_id=case,
                created_utc=datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
                native_path=Path(items[0][0]),
                native_bytes=items[0][1],
                additional_native=[(Path(name), data) for name, data in items[1:]],
                payload=payload,
                source=source,
                build={
                    "toolchain": toolchain,
                    "target_triple": f"{sys.platform}-{platform.machine()}",
                    "lockfile_digest": digest_bytes(_read_regular_file(repo / "uv.lock")),
                    "profile": "paired-native-diagnostic",
                    "flags": [],
                    "binaries": binary_inventory,
                },
                inputs=inventory,
                host=host,
                command=command,
                boundary={
                    "clock": "recorded",
                    "instrumentation": "none",
                    "start_event": "native_pair_start",
                    "end_event": "native_pair_verdict",
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
            expected_cases={FAMILY: list(payloads)},
            run_ids=runs,
        )


def validate(repo: Path, root: Path, registry: dict) -> dict:
    from benchctl import require_clean_worktree

    require_clean_worktree(repo)
    require_registration(registry)
    document = load_capture(root, profile=PROFILE, registry_digest=registry_digest(registry))
    if document["source"] != source_identity(repo, "benchmark-retrieval"):
        raise EvidenceError("pair capture source differs from the current owner")
    store = RunStore(root)
    for row in document["runs"]:
        evidence = store.load(row["run_id"])
        if not evidence["run_id"].startswith(document["capture_id"] + "-") or evidence["build"][
            "lockfile_digest"
        ] != digest_bytes(_read_regular_file(repo / "uv.lock")):
            raise EvidenceError("pair profile mixes captures or stale lockfile")
        expected = {FAMILY: replay_run(store, evidence)}
        if document["expected_cases"] != expected:
            raise EvidenceError("pair complete-profile case inventory differs from native rows")
    return document
