#!/usr/bin/env python3
"""Fail closed when executable test targets lack an authoritative CI rail.

``tools/ci/test-authority.toml`` is the machine-readable authority for Rust
integration, cargo-fuzz, and explicitly registered Python owner targets. Cargo
targets are fully inventoried:

* ``crates/*/tests/*.rs`` -- direct children are integration-test binaries;
  nested helpers such as ``tests/common/*.rs`` are modules, not targets.
* ``benchmarks/*/tests/*.rs`` -- benchmark workspace members follow the same
  rule as crates.
* ``crates/*/fuzz/fuzz_targets/*.rs`` -- cargo-fuzz executables declared by
  the sibling fuzz manifest.

The catalog must explicitly map every discovered target to an owner and a
declared rail.  P0/P1 invariants additionally need positive, negative,
recovery, and consumer proof targets.  The guard never infers a rail from a
file name or accepts an unregistered target: both are correctness gaps.
Python targets are opt-in owner files, not a repository-wide pytest inventory.
"""

from __future__ import annotations

import argparse
import ast
import itertools
import json
import math
import re
import shlex
import sys
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parents[3]
DEFAULT_CATALOG = ROOT / "tools" / "ci" / "test-authority.toml"
SUPPORTED_TIERS = frozenset({"pr", "merge", "main", "correctness", "nightly", "weekly"})
SUPPORTED_TARGET_KINDS = frozenset({"integration", "fuzz", "python"})
PROOF_ROLES = (
    "positive_target",
    "negative_target",
    "recovery_target",
    "consumer_target",
)


@dataclass(frozen=True)
class Violation:
    path: Path
    message: str


def _violation(path: Path, message: str) -> Violation:
    return Violation(path=path, message=message)


def _relative_path(
    value: object, *, catalog: Path, field: str, violations: list[Violation]
) -> str | None:
    if not isinstance(value, str) or not value:
        violations.append(_violation(catalog, f"{field} must be a non-empty relative path"))
        return None
    candidate = PurePosixPath(value)
    if candidate.is_absolute() or ".." in candidate.parts:
        violations.append(
            _violation(catalog, f"{field} must stay within the repository: {value!r}")
        )
        return None
    return candidate.as_posix()


def _string(
    value: object, *, catalog: Path, context: str, violations: list[Violation]
) -> str | None:
    if isinstance(value, str) and value.strip():
        return value
    violations.append(_violation(catalog, f"{context} must be a non-empty string"))
    return None


