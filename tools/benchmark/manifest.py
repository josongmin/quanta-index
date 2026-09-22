"""Canonical benchmark family and profile manifest loader.

The manifest is intentionally a data-only control plane.  Producers remain
Justfile recipes, while validators and the CLI consume the same declared
family, artifact, sample-floor and baseline policy rather than keeping their
own parallel tables.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_MANIFEST_PATH = ROOT / "tools" / "benchmark" / "manifest.json"
SCHEMA_VERSION = 1


class ManifestError(ValueError):
    """The benchmark control-plane manifest is malformed."""


def _object(value: object, where: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise ManifestError(f"{where} must be an object")
    return value


def _string(value: object, where: str) -> str:
    if not isinstance(value, str) or not value:
        raise ManifestError(f"{where} must be a non-empty string")
    return value


def _string_list(value: object, where: str, *, nonempty: bool = True) -> list[str]:
    if not isinstance(value, list) or (nonempty and not value):
        raise ManifestError(f"{where} must be a {'non-empty ' if nonempty else ''}array")
    if not all(isinstance(item, str) and item for item in value):
        raise ManifestError(f"{where} must contain non-empty strings")
    if len(set(value)) != len(value):
        raise ManifestError(f"{where} contains duplicate values")
    return list(value)


def load_manifest(path: Path = DEFAULT_MANIFEST_PATH) -> dict[str, Any]:
    """Load and fail closed on the full benchmark control-plane manifest."""
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ManifestError(f"cannot load {path}: {exc}") from exc
    root = _object(payload, "manifest")
    if set(root) != {"schema_version", "families", "profiles"}:
        raise ManifestError("manifest must contain exactly schema_version, families and profiles")
    if root["schema_version"] != SCHEMA_VERSION:
        raise ManifestError(f"unsupported manifest schema_version {root['schema_version']!r}")

    raw_families = _object(root["families"], "manifest.families")
    if not raw_families:
        raise ManifestError("manifest.families must not be empty")
    families: dict[str, dict[str, Any]] = {}
    for name, raw in raw_families.items():
        _string(name, "manifest family name")
        family = _object(raw, f"family {name!r}")
        expected = {
            "dimension",
            "artifact_glob",
            "producer",
            "minimum_samples",
            "host_policy",
            "baseline",
        }
        if set(family) != expected:
            raise ManifestError(f"family {name!r} must contain exactly {sorted(expected)}")
        if _string(family["dimension"], f"family {name!r}.dimension") != name:
            raise ManifestError(f"family {name!r}.dimension must equal its family name")
        _string(family["artifact_glob"], f"family {name!r}.artifact_glob")
        _string(family["producer"], f"family {name!r}.producer")
        minimum_samples = family["minimum_samples"]
        if minimum_samples is not None and (
            not isinstance(minimum_samples, int)
            or isinstance(minimum_samples, bool)
            or minimum_samples < 1
        ):
            raise ManifestError(
                f"family {name!r}.minimum_samples must be null or a positive integer"
            )
        if family["host_policy"] not in {"any", "local-diagnostic", "canonical-linux"}:
            raise ManifestError(f"family {name!r}.host_policy is not registered")
        baseline = family["baseline"]
        if baseline is not None:
            baseline_obj = _object(baseline, f"family {name!r}.baseline")
            if set(baseline_obj) != {"path", "comparator", "admission"}:
                raise ManifestError(
                    f"family {name!r}.baseline must contain exactly path, comparator and admission"
                )
            _string(baseline_obj["path"], f"family {name!r}.baseline.path")
            if baseline_obj["comparator"] != "dsl-latency":
                raise ManifestError(f"family {name!r}.baseline.comparator is not registered")
            if baseline_obj["admission"] not in {"canonical-linux", "local-diagnostic"}:
                raise ManifestError(f"family {name!r}.baseline.admission is not registered")
        families[name] = family

    raw_profiles = _object(root["profiles"], "manifest.profiles")
    if not raw_profiles:
        raise ManifestError("manifest.profiles must not be empty")
    profiles: dict[str, dict[str, Any]] = {}
    for name, raw in raw_profiles.items():
        _string(name, "manifest profile name")
        profile = _object(raw, f"profile {name!r}")
        if set(profile) != {"families", "recipes", "description"}:
            raise ManifestError(f"profile {name!r} must contain exactly families, recipes and description")
        family_names = _string_list(profile["families"], f"profile {name!r}.families")
        unknown = sorted(set(family_names) - set(families))
        if unknown:
            raise ManifestError(f"profile {name!r} names unknown family(s): {', '.join(unknown)}")
        recipes = _string_list(profile["recipes"], f"profile {name!r}.recipes", nonempty=False)
        expected_recipes = {
            families[family_name]["producer"]
            for family_name in family_names
            if families[family_name]["producer"] != "recorded-experiment"
        }
        if set(recipes) != expected_recipes:
            raise ManifestError(
                f"profile {name!r}.recipes must exactly name each runnable family producer"
            )
        _string(profile["description"], f"profile {name!r}.description")
        profiles[name] = profile

    return {"schema_version": SCHEMA_VERSION, "families": families, "profiles": profiles}


def fresh_families(manifest: dict[str, Any]) -> tuple[tuple[str, str], ...]:
    """Artifact globs for current-source BenchArtifact families."""
    families = manifest["families"]
    assert isinstance(families, dict)
    return tuple((name, family["artifact_glob"]) for name, family in families.items())


def baseline_families(manifest: dict[str, Any]) -> tuple[tuple[str, str], ...]:
    """Committed baseline paths declared by the family manifest."""
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
