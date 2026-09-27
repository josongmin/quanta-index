"""Live five-product workflow owned by benchctl; existing scorers own metrics."""

from __future__ import annotations

import json
import os
import sys
import uuid
from pathlib import Path

import corpus_release
from evidence import _read_control_file, digest_bytes
from evidence_bridge import source_identity
from producer_execution import execute

from tools.benchmark.retrieval import live_lexical_external as live
from tools.benchmark.retrieval import query_plan, run


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
    run.preflight_capture(pair)
    return pair, external


def _command(repo: Path, root: Path, name: str, args: list[str], timeout: int) -> None:
    print(f"code-search: {name}", flush=True)
    execute([sys.executable, str(repo / "tools/benchmark/benchctl.py"), *args],
            cwd=repo, env=dict(os.environ), timeout=timeout, log_dir=root / "logs" / name)


def capture(repo: Path, spec_path: Path) -> dict:
    from benchctl import require_clean_worktree, require_frozen_source, resolve_checkout_head

    require_clean_worktree(repo)
    head = resolve_checkout_head(repo)
    source = source_identity(repo, "retrieval")
    spec = _read_spec(spec_path)
    root = Path(spec["output_root"])
    corpus_release.external(root)
    if root.exists() or root.is_symlink():
        raise ValueError("workflow output must be fresh; never overwrite or resume partial runs")
    pair, external = preflight(Path(spec["pair_spec"]), Path(spec["external_spec"]))
    root.mkdir(parents=True)
    timeout = spec["timeout_secs"]
    _write(root / "workflow-spec.json", spec)
    pair["output_root"] = str(root / "native-pair")
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
        require_frozen_source(repo, head)
        if source_identity(repo, "retrieval") != source:
            raise ValueError("workflow source closure changed during execution")
        binding = live.verify(Path(external["output_root"]))["binding"]
        result = {"schema_version": 1, "status": "diagnostic_unqualified", "source": source,
            "binding": binding, "products": ["quanta_lexical", "semble_lexical_only", *live.lexical.PRODUCTS],
            "tasks": len(live._json(_read_control_file(Path(pair["query_pack"])))["tasks"]),
            "input_sha256": {name: digest_bytes(_read_control_file(root / name))
                for name in ("workflow-spec.json", "pair-spec.json", "external-spec.json", "lexical-spec.json")},
            "exclusions": ["independent_gold", "qualified_speed", "backend_indexed_universe_attestation"]}
        _write(root / "workflow.json", result)
        return result
    except BaseException as error:
        _write(root / "failure.json", {"status": "failed", "error_type": type(error).__name__,
                                       "error": str(error), "source": source})
        raise


def verify(repo: Path, root: Path) -> dict:
    from benchctl import require_clean_worktree

    require_clean_worktree(repo)
    result = live._json(_read_control_file(root / "workflow.json"))
    if result.get("status") != "diagnostic_unqualified" or result.get("source") != source_identity(repo, "retrieval"):
        raise ValueError("workflow claim or exact source identity differs")
    expected = {"workflow-spec.json", "pair-spec.json", "external-spec.json", "lexical-spec.json"}
    if set(result.get("input_sha256", {})) != expected:
        raise ValueError("workflow input inventory differs")
    for name in expected:
        if digest_bytes(_read_control_file(root / name)) != result["input_sha256"][name]:
            raise ValueError("workflow spec bytes changed")
    pair, external = preflight(root / "pair-spec.json", root / "external-spec.json")
    external_capture = live.verify(Path(external["output_root"]))
    if external_capture["binding"] != result["binding"] or external_capture["tasks"] != result["tasks"]:
        raise ValueError("workflow and external capture binding differ")
    timeout = _read_spec(root / "workflow-spec.json")["timeout_secs"]
    # Fresh log root makes validation repeatable without replacing old receipts.
    logs = root / "verifications" / uuid.uuid4().hex
    for profile, evidence in (("retrieval-diagnostic", root / "pair-evidence"),
                              ("lexical-diagnostic", root / "lexical-evidence")):
        _command(repo, logs, profile, ["validate", profile, "--evidence-root", str(evidence)], timeout)
    for family, evidence in (("retrieval-pair", root / "pair-evidence"),
                             ("lexical-file-comparison", root / "lexical-evidence")):
        _command(repo, logs, family, ["replay", "--family", family, "--evidence-root", str(evidence)], timeout)
    return result
