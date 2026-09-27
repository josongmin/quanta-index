"""Live five-product workflow owned by benchctl; existing scorers own metrics."""

from __future__ import annotations

import json
import os
import secrets
import sys
import uuid
import zipfile
from pathlib import Path

import corpus_release
from evidence import RunStore, _read_control_file, canonical_json, digest_bytes
from evidence_bridge import source_identity
from producer_execution import execute
from profile_capture import load_capture
from registry import load_registry, registry_digest

from tools.benchmark.retrieval import evaluator, query_plan, run
from tools.benchmark.retrieval import live_lexical_external as live


def _write(path: Path, value: dict) -> None:
    live._write(path, json.dumps(value, sort_keys=True, indent=2).encode() + b"\n")


def _read_spec(path: Path) -> dict:
    value = live._json(_read_control_file(path))
    if (set(value) != {"schema_version", "pair_spec", "external_spec", "output_root", "timeout_secs"}
            or type(value["schema_version"]) is not int or value["schema_version"] != 1
            or type(value["timeout_secs"]) is not int or not 60 <= value["timeout_secs"] <= 86400):
        raise ValueError("code-search workflow requires closed schema v1 and bounded timeout")
    for key in ("pair_spec", "external_spec", "output_root"):
        if not isinstance(value[key], str) or not Path(value[key]).is_absolute() or ".." in Path(value[key]).parts:
            raise ValueError(f"workflow {key} must be canonical absolute")
    return value


def preflight(pair_path: Path, external_path: Path) -> tuple[dict, dict]:
    pair, external = run.load_spec(pair_path), live._spec(external_path)
    for role in ("suite", "query_pack"):
        if _read_control_file(Path(pair[role])) != _read_control_file(Path(external[role])):
            raise ValueError(f"pair and external {role} bytes differ")
    suite_raw = _read_control_file(Path(pair["suite"]))
    pack_raw = _read_control_file(Path(pair["query_pack"]))
    suite, pack = live._json(suite_raw), live._json(pack_raw)
    live.lexical._file_universe(suite, pack)
    live.lexical._tasks(suite, pack)
    expected_profiles = {
        "quanta": query_plan.execution_profile("native"),
        "semble": {"profile_id": "semble-lexical-only-v1", "mode": "lexical-only",
                   "alpha": None, "rerank": "not_applicable"},
    }
    if (pair["execution_profiles"] != expected_profiles or pair["routes"] != ["lexical"]
            or pair["candidate_route"] != "lexical" or pair["baseline_route"] != "semble-hybrid"
            or pair["scope"] != "exploratory" or any(pair["claims"].values())
            or pair.get("repetitions", 1) != 1 or len(pair["strategies"]) != 1):
        raise ValueError("live workflow requires one exploratory pure-lexical pair")
    release = Path(external["corpus"]["release_path"])
    document = corpus_release.validate(release)
    repository = next(row for row in document["repositories"]
                      if row["recipe"]["name"] == external["corpus"]["repository"])
    manifest_raw = _read_control_file(release / repository["views"][external["corpus"]["view"]]["manifest"])
    live.corpus_binding._bind(document, manifest_raw, external["corpus"], suite_raw, pack_raw)
    if _read_control_file(Path(pair["manifest"])) != manifest_raw:
        raise ValueError("pair manifest differs from selected release view")
    corpus_release.require_complete_git(Path(pair["repo"]))
    run.preflight_capture(pair)
    return pair, external


def _command(repo: Path, root: Path, name: str, args: list[str], timeout: int) -> None:
    print(f"code-search: {name}", flush=True)
    execute([sys.executable, str(repo / "tools/benchmark/benchctl.py"), *args],
            cwd=repo, env=dict(os.environ), timeout=timeout, log_dir=root / "logs" / name)


def _source(repo: Path) -> dict:
    source = source_identity(repo, "retrieval")
    evaluator.verify_repo(repo, source["revision"])
    if source["dirty"]:
        raise ValueError("workflow source closure is dirty")
    return source


