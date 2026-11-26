"""Quality gate for replayed Native file captures and their input commitments.

This module does not authorize a product default. The opt-in signed-install
check verifies installed provenance; the operational profile remains separate.
"""

from __future__ import annotations

import hashlib
import json
import math
import stat
from collections.abc import Callable
from dataclasses import dataclass
from functools import cache
from pathlib import Path

from tools.benchmark.evidence import (
    CONTROL_DOCUMENT_BYTES,
    EvidenceError,
    RawFile,
    _read_control_file,
    file_digest,
    parse_json,
    read_control,
)
from tools.benchmark.retrieval import (
    composite_source_binding,
    evaluator,
    native_file_evidence,
    native_file_paths,
)
from tools.benchmark.retrieval.finite_json import is_finite_json_number
from tools.benchmark.retrieval.native_file_paths import resolve_path
from tools.deployment import launcher


class NativeDecisionError(ValueError):
    """A Native quality decision input is malformed or lacks authority."""


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise NativeDecisionError(message)


def _sha(path: Path) -> str:
    return file_digest(resolve_path(path))[0].removeprefix("sha256:")


def _release_tree_sha(root: Path) -> str:
    """Bind the complete frozen release directory, including names and modes."""
    import sys

    _require(root.is_dir(), "Native release input must be a directory")
    # The canonical release validator owns the symlink/special-file tree rule.
    benchmark_root = str(Path(evaluator.__file__).resolve().parents[1])
    if benchmark_root not in sys.path:
        sys.path.insert(0, benchmark_root)
    import corpus_release

    rows = [
        {
            "path": name,
            "digest": _sha(root / name),
            "mode": stat.S_IMODE((root / name).stat().st_mode),
        }
        for name in sorted(corpus_release.regular_tree(root))
    ]
    _require(rows, "Native release directory is empty")
    return hashlib.sha256(evaluator.canonical(rows)).hexdigest()


def _policy(source: Path | bytes, manifest_sha: str, manifest: dict) -> tuple[dict, str]:
    raw = read_control(resolve_path(source) if isinstance(source, Path) else source)
    value = evaluator._parse_json_text(raw.decode("utf-8"))
    _require(
        isinstance(value, dict)
        and set(value)
        == {
            "schema_version",
            "profile",
            "cohort_manifest_sha256",
            "repositories",
            "repository_strata",
            "minimum_counts",
            "no_answer_rule",
            "resource_limits",
        }
        and type(value["schema_version"]) is int
        and value["schema_version"] == 1
        and value["profile"] == "native_file_decision"
        and value["cohort_manifest_sha256"] == manifest_sha
        and value["repositories"] == manifest["repositories"]
        and value["repository_strata"] == manifest["repository_strata"]
        and value["no_answer_rule"] == "nonempty_top10_is_false_positive",
        "Native decision policy or frozen cohort differs",
    )
    minima = value["minimum_counts"]
    _require(
        isinstance(minima, dict)
        and set(minima) == {"positive", "identifier", "negative", "hard_negative"}
        and all(
            type(minima[key]) is int and minima[key] >= floor
            for key, floor in (
                ("positive", 100),
                ("identifier", 30),
                ("negative", 30),
                ("hard_negative", 30),
            )
        ),
        "Native decision minimum_counts must be frozen integer counts at or above the DOD floors",
    )
    limits = value["resource_limits"]
    _require(
        isinstance(limits, dict)
        and set(limits) == {"max_query_p95_micros"}
        and is_finite_json_number(limits["max_query_p95_micros"])
        and 0 < limits["max_query_p95_micros"] <= 100_000,
        "Native query p95 limit must be predeclared and no weaker than 100 ms",
    )
    return value, hashlib.sha256(raw).hexdigest()


_RUNTIME_KEYS = {
    "schema_version",
    "kind",
    "daemon_binary_sha256",
    "daemon_config_sha256",
    "runner_binary_sha256",
    "decision_policy_sha256",
    "socket_path",
    "config_path",
    "command",
    "pid",
    "uid",
    "gid",
    "peer_image_verified",
}


def load_selected_snapshot(path: Path, expected_sha256: str) -> tuple[RawFile, launcher.Selection]:
    """Read one atomic deployment selection without acquiring its serving lock."""
    _require(path.is_absolute(), "selection path must be absolute")
    _require(
        isinstance(expected_sha256, str)
        and len(expected_sha256) == 64
        and all(char in "0123456789abcdef" for char in expected_sha256),
        "selection digest is malformed",
    )
    payload = selected_file_bytes(path)
    observed = hashlib.sha256(payload).hexdigest()
    _require(observed == expected_sha256, "selection digest changed")
    try:
        selected = launcher.selection_from_wire(parse_json(payload.decode("utf-8")))
    except launcher.SelectionError as error:
        raise NativeDecisionError(str(error)) from error
    _require(selected.native_boot_receipt is not None, "selection is not a selected Native root")
    raw = RawFile(path, f"sha256:{observed}", len(payload))
    return raw, selected


def selected_file_bytes(path: Path) -> bytes:
    """Use the existing pinned, bounded control reader before hashing a selection."""
    return _read_control_file(path, max_bytes=launcher.MAX_SELECTION_BYTES)


