"""Verify a declared code-search diagnostic matrix from native captures.

The release supplies the repository inventory. The matrix spec supplies the
query-family inventory; every repository/family declares all three modes.
Unsupported modes and missing captures are explicit, never scored as zero.
Captured cells are replayed through their native owner. This does not qualify
independent gold or performance.
"""

from __future__ import annotations

import hashlib
import os
import re
from pathlib import Path

import code_search_workflow
import corpus_binding
import corpus_release
import pair_capture
from evidence import RunStore, _read_control_file, canonical_json, digest_bytes
from profile_capture import load_capture
from registry import load_registry, registry_digest

from tools.benchmark.retrieval import evaluator, holdout_c4, query_plan
from tools.benchmark.retrieval import lexical_file_comparison as lexical
from tools.benchmark.retrieval import live_lexical_external as live
from tools.benchmark.retrieval import run as pair_run

MODES = ("lexical-only", "semantic-only", "hybrid")
PAIR_MODES = {
    "lexical-only": ("lexical", "lexical-only"),
    "semantic-only": ("semantic", "semantic-only"),
    "hybrid": ("hybrid", "hybrid-no-rerank"),
}
LEXICAL_ONLY_FILE_POLICIES = frozenset(
    {
        "code_search_file",
        "code_search_exact_content_file",
        "code_search_typo_file",
        "code_search_components_file",
    }
)
QUERY_POLICIES = frozenset({"native", "natural_language", *LEXICAL_ONLY_FILE_POLICIES})
UNSUPPORTED_REASON = "query_policy_not_supported_for_mode"
MISSING_REASON = "capture_missing"
NO_ADMISSION_REASON = "no_admitted_tasks"
LABEL_FIELDS = frozenset(
    {
        "gold",
        "answerable",
        "file_judgments",
        "declaration_judgments",
        "label_review",
        "judgment_policy",
        "source_oracle",
    }
)
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
    version = value.get("schema_version") if isinstance(value, dict) else None
    fields = {"schema_version", "release_path", "release_digest", "query_families", "cells"}
    if version == 3:
        fields |= {"c4_admission_root", "c4_capsules", "c4_checkouts"}
    if (
        not isinstance(value, dict)
        or set(value) != fields
        or type(version) is not int
        or version not in (2, 3)
        or not isinstance(value["release_digest"], str)
        or SHA.fullmatch(value["release_digest"]) is None
    ):
        raise ValueError("matrix requires a closed schema v2/v3 and release digest")
    _path(value["release_path"], "release_path")
    if version == 3:
        for field in ("c4_admission_root", "c4_capsules", "c4_checkouts"):
            _path(value[field], field)
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
            "query_policy",
            "suite",
            "query_pack",
            "captures",
        }:
            raise ValueError("matrix cell fields differ from the closed contract")
        if not isinstance(cell["repository"], str) or not isinstance(cell["query_family"], str):
            raise ValueError("matrix cell repository/query-family must be strings")
        if not isinstance(cell["query_policy"], str) or cell["query_policy"] not in QUERY_POLICIES:
            raise ValueError("matrix cell query policy is unsupported")
        key = (cell["repository"], cell["query_family"])
        if key[0] not in repositories or key[1] not in families or key in observed:
            raise ValueError("matrix has an unknown or duplicate repository/query-family cell")
        observed.add(key)
        if not isinstance(cell["view"], str) or cell["view"] not in corpus_release.VIEWS:
            raise ValueError("matrix cell view is unsupported")
        no_admission = version == 3 and cell["suite"] is None and cell["query_pack"] is None
        if not no_admission:
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
            if no_admission:
                if capture != {"kind": "not_applicable", "reason": NO_ADMISSION_REASON}:
                    raise ValueError("matrix no-admission cell has a capture or wrong reason")
                continue
            if (
                not isinstance(capture, dict)
                or not isinstance(capture.get("kind"), str)
                or capture["kind"] not in {"workflow", "pair", "unsupported", "not_run"}
            ):
                raise ValueError("matrix capture kind is unsupported")
            if capture["kind"] in {"unsupported", "not_run"}:
                reason = UNSUPPORTED_REASON if capture["kind"] == "unsupported" else MISSING_REASON
                if set(capture) != {"kind", "reason"} or capture["reason"] != reason:
                    raise ValueError("matrix non-capture reason differs from the closed contract")
                if capture["kind"] == "unsupported" and not (
                    cell["query_policy"] in LEXICAL_ONLY_FILE_POLICIES and mode != "lexical-only"
                ):
                    raise ValueError("matrix marks a supported query policy/mode unsupported")
                if capture["kind"] == "not_run" and (
                    cell["query_policy"] in LEXICAL_ONLY_FILE_POLICIES and mode != "lexical-only"
                ):
                    raise ValueError("matrix must mark unsupported query policy/mode explicitly")
            else:
                if set(capture) != {"kind", "root"}:
                    raise ValueError("matrix capture fields differ from the closed contract")
                if cell["query_policy"] in LEXICAL_ONLY_FILE_POLICIES and mode != "lexical-only":
                    raise ValueError("matrix captures an unsupported query policy/mode")
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


