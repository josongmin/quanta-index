#!/usr/bin/env python3
from __future__ import annotations

import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "tools/ci/lint"))

from rust_attribute_policy import (  # noqa: E402
    attribute_metas,
    macro_attribute_metas,
    rust_source_files,
)
from tree_sitter_language_pack import get_parser  # noqa: E402


def has_local_duplicate_lint_override(text: str) -> bool:
    if "multiple_crate_versions" not in text:
        return False

    def overrides(metas: list[tuple[str, list[object], bool]]) -> bool:
        for name, arguments, _ in metas:
            if name not in {"allow", "expect", "warn", "deny", "forbid"}:
                continue
            if len(arguments) != 1 or arguments[0].type != "token_tree":
                continue
            tokens = [
                token
                for token in arguments[0].children[1:-1]
                if token.type not in {"line_comment", "block_comment"}
            ]
            if any(
                [token.type for token in tokens[index : index + 3]]
                == ["identifier", "::", "identifier"]
                and tokens[index].text == b"clippy"
                and tokens[index + 2].text == b"multiple_crate_versions"
                for index in range(len(tokens) - 2)
            ):
                return True
        return False

    tree = get_parser("rust").parse(text.encode("utf-8"))

    def visit(node: object) -> bool:
        if node.type in {"macro_invocation", "macro_definition"}:
            return any(overrides(metas) for _, metas in macro_attribute_metas(node))
        if node.type in {"attribute_item", "inner_attribute_item"}:
            return overrides(attribute_metas(node))
        if node.type == "ERROR" and b"#" in node.text:
            raise ValueError("invalid Rust attribute syntax")
        return any(visit(child) for child in node.children)

    return visit(tree.root_node)


def local_duplicate_lint_overrides(crate_dir: Path) -> list[Path]:
    return sorted(
        path
        for path in rust_source_files((crate_dir,))
        if has_local_duplicate_lint_override(path.read_text(encoding="utf-8"))
    )


def workspace_member_manifests() -> list[Path]:
    data = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    members = data.get("workspace", {}).get("members")
    if not isinstance(members, list) or not members:
        raise ValueError("workspace.members must name the audited crates")
    return sorted(ROOT / member / "Cargo.toml" for member in members)


def main() -> int:
    bad: list[Path] = []
    duplicate_lint_overrides: list[Path] = []
    manifests = workspace_member_manifests()

    for cargo_toml in manifests:
        data = tomllib.loads(cargo_toml.read_text(encoding="utf-8"))
        lints = data.get("lints")
        if not isinstance(lints, dict) or lints.get("workspace") is not True:
            bad.append(cargo_toml)
        duplicate_lint_overrides.extend(local_duplicate_lint_overrides(cargo_toml.parent))

    if bad:
        print("These crates do not inherit workspace lints:")
        for path in bad:
            print(f" - {path}")
        return 1

    if duplicate_lint_overrides:
        print(
            "Crate-local multiple_crate_versions overrides reactivate the "
            "allowlisted Cargo-graph lint; cargo-deny owns duplicate policy:"
        )
        for path in duplicate_lint_overrides:
            print(f" - {path}")
        return 1

    print(f"All {len(manifests)} workspace crates inherit workspace lints.")
    print("No crate-local multiple_crate_versions overrides.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