def _cohort_runtime(
    manifest_path: Path,
    family: dict,
    legacy_config: Path | None,
) -> tuple[Path, str, tuple[Path, str] | None]:
    """Resolve the frozen config and optional selected installation for a family."""
    if "daemon_config" not in family:
        _require(legacy_config is not None, "Native v1 cohort requires --daemon-config")
        return resolve_path(legacy_config), _sha(legacy_config), None

    _require(legacy_config is None, "Native v2 cohort supplies per-family daemon configs")
    config_path = resolve_path(manifest_path.parent / family["daemon_config"])
    config_sha = _sha(config_path)
    _require(
        config_sha == family["daemon_config_sha256"], "Native family daemon config digest differs"
    )
    selection = None
    if "selection_path" in family:
        selection = (
            (manifest_path.parent / family["selection_path"]).absolute(),
            family["selection_sha256"],
        )
    return config_path, config_sha, selection


def check_selected_inputs(
    selected: launcher.Selection,
    *,
    daemon_path: Path | None,
    daemon_sha: str | None,
    config_path: str | None,
    config_sha: str | None,
) -> None:
    if daemon_path is not None:
        _require(daemon_path == selected.binary, "selection binary path differs")
    _require(
        daemon_sha == selected.binary_sha256
        and config_sha == selected.native_config_sha256
        and config_path == str(selected.native_config),
        "selection binary or config differs from Native execution",
    )


def check_selected_runtime(
    selected: launcher.Selection, binding: dict, *, daemon_path: Path | None = None
) -> None:
    """Compare the selected root's existing typed receipt with the live peer."""
    receipt = selected.native_boot_receipt
    _require(receipt is not None, "selection lacks a Native boot receipt")
    _require(binding.get("command") == "serve-native", "selected Native command differs")
    check_selected_inputs(
        selected,
        daemon_path=resolve_path(daemon_path) if daemon_path is not None else None,
        daemon_sha=binding.get("daemon_binary_sha256"),
        config_path=binding.get("config_path"),
        config_sha=binding.get("daemon_config_sha256"),
    )
    expected_identity = {
        "root_nonce_hex": receipt.root_nonce_hex,
        "manifest_operation": json.loads(receipt.manifest_operation_json),
        "manifest_object_hex": receipt.manifest_object_hex,
        "source_cut_digest_hex": receipt.source_cut_digest_hex,
    }
    _require(
        evaluator.canonical(binding.get("boot_identity")) == evaluator.canonical(expected_identity),
        "selection boot identity differs",
    )


def _check_runtime(
    capture: dict,
    *,
    daemon_sha: str,
    config_sha: str,
    runner_sha: str,
    policy_sha: str,
    config_path: Path,
    socket: str,
    selected: launcher.Selection | None = None,
    daemon_path: Path | None = None,
    require_native_selection: bool = False,
) -> None:
    binding = capture.get("runtime_binding")
    _require(
        isinstance(binding, dict)
        and set(binding) in (_RUNTIME_KEYS, _RUNTIME_KEYS | {"boot_identity"})
        and type(binding["schema_version"]) is int
        and binding["schema_version"] == 1
        and binding["kind"] == "linux_connected_peer_image_v1"
        and binding["daemon_binary_sha256"] == daemon_sha
        and binding["daemon_config_sha256"] == config_sha
        and binding["runner_binary_sha256"] == runner_sha
        and binding["decision_policy_sha256"] == policy_sha
        and binding["socket_path"] == socket
        and binding["config_path"] == str(config_path)
        and binding["command"] in ("serve-native", "serve-native-candidate")
        and type(binding["pid"]) is int
        and binding["pid"] > 0
        and type(binding["uid"]) is int
        and binding["uid"] >= 0
        and type(binding["gid"]) is int
        and binding["gid"] >= 0
        and binding["peer_image_verified"] is True
        and capture.get("runner_binary_sha256") == runner_sha,
        "Native capture is not bound to the connected daemon, config, runner and policy",
    )
    if selected is not None:
        _require(daemon_path is not None, "selected daemon path is missing")
        check_selected_runtime(selected, binding, daemon_path=daemon_path)
    elif require_native_selection:
        _require(
            binding["command"] == "serve-native-candidate",
            "Native v2 serve-native requires a pinned selection",
        )


def _score_pair(rows: dict, task_id: str, field: str) -> float:
    before = rows.get((task_id, "lexical"), {}).get(field)
    after = rows.get((task_id, "hybrid"), {}).get(field)
    _require(
        is_finite_json_number(before)
        and is_finite_json_number(after)
        and 0 <= before <= 1
        and 0 <= after <= 1,
        f"Native paired {field} missing or outside [0,1]: {task_id}",
    )
    return float(after - before)


def _slice_mean(rows: list[tuple[str, str, str, float]]) -> float:
    """Equal family weight within each repository, then equal repository weight."""
    grouped: dict[str, dict[str, list[float]]] = {}
    for repo, _task, family, delta in rows:
        grouped.setdefault(repo, {}).setdefault(family, []).append(delta)
    repository_means = [
        math.fsum(math.fsum(values) / len(values) for values in families.values()) / len(families)
        for families in grouped.values()
    ]
    return math.fsum(repository_means) / len(repository_means)