def _mode(spec: dict, mode: str, policy: str = "native") -> None:
    if policy in LEXICAL_ONLY_FILE_POLICIES and mode != "lexical-only":
        raise ValueError(f"matrix {mode} is unsupported for {policy}")
    route, semble = (
        ("lexical", "lexical-file")
        if policy in LEXICAL_ONLY_FILE_POLICIES and mode == "lexical-only"
        else PAIR_MODES[mode]
    )
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
        or spec["execution_profiles"].get("quanta") != query_plan.execution_profile(policy)
        or not isinstance(spec["execution_profiles"].get("semble"), dict)
        or spec["execution_profiles"]["semble"].get("mode") != semble
        or type(spec.get("top_k")) is not int
        or spec["top_k"] != 10
    ):
        raise ValueError(f"matrix {mode} pair uses another route, mode or claim")


def _supports_lexical_workflow(suite: dict, pack: dict) -> bool:
    """The five-product scorer accepts judged bare symbols, including no-answer tasks."""
    tasks, suite_tasks = pack.get("tasks"), suite.get("tasks")
    return (
        isinstance(tasks, list)
        and bool(tasks)
        and isinstance(suite_tasks, list)
        and len(suite_tasks) == len(tasks)
        and all(
            isinstance(task, dict)
            and isinstance(task.get("query"), str)
            and lexical.BARE_SYMBOL.fullmatch(task["query"]) is not None
            for task in tasks
        )
    )


def _admit_queries(pack: dict, policy: str) -> None:
    """Reject label leakage and reuse the frozen query planner."""
    tasks = pack.get("tasks")
    if not isinstance(tasks, list) or not tasks or LABEL_FIELDS.intersection(pack):
        raise ValueError("matrix query pack has no tasks or leaks labels")
    for task in tasks:
        if (
            not isinstance(task, dict)
            or not isinstance(task.get("query"), str)
            or LABEL_FIELDS.intersection(task)
        ):
            raise ValueError("matrix blind query pack task is malformed or leaks labels")
        try:
            query_plan.plan_lexical_request(policy, task["query"])
        except query_plan.QueryPlanError as error:
            raise ValueError("matrix query pack violates its declared policy") from error


def _replay_c4(
    spec: dict, release: Path, repositories: set[str]
) -> tuple[dict[tuple[str, str], dict], dict[Path, bytes]]:
    """Re-derive a v3 C4 admission before permitting absent suite/pack cells."""
    if spec["schema_version"] != 3:
        return {}, {}
    if spec["query_families"] != list(holdout_c4.MATRIX_INTENTS):
        raise ValueError("matrix C4 intent inventory differs")
    root = _path(spec["c4_admission_root"], "c4_admission_root")
    matrix_path = root / "admission-matrix.json"
    matrix_raw = _read_control_file(matrix_path)
    payloads: dict[tuple[str, str], tuple[dict, dict, dict]] = {}
    derived = holdout_c4.derive_matrix(
        release,
        _path(spec["c4_capsules"], "c4_capsules"),
        _path(spec["c4_checkouts"], "c4_checkouts"),
        expected_repositories=len(repositories),
        _payloads=payloads,
    )
    if live._json(matrix_raw) != derived or derived["release_digest"] != spec["release_digest"]:
        raise ValueError("matrix C4 admission differs from current source and capsules")
    by_key = {(row["repository"], row["intent"]): row for row in derived["cells"]}
    if len(by_key) != len(derived["cells"]) or set(by_key) != {
        (repository, intent) for repository in repositories for intent in spec["query_families"]
    }:
        raise ValueError("matrix C4 cell inventory differs")
    bound_files = {matrix_path: matrix_raw}
    for key, row in by_key.items():
        if row["status"] == "no_admission_diagnostic":
            if key in payloads or any(
                row[field] is not None
                for field in ("suite_sha256", "blind_pack_sha256", "admission_sha256")
            ):
                raise ValueError("matrix C4 no-admission cell has emitted inputs")
            continue
        if row["status"] != "diagnostic_unqualified" or key not in payloads:
            raise ValueError("matrix C4 cell status or inputs differ")
        repository, intent = key
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*", repository):
            raise ValueError("matrix C4 repository path is invalid")
        suite, pack, report = payloads[key]
        for name, value, digest_field in (
            ("suite.json", suite, "suite_sha256"),
            ("blind-pack.json", pack, "blind_pack_sha256"),
            ("admission.json", report, "admission_sha256"),
        ):
            path = root / repository / intent / name
            raw = _read_control_file(path)
            if (
                raw != evaluator.canonical(value)
                or hashlib.sha256(raw).hexdigest() != row[digest_field]
            ):
                raise ValueError("matrix C4 emitted input differs from source-bound admission")
            bound_files[path] = raw
    return by_key, bound_files


