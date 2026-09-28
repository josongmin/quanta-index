"""Verify the declared complete code-search diagnostic matrix from native captures.

The release supplies the repository inventory. The matrix spec supplies the
query-family inventory; every repository/family must have all three modes.
Each cell is replayed through its existing native capture owner. This is a
completeness check, not independent gold or performance qualification.
"""

from __future__ import annotations

import re
from pathlib import Path

import code_search_workflow
import corpus_binding
import corpus_release
import pair_capture
from evidence import RunStore, _read_control_file, canonical_json, digest_bytes
from profile_capture import load_capture
from registry import load_registry, registry_digest

from tools.benchmark.retrieval import lexical_file_comparison as lexical
from tools.benchmark.retrieval import live_lexical_external as live
from tools.benchmark.retrieval import run as pair_run

MODES = ("lexical-only", "semantic-only", "hybrid")
PAIR_MODES = {
    "lexical-only": ("lexical", "lexical-only"),
    "semantic-only": ("semantic", "semantic-only"),
    "hybrid": ("hybrid", "hybrid-no-rerank"),
}
SHA = re.compile(r"sha256:[0-9a-f]{64}\Z")


def _path(value: object, where: str) -> Path:
    if (
        not isinstance(value, str)
        or not value
        or "\\" in value
        or "\x00" in value
        or not Path(value).is_absolute()
        or ".." in Path(value).parts
        or Path(value).as_posix() != value
    ):
        raise ValueError(f"matrix {where} must be a canonical absolute path")
    return Path(value)


def _spec(value: dict, repositories: set[str]) -> dict:
    if (
        set(value)
        != {"schema_version", "release_path", "release_digest", "query_families", "cells"}
        or type(value["schema_version"]) is not int
        or value["schema_version"] != 1
        or not isinstance(value["release_digest"], str)
        or SHA.fullmatch(value["release_digest"]) is None
    ):
        raise ValueError("matrix requires a closed schema v1 and release digest")
    _path(value["release_path"], "release_path")
    families = value["query_families"]
    if (
        not isinstance(families, list)
        or not families
        or any(
            not isinstance(name, str) or re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*", name) is None
            for name in families
        )
        or len(set(families)) != len(families)
    ):
        raise ValueError("matrix query-family inventory is empty, malformed or duplicate")
    cells = value["cells"]
    if not isinstance(cells, list):
        raise ValueError("matrix cells must be a list")
    observed: set[tuple[str, str]] = set()
    roots: set[Path] = set()
    family_input_paths: set[tuple[str, Path, Path]] = set()
    for cell in cells:
        if not isinstance(cell, dict) or set(cell) != {
            "repository",
            "view",
            "query_family",
            "suite",
            "query_pack",
            "captures",
        }:
            raise ValueError("matrix cell fields differ from the closed contract")
        if not isinstance(cell["repository"], str) or not isinstance(cell["query_family"], str):
            raise ValueError("matrix cell repository/query-family must be strings")
        key = (cell["repository"], cell["query_family"])
        if key[0] not in repositories or key[1] not in families or key in observed:
            raise ValueError("matrix has an unknown or duplicate repository/query-family cell")
        observed.add(key)
        if not isinstance(cell["view"], str) or cell["view"] not in corpus_release.VIEWS:
            raise ValueError("matrix cell view is unsupported")
        input_paths = (
            cell["repository"],
            _path(cell["suite"], "suite"),
            _path(cell["query_pack"], "query_pack"),
        )
        if input_paths in family_input_paths:
            raise ValueError("matrix reuses one suite/query pack as another query family")
        family_input_paths.add(input_paths)
        captures = cell["captures"]
        if not isinstance(captures, dict) or set(captures) != set(MODES):
            raise ValueError("matrix cell lacks a required mode")
        for mode in MODES:
            capture = captures[mode]
            if not isinstance(capture, dict) or set(capture) != {"kind", "root"}:
                raise ValueError("matrix capture fields differ from the closed contract")
            if not isinstance(capture["kind"], str) or capture["kind"] not in {"workflow", "pair"}:
                raise ValueError("matrix capture kind is unsupported")
            root = _path(capture["root"], "capture root")
            if root in roots:
                raise ValueError("matrix reuses a capture root for multiple modes")
            roots.add(root)
    if observed != {(repo, family) for repo in repositories for family in families}:
        raise ValueError("matrix omits a repository/query-family cell")
    return value


def _pair(repo: Path, root: Path, registry: dict) -> tuple[dict, dict, dict]:
    document = pair_capture.validate(repo, root, registry)
    expected_registry = registry_digest(registry)
    # validate() replays every case, but the matrix must also prove that all
    # cases belong to this one declared input and route. A first-run-only
    # comparison could admit a mixed capture with different later cases.
    loaded = load_capture(root, profile=pair_capture.PROFILE, registry_digest=expected_registry)
    if loaded != document:
        raise ValueError("matrix pair capture changed during verification")
    store = RunStore(root)
    first_spec = first_inputs = None
    for row in document["runs"]:
        run_id = row["run_id"]
        store.load(run_id)
        raw = root / "runs" / run_id / "raw"
        spec = pair_run.load_spec(raw / "original-spec.json")
        inputs = {
            role: _read_control_file(raw / f"input-{role}")
            for role in ("manifest", "suite", "query_pack")
        }
        if first_spec is None:
            first_spec, first_inputs = spec, inputs
        elif spec != first_spec or inputs != first_inputs:
            raise ValueError("matrix pair runs differ in spec or native inputs")
    if first_spec is None or first_inputs is None:
        raise ValueError("matrix pair has no validated runs")
    return document["source"], first_spec, first_inputs