def _nearest_rank(values: list[int], fraction: float) -> int:
    ordered = sorted(values)
    return ordered[math.ceil(fraction * len(ordered)) - 1]


@dataclass(frozen=True)
class _ScoredFamilyInputs:
    capture: tuple[str, int]
    digests: dict[str, str]


_REVIEW_SOURCE_ROLES = (
    "contexts",
    "control_pools",
    "review_1",
    "review_2",
    "adjudication",
    "task_contracts",
)


def _signed_review_sources(source: RawFile) -> tuple[int, dict[str, tuple[Path, str]]]:
    document = parse_json(read_control(source).decode("utf-8"))
    _require(
        isinstance(document, dict)
        and set(document) == {"schema_version", "seed", "inputs"}
        and document["schema_version"] == "native-file-review-source/v1"
        and type(document["seed"]) is int
        and isinstance(document["inputs"], dict)
        and set(document["inputs"]) == set(_REVIEW_SOURCE_ROLES),
        "Native signed quality requires original review source inputs",
    )
    inputs = {}
    for role in _REVIEW_SOURCE_ROLES:
        value = document["inputs"][role]
        _require(
            isinstance(value, dict)
            and set(value) == {"path", "sha256"}
            and isinstance(value["path"], str)
            and bool(value["path"])
            and isinstance(value["sha256"], str)
            and len(value["sha256"]) == 64
            and all(char in "0123456789abcdef" for char in value["sha256"]),
            "Native original review source reference differs",
        )
        inputs[role] = source.path.parent / value["path"], value["sha256"]
    return document["seed"], inputs