def build_c4_spec(
    release: Path,
    capsules: Path,
    checkouts: Path,
    admission_root: Path,
    capture_roots: dict[str, str | None],
) -> dict:
    """Prepare a closed diagnostic v3 spec from current C4 source evidence.

    Every admitted lexical cell must be declared as a capture root or ``None``
    (not run). No-admission cells are derived, never caller-declared.
    """
    release = _path(str(release), "release_path")
    document = corpus_release.validate(release)
    repositories = {row["recipe"]["name"] for row in document["repositories"]}
    if len(repositories) != len(document["repositories"]) or not repositories:
        raise ValueError("matrix release repository inventory differs")
    spec = {
        "schema_version": 3,
        "release_path": str(release),
        "release_digest": document["digest"],
        "query_families": list(holdout_c4.MATRIX_INTENTS),
        "c4_admission_root": str(_path(str(admission_root), "c4_admission_root")),
        "c4_capsules": str(_path(str(capsules), "c4_capsules")),
        "c4_checkouts": str(_path(str(checkouts), "c4_checkouts")),
        "cells": [],
    }
    admitted, bound_files = _replay_c4(spec, release, repositories)
    expected_roots = {
        f"{repository}/{intent}"
        for (repository, intent), row in admitted.items()
        if row["status"] == "diagnostic_unqualified"
    }
    if not isinstance(capture_roots, dict) or set(capture_roots) != expected_roots:
        raise ValueError("matrix C4 capture-root inventory differs from admitted cells")
    for repository, intent in sorted(admitted):
        row = admitted[repository, intent]
        selected = row["status"] == "diagnostic_unqualified"
        cell_root = admission_root / repository / intent
        if selected:
            root = capture_roots[f"{repository}/{intent}"]
            lexical = (
                {"kind": "not_run", "reason": MISSING_REASON}
                if root is None
                else {"kind": "pair", "root": str(_path(root, "capture root"))}
            )
            captures = {
                "lexical-only": lexical,
                "semantic-only": {"kind": "unsupported", "reason": UNSUPPORTED_REASON},
                "hybrid": {"kind": "unsupported", "reason": UNSUPPORTED_REASON},
            }
        else:
            captures = {
                mode: {"kind": "not_applicable", "reason": NO_ADMISSION_REASON} for mode in MODES
            }
        spec["cells"].append(
            {
                "repository": repository,
                "view": "code_only",
                "query_family": intent,
                "query_policy": holdout_c4._execution_policy(intent),
                "suite": str(cell_root / "suite.json") if selected else None,
                "query_pack": str(cell_root / "blind-pack.json") if selected else None,
                "captures": captures,
            }
        )
    _spec(spec, repositories)
    if any(_read_control_file(path) != before for path, before in bound_files.items()):
        raise ValueError("matrix C4 admission changed during spec preparation")
    return spec