def _load_catalog(catalog: Path, violations: list[Violation]) -> dict[str, Any] | None:
    if not catalog.is_file():
        violations.append(_violation(catalog, "test-authority catalog is missing"))
        return None
    try:
        parsed = tomllib.loads(catalog.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        violations.append(_violation(catalog, f"cannot parse TOML: {error}"))
        return None
    if not isinstance(parsed, dict):
        violations.append(_violation(catalog, "catalog root must be a TOML table"))
        return None
    return parsed


def _discover_integration_targets(root: Path) -> set[str]:
    patterns = ("crates/*/tests/*.rs", "benchmarks/*/tests/*.rs")
    found: set[str] = set()
    for pattern in patterns:
        for path in root.glob(pattern):
            if path.is_file():
                found.add(path.relative_to(root).as_posix())
    return found


def _discover_fuzz_targets(root: Path) -> set[str]:
    return {
        path.relative_to(root).as_posix()
        for path in root.glob("crates/*/fuzz/fuzz_targets/*.rs")
        if path.is_file()
    }


def _load_workflow(
    *, root: Path, catalog: Path, workflow_path: str, violations: list[Violation]
) -> dict[str, Any] | None:
    """Load a repository-owned GitHub workflow without silently accepting drift."""
    path = root / workflow_path
    if not path.is_file():
        violations.append(_violation(catalog, f"rail workflow does not exist: {workflow_path}"))
        return None
    try:
        import yaml
    except ModuleNotFoundError:
        violations.append(_violation(catalog, "PyYAML is required to validate CI rail bindings"))
        return None

    class UniqueKeyLoader(yaml.SafeLoader):
        def construct_mapping(self, node, deep=False):
            self.flatten_mapping(node)
            result = {}
            for key_node, value_node in node.value:
                key = self.construct_object(key_node, deep=deep)
                try:
                    duplicate = key in result
                except TypeError as error:
                    raise yaml.constructor.ConstructorError(
                        None, None, "workflow key must be scalar", key_node.start_mark
                    ) from error
                if duplicate:
                    raise yaml.constructor.ConstructorError(
                        None, None, f"duplicate workflow key: {key!r}", key_node.start_mark
                    )
                result[key] = self.construct_object(value_node, deep=deep)
            return result

    try:
        parsed = yaml.load(path.read_text(encoding="utf-8"), Loader=UniqueKeyLoader)
    except (OSError, yaml.YAMLError) as error:
        violations.append(_violation(path, f"cannot parse workflow YAML: {error}"))
        return None
    if not isinstance(parsed, dict):
        violations.append(_violation(path, "workflow root must be a YAML mapping"))
        return None
    return parsed


def _validate_rail_binding(
    *,
    root: Path,
    catalog: Path,
    rail_id: str,
    raw_rail: dict[str, Any],
    command: str,
    violations: list[Violation],
) -> None:
    workflow_path = _relative_path(
        raw_rail.get("workflow"),
        catalog=catalog,
        field=f"rail {rail_id}.workflow",
        violations=violations,
    )
    job_id = _string(
        raw_rail.get("job"),
        catalog=catalog,
        context=f"rail {rail_id}.job",
        violations=violations,
    )
    step_name = _string(
        raw_rail.get("step"),
        catalog=catalog,
        context=f"rail {rail_id}.step",
        violations=violations,
    )
    if workflow_path is None or job_id is None or step_name is None:
        return
    workflow = _load_workflow(
        root=root, catalog=catalog, workflow_path=workflow_path, violations=violations
    )
    if workflow is None:
        return
    trigger = workflow.get("on", workflow.get(True))
    required_events = {
        "pr": {"pull_request"},
        "merge": {"merge_group"},
        "main": {"push"},
        "nightly": {"schedule"},
        "weekly": {"schedule"},
        "correctness": {"schedule", "workflow_dispatch"},
    }.get(raw_rail.get("tier"), set())
    enabled_events = (
        set(trigger)
        if isinstance(trigger, dict)
        else (
            {trigger}
            if isinstance(trigger, str)
            else set(trigger)
            if isinstance(trigger, list)
            else set()
        )
    )
    if not enabled_events.intersection(required_events):
        violations.append(
            _violation(catalog, f"rail {rail_id} workflow cannot run for its tier event")
        )
        return
    jobs = workflow.get("jobs")
    if not isinstance(jobs, dict) or not isinstance(jobs.get(job_id), dict):
        violations.append(
            _violation(catalog, f"rail {rail_id} workflow job does not exist: {job_id}")
        )
        return
    if jobs[job_id].get("continue-on-error", False) is not False or not _condition_allows_tier(
        jobs[job_id].get("if"),
        enabled_events.intersection(required_events),
        require_guaranteed=True,
    ):
        violations.append(_violation(catalog, f"rail {rail_id} workflow job is disabled"))
        return
    steps = jobs[job_id].get("steps")
    if not isinstance(steps, list):
        violations.append(_violation(catalog, f"rail {rail_id} job {job_id} has no steps"))
        return
    for step in steps:
        if not isinstance(step, dict) or step.get("name") != step_name:
            continue
        if step.get("continue-on-error", False) is not False or not _condition_allows_tier(
            _joint_condition(jobs[job_id].get("if"), step.get("if")),
            enabled_events.intersection(required_events),
            require_guaranteed=True,
        ):
            violations.append(_violation(catalog, f"rail {rail_id} workflow step is disabled"))
            return
        shell = _rail_shell(workflow, jobs[job_id], step)
        if shell is None:
            violations.append(
                _violation(catalog, f"rail {rail_id} shell does not prove failure propagation")
            )
            return
        run = step.get("run")
        if isinstance(run, str) and _executes_declared_command(run, command, shell_pipefail=shell):
            return
        violations.append(
            _violation(
                catalog,
                f"rail {rail_id} workflow step {step_name!r} does not execute declared command",
            )
        )
        return
    violations.append(
        _violation(catalog, f"rail {rail_id} workflow step does not exist: {step_name!r}")
    )


def _rail_shell(workflow: dict, job: dict, step: dict) -> bool | None:
    """Return the shell's initial pipefail state, or reject unknown semantics.

    GitHub's default Linux/macOS shell uses errexit; explicit bash also uses
    pipefail. Custom templates without errexit can swallow a failed rail by
    executing a later successful command. Only known templates establish proof.
    """
    for scope in (workflow, job, step):
        environment = scope.get("env", {})
        if not isinstance(environment, dict) or any(
            isinstance(key, str) and key.upper() in {"BASH_ENV", "ENV", "SHELLOPTS", "BASHOPTS"}
            for key in environment
        ):
            return None
    shell = None
    for scope in (workflow, job):
        defaults = scope.get("defaults", {})
        if not isinstance(defaults, dict) or not isinstance(defaults.get("run", {}), dict):
            return None
        run = defaults.get("run", {})
        if "shell" in run:
            shell = run["shell"]
    if "shell" in step:
        shell = step["shell"]
    if shell is None:
        runner = job.get("runs-on")
        labels = [runner] if isinstance(runner, str) else runner if isinstance(runner, list) else []
        if any(
            isinstance(label, str) and ("windows" in label.lower() or "${{" in label)
            for label in labels
        ):
            return None
        return False
    if not isinstance(shell, str):
        return None
    known = {
        "bash": True,
        "sh": False,
        "bash --noprofile --norc -eo pipefail {0}": True,
        "bash -e {0}": False,
        "sh -e {0}": False,
    }
    return known.get(shell.strip())


def _literal_shell_argv(text: str) -> list[str] | None:
    """Decode only literal Bash words; expansion is not execution authority.

    shlex is not a Bash lexer: it mishandles escaped newlines, double-quote
    escapes and embedded comment markers. All command-name and argv decisions
    share this restricted decoder. Unknown syntax fails closed, never expands.
    """
    words: list[str] = []
    word: list[str] = []
    active = False
    quote: str | None = None
    index = 0
    while index < len(text):
        char = text[index]
        if quote == "'":
            if char == "'":
                quote = None
            else:
                word.append(char)
        elif char == "\\":
            index += 1
            if index == len(text):
                return None
            escaped = text[index]
            if escaped != "\n":
                if quote == '"' and escaped not in '\\"$`':
                    word.append("\\")
                word.append(escaped)
                active = True
        elif quote == '"':
            if char == '"':
                quote = None
            elif char in "$`":
                return None
            else:
                word.append(char)
        elif char in "'\"":
            quote = char
            active = True
        elif char in " \t\n":
            if active:
                words.append("".join(word))
                word = []
                active = False
        elif char == "#" and not active:
            break
        elif char in "$`*?[]{}~;|&<>()":
            return None
        else:
            word.append(char)
            active = True
        index += 1
    if quote is not None:
        return None
    if active:
        words.append("".join(word))
    return words or None


def _executes_declared_command(run: str, command: str, *, shell_pipefail: bool = False) -> bool:
    """Require a foreground top-level command; shell text is not execution.

    Authority rails use straight-line shell commands. Conditional lists,
    functions, loops, heredocs and background jobs cannot establish a binding.
    """
    from tree_sitter_language_pack import get_parser

    if "${{" in run:
        # Actions renders expressions before shell parsing. An unknown input
        # can inject control flow even inside a quoted echo argument. Runtime
        # data belongs in env, whose shell expansion cannot insert syntax.
        return False
    source = run.encode("utf-8")
    tree = get_parser("bash").parse(source)
    if tree.root_node.has_error:
        return False
    declared = _literal_shell_argv(command)
    if declared is None:
        return False
    pipefail_enabled = shell_pipefail
    for node in tree.root_node.children:
        if node.type in {"comment", ";", "\n"}:
            continue
        if node.type == "&":
            return False
        if node.type == "redirected_statement":
            body = node.child_by_field_name("body")
            if body is None:
                return False
            node = body
        if node.type == "list":
            parts = node.named_children
            guard = parts[0].child_by_field_name("name") if len(parts) == 2 else None
            if (
                len(parts) != 2
                or parts[0].type != "command"
                or parts[1].type != "command"
                or guard is None
                or source[guard.start_byte : guard.end_byte] != b"cd"
                or source[parts[0].end_byte : parts[1].start_byte].strip() != b"&&"
            ):
                return False
            if any(
                later.start_byte >= node.end_byte and later.type != "comment"
                for later in tree.root_node.named_children
            ):
                return False
            # A failed cd keeps the AND list failed. Its successful path must
            # execute the foreground rail. It must be the last statement;
            # errexit does not abort on an AND list's failed left operand.
            node = parts[1]
        if node.type == "command":
            if any(child.type == "variable_assignment" for child in node.named_children):
                # Prefix assignments are not argv[0] and can change command
                # resolution. Never let them hide a shell-control builtin.
                return False
            words = _literal_shell_argv(source[node.start_byte : node.end_byte].decode())
            if words is None:
                return False
            executable = words[0]
            if executable in {
                "exit",
                "return",
                "exec",
                "false",
                "eval",
                "source",
                ".",
                "builtin",
                "command",
                "alias",
                "unalias",
                "trap",
                "enable",
                "shopt",
            }:
                # These builtins can alter control flow, replace the declared
                # executable, or swallow an error in the current shell.
                return False
            if executable == "set":
                # Admit a small, literal option grammar, not substrings of
                # shell text. Quoted/concatenated words retain their meaning.
                if any(
                    descendant.type in {"expansion", "simple_expansion", "command_substitution"}
                    for descendant in _shell_nodes(node)
                ):
                    return False
                options = iter(words[1:])
                for option in options:
                    if option in {"-o", "+o"}:
                        value = next(options, None)
                        if value == "pipefail":
                            pipefail_enabled = option == "-o"
                        elif option != "-o" or value not in {"errexit", "nounset", "xtrace"}:
                            return False
                    elif re.fullmatch(r"-[euxE]+o", option):
                        if next(options, None) != "pipefail":
                            return False
                        pipefail_enabled = True
                    elif re.fullmatch(r"-[euxE]+", option) is None:
                        return False
                if len(words) < 2:
                    return False
                continue
        elif node.type != "pipeline":
            # Setup assignments/declarations are harmless. Unknown control
            # flow before the rail cannot prove that the rail is reachable.
            if node.type in {"variable_assignment", "declaration_command"}:
                continue
            return False
        candidate = node.named_children[0] if node.type == "pipeline" else node
        if candidate.type == "redirected_statement":
            candidate = candidate.child_by_field_name("body")
        if candidate is not None and candidate.type == "command":
            if any(
                descendant.type
                in {
                    "expansion",
                    "simple_expansion",
                    "command_substitution",
                    "ansi_c_string",
                    "variable_assignment",
                }
                for descendant in _shell_nodes(candidate)
            ):
                continue
            invocation = _literal_shell_argv(
                source[candidate.start_byte : candidate.end_byte].decode()
            )
            if invocation is None or invocation != declared:
                continue
            # A trailing '&' is a separate program child, outside the command.
            following = source[node.end_byte :].lstrip()
            if following.startswith(b"&"):
                return False
            if node.type == "pipeline" and not pipefail_enabled:
                continue
            return True
    return False


def _shell_nodes(node: Any):
    yield node
    for child in node.named_children:
        yield from _shell_nodes(child)


def _joint_condition(job: object, step: object) -> object:
    def expression(value: object) -> str:
        if value is None or value is True:
            return "true"
        if value is False or not isinstance(value, str):
            return "false"
        value = value.strip()
        return value[3:-2].strip() if value.startswith("${{") and value.endswith("}}") else value

    return f"({expression(job)}) && ({expression(step)})"


def _is_disabled(value: object) -> bool:
    if value is False:
        return True
    if not isinstance(value, str):
        return False
    return value.strip().lower() in {"false", "${{ false }}"}


def _parse_actions_condition(expression: str) -> ast.Expression:
    """Parse the supported Actions grammar with its own operator precedence.

    Python's ``not`` binds below equality; Actions ``!`` binds above it. Build
    nodes explicitly instead of translating text into Python expressions.
    """
    token_pattern = re.compile(
        r"'(?:[^']|'')*'|[a-zA-Z_][a-zA-Z_0-9]*(?:\.[a-zA-Z_][a-zA-Z_0-9]*)*"
        r"|-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?"
        r"|&&|\|\||==|!=|[!()]"
    )
    tokens: list[str] = []
    offset = 0
    while offset < len(expression):
        if expression[offset].isspace():
            offset += 1
            continue
        match = token_pattern.match(expression, offset)
        if match is None:
            raise ValueError("unsupported Actions token")
        tokens.append(match.group())
        offset = match.end()
        if len(tokens) > 128:
            raise ValueError("Actions expression exceeds parser budget")
    position = 0

    def take(token: str) -> bool:
        nonlocal position
        if position < len(tokens) and tokens[position] == token:
            position += 1
            return True
        return False

    def unary() -> ast.expr:
        nonlocal position
        if take("!"):
            return ast.UnaryOp(op=ast.Not(), operand=unary())
        if take("("):
            node = disjunction()
            if not take(")"):
                raise ValueError("unclosed Actions group")
            return node
        if position == len(tokens):
            raise ValueError("missing Actions operand")
        token = tokens[position]
        position += 1
        if token.startswith("'"):
            return ast.Constant(value=token[1:-1].replace("''", "'"))
        constants = {"true": True, "false": False, "null": None}
        if token.lower() in constants:
            return ast.Constant(value=constants[token.lower()])
        if token[0].isdigit() or token[0] == "-":
            return ast.Constant(value=float(json.loads(token)))
        if not re.fullmatch(r"[a-zA-Z_][a-zA-Z_0-9]*(?:\.[a-zA-Z_][a-zA-Z_0-9]*)*", token):
            raise ValueError("invalid Actions operand")
        parts = token.split(".")
        node = ast.Name(id=parts[0], ctx=ast.Load())
        for part in parts[1:]:
            node = ast.Attribute(value=node, attr=part, ctx=ast.Load())
        if take("("):
            if not take(")"):
                raise ValueError("only zero-argument status calls are supported")
            return ast.Call(func=node, args=[], keywords=[])
        return node

    def equality() -> ast.expr:
        node = unary()
        if take("=="):
            return ast.Compare(left=node, ops=[ast.Eq()], comparators=[unary()])
        if take("!="):
            return ast.Compare(left=node, ops=[ast.NotEq()], comparators=[unary()])
        return node

    def conjunction() -> ast.expr:
        values = [equality()]
        while take("&&"):
            values.append(equality())
        return values[0] if len(values) == 1 else ast.BoolOp(op=ast.And(), values=values)

    def disjunction() -> ast.expr:
        values = [conjunction()]
        while take("||"):
            values.append(conjunction())
        return values[0] if len(values) == 1 else ast.BoolOp(op=ast.Or(), values=values)

    result = disjunction()
    if position != len(tokens):
        raise ValueError("unsupported Actions expression suffix")
    return ast.Expression(body=result)


def _condition_allows_tier(
    value: object, events: set[str], *, require_guaranteed: bool = False
) -> bool:
    """Find a concrete scalar witness, never independent comparison booleans.

    Supported syntax is literals, context paths, equality, Boolean operators,
    and zero-argument status checks. Equality/truthiness follow GitHub's scalar
    rules, including case-insensitive strings and numeric coercion. Candidate
    values form a bounded witness search, not a complete Actions interpreter:
    unsupported syntax or an exhausted budget cannot establish a rail.
    """
    if _is_disabled(value):
        return False
    if value is None or value is True:
        return bool(events)
    if not isinstance(value, str) or not events:
        return False
    expression = value.strip()
    if expression.startswith("${{") and expression.endswith("}}"):
        expression = expression[3:-2].strip()
    if not expression:
        return False

    try:
        tree = _parse_actions_condition(expression)
    except (ValueError, RecursionError):
        return False

    symbols: set[str] = set()
    literals: list[object] = [None, False, True, 0, 1, "", "__authority_other__"]
    status_used = False

    def context_path(node: ast.AST) -> str | None:
        if isinstance(node, ast.Name):
            return node.id.lower()
        if isinstance(node, ast.Attribute):
            prefix = context_path(node.value)
            return f"{prefix}.{node.attr.lower()}" if prefix else None
        return None

    def supported(node: ast.AST) -> bool:
        nonlocal status_used
        if isinstance(node, ast.Expression):
            return supported(node.body)
        if isinstance(node, ast.Constant):
            if node.value is None or type(node.value) in (bool, str, int, float):
                if isinstance(node.value, float) and not math.isfinite(node.value):
                    return False
                # OrdinalIgnoreCase Unicode folding is runner-specific. ASCII
                # witnesses are supported; other strings cannot prove a rail.
                if isinstance(node.value, str) and not node.value.isascii():
                    return False
                literals.append(node.value)
                return True
            return False
        if isinstance(node, (ast.Name, ast.Attribute)):
            path = context_path(node)
            if path is None or "." not in path:
                return False
            parts = path.split(".")
            if parts[0] not in {
                "github",
                "inputs",
                "env",
                "vars",
                "secrets",
                "matrix",
                "steps",
                "needs",
                "strategy",
                "job",
                "runner",
            }:
                return False
            if path in {"github.event", "job.container", "job.services"}:
                return False
            if parts[0] in {"steps", "needs"} and (
                len(parts) < 3 or (len(parts) == 3 and parts[2] == "outputs")
            ):
                return False
            if path != "github.event_name":
                symbols.add(path)
            return True
        if isinstance(node, ast.BoolOp) and isinstance(node.op, (ast.And, ast.Or)):
            return all(supported(child) for child in node.values)
        if isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.Not):
            return supported(node.operand)
        if isinstance(node, ast.Compare):
            return (
                len(node.ops) == 1
                and isinstance(node.ops[0], (ast.Eq, ast.NotEq))
                and supported(node.left)
                and supported(node.comparators[0])
            )
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Name):
            if node.args or node.keywords:
                return False
            if node.func.id.lower() == "always":
                return True
            if node.func.id.lower() in {"success", "failure", "cancelled"}:
                status_used = True
                return True
        return False

    if not supported(tree):
        return False
    keys = sorted(symbols)
    # A scalar and a child of that same scalar cannot be assigned separately.
    paths = [*keys, "github.event_name"]
    if any(right.startswith(left + ".") for left in paths for right in paths if left != right):
        return False

    def number(item: object) -> float:
        if item is None:
            return 0.0
        if type(item) in (bool, int, float):
            try:
                return float(item)
            except OverflowError:
                return math.nan
        if isinstance(item, str):
            text = item.strip()
            if not text:
                return 0.0
            # Runner ExpressionUtility.ParseNumber: trimmed decimal strings,
            # Infinity, and signed 32-bit hex/octal bit patterns. JSON number
            # parsing alone differs for '+1', '01', '1.', and '0x10'.
            try:
                radix = 16 if text.startswith("0x") else 8 if text.startswith("0o") else None
                if radix is not None:
                    digits = text[2:]
                    pattern = r"[0-9a-fA-F]+" if radix == 16 else r"[0-7]+"
                    if not re.fullmatch(pattern, digits):
                        return math.nan
                    parsed = int(digits, radix)
                    if parsed > 0xFFFFFFFF:
                        return math.nan
                    return float(parsed if parsed <= 0x7FFFFFFF else parsed - 0x100000000)
                if re.fullmatch(
                    r"[+-]?(?:(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?|Infinity)",
                    text,
                    re.IGNORECASE,
                ):
                    return float(text)
            except (ValueError, OverflowError):
                pass
        return math.nan

    def equal(left: object, right: object) -> bool:
        if isinstance(left, str) and isinstance(right, str):
            return left.lower() == right.lower()
        if type(left) is type(right):
            return left == right
        return number(left) == number(right)

    if require_guaranteed:
        # An existential scalar witness is insufficient for an authority rail:
        # the configured context might never have that value. Prove a tier
        # event without assumptions about env, inputs, matrix, or step outputs.
        # Unknown conditions remain unknown; a literal event branch can still
        # establish unconditional coverage (e.g. schedule || dispatch input).
        unknown = object()

        def known_value(node: ast.AST, event: str) -> object:
            if isinstance(node, ast.Constant):
                return node.value
            if isinstance(node, (ast.Name, ast.Attribute)):
                return event if context_path(node) == "github.event_name" else unknown
            if isinstance(node, ast.Call):
                return node.func.id.lower() in {"always", "success"}
            if isinstance(node, (ast.Compare, ast.UnaryOp)):
                result = truth(node, event)
                return unknown if result is None else result
            return unknown

        def truth(node: ast.AST, event: str) -> bool | None:
            if isinstance(node, ast.Expression):
                return truth(node.body, event)
            if isinstance(node, ast.BoolOp):
                values = [truth(child, event) for child in node.values]
                if isinstance(node.op, ast.And):
                    return False if False in values else None if None in values else True
                return True if True in values else None if None in values else False
            if isinstance(node, ast.UnaryOp):
                result = truth(node.operand, event)
                return None if result is None else not result
            if isinstance(node, ast.Compare):
                left = known_value(node.left, event)
                right = known_value(node.comparators[0], event)
                if left is unknown or right is unknown:
                    return None
                same = equal(left, right)
                return same if isinstance(node.ops[0], ast.Eq) else not same
            result = known_value(node, event)
            return None if result is unknown else bool(result)

        return any(truth(tree, event) is True for event in sorted(events))

    # Include numeric witnesses for mixed-type equality and a fresh string
    # outside every literal class. Every successful assignment is concrete.
    candidates: list[object] = []
    for literal in literals:
        if not any(type(literal) is type(item) and literal == item for item in candidates):
            candidates.append(literal)
        numeric = number(literal)
        if math.isfinite(numeric) and numeric not in candidates:
            candidates.append(numeric)
    fresh = "__authority_other__"
    while any(isinstance(item, str) and equal(fresh, item) for item in literals):
        fresh += "_"
    candidates.append(fresh)
    statuses = ("success", "failure", "cancelled") if status_used else ("success",)
    if len(candidates) ** len(keys) * len(statuses) * len(events) > 4096:
        return False

    def evaluate(node: ast.AST, assignment: dict[str, object], status: str) -> object:
        if isinstance(node, ast.Expression):
            return evaluate(node.body, assignment, status)
        if isinstance(node, ast.Constant):
            return node.value
        if isinstance(node, (ast.Name, ast.Attribute)):
            return assignment[context_path(node)]
        if isinstance(node, ast.BoolOp):
            result: object = None
            for child in node.values:
                result = evaluate(child, assignment, status)
                if isinstance(node.op, ast.And) and not result:
                    break
                if isinstance(node.op, ast.Or) and result:
                    break
            return result
        if isinstance(node, ast.UnaryOp):
            return not evaluate(node.operand, assignment, status)
        if isinstance(node, ast.Compare):
            same = equal(
                evaluate(node.left, assignment, status),
                evaluate(node.comparators[0], assignment, status),
            )
            return same if isinstance(node.ops[0], ast.Eq) else not same
        if isinstance(node, ast.Call):
            return node.func.id.lower() == "always" or node.func.id.lower() == status
        raise AssertionError("unsupported node escaped validation")

    return any(
        bool(evaluate(tree, {**dict(zip(keys, values)), "github.event_name": event}, status))
        for event in sorted(events)
        for status in statuses
        for values in itertools.product(candidates, repeat=len(keys))
    )


