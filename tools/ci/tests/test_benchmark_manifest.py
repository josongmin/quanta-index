"""The benchmark registry must not point at phantom Just producers."""

from __future__ import annotations

import importlib.util
import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[3]
MANIFEST_PATH = REPO_ROOT / "tools" / "benchmark" / "manifest.py"
JUSTFILE = REPO_ROOT / "Justfile"
RECIPE = re.compile(r"^([a-z0-9][a-z0-9-]*)(?:\s+[^:]*)?:", re.MULTILINE)


def _load_manifest_module():
    spec = importlib.util.spec_from_file_location("benchmark_manifest", MANIFEST_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_every_runnable_family_producer_exists_in_the_justfile() -> None:
    manifest = _load_manifest_module().load_manifest()
    recipes = set(RECIPE.findall(JUSTFILE.read_text(encoding="utf-8")))
    missing = sorted(
        family["producer"]
        for family in manifest["families"].values()
        if family["producer"] != "recorded-experiment" and family["producer"] not in recipes
    )
    assert missing == [], f"benchmark manifest names missing Just recipe(s): {missing}"


def test_profile_recipes_are_the_exact_non_experimental_family_producers() -> None:
    manifest = _load_manifest_module().load_manifest()
    families = manifest["families"]
    for profile_name, profile in manifest["profiles"].items():
        expected = {
            families[name]["producer"]
            for name in profile["families"]
            if families[name]["producer"] != "recorded-experiment"
        }
        assert set(profile["recipes"]) == expected, profile_name