def write_c4_spec(
    release: Path,
    capsules: Path,
    checkouts: Path,
    admission_root: Path,
    capture_roots_path: Path,
    output: Path,
) -> dict:
    """Write a source-derived C4 spec to a fresh external file.

    The capture-root inventory is a JSON object mapping every admitted
    ``repository/intent`` to an absolute capture root or null for not-run.
    """
    output = _path(str(output), "output")
    capture_roots_path = _path(str(capture_roots_path), "capture_roots_path")
    destination = output.resolve(strict=False)
    protected = (Path(__file__).resolve().parents[2], release, capsules, checkouts, admission_root)
    if (
        output.exists()
        or output.is_symlink()
        or any(
            destination.is_relative_to(root.resolve()) or root.resolve().is_relative_to(destination)
            for root in protected
        )
    ):
        raise ValueError("matrix output must be fresh and external to the source and inputs")
    capture_roots_raw = _read_control_file(capture_roots_path)
    capture_roots = live._json(capture_roots_raw)
    spec = build_c4_spec(release, capsules, checkouts, admission_root, capture_roots)
    if _read_control_file(capture_roots_path) != capture_roots_raw:
        raise ValueError("matrix capture-root inventory changed during spec preparation")
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    descriptor = os.open(output, flags, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(canonical_json(spec).encode("utf-8"))
            stream.flush()
            os.fsync(stream.fileno())
    except BaseException:
        output.unlink(missing_ok=True)
        raise
    return spec


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
    c4_cells, c4_files = _replay_c4(spec, release, repositories)
    registry = load_registry(repo / "tools/benchmark/registry.toml")
    workflows = pairs = unsupported = not_run = no_admission = 0
    no_admission_rows = []
    bindings = []
    family_input_digests: set[tuple[str, str, str]] = set()
    for cell in spec["cells"]:
        if c4_cells:
            c4 = c4_cells[cell["repository"], cell["query_family"]]
            if cell["view"] != "code_only" or cell["query_policy"] != holdout_c4._execution_policy(
                cell["query_family"]
            ):
                raise ValueError("matrix C4 view or request policy differs")
            if c4["status"] == "no_admission_diagnostic":
                if cell["suite"] is not None or cell["query_pack"] is not None:
                    raise ValueError("matrix C4 no-admission cell invents inputs")
                no_admission += 1
                no_admission_rows.append(
                    {
                        "repository": cell["repository"],
                        "query_family": cell["query_family"],
                        "reason": c4["reason"],
                        "candidate_task_ids": c4["candidate_task_ids"],
                        "excluded": c4["excluded"],
                    }
                )
                continue
            cell_root = (
                _path(spec["c4_admission_root"], "c4_admission_root")
                / cell["repository"]
                / cell["query_family"]
            )
            if cell["suite"] != str(cell_root / "suite.json") or cell["query_pack"] != str(
                cell_root / "blind-pack.json"
            ):
                raise ValueError("matrix C4 cell inputs differ from emitted admission")
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
        policy = cell["query_policy"]
        _admit_queries(pack_data, policy)
        bare = _supports_lexical_workflow(suite_data, pack_data)
        if bare and policy == "native":
            lexical._file_universe(suite_data, pack_data)
            lexical._tasks(suite_data, pack_data)
        for mode in MODES:
            capture = cell["captures"][mode]
            if capture["kind"] == "unsupported":
                unsupported += 1
                continue
            if capture["kind"] == "not_run":
                not_run += 1
                continue
            root = _path(capture["root"], "capture root")
            expected_kind = (
                "workflow" if mode == "lexical-only" and bare and policy == "native" else "pair"
            )
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
            _mode(pair_spec, mode, policy)
        bindings.append(
            {
                "repository": cell["repository"],
                "query_family": cell["query_family"],
                "query_policy": policy,
                "binding": binding,
            }
        )
    if code_search_workflow._source(repo) != source:
        raise ValueError("matrix source changed during verification")
    if any(_read_control_file(path) != before for path, before in c4_files.items()):
        raise ValueError("matrix C4 admission changed during verification")
    result = {
        "schema_version": spec["schema_version"],
        "status": (
            "diagnostic_no_admission"
            if no_admission == len(spec["cells"])
            else "diagnostic_incomplete"
            if not_run
            else "diagnostic_partial_admission"
            if no_admission
            else "diagnostic_unqualified"
        ),
        "source": source,
        "release_digest": document["digest"],
        "expected_cells": len(repositories) * len(spec["query_families"]) * len(MODES),
        "verified_cells": workflows + pairs,
        "workflows": workflows,
        "pairs": pairs,
        "unsupported_cells": unsupported,
        "not_run_cells": not_run,
        "bindings": bindings,
        "exclusions": [
            "independent_gold",
            "qualified_speed",
            "backend_indexed_universe_attestation",
        ],
    }
    if spec["schema_version"] == 3:
        result["no_admission_cells"] = no_admission
        result["no_admission_mode_cells"] = no_admission * len(MODES)
        result["no_admission"] = no_admission_rows
        result["c4_admission_matrix_sha256"] = hashlib.sha256(
            c4_files[
                _path(spec["c4_admission_root"], "c4_admission_root") / "admission-matrix.json"
            ]
        ).hexdigest()
    return result