def _validate_rails(
    *, root: Path, data: dict[str, Any], catalog: Path, violations: list[Violation]
) -> dict[str, dict[str, str]]:
    raw_rails = data.get("rails")
    if not isinstance(raw_rails, dict) or not raw_rails:
        violations.append(_violation(catalog, "[rails] must declare at least one rail"))
        return {}

    rails: dict[str, dict[str, str]] = {}
    for rail_id, raw_rail in sorted(raw_rails.items()):
        if not isinstance(rail_id, str) or not rail_id or not isinstance(raw_rail, dict):
            violations.append(
                _violation(catalog, "each [rails.<id>] entry must be a non-empty table")
            )
            continue
        tier = _string(
            raw_rail.get("tier"),
            catalog=catalog,
            context=f"rail {rail_id}.tier",
            violations=violations,
        )
        command = _string(
            raw_rail.get("command"),
            catalog=catalog,
            context=f"rail {rail_id}.command",
            violations=violations,
        )
        target_kind = _string(
            raw_rail.get("target_kind"),
            catalog=catalog,
            context=f"rail {rail_id}.target_kind",
            violations=violations,
        )
        if tier is not None and tier not in SUPPORTED_TIERS:
            violations.append(
                _violation(catalog, f"rail {rail_id}.tier must be one of {sorted(SUPPORTED_TIERS)}")
            )
        if target_kind is not None and target_kind not in SUPPORTED_TARGET_KINDS:
            violations.append(
                _violation(
                    catalog,
                    f"rail {rail_id}.target_kind must be one of {sorted(SUPPORTED_TARGET_KINDS)}",
                )
            )
        if tier is not None and command is not None and target_kind is not None:
            rails[rail_id] = {"tier": tier, "command": command, "target_kind": target_kind}
            _validate_rail_binding(
                root=root,
                catalog=catalog,
                rail_id=rail_id,
                raw_rail=raw_rail,
                command=command,
                violations=violations,
            )
    return rails


