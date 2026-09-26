#!/usr/bin/env python3
"""Fail-closed reader for the single benchmark registry (``registry.toml``).

The registry is the only data-only control plane for benchmark families. This
module exists so that the common CLI, the CI policy guard and the evidence
writer all resolve families, producers, validators, scorers and closures from
one place instead of keeping parallel tables.

Everything here fails closed: an unknown key, an unknown enum value, a
dangling producer/validator/scorer/closure reference, a producer whose recipe
or module is absent from the checkout, a family that no profile selects, or a
profile that selects one family twice is a typed refusal. Absence is never
turned into a default.
"""

from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path
from typing import Any

try:  # Python >= 3.11
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - pyproject pins tomli for < 3.11
    import tomli as tomllib

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_REGISTRY_PATH = ROOT / "tools" / "benchmark" / "registry.toml"
SCHEMA_VERSION = 1

PAYLOAD_KINDS = frozenset(
    {
        "micro",
        "latency",
        "load",
        "freshness",
        "retrieval",
        "agent_outcome",
        "recorded_experiment",
        "proof",
    }
)
PURPOSES = frozenset(
    {
        "dsl-latency",
        "search-quality",
        "systems",
        "retrieval",
        "agent-outcome",
        "micro",
        "recorded-experiment",
        "correctness-diagnostic",
    }
)
HOST_POLICIES = frozenset({"any", "local-diagnostic", "canonical-linux"})
GATE_TIERS = frozenset({"authority", "advisory", "diagnostic", "contract"})
AUTHORITIES = frozenset({"registry", "legacy", "retired"})
PRODUCER_KINDS = frozenset({"just-recipe", "cargo-bench", "python-module", "recorded-input"})
TOOL_KINDS = frozenset({"python-module", "rust-test"})

FAMILY_KEYS = frozenset(
    {
        "title",
        "purpose",
        "payload",
        "result_unit",
        "producer",
        "validator",
        "scorer",
        "closure",
        "host_policy",
        "gate_tier",
        "sample_floor",
        "baseline",
        "native_schema",
        "authority",
    }
)
#: kind -> (required keys, allowed keys). Optional keys are explicitly listed so
#: a typo in an optional key is still refused rather than silently ignored.
PRODUCER_KEYS: dict[str, tuple[frozenset[str], frozenset[str]]] = {
    "just-recipe": (
        frozenset({"kind", "recipe", "outputs"}),
        frozenset({"kind", "recipe", "outputs", "requires_out"}),
    ),
    "cargo-bench": (
        frozenset({"kind", "package", "target", "outputs"}),
        frozenset({"kind", "package", "target", "outputs"}),
    ),
    "python-module": (
        frozenset({"kind", "module", "argv", "outputs"}),
        frozenset({"kind", "module", "argv", "outputs", "requires_spec"}),
    ),
    "recorded-input": (frozenset({"kind"}), frozenset({"kind"})),
}
TOOL_KEYS = frozenset({"kind", "module", "argv", "package", "target"})

JUST_RECIPE_RE = re.compile(r"^([A-Za-z0-9][A-Za-z0-9_-]*)(\s+[^:]*)?:", re.MULTILINE)
NONE = "none"


class RegistryError(ValueError):
    """The benchmark registry is malformed, unreachable or ambiguous."""


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise RegistryError(message)


def _table(value: object, where: str, *, required: bool = True) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise RegistryError(f"{where} must be a table")
    if required and not value:
        raise RegistryError(f"{where} must not be empty")
    return value


def _nonempty_string(value: object, where: str) -> str:
    if not isinstance(value, str) or not value:
        raise RegistryError(f"{where} must be a non-empty string")
    return value


def _enum(value: object, allowed: frozenset[str], where: str) -> str:
    text = _nonempty_string(value, where)
    if text not in allowed:
        raise RegistryError(f"{where} has unregistered value {text!r}")
    return text


def justfile_recipes(repo_root: Path) -> frozenset[str]:
    """Recipe names declared by the repository Justfile."""
    path = repo_root / "Justfile"
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise RegistryError(f"cannot read Justfile: {exc}") from exc
    return frozenset(match.group(1) for match in JUST_RECIPE_RE.finditer(text))


def _check_module(repo_root: Path, value: object, where: str) -> str:
    module = _nonempty_string(value, where)
    _require(".." not in Path(module).parts, f"{where} must stay inside the repository")
    target = repo_root / module
    _require(target.is_file(), f"{where} does not exist: {module}")
    return module