def _native_pair_root(pair: dict, workflow_root: Path) -> tuple[Path, Path | None]:
    """Reserve a fresh short runtime path; permanent custody stays in evidence."""
    candidate = workflow_root / "native-pair"
    try:
        run.preflight_daemon_socket_paths(candidate.with_name(candidate.name + ".staging"),
                                         pair["strategies"], repetitions=1, paired=True)
        return candidate, None
    except run.RunError:
        if run._unix_socket_path_limit() is None:
            raise
    parent = Path("/tmp").resolve()
    for _ in range(128):
        candidate = parent / secrets.token_hex(2)
        reservation = parent / (candidate.name + ".code-search-reservation")
        try:
            descriptor = os.open(reservation, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        except FileExistsError:
            continue
        try:
            os.write(descriptor, str(workflow_root).encode())
        finally:
            os.close(descriptor)
        if candidate.exists() or candidate.with_name(candidate.name + ".staging").exists():
            reservation.unlink()
            continue
        try:
            run.preflight_daemon_socket_paths(candidate.with_name(candidate.name + ".staging"),
                                             pair["strategies"], repetitions=1, paired=True)
        except BaseException:
            reservation.unlink()
            raise
        return candidate, reservation
    raise ValueError("cannot reserve a fresh native pair path within the Unix socket limit")


def _cross_inputs(evidence: dict, expected: dict[str, str]) -> None:
    rows = evidence["inputs"]
    admitted = {row["id"]: row["digest"] for row in rows if row["availability"] == "present"}
    if len({row["id"] for row in rows}) != len(rows) or any(admitted.get(role) != digest
                                                          for role, digest in expected.items()):
        raise ValueError("lexical inputs differ from the workflow native observations")


def _components(root: Path, pair: dict, external: dict) -> dict:
    """Bind both complete captures and re-derive their shared observation bytes."""
    external_root = root / "native-external"
    if _read_control_file(external_root / "spec.json") != _read_control_file(root / "external-spec.json"):
        raise ValueError("external native spec differs from the workflow spec")
    registry = registry_digest(load_registry())
    documents = {
        name: load_capture(root / evidence, profile=profile, registry_digest=registry)
        for name, evidence, profile in (
            ("pair", "pair-evidence", "retrieval-diagnostic"),
            ("lexical", "lexical-evidence", "lexical-diagnostic"))
    }
    expected = {f"{name}_rows": "sha256:" + digest for name, digest in external["rows_sha256"].items()}
    native = root / "pair-evidence/runs" / documents["pair"]["runs"][0]["run_id"] / "raw/native-tree.zip"
    strategy = pair["strategies"][0]["name"]
    entries = {"pair_report": f"report-semble-hybrid-vs-lexical-{strategy}.json",
               "pair_lock": "protocol-lock.json", "semble_native": "rep-00/semble/native.json",
               "pair_verdict": "verdict.json"}
    with zipfile.ZipFile(native) as held:
        for role, name in entries.items():
            with held.open(name) as stream:
                raw = stream.read(16 * 1024 * 1024 + 1)
            if len(raw) > 16 * 1024 * 1024:
                raise ValueError("paired observation exceeds 16 MiB control limit")
            expected[role] = digest_bytes(raw)
    store = RunStore(root / "lexical-evidence")
    for row in documents["lexical"]["runs"]:
        _cross_inputs(store.load(row["run_id"]), expected)
    return {"native_external": digest_bytes(_read_control_file(external_root / "capture.json")),
            **{name: {"capture_id": document["capture_id"], "digest": document["digest"]}
               for name, document in documents.items()}}


def capture(repo: Path, spec_path: Path) -> dict:
    source = _source(repo)
    spec = _read_spec(spec_path)
    root = Path(spec["output_root"])
    corpus_release.external(root)
    if root.exists() or root.is_symlink():
        raise ValueError("workflow output must be fresh; never overwrite or resume partial runs")
    pair, external = preflight(Path(spec["pair_spec"]), Path(spec["external_spec"]))
    root.mkdir(parents=True)
    timeout = spec["timeout_secs"]
    _write(root / "workflow-spec.json", spec)
    native_pair, reservation = _native_pair_root(pair, root)
    pair["output_root"] = str(native_pair)
    pair["run_id"] = root.name
    external["output_root"] = str(root / "native-external")
    pair_path, external_path = root / "pair-spec.json", root / "external-spec.json"
    _write(pair_path, pair)
    _write(external_path, external)
    pair_evidence, lexical_evidence = root / "pair-evidence", root / "lexical-evidence"
    try:
        # Refuse service/auth/revision problems before paying for the SDK pair.
        _command(repo, root, "external-live", ["code-search", "external", "--spec", str(external_path)], timeout)
        _command(repo, root, "external-verify", ["code-search", "external-verify", "--capture", external["output_root"]], timeout)
        _command(repo, root, "pair-live", ["run", "retrieval-diagnostic", "--pair-spec", str(pair_path),
            "--evidence-root", str(pair_evidence), "--producer-timeout", str(timeout)], timeout)
        pair_root = Path(pair["output_root"])
        strategy = pair["strategies"][0]["name"]
        lexical_spec = {"schema_version": 2, "corpus": external["corpus"], "inputs": {
            "suite": pair["suite"], "query_pack": pair["query_pack"],
            "pair_report": str(pair_root / f"report-semble-hybrid-vs-lexical-{strategy}.json"),
            "pair_lock": str(pair_root / "protocol-lock.json"),
            "semble_native": str(pair_root / "rep-00/semble/native.json"),
            "pair_verdict": str(pair_root / "verdict.json"),
            **{f"{name}_rows": str(Path(external["output_root"]) / f"{name}_rows.jsonl")
               for name in live.lexical.PRODUCTS},
        }}
        lexical_path = root / "lexical-spec.json"
        _write(lexical_path, lexical_spec)
        _command(repo, root, "five-product-score", ["run", "lexical-diagnostic", "--lexical-spec", str(lexical_path),
            "--evidence-root", str(lexical_evidence), "--producer-timeout", str(timeout)], timeout)
        for profile, evidence in (("retrieval-diagnostic", pair_evidence), ("lexical-diagnostic", lexical_evidence)):
            _command(repo, root, profile + "-validate", ["validate", profile, "--evidence-root", str(evidence)], timeout)
        for family, evidence in (("retrieval-pair", pair_evidence), ("lexical-file-comparison", lexical_evidence)):
            _command(repo, root, family + "-replay", ["replay", "--family", family, "--evidence-root", str(evidence)], timeout)
        external_capture = live.verify(Path(external["output_root"]))
        binding = external_capture["binding"]
        components = _components(root, pair, external_capture)
        result = {"schema_version": 1, "status": "diagnostic_unqualified", "source": source,
            "binding": binding, "components": components, "products": ["quanta_lexical", "semble_lexical_only", *live.lexical.PRODUCTS],
            "tasks": len(live._json(_read_control_file(Path(pair["query_pack"])))["tasks"]),
            "input_sha256": {name: digest_bytes(_read_control_file(root / name))
                for name in ("workflow-spec.json", "pair-spec.json", "external-spec.json", "lexical-spec.json")},
            "exclusions": ["independent_gold", "qualified_speed", "backend_indexed_universe_attestation"]}
        if _source(repo) != source:
            raise ValueError("workflow source closure changed during execution")
        _write(root / "workflow.json", result)
        return result
    except BaseException as error:
        _write(root / "failure.json", {"status": "failed", "error_type": type(error).__name__,
                                       "error": str(error), "source": source})
        raise
    finally:
        if reservation is not None:
            reservation.unlink()


def verify(repo: Path, root: Path) -> dict:
    result = live._json(_read_control_file(root / "workflow.json"))
    if (set(result) != {"schema_version", "status", "source", "binding", "products", "tasks",
                        "input_sha256", "exclusions", "components"}
            or type(result.get("schema_version")) is not int or result["schema_version"] != 1
            or type(result.get("tasks")) is not int or result["tasks"] <= 0
            or not isinstance(result.get("components"), dict)
            or set(result["components"]) != {"native_external", "pair", "lexical"}
            or result.get("products") != ["quanta_lexical", "semble_lexical_only", *live.lexical.PRODUCTS]
            or result.get("exclusions") != ["independent_gold", "qualified_speed",
                                            "backend_indexed_universe_attestation"]):
        raise ValueError("unsupported workflow metadata or claim")
    if result.get("status") != "diagnostic_unqualified" or canonical_json(result.get("source")) != canonical_json(_source(repo)):
        raise ValueError("workflow claim or exact source identity differs")
    expected = {"workflow-spec.json", "pair-spec.json", "external-spec.json", "lexical-spec.json"}
    if set(result.get("input_sha256", {})) != expected:
        raise ValueError("workflow input inventory differs")
    for name in expected:
        if digest_bytes(_read_control_file(root / name)) != result["input_sha256"][name]:
            raise ValueError("workflow spec bytes changed")
    pair, external = preflight(root / "pair-spec.json", root / "external-spec.json")
    external_capture = live.verify(Path(external["output_root"]))
    if canonical_json(external_capture["binding"]) != canonical_json(result["binding"]) or external_capture["tasks"] != result["tasks"]:
        raise ValueError("workflow and external capture binding differ")
    if result["components"] != _components(root, pair, external_capture):
        raise ValueError("workflow native/component capture identities differ")
    timeout = _read_spec(root / "workflow-spec.json")["timeout_secs"]
    # Fresh log root makes validation repeatable without replacing old receipts.
    logs = root / "verifications" / uuid.uuid4().hex
    for profile, evidence in (("retrieval-diagnostic", root / "pair-evidence"),
                              ("lexical-diagnostic", root / "lexical-evidence")):
        _command(repo, logs, profile, ["validate", profile, "--evidence-root", str(evidence)], timeout)
    for family, evidence in (("retrieval-pair", root / "pair-evidence"),
                             ("lexical-file-comparison", root / "lexical-evidence")):
        _command(repo, logs, family, ["replay", "--family", family, "--evidence-root", str(evidence)], timeout)
    if canonical_json(result["source"]) != canonical_json(_source(repo)):
        raise ValueError("workflow source closure changed during verification")
    return result