def _table_array(
    data: dict[str, Any], name: str, catalog: Path, violations: list[Violation]
) -> list[dict[str, Any]]:
    raw_entries = data.get(name, [])
    if not isinstance(raw_entries, list):
        violations.append(_violation(catalog, f"{name} must be an array of tables"))
        return []
    entries: list[dict[str, Any]] = []
    for index, entry in enumerate(raw_entries):
        if not isinstance(entry, dict):
            violations.append(_violation(catalog, f"{name}[{index}] must be a table"))
            continue
        entries.append(entry)
    return entries


def _validate_targets(
    *,
    root: Path,
    catalog: Path,
    rails: dict[str, dict[str, str]],
    entries: list[dict[str, Any]],
    collection: str,
    expected_kind: str,
    discovered: set[str],
    violations: list[Violation],
) -> dict[str, dict[str, str]]:
    targets: dict[str, dict[str, str]] = {}
    paths: set[str] = set()
    for index, entry in enumerate(entries):
        prefix = f"{collection}[{index}]"
        target_id = _string(
            entry.get("id"), catalog=catalog, context=f"{prefix}.id", violations=violations
        )
        path = _relative_path(
            entry.get("path"), catalog=catalog, field=f"{prefix}.path", violations=violations
        )
        owner = _string(
            entry.get("owner"), catalog=catalog, context=f"{prefix}.owner", violations=violations
        )
        cargo_target = entry.get("target")
        if cargo_target is not None:
            cargo_target = _string(
                cargo_target,
                catalog=catalog,
                context=f"{prefix}.target",
                violations=violations,
            )
        rail = _string(
            entry.get("rail"), catalog=catalog, context=f"{prefix}.rail", violations=violations
        )
        if target_id is None or path is None or owner is None or rail is None:
            continue
        if target_id in targets:
            violations.append(_violation(catalog, f"duplicate target id: {target_id}"))
            continue
        if path in paths:
            violations.append(_violation(catalog, f"duplicate catalog target path: {path}"))
            continue
        paths.add(path)
        if rail not in rails:
            violations.append(
                _violation(catalog, f"target {target_id} references unknown rail {rail}")
            )
        elif rails[rail]["target_kind"] != expected_kind:
            violations.append(
                _violation(
                    catalog,
                    f"target {target_id} has kind {expected_kind} but rail {rail} is {rails[rail]['target_kind']}",
                )
            )
        if path not in discovered:
            violations.append(_violation(catalog, f"catalog target does not exist on disk: {path}"))
        targets[target_id] = {
            "path": path,
            "owner": owner,
            "rail": rail,
            "kind": expected_kind,
            "target": cargo_target or Path(path).stem,
        }

    for path in sorted(discovered - paths):
        violations.append(
            _violation(root / path, f"orphan {expected_kind} test target: no catalog entry")
        )
    return targets