def _read_signed_review_input(source: RawFile, *, form: bool):
    if not form:
        return parse_json(read_control(source).decode("utf-8"))
    from tools.benchmark.retrieval.holdout_review import MAX_REVIEW_BYTES

    # The export budget covers both templates' source text. A submitted form
    # adds JSON escaping and bounded control metadata to one copy.
    limit = 6 * (MAX_REVIEW_BYTES // 2) + CONTROL_DOCUMENT_BYTES
    raw = _read_control_file(source.path, max_bytes=limit)
    _require(
        (f"sha256:{hashlib.sha256(raw).hexdigest()}", len(raw)) == (source.sha256, source.size),
        "Native original review form bytes differ from the frozen commitment",
    )
    return parse_json(raw.decode("utf-8"))


def verify_controlled_capture_runs(
    manifest_path: Path,
    policy_path: Path,
    daemon_binary: Path,
    daemon_config: Path | None,
    runner_binary: Path,
    evidence_root: Path,
    run_ids: list[str],
    *,
    release_validator: Callable[[Path], dict] | None = None,
    scored_inputs: dict[str, _ScoredFamilyInputs] | None = None,
    require_signed_install: bool = False,
    signed_install_identity: dict[str, str] | None = None,
    signed_install_recheck: list[tuple[dict, Path, str]] | None = None,
) -> dict[str, str]:
    """Return the validated capture commitment for each owned family run."""
    import sys

    benchmark_root = str(Path(evaluator.__file__).resolve().parents[1])
    if benchmark_root not in sys.path:
        sys.path.insert(0, benchmark_root)
    import native_file_capture
    from evidence import RunStore, file_digest
    from evidence_bridge import source_identity
    from registry import load_registry, registry_digest

    manifest_path = resolve_path(manifest_path)
    _raw, manifest = evaluator._native_cohort_manifest(manifest_path)
    families = {entry["family_id"]: entry for entry in manifest["families"]}
    per_family = manifest["schema_version"] == 2
    if require_signed_install:
        _require(per_family, "Native signed installation requires a v2 selected cohort")
    if scored_inputs is not None:
        _require(
            scored_inputs.keys() == families.keys(), "Native controlled scoring roster differs"
        )
    _require(
        isinstance(run_ids, list)
        and len(run_ids) == len(families)
        and all(isinstance(run_id, str) for run_id in run_ids)
        and len(set(run_ids)) == len(run_ids),
        "Native controlled run inventory is missing or repeated",
    )
    code_root = Path(__file__).resolve().parents[3]
    current_source = source_identity(code_root, "benchmark-retrieval")
    store = RunStore(evidence_root)
    registry_sha = registry_digest(load_registry())
    commitments: dict[str, str] = {}
    installed_identity: dict[str, str] | None = None
    representative: tuple[dict, Path, str] | None = None
    selections: set[tuple[str, str] | None] = set()
    expected_paths = {
        "cohort_manifest": manifest_path,
        "decision_policy": resolve_path(policy_path),
        "daemon_binary": resolve_path(daemon_binary),
        "runner_binary": resolve_path(runner_binary),
    }
    for run_id in run_ids:
        evidence = native_file_capture.load_published_run(
            store,
            run_id,
            registered_digest=registry_sha,
        )
        _require(
            evidence["source"] == current_source,
            "Native controlled capture code source is stale",
        )
        _require(
            evidence["build"]["lockfile_digest"] == file_digest(code_root / "Cargo.lock")[0],
            "Native controlled capture lockfile is stale",
        )
        plan = native_file_capture._plan(
            native_file_capture._archived_raw(store, evidence, "plan.json"),
            require_signed_install=require_signed_install,
        )
        family_id = evidence["case_id"]
        _require(
            family_id in families
            and family_id not in commitments
            and plan["family_id"] == family_id,
            "Native controlled capture family is foreign or repeated",
        )
        if scored_inputs is not None:
            expected = scored_inputs[family_id]
            raw = native_file_capture._archived_raw(store, evidence, "native-capture.json")
            _require(
                (raw.sha256, raw.size) == expected.capture,
                "Native controlled execution raw bytes differ from scored capture",
            )
            inputs = {
                row["id"]: row["digest"]
                for row in evidence["inputs"]
                if row["availability"] == "present"
            }
            _require(
                all(inputs.get(key) == digest for key, digest in expected.digests.items()),
                "Native controlled execution inputs differ from scored family",
            )
        config_path, _, selected = _cohort_runtime(
            manifest_path, families[family_id], daemon_config
        )
        plan_selected = (
            (Path(plan["selection_path"]), plan["selection_sha256"])
            if "selection_path" in plan
            else None
        )
        if per_family:
            _require(
                plan_selected == selected,
                "Native controlled capture selection differs from its family",
            )
        else:
            selections.add(plan_selected)
        _require(
            resolve_path(Path(plan["daemon_config"])) == config_path
            and all(resolve_path(Path(plan[key])) == path for key, path in expected_paths.items()),
            "Native controlled capture belongs to a different input cohort",
        )
        commitment = native_file_capture.replay_run(
            store,
            evidence,
            release_validator=release_validator,
            require_signed_install=require_signed_install,
        )
        if require_signed_install:
            _require(selected is not None, "Native signed installation lacks selected BOOT")
            approval = plan[native_file_capture.SIGNED_INSTALL]
            payload = approval["payload"]
            identity = {
                key: payload[key]
                for key in (
                    "active_generation_sha256",
                    "release_manifest_sha256",
                    "q_elf_sha256",
                )
            }
            _require(
                installed_identity is None or installed_identity == identity,
                "Native signed installation differs across cohort families",
            )
            installed_identity = identity
            if representative is None:
                representative = (plan, Path(plan["selection_path"]), plan["selection_sha256"])
        commitments[family_id] = commitment
    _require(
        commitments.keys() == families.keys(), "Native controlled capture family coverage differs"
    )
    if not per_family:
        _require(len(selections) == 1, "Native controlled capture selection commitments differ")
    if require_signed_install:
        _require(installed_identity is not None, "Native signed installation proof is absent")
        if signed_install_identity is not None:
            signed_install_identity.update(installed_identity)
        if signed_install_recheck is not None:
            _require(representative is not None, "Native signed installation proof is absent")
            signed_install_recheck.append(representative)
    return commitments


def build_native_file_decision(
    manifest_path: Path,
    policy_path: Path,
    daemon_binary: Path,
    daemon_config: Path | None,
    runner_binary: Path,
    *,
    execution_evidence_root: Path | None = None,
    execution_run_ids: list[str] | None = None,
    reviewed_labels_path: Path | None = None,
    output: native_file_paths.Output | None = None,
    require_signed_install: bool = False,
) -> dict:
    """Replay all raw Native families and apply fixed quality thresholds.

    Capture fields are compared with pinned files. A signed quality decision
    requires controlled runs and the original review forms for each family.
    The 960-second production profile remains separate.
    """
    _require(
        (execution_evidence_root is None) == (execution_run_ids is None),
        "Native controlled execution needs both evidence root and run IDs",
    )
    if require_signed_install:
        _require(
            execution_evidence_root is not None and execution_run_ids is not None,
            "Native signed installation requires controlled execution root and run IDs",
        )
    # Bind relative inputs to the same resolved manifest location we consume.
    # A top-level alias (for example /tmp on macOS) is not a release-tree link.
    declared_manifest = manifest_path
    manifest_path = resolve_path(manifest_path)
    raw_manifest, manifest = evaluator._native_cohort_manifest(manifest_path)
    if require_signed_install:
        _require(
            manifest["schema_version"] == 2
            and all("selection_path" in family for family in manifest["families"]),
            "Native signed installation requires a v2 selected cohort",
        )
        _require(
            reviewed_labels_path is not None,
            "Native signed quality decision requires independently reviewed labels",
        )
    if output is not None:
        output.protect(
            (role, path)
            for role, path in (
                ("cohort manifest", declared_manifest),
                ("decision policy", policy_path),
                ("daemon binary", daemon_binary),
                ("runner binary", runner_binary),
                ("daemon config", daemon_config),
                ("reviewed labels", reviewed_labels_path),
                *native_file_paths.cohort_inputs(manifest_path, manifest),
            )
            if path is not None
        )
        if execution_evidence_root is not None:
            output.protect(
                ("benchmark custody paths", path)
                for path in native_file_paths.custody_paths(execution_evidence_root)
            )
    manifest_sha = hashlib.sha256(raw_manifest).hexdigest()
    declared_files = (policy_path, daemon_binary, runner_binary)
    resolved_files = tuple(resolve_path(path) for path in declared_files)
    policy_file, daemon_file, runner_file = resolved_files
    policy, policy_sha = _policy(policy_file, manifest_sha, manifest)
    daemon_sha, runner_sha = _sha(daemon_file), _sha(runner_file)
    tracked_files: dict[Path, str] = {}
    tracked_path_targets = list(zip(declared_files, resolved_files, strict=True))
    tracked_path_targets.append((declared_manifest, manifest_path))
    tracked_selections: dict[Path, str] = {}

    def bind_digest(tracked: dict[Path, str], path: Path, observed: str) -> None:
        _require(
            tracked.setdefault(path, observed) == observed,
            "Native decision input path is reused with different bytes",
        )

    for path, observed in (
        (manifest_path, manifest_sha),
        (policy_file, policy_sha),
        (daemon_file, daemon_sha),
        (runner_file, runner_sha),
    ):
        bind_digest(tracked_files, path, observed)

    runtime = {}
    roots, sockets = set(), set()
    common_recipe = None
    per_family = manifest["schema_version"] == 2
    legacy_config = daemon_config
    if not per_family and daemon_config is not None:
        legacy_config = resolve_path(daemon_config)
        tracked_path_targets.append((daemon_config, legacy_config))
    for family in manifest["families"]:
        config_path, config_sha, selection = _cohort_runtime(manifest_path, family, legacy_config)
        raw_config = read_control(config_path)
        _require(
            hashlib.sha256(raw_config).hexdigest() == config_sha,
            "Native daemon config changed during read",
        )
        config = evaluator._parse_json_text(raw_config.decode("utf-8"))
        _require(
            isinstance(config, dict)
            and isinstance(config.get("socket_path"), str)
            and Path(config["socket_path"]).is_absolute(),
            "Native daemon config lacks an absolute socket_path",
        )
        socket = config["socket_path"]
        selected = None
        if per_family:
            state_root = config.get("state_root")
            _require(
                isinstance(state_root, str) and Path(state_root).is_absolute(),
                "Native family daemon config lacks an absolute state_root",
            )
            root, socket_path = resolve_path(Path(state_root)), resolve_path(Path(socket))
            _require(
                root not in roots and socket_path not in sockets,
                "Native family state root or socket is shared",
            )
            roots.add(root)
            sockets.add(socket_path)
            recipe = evaluator.canonical(
                {
                    key: value
                    for key, value in config.items()
                    if key not in {"state_root", "socket_path"}
                }
            )
            _require(
                common_recipe is None or recipe == common_recipe,
                "Native family daemon configs differ beyond state root and socket",
            )
            common_recipe = recipe
        if selection is not None:
            selection_path, selection_sha = selection
            _, selected = load_selected_snapshot(selection_path, selection_sha)
            _require(
                selected.state_root == Path(config["state_root"]),
                "Native family selection root differs",
            )
            bind_digest(tracked_selections, selection_path, selection_sha)
        bind_digest(tracked_files, config_path, config_sha)
        runtime[family["family_id"]] = config_path, config_sha, socket, selected
    reviewed: dict[str, Path] = {}
    reviewed_bytes: dict[Path, tuple[str, int]] = {}
    signed_reviews: dict[str, tuple[int, dict[str, RawFile]]] = {}
    reviewed_sha = None
    if reviewed_labels_path is not None:
        declared_review_manifest = reviewed_labels_path
        reviewed_labels_path = resolve_path(reviewed_labels_path)
        tracked_path_targets.append((declared_review_manifest, reviewed_labels_path))
        raw_reviewed = read_control(reviewed_labels_path)
        rows = evaluator._parse_json_text(raw_reviewed.decode("utf-8"))
        _require(
            isinstance(rows, dict)
            and set(rows) == {family["family_id"] for family in manifest["families"]}
            and all(isinstance(value, str) and bool(value) for value in rows.values()),
            "Native reviewed label file roster differs from the frozen cohort",
        )
        declared_reviews = {
            family: reviewed_labels_path.parent / location for family, location in rows.items()
        }
        if output is not None:
            output.protect(("reviewed label receipt", path) for path in declared_reviews.values())
        reviewed = {family: resolve_path(path) for family, path in declared_reviews.items()}
        _require(
            len(set(reviewed.values())) == len(reviewed), "Native reviewed label file is repeated"
        )
        tracked_path_targets.extend(
            (declared, reviewed[family]) for family, declared in declared_reviews.items()
        )
        reviewed_sha = hashlib.sha256(raw_reviewed).hexdigest()
        bind_digest(tracked_files, reviewed_labels_path, reviewed_sha)
        for receipt in reviewed.values():
            receipt_digest = file_digest(receipt)
            bind_digest(tracked_files, receipt, receipt_digest[0].removeprefix("sha256:"))
            reviewed_bytes[receipt] = receipt_digest
        if require_signed_install:
            for family, receipt in reviewed.items():
                seed, paths = _signed_review_sources(RawFile(receipt, *reviewed_bytes[receipt]))
                inputs = {}
                if output is not None:
                    output.protect(("original review source", path) for path, _ in paths.values())
                for role, (declared, expected) in paths.items():
                    current = resolve_path(declared)
                    source = RawFile.capture(current)
                    _require(
                        source.sha256 == f"sha256:{expected}",
                        "Native original review source differs from its manifest",
                    )
                    _require(
                        tracked_selections.get(current, expected) == expected,
                        "Native decision input path is reused with different bytes",
                    )
                    bind_digest(tracked_files, current, expected)
                    tracked_path_targets.append((declared, current))
                    inputs[role] = source
                signed_reviews[family] = seed, inputs

    def path(entry: dict, key: str) -> Path:
        return resolve_path(manifest_path.parent / entry[key])

    def release_path(entry: dict, key: str) -> Path:
        # Keep the lexical path so the canonical release validator sees aliases.
        return (manifest_path.parent / entry[key]).absolute()

    release_paths = {
        release_path(entry, key)
        for entry in manifest["families"]
        for key in native_file_paths.RELEASE_ROOTS
    }
    tracked_releases = {root: _release_tree_sha(root) for root in release_paths}

    @cache
    def validate_release(root: Path) -> dict:
        # Reuse canonical Git replay only inside the release trees whose
        # exact bytes and modes are checked again before a decision is issued.
        _require(
            any(root.is_relative_to(release) for release in tracked_releases),
            "Native replay selected a release outside the tracked input trees",
        )
        return composite_source_binding.validate_release(root)

    file_keys = native_file_paths.COHORT_FILES
    family_paths = {
        entry["family_id"]: {key: path(entry, key) for key in ("repo", *file_keys)}
        for entry in manifest["families"]
    }
    tracked_input_bytes: dict[Path, tuple[str, int]] = {}
    for paths in family_paths.values():
        for key in file_keys:
            source = paths[key]
            observed_bytes = file_digest(source)
            observed = observed_bytes[0].removeprefix("sha256:")
            _require(
                tracked_selections.get(source, observed) == observed,
                "Native decision input path is reused with different bytes",
            )
            bind_digest(tracked_files, source, observed)
            tracked_input_bytes[source] = observed_bytes

    signed_install_identity: dict[str, str] = {}
    signed_install_recheck: list[tuple[dict, Path, str]] = []

    def verify_controlled() -> dict[str, str]:
        scored_inputs = {}
        for entry in manifest["families"]:
            family_id = entry["family_id"]
            paths = family_paths[family_id]
            digests = {
                "cohort": f"sha256:{manifest_sha}",
                "decision_policy": f"sha256:{policy_sha}",
                "daemon_binary": f"sha256:{daemon_sha}",
                "daemon_config": f"sha256:{runtime[family_id][1]}",
                "runner_binary": f"sha256:{runner_sha}",
                **{
                    key: tracked_input_bytes[paths[key]][0] for key in file_keys if key != "capture"
                },
                **{
                    key: f"sha256:{tracked_releases[release_path(entry, key)]}"
                    for key in ("base_release", "target_releases")
                },
            }
            if "selection_sha256" in entry:
                digests["selection"] = f"sha256:{entry['selection_sha256']}"
            scored_inputs[family_id] = _ScoredFamilyInputs(
                tracked_input_bytes[paths["capture"]],
                digests,
            )
        options = (
            {
                "require_signed_install": True,
                "signed_install_identity": signed_install_identity,
                "signed_install_recheck": signed_install_recheck,
            }
            if require_signed_install
            else {}
        )
        return verify_controlled_capture_runs(
            manifest_path,
            policy_path,
            daemon_binary,
            daemon_config,
            runner_binary,
            execution_evidence_root,
            execution_run_ids,
            release_validator=validate_release,
            scored_inputs=scored_inputs,
            **options,
        )

    # The opt-in approval must be checked before any quality replay is scored.
    controlled = verify_controlled() if require_signed_install else None
    families = []
    runtime_command = None
    for entry in manifest["families"]:
        family_id = entry["family_id"]
        paths = family_paths[family_id]
        for key in file_keys:
            source = paths[key]
            _require(
                tracked_files[source] == _sha(source),
                "Native decision input changed before family replay",
            )
        if require_signed_install:
            from tools.benchmark.retrieval.holdout_review import replay_native_review_labels

            seed, sources = signed_reviews[family_id]
            documents = {
                role: _read_signed_review_input(
                    source,
                    form=role in {"review_1", "review_2", "adjudication"},
                )
                for role, source in sources.items()
            }
            issued = replay_native_review_labels(
                paths["repo"],
                paths["suite"],
                paths["query_pack"],
                paths["capture"],
                source_inputs=(
                    paths["source_universe"],
                    paths["source_binding"],
                    release_path(entry, "base_release"),
                    release_path(entry, "target_releases"),
                    family_id,
                ),
                contexts=documents["contexts"],
                control_pools=documents["control_pools"],
                completed_forms=[documents["review_1"], documents["review_2"]],
                adjudicated_form=documents["adjudication"],
                task_contracts=documents["task_contracts"],
                seed=seed,
                expected_capture_sha256=tracked_input_bytes[paths["capture"]][0].removeprefix(
                    "sha256:"
                ),
            )
            options = {"reviewed_labels": evaluator.canonical(issued)}
        elif (receipt := reviewed.get(family_id)) is not None:
            options = {
                "reviewed_labels": receipt,
                "expected_reviewed_labels": reviewed_bytes[receipt],
            }
        else:
            options = {}
        replay = evaluator.load_native_file_replay(
            paths["repo"],
            paths["suite"],
            paths["query_pack"],
            paths["capture"],
            (
                paths["source_universe"],
                paths["source_binding"],
                release_path(entry, "base_release"),
                release_path(entry, "target_releases"),
                family_id,
            ),
            release_validator=validate_release,
            expected_suite=tracked_input_bytes[paths["suite"]],
            expected_query_pack=tracked_input_bytes[paths["query_pack"]],
            expected_capture=tracked_input_bytes[paths["capture"]],
            expected_source_universe=tracked_input_bytes[paths["source_universe"]],
            expected_source_binding=tracked_input_bytes[paths["source_binding"]],
            **options,
        )
        config_path, config_sha, socket, selected = runtime[family_id]
        _check_runtime(
            replay["capture"],
            daemon_sha=daemon_sha,
            config_sha=config_sha,
            runner_sha=runner_sha,
            policy_sha=policy_sha,
            config_path=config_path,
            socket=socket,
            selected=selected,
            daemon_path=daemon_binary,
            require_native_selection=per_family,
        )
        command = replay["capture"]["runtime_binding"]["command"]
        _require(
            runtime_command is None or runtime_command == command,
            "Native cohort families mix serving modes",
        )
        runtime_command = command
        families.append(replay)
    # This existing gate checks the complete source and target roster, family
    # identities, captures, replay commitments and independent MRR rows.
    cohort = native_file_evidence.repository_cluster_ci_from_native_families(
        families, manifest["repositories"], manifest["repository_strata"]
    )
    positive: list[tuple[str, str, str, float]] = []
    recall: list[tuple[str, str, str, float]] = []
    identifier: list[tuple[str, str, str, float]] = []
    negative: list[tuple[str, str, str, float]] = []
    hard_negative: list[tuple[str, str, str, float]] = []
    timings: list[int] = []
    for entry, replay in zip(manifest["families"], families, strict=True):
        suite, capture, report = (replay[key] for key in ("suite", "capture", "report"))
        family = entry["family_id"]
        target = report["composite_source_binding"]["target_repository"]
        _require(family == report["composite_source_binding"]["family_id"], "Native family differs")
        by_key = {(row["task_id"], row["route"]): row for row in report["per_query"]}
        _require(len(by_key) == len(report["per_query"]), "Native query pair is duplicated")
        captured = {row["task_id"]: row for row in capture["tasks"]}
        _require(len(captured) == len(capture["tasks"]), "Native timed task is duplicated")
        for task in suite["tasks"]:
            if task["split"] != "eval":
                continue
            task_id = task["task_id"]
            timing = captured[task_id].get("hybrid_micros")
            _require(
                type(timing) is int and 0 <= timing <= 60_000_000, "Native hybrid timing is invalid"
            )
            timings.append(timing)
            if task["answerable"]:
                mrr = _score_pair(by_key, task_id, "file_mrr_at_10")
                rec = _score_pair(by_key, task_id, "file_recall_at_10")
                positive.append((target, task_id, family, mrr))
                recall.append((target, task_id, family, rec))
                if task.get("category") == "identifier":
                    identifier.append((target, task_id, family, mrr))
            else:
                before = by_key.get((task_id, "lexical"), {}).get("status")
                after = by_key.get((task_id, "hybrid"), {}).get("status")
                _require(
                    before in {"abstained", "success"} and after in {"abstained", "success"},
                    "Native negative task lacks an observed abstention status",
                )
                delta = float(after == "success") - float(before == "success")
                negative.append((target, task_id, family, delta))
                if task.get("category") == "hard_negative":
                    hard_negative.append((target, task_id, family, delta))
    _require(timings and positive, "Native quality cohort has no timed positive tasks")
    primary_ci = evaluator.repository_cluster_ci(
        positive,
        cohort["cohort_sha256"],
        manifest["repositories"],
        manifest["repository_strata"],
    )
    recall_ci = evaluator.repository_cluster_ci(
        recall,
        cohort["cohort_sha256"],
        manifest["repositories"],
        manifest["repository_strata"],
    )
    counts = {
        "positive": len(positive),
        "identifier": len(identifier),
        "negative": len(negative),
        "hard_negative": len(hard_negative),
        "repositories": len(manifest["repositories"]),
        "identifier_repositories": len({row[0] for row in identifier}),
        "negative_repositories": len({row[0] for row in negative}),
        "hard_negative_repositories": len({row[0] for row in hard_negative}),
    }
    identifier_mean = _slice_mean(identifier) if identifier else None
    negative_mean = _slice_mean(negative) if negative else None
    hard_negative_mean = _slice_mean(hard_negative) if hard_negative else None
    p95, p99 = _nearest_rank(timings, 0.95), _nearest_rank(timings, 0.99)
    reasons = []
    if counts["positive"] < policy["minimum_counts"]["positive"]:
        reasons.append("positive_denominator_below_frozen_minimum")
    if primary_ci["status"] != "available" or recall_ci["status"] != "available":
        reasons.append("repository_ci_unavailable")
    elif primary_ci["mean"] < 0.02 or primary_ci["lower_95"] <= 0:
        reasons.append("mrr_gain_or_lower_bound_failed")
    if recall_ci["status"] == "available" and recall_ci["lower_95"] < -0.01:
        reasons.append("recall_lower_bound_failed")
    if (
        counts["identifier"] < policy["minimum_counts"]["identifier"]
        or counts["identifier_repositories"] < 4
    ):
        reasons.append("identifier_denominator_insufficient")
    elif identifier_mean < -0.01:
        reasons.append("identifier_mrr_regressed")
    if (
        counts["negative"] < policy["minimum_counts"]["negative"]
        or counts["negative_repositories"] < 4
    ):
        reasons.append("negative_denominator_insufficient")
    elif negative_mean > 0.01:
        reasons.append("negative_false_positive_regressed")
    if (
        counts["hard_negative"] < policy["minimum_counts"]["hard_negative"]
        or counts["hard_negative_repositories"] < 4
    ):
        reasons.append("hard_negative_denominator_insufficient")
    elif hard_negative_mean > 0.01:
        reasons.append("hard_negative_false_positive_regressed")
    if p95 > policy["resource_limits"]["max_query_p95_micros"] or p99 > 500_000:
        reasons.append("observed_query_latency_exceeded")
    controlled_capture_verified = execution_evidence_root is not None
    if controlled_capture_verified:
        if controlled is None:
            controlled = verify_controlled()
        scored = {
            entry["family_id"]: replay["report"]["capture_commitment_sha256"]
            for entry, replay in zip(manifest["families"], families, strict=True)
        }
        _require(controlled == scored, "Native controlled execution differs from scored capture")
    if require_signed_install:
        _require(
            set(signed_install_identity)
            == {"active_generation_sha256", "release_manifest_sha256", "q_elf_sha256"},
            "Native signed installation identity is absent",
        )
    evaluator.verify_native_replay_sources(
        (family_paths[entry["family_id"]]["repo"], replay)
        for entry, replay in zip(manifest["families"], families, strict=True)
    )
    if require_signed_install:
        _require(len(signed_install_recheck) == 1, "Native signed installation recheck is absent")
        plan, selection_path, selection_sha = signed_install_recheck[0]
        _raw, selected = load_selected_snapshot(selection_path, selection_sha)
        import native_file_capture

        native_file_capture._verify_signed_install(plan, selected, policy_sha)

    # Finish source, installation and release-tree checks before rechecking control inputs.
    _require(
        all(_release_tree_sha(path) == sha for path, sha in tracked_releases.items())
        and all(_sha(path) == sha for path, sha in tracked_files.items())
        and all(resolve_path(declared) == target for declared, target in tracked_path_targets)
        and all(
            path(entry, key) == frozen
            for entry in manifest["families"]
            for key, frozen in family_paths[entry["family_id"]].items()
        )
        and all(
            path(entry, "daemon_config") == runtime[entry["family_id"]][0]
            for entry in manifest["families"]
            if per_family
        ),
        "Native decision inputs changed during replay",
    )
    for selection_path, selection_sha in tracked_selections.items():
        try:
            load_selected_snapshot(selection_path, selection_sha)
        except EvidenceError as error:
            raise NativeDecisionError(f"Native selection changed during replay: {error}") from error
    return {
        "status": "quality_reject" if reasons else "quality_admit",
        "reasons": reasons,
        "product_default_decision": False,
        "runtime_qualified": False,
        "execution_binding_verified": False,
        "controlled_capture_verified": controlled_capture_verified,
        "controlled_capture_scope": "owned_subprocess_record_and_replayed_raw_capture_only",
        **(
            {
                "signed_install_verified": True,
                "signed_install_scope": "verified_installed_approval_for_selected_native_cohort",
                "signed_install": signed_install_identity,
                "review_source_manifest_sha256": {
                    family: reviewed_bytes[path][0].removeprefix("sha256:")
                    for family, path in sorted(reviewed.items())
                },
            }
            if require_signed_install
            else {}
        ),
        "capture_binding_consistent": True,
        "capture_consistency_scope": "replayed_capture_commitments_and_current_input_files",
        "qualification_scope": "replayed_file_quality_and_capture_consistency_only",
        "cohort_manifest_sha256": manifest_sha,
        **({"reviewed_labels_manifest_sha256": reviewed_sha} if reviewed_sha is not None else {}),
        "decision_policy_sha256": policy_sha,
        "cohort_sha256": cohort["cohort_sha256"],
        "minimum_counts": policy["minimum_counts"],
        "counts": counts,
        "metrics": {
            "mrr": {"repository_cluster_ci": primary_ci},
            "recall": {"repository_cluster_ci": recall_ci},
            "identifier_mrr_delta": identifier_mean,
            "negative_false_positive_delta": negative_mean,
            "hard_negative_false_positive_delta": hard_negative_mean,
            "query_p95_micros": p95,
            "query_p99_micros": p99,
        },
    }
