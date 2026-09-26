"""Registry-backed view of the benchmark control plane.

`tools/benchmark/registry.toml` is the single data-only authority (see
`tools/benchmark/registry.py`). This module projects it into the artifact
checker's family/profile shape so that `check-bench-artifacts.py`,
`quality_integration_summary.py` and `benchctl.py` consume the same declared
family, artifact, sample-floor, host-policy, verdict and baseline policy
instead of keeping a parallel table.

There is no second data source: `tools/benchmark/manifest.json` was removed
when the registry landed. An artifact family is one whose registered producer
declares at least one output path; families with no native artifact (crate-local
Criterion benches, retrieval rails and recorded-only evaluators) are not
`BenchArtifactV1` families and are intentionally absent from this projection.
"""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from registry import RegistryError, load_registry  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_MANIFEST_PATH = ROOT / "tools" / "benchmark" / "registry.toml"
SCHEMA_VERSION = 2

#: Purposes whose rails must carry an explicit rail verdict in their artifact.
VERDICT_PURPOSES = frozenset({"search-quality", "systems"})
#: Comparator ids the artifact checker knows how to compare.
BASELINE_COMPARATORS = frozenset({"dsl-latency"})


class ManifestError(ValueError):
    """The benchmark control plane is malformed."""


def _artifact_glob(registry: dict[str, Any], family: dict[str, Any]) -> str | None:
    if family["native_schema"] != "BenchArtifactV1:2":
        return None
    reference = family["producer"]
    if reference == "none":
        return None
    producer = registry["producers"].get(reference)
    if not isinstance(producer, dict):
        raise ManifestError(f"family references unknown producer {reference!r}")
    outputs = producer.get("outputs") or []
    if not outputs:
        return None
    return outputs[0]


def _baseline(family: dict[str, Any]) -> dict[str, str] | None:
    path = family["baseline"]
    if path == "none":
        return None
    scorer = family["scorer"]
    if scorer == "none" or scorer not in BASELINE_COMPARATORS:
        raise ManifestError(
            f"family baseline {path!r} needs a registered comparator, found {scorer!r}"
        )
    admission = (
        "canonical-linux" if family["host_policy"] == "canonical-linux" else "local-diagnostic"
    )
    return {"path": path, "comparator": scorer, "admission": admission}


def load_manifest(path: Path | None = None, repo_root: Path | None = None) -> dict[str, Any]:
    """Load the registry and project it into the artifact-checker shape."""
    try:
        registry = load_registry(path or DEFAULT_MANIFEST_PATH, repo_root=repo_root or ROOT)
    except RegistryError as exc:
        raise ManifestError(str(exc)) from exc

    families: dict[str, dict[str, Any]] = {}
    for name, family in registry["families"].items():
        glob = _artifact_glob(registry, family)
        if glob is None:
            continue
        families[name] = {
            "dimension": name,
            "artifact_glob": glob,
            "producer": name,
            "payload": family["payload"],
            "minimum_samples": family["sample_floor"] or None,
            "host_policy": family["host_policy"],
            "requires_verdict": family["purpose"] in VERDICT_PURPOSES,
            "baseline": _baseline(family),
        }

    profiles: dict[str, dict[str, Any]] = {}
    for name, profile in registry["profiles"].items():
        selected = [family for family in profile["families"] if family in families]
        if not selected:
            continue
        recipes = [
            registry["producers"][registry["families"][family]["producer"]]["recipe"]
            for family in selected
            if registry["families"][family]["producer"] != "none"
            and registry["producers"][registry["families"][family]["producer"]]["kind"]
            == "just-recipe"
        ]
        profiles[name] = {
            "families": selected,
            "recipes": recipes,
            "description": profile["description"],
        }

    return {"schema_version": SCHEMA_VERSION, "families": families, "profiles": profiles}


def fresh_families(manifest: dict[str, Any]) -> tuple[tuple[str, str], ...]:
    """Artifact globs for current-source BenchArtifact families."""
    families = manifest["families"]
    assert isinstance(families, dict)
    return tuple((name, family["artifact_glob"]) for name, family in families.items())


def baseline_families(manifest: dict[str, Any]) -> tuple[tuple[str, str], ...]:
    """Committed baseline paths declared by the family registry."""
    families = manifest["families"]
    assert isinstance(families, dict)
    return tuple(
        (name, family["baseline"]["path"])
        for name, family in families.items()
        if family["baseline"] is not None
    )


def profile_families(manifest: dict[str, Any]) -> dict[str, tuple[str, ...]]:
    """Profile membership in validator-ready tuple form."""
    profiles = manifest["profiles"]
    assert isinstance(profiles, dict)
    return {name: tuple(profile["families"]) for name, profile in profiles.items()}