def _validate_grouped_integration_targets(
    *,
    root: Path,
    catalog: Path,
    targets: dict[str, dict[str, str]],
    violations: list[Violation],
) -> None:
    """Ensure source rows mapped to one Cargo test cannot silently disappear."""
    groups: dict[tuple[str, str], list[dict[str, str]]] = {}
    for target in targets.values():
        key = (target["owner"], target["target"])
        groups.setdefault(key, []).append(target)

    for (owner, cargo_target), members in sorted(groups.items()):
        source_members = [member for member in members if Path(member["path"]).stem != cargo_target]
        if not source_members:
            continue
        launchers = [member for member in members if Path(member["path"]).stem == cargo_target]
        if len(launchers) != 1:
            violations.append(
                _violation(
                    catalog,
                    f"grouped test {owner}:{cargo_target} must have exactly one cataloged launcher",
                )
            )
            continue

        launcher_path = PurePosixPath(launchers[0]["path"])
        launcher = root / launcher_path
        try:
            launcher_text = launcher.read_text(encoding="utf-8")
        except OSError as error:
            violations.append(_violation(launcher, f"cannot read grouped test launcher: {error}"))
            continue
        declared_modules: set[str] = set()
        launcher_lines = launcher_text.splitlines()
        line_index = 0
        while line_index < len(launcher_lines):
            line = launcher_lines[line_index].strip()
            if (
                not line
                or line.startswith("//")
                or line == "#![forbid(unsafe_code)]"
                or re.fullmatch(r"use\s+[A-Za-z_][\w:]*\s+as\s+[A-Za-z_]\w*;", line)
            ):
                line_index += 1
                continue
            path_match = re.fullmatch(r'#\[\s*path\s*=\s*"([A-Za-z0-9_./-]+)"\s*\]', line)
            next_line = (
                launcher_lines[line_index + 1].strip()
                if line_index + 1 < len(launcher_lines)
                else ""
            )
            if path_match and re.fullmatch(r"mod\s+[A-Za-z_]\w*;", next_line):
                declared_modules.add(path_match.group(1))
                line_index += 2
                continue
            violations.append(
                _violation(
                    launcher,
                    f"grouped test {owner}:{cargo_target} has unsupported launcher syntax on line {line_index + 1}",
                )
            )
            line_index += 1
        for member in source_members:
            member_path = PurePosixPath(member["path"])
            try:
                relative_member = member_path.relative_to(launcher_path.parent).as_posix()
            except ValueError:
                violations.append(
                    _violation(
                        catalog,
                        f"grouped test {owner}:{cargo_target} source must be beside its launcher: {member_path}",
                    )
                )
                continue
            if relative_member not in declared_modules:
                violations.append(
                    _violation(
                        launcher,
                        f"grouped test {owner}:{cargo_target} omits cataloged source {relative_member}",
                    )
                )

        manifest = launcher.parent.parent / "Cargo.toml"
        try:
            manifest_data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        except (OSError, tomllib.TOMLDecodeError) as error:
            violations.append(_violation(manifest, f"cannot parse grouped test manifest: {error}"))
            continue
        package = manifest_data.get("package")
        if not isinstance(package, dict) or package.get("autotests") is not False:
            violations.append(
                _violation(
                    manifest,
                    f"grouped test {owner}:{cargo_target} requires package.autotests = false",
                )
            )
        raw_tests = manifest_data.get("test", [])
        declared = False
        if isinstance(raw_tests, list):
            for raw_test in raw_tests:
                if not isinstance(raw_test, dict) or raw_test.get("name") != cargo_target:
                    continue
                path = raw_test.get("path")
                if (
                    isinstance(path, str)
                    and (manifest.parent / path).resolve() == launcher.resolve()
                ):
                    declared = True
                    break
        if not declared:
            violations.append(
                _violation(
                    manifest,
                    f"grouped test {owner}:{cargo_target} launcher is not an explicit [[test]] target",
                )
            )


