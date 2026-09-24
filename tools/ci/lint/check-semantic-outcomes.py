#!/usr/bin/env python3
"""Guard closed Rust outcome enums against success laundering.

This deliberately complements, rather than duplicates, the repository's
Semgrep Result/error-fallback rules. It is a syntactic guard over explicitly
registered enum families, not a Rust type checker or a proof of every flow.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[3]
POLICY_PATH = Path(__file__).with_name("semantic-outcome-policy.json")
EXCLUDED_PARTS = frozenset({"tests", "benches", "examples", "fuzz"})


@dataclass(frozen=True)
class EnumFamily:
    name: str
    negative: frozenset[str]
    positive: frozenset[str]
    neutral: frozenset[str]
    error_projection: dict[str, str] | None = None
    preserve_payload: frozenset[str] = frozenset()


@dataclass(frozen=True)
class Finding:
    path: Path
    line: int
    rule: str
    detail: str

    def render(self) -> str:
        return f"{self.path}:{self.line}: {self.rule}: {self.detail}"


def load_policy(path: Path) -> dict[str, EnumFamily]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data, dict) or set(data) != {"schema_version", "enum_families"}:
        raise ValueError("invalid semantic outcome policy shape")
    if data["schema_version"] != 1 or not isinstance(data["enum_families"], list):
        raise ValueError("invalid semantic outcome policy version or families")
    families: dict[str, EnumFamily] = {}
    for row in data["enum_families"]:
        required = {
            "type",
            "negative_variants",
            "positive_variants",
            "neutral_variants",
        }
        optional = {"error_projection", "preserve_payload"}
        if not isinstance(row, dict) or not required <= set(row) or set(row) - required - optional:
            raise ValueError("invalid enum family policy row")
        name = row["type"]
        negative = row["negative_variants"]
        positive = row["positive_variants"]
        neutral = row["neutral_variants"]
        projection = row.get("error_projection", {})
        preserve = row.get("preserve_payload", [])
        if (
            not isinstance(name, str)
            or not re.fullmatch(r"[A-Z][A-Za-z0-9_]*", name)
            or name in families
            or not isinstance(negative, list)
            or not isinstance(positive, list)
            or not isinstance(neutral, list)
            or not negative
            or not positive
            or any(not isinstance(item, str) or not item for item in negative + positive + neutral)
            or len(set(negative)) != len(negative)
            or len(set(positive)) != len(positive)
            or len(set(neutral)) != len(neutral)
            or set(negative) & set(positive)
            or set(negative) & set(neutral)
            or set(positive) & set(neutral)
            or not isinstance(projection, dict)
            or any(
                not isinstance(key, str)
                or not isinstance(value, str)
                or not re.fullmatch(r"[A-Z][A-Za-z0-9_]*::[A-Z][A-Za-z0-9_]*", value)
                for key, value in projection.items()
            )
            or (projection and set(projection) != set(negative))
            or not isinstance(preserve, list)
            or any(not isinstance(item, str) for item in preserve)
            or len(set(preserve)) != len(preserve)
            or not set(preserve) <= set(projection)
        ):
            raise ValueError(f"invalid polarity or duplicate family: {name!r}")
        families[name] = EnumFamily(
            name,
            frozenset(negative),
            frozenset(positive),
            frozenset(neutral),
            projection or None,
            frozenset(preserve),
        )
    if not families:
        raise ValueError("semantic outcome policy must register an enum")
    return families


def rust_parser() -> Any:
    try:
        from tree_sitter_language_pack import get_parser
    except ImportError as error:
        raise RuntimeError(
            "tree-sitter-language-pack is required for semantic outcome lint"
        ) from error
    return get_parser("rust")


def node_text(source: bytes, node: Any) -> str:
    return source[node.start_byte : node.end_byte].decode("utf-8")


def walk(node: Any) -> Iterator[Any]:
    stack = [node]
    while stack:
        current = stack.pop()
        yield current
        stack.extend(reversed(current.named_children))


def cfg_implies_test(expression: str) -> bool:
    expression = expression.strip()
    if expression == "test":
        return True
    head, separator, tail = expression.partition("(")
    head = head.strip()
    if not separator or not tail.endswith(")") or head not in {"all", "any"}:
        return False
    operands: list[str] = []
    depth = 0
    start = 0
    body = tail[:-1]
    for index, character in enumerate(body):
        if character == "(":
            depth += 1
        elif character == ")":
            depth -= 1
        elif character == "," and depth == 0:
            operands.append(body[start:index])
            start = index + 1
    operands.append(body[start:])
    if head == "all":
        return any(cfg_implies_test(part) for part in operands)
    return bool(operands) and all(cfg_implies_test(part) for part in operands)


def is_test_attribute(source: bytes, node: Any) -> bool:
    match = re.fullmatch(r"#\s*\[\s*cfg\s*\((.*)\)\s*\]", node_text(source, node), re.DOTALL)
    return match is not None and cfg_implies_test(match.group(1))


def production_nodes(node: Any, source: bytes) -> Iterator[Any]:
    attributes: list[Any] = []
    for child in node.named_children:
        if child.type == "attribute_item":
            attributes.append(child)
            continue
        test_only = any(is_test_attribute(source, attribute) for attribute in attributes)
        attributes.clear()
        if test_only:
            continue
        yield child
        yield from production_nodes(child, source)


def enclosing_impl_type(node: Any, source: bytes) -> str | None:
    current = node.parent
    while current is not None:
        if current.type == "impl_item":
            target = current.child_by_field_name("type")
            if target is not None:
                return node_text(source, target).split("::")[-1]
        current = current.parent
    return None


def variant_refs(node: Any, source: bytes, families: dict[str, EnumFamily]) -> set[tuple[str, str]]:
    refs: set[tuple[str, str]] = set()
    for child in walk(node):
        if child.type not in {"scoped_identifier", "scoped_type_identifier"}:
            continue
        parts = node_text(source, child).split("::")
        if len(parts) < 2:
            continue
        family_name = parts[-2]
        if family_name == "Self":
            family_name = enclosing_impl_type(child, source) or ""
        family = families.get(family_name)
        if family and parts[-1] in family.negative | family.positive | family.neutral:
            refs.add((family_name, parts[-1]))
    return refs


def direct_pattern_families(
    pattern: Any, source: bytes, families: dict[str, EnumFamily]
) -> set[str]:
    """Exclude wrapped Option/tuple patterns whose wildcard covers other types."""
    text = node_text(source, pattern).strip()
    refs = variant_refs(pattern, source, families)
    return {
        family
        for family, _variant in refs
        if re.match(rf"^(?:[A-Za-z_][A-Za-z0-9_]*::)*{re.escape(family)}::", text)
        or (text.startswith("Self::") and enclosing_impl_type(pattern, source) == family)
    }


def terminal_expression(node: Any) -> Any:
    current = node
    while current.type in {
        "block",
        "parenthesized_expression",
        "return_expression",
        "expression_statement",
    }:
        if not current.named_children:
            break
        current = current.named_children[-1]
    return current


def fail_closed_wildcard(value: Any, source: bytes, families: dict[str, EnumFamily]) -> bool:
    if contains_success(value, source, families):
        return False
    terminal = terminal_expression(value)
    if terminal.type == "call_expression":
        function = terminal.child_by_field_name("function")
        if function is not None and node_text(source, function) == "Err":
            return True
        terminal = function if function is not None else terminal
    elif terminal.type == "struct_expression":
        name = terminal.child_by_field_name("name")
        terminal = name if name is not None else terminal
    if terminal.type not in {"scoped_identifier", "scoped_type_identifier"}:
        return False
    refs = variant_refs(terminal, source, families)
    return bool(refs) and all(variant in families[name].negative for name, variant in refs)


def contains_success(value: Any, source: bytes, families: dict[str, EnumFamily]) -> bool:
    if any(
        node.type == "call_expression"
        and (function := node.child_by_field_name("function")) is not None
        and node_text(source, function) == "Ok"
        for node in walk(value)
    ):
        return True
    return any(
        variant in families[name].positive
        for name, variant in variant_refs(value, source, families)
    )


def projected_error(value: Any, source: bytes) -> Any | None:
    """Return the typed error inside a direct terminal Err(...), if present."""
    terminal = terminal_expression(value)
    if terminal.type != "call_expression":
        return None
    function = terminal.child_by_field_name("function")
    arguments = terminal.child_by_field_name("arguments")
    if function is None or node_text(source, function) != "Err" or arguments is None:
        return None
    children = arguments.named_children
    return children[0] if len(children) == 1 else None


def bound_payload_name(pattern: Any, source: bytes, family: str, variant: str) -> str | None:
    """Only a direct, named tuple payload can establish reason preservation."""
    for node in walk(pattern):
        if node.type != "tuple_struct_pattern":
            continue
        children = node.named_children
        if not children:
            continue
        head = node_text(source, children[0])
        if not (
            head == f"{family}::{variant}"
            or head.endswith(f"::{family}::{variant}")
            or (head == f"Self::{variant}" and enclosing_impl_type(node, source) == family)
        ):
            continue
        if len(children) != 2 or children[1].type != "identifier":
            return None
        name = node_text(source, children[1])
        return name if name != "_" and not name.startswith("_") else None
    return None


def projected_error_name(error: Any, source: bytes) -> str | None:
    if error.type == "call_expression":
        function = error.child_by_field_name("function")
        return node_text(source, function) if function is not None else None
    if error.type in {"scoped_identifier", "scoped_type_identifier"}:
        return node_text(source, error)
    return None


def payload_reaches_error(error: Any, source: bytes, name: str) -> bool:
    return any(
        node.type == "identifier" and node_text(source, node) == name for node in walk(error)
    )


def shadows_payload(value: Any, source: bytes, name: str) -> bool:
    """A new binding with the same spelling cannot prove the original reason survived."""
    for node in walk(value):
        if node.type == "let_declaration":
            pattern = node.child_by_field_name("pattern")
            if pattern is not None and node_text(source, pattern).strip() == name:
                return True
        if node.type == "closure_parameters" and any(
            child.type == "identifier" and node_text(source, child) == name for child in walk(node)
        ):
            return True
    return False


def reject_target_parse_errors(
    root: Any, source: bytes, path: Path, families: dict[str, EnumFamily]
) -> None:
    if not root.has_error:
        return
    targets = tuple(name.encode() for name in families)
    boundaries = {
        "enum_item",
        "expression_statement",
        "let_declaration",
        "match_arm",
        "match_expression",
    }
    for node in walk(root):
        if node.type != "ERROR" and not node.is_missing:
            continue
        evidence = node
        while evidence.parent is not None and evidence.type not in boundaries:
            if evidence.parent.type == "source_file":
                break
            evidence = evidence.parent
        snippet = source[evidence.start_byte : evidence.end_byte]
        self_target = b"Self::" in snippet and enclosing_impl_type(evidence, source) in families
        if any(target in snippet for target in targets) or self_target:
            raise ValueError(
                f"{path}:{node.start_point[0] + 1}: Rust parse error overlaps a registered outcome"
            )


def findings_for_source(
    source: bytes, path: Path, families: dict[str, EnumFamily], parser: Any
) -> tuple[list[Finding], dict[str, set[str]]]:
    root = parser.parse(source).root_node
    reject_target_parse_errors(root, source, path, families)
    findings: list[Finding] = []
    definitions: dict[str, set[str]] = {}
    for node in production_nodes(root, source):
        if node.type == "enum_item":
            name_node = node.child_by_field_name("name")
            name = node_text(source, name_node) if name_node is not None else ""
            if name in families:
                if name in definitions:
                    raise ValueError(f"{path}: duplicate enum definition for {name}")
                definitions[name] = {
                    node_text(source, variant.child_by_field_name("name"))
                    for variant in walk(node)
                    if variant.type == "enum_variant"
                    and variant.child_by_field_name("name") is not None
                }
        if node.type != "match_expression":
            continue
        body = node.child_by_field_name("body")
        if body is None:
            continue
        direct_families: set[str] = set()
        wildcard: Any = None
        for arm in body.named_children:
            if arm.type != "match_arm":
                continue
            pattern = arm.child_by_field_name("pattern")
            value = arm.child_by_field_name("value")
            if pattern is None or value is None:
                continue
            pattern_text = node_text(source, pattern).strip()
            if pattern_text == "_" or re.fullmatch(r"[a-z][a-z0-9_]*", pattern_text):
                wildcard = arm
                continue
            direct_families.update(direct_pattern_families(pattern, source, families))
            refs = variant_refs(pattern, source, families)
            outputs = variant_refs(value, source, families)
            for name, variant in sorted(refs):
                family = families[name]
                promoted = sorted(
                    (output_name, output_variant)
                    for output_name, output_variant in outputs
                    if output_variant in families[output_name].positive
                )
                if variant in family.negative and promoted:
                    findings.append(
                        Finding(
                            path,
                            arm.start_point[0] + 1,
                            "SO-01",
                            f"{name}::{variant} reaches positive {promoted}",
                        )
                    )
                if variant in family.negative and family.error_projection and not promoted:
                    error = projected_error(value, source)
                    expected = family.error_projection[variant]
                    actual = projected_error_name(error, source) if error is not None else None
                    if actual != expected or contains_success(value, source, families):
                        findings.append(
                            Finding(
                                path,
                                arm.start_point[0] + 1,
                                "SO-03",
                                f"{name}::{variant} must project to Err({expected}) without success paths",
                            )
                        )
                    elif variant in family.preserve_payload:
                        binding = bound_payload_name(pattern, source, name, variant)
                        if (
                            binding is None
                            or shadows_payload(value, source, binding)
                            or not payload_reaches_error(error, source, binding)
                        ):
                            findings.append(
                                Finding(
                                    path,
                                    arm.start_point[0] + 1,
                                    "SO-04",
                                    f"{name}::{variant} must preserve its reason in {expected}",
                                )
                            )
        if wildcard is not None:
            value = wildcard.child_by_field_name("value")
            if value is not None:
                for name in sorted(direct_families):
                    binding = node_text(source, wildcard.child_by_field_name("pattern")).strip()
                    if binding != "_" and node_text(source, value).strip() == binding:
                        continue
                    if not fail_closed_wildcard(value, source, families):
                        findings.append(
                            Finding(
                                path,
                                wildcard.start_point[0] + 1,
                                "SO-02",
                                f"{name} wildcard is not an explicit negative terminal",
                            )
                        )
    return findings, definitions


def rust_source_paths(root: Path) -> tuple[Path, ...]:
    result = subprocess.run(
        [
            "git",
            "-C",
            str(root),
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            "*.rs",
        ],
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(f"Git Rust inventory failed: {result.stderr.decode(errors='replace')}")
    paths: list[Path] = []
    for raw in result.stdout.split(b"\0"):
        if not raw:
            continue
        relative = Path(raw.decode("utf-8"))
        parts = relative.parts
        if (
            len(parts) < 4
            or parts[0] not in {"crates", "benchmarks"}
            or "src" not in parts
            or any(part in EXCLUDED_PARTS for part in parts)
            or relative.name == "tests.rs"
            or relative.name.endswith(("_test.rs", "_tests.rs"))
        ):
            continue
        path = root / relative
        if path.is_file():
            paths.append(path)
    return tuple(sorted(paths))


def audit_repository(root: Path, families: dict[str, EnumFamily]) -> list[Finding]:
    parser = rust_parser()
    findings: list[Finding] = []
    definitions: dict[str, set[str]] = {}
    paths = rust_source_paths(root)
    if not paths:
        raise ValueError("Rust source inventory is empty")
    for path in paths:
        source = path.read_bytes()
        if not any(name.encode() in source for name in families):
            continue
        source_findings, source_definitions = findings_for_source(
            source, path.relative_to(root), families, parser
        )
        findings.extend(source_findings)
        for name, variants in source_definitions.items():
            if name in definitions:
                raise ValueError(f"duplicate registered enum definition: {name}")
            definitions[name] = variants
    for name, family in families.items():
        variants = definitions.get(name)
        if variants is None:
            raise ValueError(f"registered enum definition missing: {name}")
        declared = family.negative | family.positive | family.neutral
        if declared != variants:
            raise ValueError(
                f"{name} policy does not cover exact enum variants: "
                f"missing={sorted(variants - declared)}, stale={sorted(declared - variants)}"
            )
    return sorted(findings, key=lambda finding: (finding.path, finding.line, finding.rule))


def main() -> int:
    args = argparse.ArgumentParser(description=__doc__)
    args.add_argument("--root", type=Path, default=ROOT)
    args.add_argument("--policy", type=Path, default=POLICY_PATH)
    options = args.parse_args()
    try:
        families = load_policy(options.policy)
        findings = audit_repository(options.root.resolve(), families)
    except (OSError, RuntimeError, ValueError) as error:
        print(f"semantic outcome lint blocked: {error}", file=sys.stderr)
        return 2
    for finding in findings:
        print(finding.render())
    if findings:
        return 1
    print(f"semantic outcome lint: clean ({len(families)} registered enum families)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