def _mode(spec: dict, mode: str) -> None:
    route, semble = PAIR_MODES[mode]
    baseline = pair_run.SEMBLE_ROUTE_BY_MODE[semble]
    if (
        spec.get("scope") != "exploratory"
        or spec.get("claims")
        != {"quality": False, "speed": False, "same_model": False, "incremental": False}
        or spec.get("routes") != [route]
        or spec.get("candidate_route") != route
        or spec.get("baseline_route") != baseline
        or spec.get("semble_route") != baseline
        or not isinstance(spec.get("execution_profiles"), dict)
        or not isinstance(spec["execution_profiles"].get("semble"), dict)
        or spec["execution_profiles"]["semble"].get("mode") != semble
        or type(spec.get("top_k")) is not int
        or spec["top_k"] != 10
    ):
        raise ValueError(f"matrix {mode} pair uses another route, mode or claim")


def verify(repo: Path, spec_path: Path) -> dict:
    """Fail on any omitted, mismatched or unverified applicable matrix cell."""
    source = code_search_workflow._source(repo)
    # Read enough to discover the release path, then validate the release and
    # parse the complete matrix against its actual repository inventory.
    raw = live._json(_read_control_file(spec_path))
    if not isinstance(raw.get("release_path"), str):
        raise ValueError("matrix release_path is absent")
    release = _path(raw["release_path"], "release_path")
    document = corpus_release.validate(release)
    repositories = {row["recipe"]["name"] for row in document["repositories"]}
    if len(repositories) != len(document["repositories"]) or not repositories:
        raise ValueError("release repository inventory is empty or duplicate")
    spec = _spec(raw, repositories)
    if spec["release_digest"] != document["digest"]:
        raise ValueError("matrix release digest differs from the validated release")
    registry = load_registry(repo / "tools/benchmark/registry.toml")
    workflows = pairs = 0
    bindings = []
    family_input_digests: set[tuple[str, str, str]] = set()
    for cell in spec["cells"]:
        selection = {
            "release_path": str(release),
            "release_digest": document["digest"],
            "repository": cell["repository"],
            "view": cell["view"],
        }
        repository = next(
            row for row in document["repositories"] if row["recipe"]["name"] == cell["repository"]
        )
        manifest = _read_control_file(release / repository["views"][cell["view"]]["manifest"])
        suite = _read_control_file(_path(cell["suite"], "suite"))
        pack = _read_control_file(_path(cell["query_pack"], "query_pack"))
        input_digests = (
            cell["repository"],
            digest_bytes(suite),
            digest_bytes(pack),
        )
        if input_digests in family_input_digests:
            raise ValueError("matrix duplicates suite/query pack bytes across query families")
        family_input_digests.add(input_digests)
        binding = corpus_binding._bind(document, manifest, selection, suite, pack)
        suite_data, pack_data = live._json(suite), live._json(pack)
        tasks = pack_data.get("tasks")
        bare = (
            isinstance(tasks, list)
            and bool(tasks)
            and all(
                isinstance(task, dict)
                and isinstance(task.get("query"), str)
                and lexical.BARE_SYMBOL.fullmatch(task["query"]) is not None
                for task in tasks
            )
        )
        if bare:
            lexical._file_universe(suite_data, pack_data)
            lexical._tasks(suite_data, pack_data)
        for mode in MODES:
            capture = cell["captures"][mode]
            root = _path(capture["root"], "capture root")
            expected_kind = "workflow" if mode == "lexical-only" and bare else "pair"
            if capture["kind"] != expected_kind:
                raise ValueError(f"matrix {mode} capture kind differs from query support")
            if expected_kind == "workflow":
                result = code_search_workflow.verify(repo, root)
                observed_source = result["source"]
                observed_binding = result["binding"]
                source_matches = canonical_json(observed_source) == canonical_json(source)
                pair_spec = pair_run.load_spec(root / "pair-spec.json")
                workflows += 1
            else:
                observed_source, pair_spec, inputs = _pair(repo, root, registry)
                # The pair capture validates its own benchmark-retrieval closure;
                # the composed workflow uses the distinct retrieval closure.
                source_matches = (
                    observed_source.get("revision") == source["revision"]
                    and observed_source.get("dirty") is False
                )
                observed_binding = corpus_binding._bind(
                    document, inputs["manifest"], selection, inputs["suite"], inputs["query_pack"]
                )
                if (
                    digest_bytes(inputs["manifest"]) != digest_bytes(manifest)
                    or inputs["suite"] != suite
                    or inputs["query_pack"] != pack
                ):
                    raise ValueError("matrix pair native inputs differ from the declared cell")
                pairs += 1
            if not source_matches or canonical_json(observed_binding) != canonical_json(binding):
                raise ValueError("matrix capture source or corpus/query binding differs")
            _mode(pair_spec, mode)
        bindings.append(
            {
                "repository": cell["repository"],
                "query_family": cell["query_family"],
                "binding": binding,
            }
        )
    if code_search_workflow._source(repo) != source:
        raise ValueError("matrix source changed during verification")
    return {
        "schema_version": 1,
        "status": "diagnostic_unqualified",
        "source": source,
        "release_digest": document["digest"],
        "expected_cells": len(repositories) * len(spec["query_families"]) * len(MODES),
        "verified_cells": workflows + pairs,
        "workflows": workflows,
        "pairs": pairs,
        "bindings": bindings,
        "exclusions": [
            "independent_gold",
            "qualified_speed",
            "backend_indexed_universe_attestation",
        ],
    }