def _validate_workflow_test_selectors(
    *, root: Path, catalog: Path, targets: dict[str, dict[str, str]], violations: list[Violation]
) -> None:
    """Reject explicit CI --test selectors that no longer name Cargo targets."""

    def flag_values(words: list[str], names: set[str]) -> list[str]:
        values: list[str] = []
        for index, word in enumerate(words):
            if word in names and index + 1 < len(words):
                values.append(words[index + 1])
            else:
                values.extend(
                    word[len(name) + 1 :] for name in names if word.startswith(name + "=")
                )
        return values

    valid_pairs = {(target["owner"], target["target"]) for target in targets.values()}
    workflows = sorted((root / ".github" / "workflows").glob("*.yml"))
    workflows += sorted((root / ".github" / "workflows").glob("*.yaml"))
    for workflow_path in workflows:
        workflow = _load_workflow(
            root=root,
            catalog=catalog,
            workflow_path=workflow_path.relative_to(root).as_posix(),
            violations=violations,
        )
        if workflow is None:
            continue
        jobs = workflow.get("jobs", {})
        if not isinstance(jobs, dict):
            continue
        for job_id, job in jobs.items():
            if not isinstance(job, dict):
                continue
            steps = job.get("steps", [])
            if not isinstance(steps, list):
                continue
            for step in steps:
                if not isinstance(step, dict) or not isinstance(step.get("run"), str):
                    continue
                run = re.sub(r"\\\r?\n[ \t]*", " ", step["run"])
                for raw_line in run.splitlines():
                    if "--test" not in raw_line or raw_line.lstrip().startswith("#"):
                        continue
                    try:
                        words = shlex.split(raw_line, comments=True)
                    except ValueError as error:
                        violations.append(
                            _violation(
                                workflow_path, f"job {job_id} has invalid test command: {error}"
                            )
                        )
                        continue
                    if "--" in words:
                        words = words[: words.index("--")]
                    packages = flag_values(words, {"-p", "--package"})
                    test_names = flag_values(words, {"--test"})
                    if not test_names:
                        if "--test" in words:
                            violations.append(
                                _violation(
                                    workflow_path, f"job {job_id} has --test without a target"
                                )
                            )
                        continue
                    if not packages:
                        violations.append(
                            _violation(
                                workflow_path,
                                f"job {job_id} explicit --test requires -p package binding",
                            )
                        )
                    for package in packages:
                        for test_name in test_names:
                            if (package, test_name) not in valid_pairs:
                                violations.append(
                                    _violation(
                                        workflow_path,
                                        f"job {job_id} selects unknown Cargo test target {package}:{test_name}",
                                    )
                                )


def _validate_fuzz_manifest_bindings(
    *,
    root: Path,
    catalog: Path,
    entries: list[dict[str, Any]],
    targets: dict[str, dict[str, str]],
    violations: list[Violation],
) -> None:
    for index, entry in enumerate(entries):
        prefix = f"fuzz_targets[{index}]"
        target_id = entry.get("id")
        if not isinstance(target_id, str) or target_id not in targets:
            continue
        manifest_path = _relative_path(
            entry.get("manifest"),
            catalog=catalog,
            field=f"{prefix}.manifest",
            violations=violations,
        )
        target_name = _string(
            entry.get("target"), catalog=catalog, context=f"{prefix}.target", violations=violations
        )
        if manifest_path is None or target_name is None:
            continue
        manifest = root / manifest_path
        if not manifest.is_file():
            violations.append(
                _violation(
                    catalog, f"fuzz target {target_id} manifest does not exist: {manifest_path}"
                )
            )
            continue
        try:
            manifest_data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        except (OSError, tomllib.TOMLDecodeError) as error:
            violations.append(_violation(manifest, f"cannot parse fuzz manifest: {error}"))
            continue
        raw_bins = manifest_data.get("bin", [])
        if not isinstance(raw_bins, list):
            violations.append(_violation(manifest, "[[bin]] entries must be an array"))
            continue
        expected_path = targets[target_id]["path"]
        matched = False
        for raw_bin in raw_bins:
            if not isinstance(raw_bin, dict):
                continue
            if raw_bin.get("name") != target_name or not isinstance(raw_bin.get("path"), str):
                continue
            resolved = (manifest.parent / raw_bin["path"]).resolve()
            if resolved == (root / expected_path).resolve():
                matched = True
                break
        if not matched:
            violations.append(
                _violation(
                    catalog,
                    f"fuzz target {target_id} is not declared as [[bin]] {target_name!r} at {expected_path}",
                )
            )


def _validate_local_scopes(
    *,
    root: Path,
    data: dict[str, Any],
    catalog: Path,
    integration_targets: dict[str, dict[str, str]],
    violations: list[Violation],
) -> None:
    """Validate local nextest selections against the CI target authority."""
    raw_scopes = data.get("local_scopes")
    if raw_scopes is None:
        return
    if not isinstance(raw_scopes, dict) or not raw_scopes:
        violations.append(_violation(catalog, "[local_scopes] must be a non-empty table"))
        return

    known_owners = {target["owner"] for target in integration_targets.values()}
    scope_names = set(raw_scopes)
    include_graph: dict[str, list[str]] = {}
    for scope_name, raw_scope in sorted(raw_scopes.items()):
        context = f"local_scopes.{scope_name}"
        if not isinstance(scope_name, str) or not scope_name or not isinstance(raw_scope, dict):
            violations.append(_violation(catalog, "each local scope must be a non-empty table"))
            continue
        lane = _string(
            raw_scope.get("lane"),
            catalog=catalog,
            context=f"{context}.lane",
            violations=violations,
        )
        if lane is not None and not lane.endswith("-lane"):
            violations.append(_violation(catalog, f"{context}.lane must end with '-lane'"))
        test_threads = raw_scope.get("test_threads")
        if not isinstance(test_threads, int) or isinstance(test_threads, bool) or test_threads < 1:
            violations.append(
                _violation(catalog, f"{context}.test_threads must be a positive integer")
            )

        raw_targets = raw_scope.get("targets")
        raw_owners = raw_scope.get("owners")
        raw_includes = raw_scope.get("includes", [])
        raw_packages = raw_scope.get("packages", [])
        include_lib = raw_scope.get("lib", False)
        if raw_targets is not None and raw_owners is not None:
            violations.append(
                _violation(catalog, f"{context} cannot declare both targets and owners")
            )
            continue
        if not isinstance(raw_includes, list):
            violations.append(_violation(catalog, f"{context}.includes must be a list"))
            raw_includes = []
        includes: list[str] = []
        for included in raw_includes:
            if not isinstance(included, str) or not included:
                violations.append(
                    _violation(catalog, f"{context}.includes must contain non-empty strings")
                )
                continue
            includes.append(included)
            if included not in scope_names:
                violations.append(
                    _violation(catalog, f"{context} includes unknown local scope {included}")
                )
        include_graph[scope_name] = includes

        if not isinstance(raw_packages, list):
            violations.append(_violation(catalog, f"{context}.packages must be a list"))
            raw_packages = []
        for package in raw_packages:
            if not isinstance(package, str) or not package:
                violations.append(
                    _violation(catalog, f"{context}.packages must contain non-empty strings")
                )
                continue
            if not (root / "crates" / package / "Cargo.toml").is_file():
                violations.append(
                    _violation(catalog, f"{context} references unknown package {package}")
                )
        if not isinstance(include_lib, bool):
            violations.append(_violation(catalog, f"{context}.lib must be boolean"))
            include_lib = False
        if include_lib and (raw_targets is not None or raw_owners is not None or includes):
            violations.append(
                _violation(
                    catalog,
                    f"{context} must keep library and integration selectors separate",
                )
            )
        if (
            raw_targets is None
            and raw_owners is None
            and not includes
            and not (include_lib and raw_packages)
        ):
            violations.append(_violation(catalog, f"{context} selects no tests"))
            continue

        if raw_targets is not None:
            if not isinstance(raw_targets, list) or not raw_targets:
                violations.append(
                    _violation(catalog, f"{context}.targets must be a non-empty list")
                )
                continue
            seen: set[str] = set()
            for target_id in raw_targets:
                if not isinstance(target_id, str) or not target_id:
                    violations.append(
                        _violation(catalog, f"{context}.targets must contain non-empty strings")
                    )
                    continue
                if target_id in seen:
                    violations.append(_violation(catalog, f"{context} repeats target {target_id}"))
                seen.add(target_id)
                if target_id not in integration_targets:
                    violations.append(
                        _violation(
                            catalog, f"{context} references unknown integration target {target_id}"
                        )
                    )
        elif raw_owners is not None:
            if not isinstance(raw_owners, list) or not raw_owners:
                violations.append(_violation(catalog, f"{context}.owners must be a non-empty list"))
                continue
            seen_owners: set[str] = set()
            for owner in raw_owners:
                if not isinstance(owner, str) or not owner:
                    violations.append(
                        _violation(catalog, f"{context}.owners must contain non-empty strings")
                    )
                    continue
                if owner in seen_owners:
                    violations.append(_violation(catalog, f"{context} repeats owner {owner}"))
                seen_owners.add(owner)
                if owner not in known_owners:
                    violations.append(
                        _violation(catalog, f"{context} references unknown owner {owner}")
                    )

    def visit(scope_name: str, trail: tuple[str, ...]) -> None:
        if scope_name in trail:
            cycle = " -> ".join((*trail, scope_name))
            violations.append(_violation(catalog, f"local scope include cycle: {cycle}"))
            return
        for included in include_graph.get(scope_name, []):
            if included in include_graph:
                visit(included, (*trail, scope_name))

    for scope_name in sorted(include_graph):
        visit(scope_name, ())