def load_registry(
    path: Path | None = None,
    *,
    repo_root: Path | None = None,
    check_reachability: bool = True,
) -> dict[str, Any]:
    """Load and fully validate the registry; never return a partial view."""
    registry_path = path or DEFAULT_REGISTRY_PATH
    root = repo_root or registry_path.resolve().parents[2]
    try:
        raw = tomllib.loads(registry_path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise RegistryError(f"cannot load {registry_path}: {exc}") from exc

    expected_top = {
        "schema_version",
        "closures",
        "external_inputs",
        "producers",
        "validators",
        "scorers",
        "families",
        "profiles",
    }
    _require(
        set(raw) == expected_top,
        f"registry must contain exactly {sorted(expected_top)}, found {sorted(raw)}",
    )
    _require(
        raw["schema_version"] == SCHEMA_VERSION,
        f"unsupported registry schema_version {raw['schema_version']!r}",
    )

    closures = _table(raw["closures"], "closures")
    for name, entry in closures.items():
        _nonempty_string(name, "closure id")
        table = _table(entry, f"closures.{name}")
        _require(
            set(table) == {"profile", "description"},
            f"closures.{name} must contain exactly profile and description",
        )
        _nonempty_string(table["profile"], f"closures.{name}.profile")
        _nonempty_string(table["description"], f"closures.{name}.description")

    external_inputs = _table(raw["external_inputs"], "external_inputs")
    for name, entry in external_inputs.items():
        _nonempty_string(name, "external input id")
        table = _table(entry, f"external_inputs.{name}")
        _require(
            set(table) == {"owner", "description", "required_for", "status"},
            f"external_inputs.{name} must contain exactly owner, description, "
            "required_for and status",
        )
        for key in ("owner", "description", "status"):
            _nonempty_string(table[key], f"external_inputs.{name}.{key}")
        required_for = table["required_for"]
        _require(
            isinstance(required_for, list)
            and required_for
            and all(isinstance(item, str) and item for item in required_for),
            f"external_inputs.{name}.required_for must be a non-empty string list",
        )

    recipes = justfile_recipes(root) if check_reachability else frozenset()

    producers = _table(raw["producers"], "producers")
    producer_refs: dict[str, str] = {}
    for name, entry in producers.items():
        _nonempty_string(name, "producer id")
        table = _table(entry, f"producers.{name}")
        kind = _enum(table.get("kind"), PRODUCER_KINDS, f"producers.{name}.kind")
        required_keys, allowed_keys = PRODUCER_KEYS[kind]
        _require(
            required_keys <= set(table) and set(table) <= allowed_keys,
            f"producers.{name} of kind {kind} must contain "
            f"{sorted(required_keys)} and only optionally {sorted(allowed_keys - required_keys)}",
        )
        outputs = table.get("outputs", [])
        _require(
            isinstance(outputs, list) and all(isinstance(item, str) and item for item in outputs),
            f"producers.{name}.outputs must be a string list",
        )
        if kind == "just-recipe":
            recipe = _nonempty_string(table["recipe"], f"producers.{name}.recipe")
            if check_reachability:
                _require(
                    recipe in recipes,
                    f"producers.{name} names missing Justfile recipe {recipe!r}",
                )
            ref = f"just:{recipe}"
        elif kind == "cargo-bench":
            package = _nonempty_string(table["package"], f"producers.{name}.package")
            target = _nonempty_string(table["target"], f"producers.{name}.target")
            ref = f"cargo-bench:{package}:{target}"
        elif kind == "python-module":
            module = _check_module(root, table["module"], f"producers.{name}.module")
            argv = table["argv"]
            _require(
                isinstance(argv, list) and all(isinstance(item, str) for item in argv),
                f"producers.{name}.argv must be a string list",
            )
            ref = f"python:{module}:{' '.join(argv)}"
        else:
            ref = f"recorded:{name}"
        duplicate_owner = next(
            (key for key, value in producer_refs.items() if value == ref), None
        )
        if duplicate_owner is not None:
            raise RegistryError(
                f"producers.{name} duplicates producer {ref!r} already owned by {duplicate_owner}"
            )
        producer_refs[name] = ref

    def _load_tools(section: str) -> dict[str, dict[str, Any]]:
        tools = _table(raw[section], section)
        for name, entry in tools.items():
            _nonempty_string(name, f"{section} id")
            table = _table(entry, f"{section}.{name}")
            kind = _enum(table.get("kind"), TOOL_KINDS, f"{section}.{name}.kind")
            if kind == "python-module":
                allowed = frozenset({"kind", "module", "argv"})
                _require(
                    set(table) <= allowed and {"kind", "module"} <= set(table),
                    f"{section}.{name} of kind python-module must contain exactly "
                    "kind, module and optional argv",
                )
                _check_module(root, table["module"], f"{section}.{name}.module")
                argv = table.get("argv", [])
                _require(
                    isinstance(argv, list) and all(isinstance(item, str) for item in argv),
                    f"{section}.{name}.argv must be a string list",
                )
            else:
                allowed = frozenset({"kind", "package", "target"})
                _require(
                    set(table) == allowed,
                    f"{section}.{name} of kind rust-test must contain exactly "
                    "kind, package and target",
                )
                _nonempty_string(table["package"], f"{section}.{name}.package")
                _nonempty_string(table["target"], f"{section}.{name}.target")
        return tools

    validators = _load_tools("validators")
    scorers = _load_tools("scorers")

    families = _table(raw["families"], "families")
    for name, entry in families.items():
        _nonempty_string(name, "family id")
        table = _table(entry, f"families.{name}")
        _require(
            set(table) == FAMILY_KEYS,
            f"families.{name} must contain exactly {sorted(FAMILY_KEYS)}, "
            f"found {sorted(table)}",
        )
        _nonempty_string(table["title"], f"families.{name}.title")
        _enum(table["purpose"], PURPOSES, f"families.{name}.purpose")
        _enum(table["payload"], PAYLOAD_KINDS, f"families.{name}.payload")
        _nonempty_string(table["result_unit"], f"families.{name}.result_unit")
        _enum(table["host_policy"], HOST_POLICIES, f"families.{name}.host_policy")
        _enum(table["gate_tier"], GATE_TIERS, f"families.{name}.gate_tier")
        _enum(table["authority"], AUTHORITIES, f"families.{name}.authority")
        _nonempty_string(table["native_schema"], f"families.{name}.native_schema")
        floor = table["sample_floor"]
        _require(
            isinstance(floor, int) and not isinstance(floor, bool) and floor >= 0,
            f"families.{name}.sample_floor must be a non-negative integer",
        )
        producer = _nonempty_string(table["producer"], f"families.{name}.producer")
        if producer != NONE:
            _require(
                producer in producers,
                f"families.{name} references unknown producer {producer!r}",
            )
            _require(
                producers[producer]["kind"] != "recorded-input",
                f"families.{name} must not use the recorded-input producer as a command owner",
            )
        else:
            _require(
                table["payload"] in {"agent_outcome", "recorded_experiment"},
                f"families.{name} may only omit a producer for recorded payload kinds",
            )
        for key in ("validator", "scorer"):
            reference = _nonempty_string(table[key], f"families.{name}.{key}")
            if reference == NONE:
                _require(
                    key == "scorer",
                    f"families.{name}.validator is required; only the scorer may be none",
                )
                continue
            pool = validators if key == "validator" else scorers
            _require(
                reference in pool,
                f"families.{name} references unknown {key} {reference!r}",
            )
        closure = _nonempty_string(table["closure"], f"families.{name}.closure")
        _require(closure in closures, f"families.{name} references unknown closure {closure!r}")
        baseline = _nonempty_string(table["baseline"], f"families.{name}.baseline")
        if baseline != NONE:
            _require(
                ".." not in Path(baseline).parts and not Path(baseline).is_absolute(),
                f"families.{name}.baseline must be a repository-relative path",
            )

    profiles = _table(raw["profiles"], "profiles")
    membership: dict[str, list[str]] = {name: [] for name in families}
    for name, entry in profiles.items():
        _nonempty_string(name, "profile id")
        table = _table(entry, f"profiles.{name}")
        _require(
            set(table) == {"description", "families"},
            f"profiles.{name} must contain exactly description and families",
        )
        _nonempty_string(table["description"], f"profiles.{name}.description")
        selected = table["families"]
        _require(
            isinstance(selected, list)
            and selected
            and all(isinstance(item, str) and item for item in selected),
            f"profiles.{name}.families must be a non-empty string list",
        )
        _require(
            len(set(selected)) == len(selected),
            f"profiles.{name} selects the same family twice (ambiguous membership)",
        )
        unknown = sorted(set(selected) - set(families))
        _require(not unknown, f"profiles.{name} names unknown families: {', '.join(unknown)}")
        for family in selected:
            membership[family].append(name)

    orphans = sorted(name for name, owners in membership.items() if not owners)
    _require(not orphans, f"families selected by no profile: {', '.join(orphans)}")

    return {
        "schema_version": SCHEMA_VERSION,
        "closures": closures,
        "external_inputs": external_inputs,
        "producers": producers,
        "validators": validators,
        "scorers": scorers,
        "families": families,
        "profiles": profiles,
        "membership": {name: tuple(owners) for name, owners in membership.items()},
    }


def canonical_registry_bytes(registry: dict[str, Any]) -> bytes:
    """Canonical, digest-stable projection of the validated registry."""
    core = {
        key: registry[key]
        for key in ("schema_version", "closures", "external_inputs", "producers",
                    "validators", "scorers", "families", "profiles")
    }
    return json.dumps(core, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def registry_digest(registry: dict[str, Any]) -> str:
    """``sha256:<hex>`` over the canonical registry projection."""
    return "sha256:" + hashlib.sha256(canonical_registry_bytes(registry)).hexdigest()


def family_names(registry: dict[str, Any]) -> tuple[str, ...]:
    families = registry["families"]
    assert isinstance(families, dict)
    return tuple(sorted(families))


def producer_of(registry: dict[str, Any], family: str) -> dict[str, Any]:
    """Resolved producer table for one family (``recorded-input`` when absent)."""
    families = registry["families"]
    assert isinstance(families, dict)
    reference = families[family]["producer"]
    if reference == NONE:
        return {"kind": "recorded-input"}
    producers = registry["producers"]
    assert isinstance(producers, dict)
    return producers[reference]