def _python_command_selects_path(root: Path, command: str, path: str) -> bool:
    if command.startswith("just "):
        recipe = command.removeprefix("just ")
        lines = (root / "Justfile").read_text(encoding="utf-8").splitlines()
        header = re.compile(rf"^{re.escape(recipe)}(?:\s+[^:]*)?:\s*(?:#.*)?$")
        body: list[str] = []
        for index, line in enumerate(lines):
            if header.fullmatch(line):
                for candidate in lines[index + 1 :]:
                    if candidate and not candidate[0].isspace():
                        break
                    body.append(candidate.strip())
                break
        commands = re.sub(r"\\\r?\n[ \t]*", " ", "\n".join(body)).splitlines()
    else:
        commands = [command]
    for line in commands:
        line = line.lstrip("@").strip()
        if not line or line.startswith("#"):
            continue
        if any(operator in line for operator in ("||", "&&", ";", "|", "$(", "`")):
            continue
        tokens = _literal_shell_argv(line)
        if tokens is None:
            continue
        if tokens[:3] != ["python3", "-m", "pytest"]:
            continue
        selectors = tokens[3:]
        # A target path beside --help/--co/--lf or attached filters is not
        # complete execution. Only presentation flags and full files bind it.
        presentation = {"-q", "-v", "-vv", "-ra", "--disable-warnings"}
        if not selectors or any(
            item not in presentation
            and re.fullmatch(r"tools/ci/tests/test_[a-z0-9_]+\.py", item) is None
            for item in selectors
        ):
            continue
        if path in selectors:
            return True
    return False


def _validate_python_targets(
    *,
    root: Path,
    catalog: Path,
    entries: list[dict[str, Any]],
    rails: dict[str, dict[str, str]],
    other_ids: set[str],
    violations: list[Violation],
) -> dict[str, str]:
    targets: dict[str, str] = {}
    paths: set[str] = set()
    for index, entry in enumerate(entries):
        prefix = f"python_targets[{index}]"
        target_id = _string(
            entry.get("id"), catalog=catalog, context=f"{prefix}.id", violations=violations
        )
        path = _relative_path(
            entry.get("path"), catalog=catalog, field=f"{prefix}.path", violations=violations
        )
        owner = _relative_path(
            entry.get("owner"), catalog=catalog, field=f"{prefix}.owner", violations=violations
        )
        rail = _string(
            entry.get("rail"), catalog=catalog, context=f"{prefix}.rail", violations=violations
        )
        if target_id is None or path is None or owner is None or rail is None:
            continue
        if target_id in targets or target_id in other_ids:
            violations.append(_violation(catalog, f"duplicate target id: {target_id}"))
        if path in paths:
            violations.append(_violation(catalog, f"duplicate catalog target path: {path}"))
        paths.add(path)
        if not path.startswith("tools/ci/tests/test_") or not path.endswith(".py"):
            violations.append(
                _violation(catalog, f"python target path is not a proof test: {path}")
            )
        if not (root / path).is_file():
            violations.append(_violation(catalog, f"python target file does not exist: {path}"))
        if not (root / owner).is_file():
            violations.append(_violation(catalog, f"python target owner does not exist: {owner}"))
        if rail not in rails:
            violations.append(
                _violation(catalog, f"python target {target_id} references unknown rail {rail}")
            )
        elif rails[rail]["target_kind"] != "python":
            violations.append(
                _violation(
                    catalog,
                    f"python target {target_id} has kind python but rail {rail} is {rails[rail]['target_kind']}",
                )
            )
        else:
            try:
                selected = _python_command_selects_path(root, rails[rail]["command"], path)
            except OSError as error:
                violations.append(
                    _violation(catalog, f"cannot inspect Python rail recipe: {error}")
                )
                selected = False
            if not selected:
                violations.append(
                    _violation(catalog, f"python rail {rail} does not execute {path}")
                )
        targets[target_id] = path
    return targets


def _validate_python_scopes(
    *,
    data: dict[str, Any],
    catalog: Path,
    targets: dict[str, str],
    violations: list[Violation],
) -> None:
    scopes = data.get("python_scopes", {})
    if not isinstance(scopes, dict):
        violations.append(_violation(catalog, "[python_scopes] must be a table"))
        return
    selected: set[str] = set()
    for scope_id, scope in scopes.items():
        if not isinstance(scope_id, str) or not scope_id or not isinstance(scope, dict):
            violations.append(_violation(catalog, "each Python scope must be a non-empty table"))
            continue
        members = scope.get("targets")
        if not isinstance(members, list) or not members:
            violations.append(_violation(catalog, f"python scope {scope_id} requires targets"))
            continue
        if len(members) != len(set(str(item) for item in members)):
            violations.append(_violation(catalog, f"python scope {scope_id} repeats a target"))
        for target_id in members:
            if not isinstance(target_id, str) or target_id not in targets:
                violations.append(
                    _violation(
                        catalog, f"python scope {scope_id} names unknown python target {target_id}"
                    )
                )
            else:
                selected.add(target_id)
    for target_id in sorted(targets.keys() - selected):
        violations.append(
            _violation(catalog, f"python target is not selected by any python scope: {target_id}")
        )


def _validate_invariants(
    *,
    root: Path,
    catalog: Path,
    data: dict[str, Any],
    entries: list[dict[str, Any]],
    rails: dict[str, dict[str, str]],
    targets: dict[str, dict[str, str]],
    violations: list[Violation],
) -> None:
    universe = _table_array(data, "invariant_universe", catalog, violations)
    universe_by_id: dict[str, dict[str, str]] = {}
    for index, entry in enumerate(universe):
        prefix = f"invariant_universe[{index}]"
        invariant_id = _string(
            entry.get("id"), catalog=catalog, context=f"{prefix}.id", violations=violations
        )
        risk = _string(
            entry.get("risk"), catalog=catalog, context=f"{prefix}.risk", violations=violations
        )
        owner = _string(
            entry.get("owner"), catalog=catalog, context=f"{prefix}.owner", violations=violations
        )
        source = _relative_path(
            entry.get("source"), catalog=catalog, field=f"{prefix}.source", violations=violations
        )
        if invariant_id is None or risk is None or owner is None or source is None:
            continue
        if invariant_id in universe_by_id:
            violations.append(
                _violation(catalog, f"duplicate invariant universe id: {invariant_id}")
            )
            continue
        if risk not in {"P0", "P1", "P2", "P3"}:
            violations.append(_violation(catalog, f"{prefix}.risk must be one of P0, P1, P2, P3"))
            continue
        if not (root / source).is_file():
            violations.append(
                _violation(catalog, f"invariant universe source does not exist: {source}")
            )
        universe_by_id[invariant_id] = {"risk": risk, "owner": owner, "source": source}

    if entries and not universe_by_id:
        violations.append(
            _violation(catalog, "invariant_universe must declare every P0/P1 invariant")
        )

    ids: set[str] = set()
    for index, entry in enumerate(entries):
        prefix = f"invariants[{index}]"
        invariant_id = _string(
            entry.get("id"), catalog=catalog, context=f"{prefix}.id", violations=violations
        )
        risk = _string(
            entry.get("risk"), catalog=catalog, context=f"{prefix}.risk", violations=violations
        )
        owner = _string(
            entry.get("owner"), catalog=catalog, context=f"{prefix}.owner", violations=violations
        )
        source = _relative_path(
            entry.get("source"), catalog=catalog, field=f"{prefix}.source", violations=violations
        )
        if invariant_id is not None:
            if invariant_id in ids:
                violations.append(_violation(catalog, f"duplicate invariant id: {invariant_id}"))
            ids.add(invariant_id)
            expected = universe_by_id.get(invariant_id)
            if expected is None:
                violations.append(
                    _violation(
                        catalog, f"invariant {invariant_id} is absent from invariant_universe"
                    )
                )
            elif (
                owner is not None
                and source is not None
                and (
                    expected["owner"] != owner
                    or expected["source"] != source
                    or expected["risk"] != risk
                )
            ):
                violations.append(
                    _violation(
                        catalog, f"invariant {invariant_id} disagrees with invariant_universe"
                    )
                )
        if risk is not None and risk not in {"P0", "P1", "P2", "P3"}:
            violations.append(_violation(catalog, f"{prefix}.risk must be one of P0, P1, P2, P3"))
        if owner is not None and source is not None and not (root / source).is_file():
            violations.append(
                _violation(
                    catalog, f"invariant {invariant_id or prefix} source does not exist: {source}"
                )
            )
        if risk not in {"P0", "P1"}:
            continue
        role_targets: list[str] = []
        for role in PROOF_ROLES:
            target_id = entry.get(role)
            if not isinstance(target_id, str) or not target_id:
                violations.append(
                    _violation(
                        catalog,
                        f"invariant {invariant_id or prefix} missing required proof role {role}",
                    )
                )
            elif target_id not in targets:
                violations.append(
                    _violation(
                        catalog,
                        f"invariant {invariant_id or prefix} {role} references unknown target id {target_id}",
                    )
                )
            else:
                role_targets.append(target_id)
                if role in {"positive_target", "negative_target"} and owner is not None:
                    if targets[target_id]["owner"] != owner:
                        violations.append(
                            _violation(
                                catalog,
                                f"invariant {invariant_id or prefix} {role} must be owned by {owner}",
                            )
                        )
        if len(role_targets) != len(set(role_targets)):
            violations.append(
                _violation(
                    catalog,
                    f"invariant {invariant_id or prefix} proof roles must name distinct targets",
                )
            )
        for tier_role, expected_tier in (
            ("pr_rail", "pr"),
            ("merge_rail", "merge"),
            ("nightly_rail", "nightly"),
        ):
            rail_id = entry.get(tier_role)
            if not isinstance(rail_id, str) or rail_id not in rails:
                violations.append(
                    _violation(
                        catalog,
                        f"invariant {invariant_id or prefix} {tier_role} references unknown rail",
                    )
                )
            elif rails[rail_id]["tier"] != expected_tier:
                violations.append(
                    _violation(
                        catalog,
                        f"invariant {invariant_id or prefix} {tier_role} must reference {expected_tier} rail",
                    )
                )
    required = {
        invariant_id
        for invariant_id, entry in universe_by_id.items()
        if entry["risk"] in {"P0", "P1"}
    }
    missing = sorted(required - ids)
    for invariant_id in missing:
        violations.append(_violation(catalog, f"P0/P1 invariant missing proof row: {invariant_id}"))


def audit_catalog(root: Path = ROOT, catalog: Path = DEFAULT_CATALOG) -> list[Violation]:
    """Return all fail-closed catalog violations without writing repository state."""
    root = root.resolve()
    catalog = catalog.resolve()
    violations: list[Violation] = []
    data = _load_catalog(catalog, violations)
    if data is None:
        return violations
    if data.get("format_version") != 2:
        violations.append(_violation(catalog, "format_version must equal 2"))

    rails = _validate_rails(root=root, data=data, catalog=catalog, violations=violations)
    integration_entries = _table_array(data, "integration_targets", catalog, violations)
    fuzz_entries = _table_array(data, "fuzz_targets", catalog, violations)
    targets = _validate_targets(
        root=root,
        catalog=catalog,
        rails=rails,
        entries=integration_entries,
        collection="integration_targets",
        expected_kind="integration",
        discovered=_discover_integration_targets(root),
        violations=violations,
    )
    _validate_grouped_integration_targets(
        root=root,
        catalog=catalog,
        targets=targets,
        violations=violations,
    )
    _validate_workflow_test_selectors(
        root=root,
        catalog=catalog,
        targets=targets,
        violations=violations,
    )
    _validate_local_scopes(
        root=root,
        data=data,
        catalog=catalog,
        integration_targets=targets,
        violations=violations,
    )
    fuzz_targets = _validate_targets(
        root=root,
        catalog=catalog,
        rails=rails,
        entries=fuzz_entries,
        collection="fuzz_targets",
        expected_kind="fuzz",
        discovered=_discover_fuzz_targets(root),
        violations=violations,
    )
    targets.update(fuzz_targets)
    python_targets = _validate_python_targets(
        root=root,
        catalog=catalog,
        entries=_table_array(data, "python_targets", catalog, violations),
        rails=rails,
        other_ids=set(targets),
        violations=violations,
    )
    _validate_python_scopes(
        data=data,
        catalog=catalog,
        targets=python_targets,
        violations=violations,
    )
    _validate_fuzz_manifest_bindings(
        root=root,
        catalog=catalog,
        entries=fuzz_entries,
        targets=fuzz_targets,
        violations=violations,
    )
    _validate_invariants(
        root=root,
        catalog=catalog,
        data=data,
        entries=_table_array(data, "invariants", catalog, violations),
        rails=rails,
        targets=targets,
        violations=violations,
    )
    return violations


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root", type=Path, default=ROOT, help="repository root (default: inferred)"
    )
    parser.add_argument("--catalog", type=Path, default=DEFAULT_CATALOG, help="catalog TOML path")
    args = parser.parse_args(argv)
    violations = audit_catalog(args.root, args.catalog)
    if violations:
        for violation in violations:
            print(f"{violation.path}: {violation.message}", file=sys.stderr)
        return 1
    print(f"test authority: OK ({args.catalog})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
