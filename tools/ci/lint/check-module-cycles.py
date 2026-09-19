#!/usr/bin/env python3
"""Keep every crate's module dependency graph acyclic (QI-BB-013 보완 #2).

A module that reaches into a sibling which reaches back has no dependency
direction: neither can change, be tested or be read without the other,
and the split is a file split, not an ownership split. This lint builds,
per workspace crate, the graph of production modules — one node per
module file, one edge per path a module names another module's item by
(`crate::…`, `super::…`, `self::…`, in `use` trees and in qualified
expressions) — and fails on every strongly connected component larger
than one module.

What is not an edge:

  * the crate root (`lib.rs` / `main.rs`): it declares the modules and may
    hold the shared type definitions every module uses;
  * a module and its own submodules: a module owns its subtree;
  * anything inside `#[cfg(test)]` items, and modules declared
    `#[cfg(test)]` (test modules, test-support files);
  * comments, doc comments (intra-doc links) and string literals.

A path through a facade (`mod.rs` re-export) is resolved to the submodule
that defines the item, so a facade does not hide a cycle.

`baselines/module-cycles.txt` names the cycles still tolerated, each with
why it is not yet broken. The lint fails on any cycle not listed and on any
listed line that no longer matches a cycle exactly (grown, shrunk or gone),
so the list only ever changes by a reviewed edit (`--update-baseline`).
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parents[3]
WORKSPACE_TOML = ROOT / "Cargo.toml"
BASELINE = ROOT / "tools" / "ci" / "lint" / "baselines" / "module-cycles.txt"

IDENT = r"[A-Za-z_][A-Za-z0-9_]*"
DEFINITION_RE = re.compile(
    r"^\s*(?:pub(?:\([a-z ]+\))?\s+)?(?:(?:const|async|unsafe)\s+)*"
    r"(?:fn|struct|enum|trait|type|const|static|mod|union)\s+(" + IDENT + r")",
    re.M,
)
MACRO_RE = re.compile(r"^\s*macro_rules!\s+(" + IDENT + r")", re.M)
MOD_DECL_RE = re.compile(
    r"^[ \t]*(?:pub(?:\([a-z ]+\))?\s+)?mod\s+(" + IDENT + r")\s*;", re.M
)


def blank_non_code(text: str) -> str:
    """Replace comments and string/char literal contents with spaces.

    The result has the same length and the same newlines as `text`, so
    offsets and line numbers carry over, and brace matching and path
    search see code only.
    """
    out = list(text)
    n = len(text)
    i = 0

    def blank(start: int, end: int) -> None:
        for k in range(start, min(end, n)):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        c = text[i]
        if text.startswith("//", i):
            end = text.find("\n", i)
            end = n if end < 0 else end
            blank(i, end)
            i = end
            continue
        if text.startswith("/*", i):
            depth = 0
            j = i
            while j < n:
                if text.startswith("/*", j):
                    depth += 1
                    j += 2
                elif text.startswith("*/", j):
                    depth -= 1
                    j += 2
                    if depth == 0:
                        break
                else:
                    j += 1
            blank(i, j)
            i = j
            continue
        prev_ident = i > 0 and (text[i - 1].isalnum() or text[i - 1] == "_")
        raw = re.match(r"b?r(#*)\"", text[i : i + 260]) if not prev_ident else None
        if raw:
            close = '"' + raw.group(1)
            end = text.find(close, i + raw.end())
            end = n if end < 0 else end + len(close)
            blank(i, end)
            i = end
            continue
        if c == '"' or (c == "b" and not prev_ident and text.startswith('b"', i)):
            j = i + (2 if c == "b" else 1)
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            blank(i, j + 1)
            i = j + 1
            continue
        if c == "'":
            # a char literal ('x', '\n', '\u{1F600}', 'é') or a lifetime ('a)
            lit = re.match(r"'(?:\\(?:u\{[0-9A-Fa-f]+\}|x[0-9A-Fa-f]{2}|.)|[^\\'\n])'", text[i : i + 14])
            if lit:
                blank(i, i + lit.end())
                i += lit.end()
                continue
        i += 1
    return "".join(out)


def matching_brace(code: str, open_at: int) -> int:
    depth = 0
    for k in range(open_at, len(code)):
        if code[k] == "{":
            depth += 1
        elif code[k] == "}":
            depth -= 1
            if depth == 0:
                return k
    raise ValueError(f"unbalanced brace at offset {open_at}")


def top_level_cfg_test_spans(code: str) -> list[tuple[int, int]]:
    """Spans of every top-level item carrying `#[cfg(test)]`."""
    spans = []
    depth = 0
    k = 0
    n = len(code)
    while k < n:
        ch = code[k]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
        elif depth == 0 and code.startswith("#[cfg(test)]", k):
            start = k
            j = k
            # the item ends at its first top-level `;` or its body's brace
            while j < n and code[j] not in ";{":
                if code[j] == "[":
                    # skip the attribute's brackets (they may hold `;`-free text)
                    depth_b = 0
                    while j < n:
                        if code[j] == "[":
                            depth_b += 1
                        elif code[j] == "]":
                            depth_b -= 1
                            if depth_b == 0:
                                break
                        j += 1
                j += 1
            end = matching_brace(code, j) if j < n and code[j] == "{" else j
            spans.append((start, end + 1))
            k = end + 1
            continue
        k += 1
    return spans


def remove_spans(code: str, spans: list[tuple[int, int]]) -> str:
    out = list(code)
    for start, end in spans:
        for k in range(start, min(end, len(out))):
            if out[k] != "\n":
                out[k] = " "
    return "".join(out)


def expand_use_tree(prefix: list[str], tree: str) -> list[list[str]]:
    """Every leaf path of a `use` tree body (`a::{b, c::{d, e}}`)."""
    tree = tree.strip()
    if not tree:
        return []
    brace = tree.find("{")
    if brace < 0:
        head = tree.split(" as ")[0].strip()
        parts = [p for p in head.split("::") if p]
        return [prefix + parts]
    head = [p for p in tree[:brace].split("::") if p.strip()]
    inner = tree[brace + 1 : tree.rfind("}")]
    items, depth, current = [], 0, ""
    for ch in inner:
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
        if ch == "," and depth == 0:
            items.append(current)
            current = ""
        else:
            current += ch
    items.append(current)
    leaves = []
    for item in items:
        leaves.extend(expand_use_tree(prefix + [p.strip() for p in head], item))
    return leaves


PATH_START_RE = re.compile(r"(?<![A-Za-z0-9_:])(crate|super|self)::")


def referenced_paths(code: str) -> list[list[str]]:
    """Every `crate::`/`super::`/`self::` path in `code`, use trees expanded."""
    paths = []
    for m in PATH_START_RE.finditer(code):
        k = m.end()
        # read `ident::ident::...` and an optional `{ ... }` group
        seg = re.match(r"((?:" + IDENT + r"::)*)(" + IDENT + r"|\{)?", code[k:])
        if not seg:
            continue
        head = [p for p in seg.group(1).split("::") if p]
        tail = seg.group(2)
        if tail == "{":
            open_at = k + seg.end() - 1
            close = matching_brace(code, open_at)
            for leaf in expand_use_tree([], code[open_at : close + 1]):
                paths.append([m.group(1)] + head + leaf)
        else:
            paths.append([m.group(1)] + head + ([tail] if tail else []))
    return paths


@dataclass
class Crate:
    name: str
    modules: dict[str, str] = field(default_factory=dict)  # module path -> production code
    definitions: dict[str, set[str]] = field(default_factory=dict)
    owners: dict[str, list[str]] = field(default_factory=dict)  # name -> defining modules


def module_path_of(src: Path, file: Path) -> str | None:
    rel = file.relative_to(src).with_suffix("")
    parts = list(rel.parts)
    if parts in (["lib"], ["main"]):
        return ""
    if parts and parts[0] == "bin":
        return None
    if parts[-1] == "mod":
        parts = parts[:-1]
    return "::".join(parts)


def load_crate(crate_dir: Path) -> Crate:
    src = crate_dir / "src"
    crate = Crate(crate_dir.name)
    raw: dict[str, str] = {}
    for file in sorted(src.rglob("*.rs")):
        path = module_path_of(src, file)
        if path is None:
            continue
        raw[path] = blank_non_code(file.read_text(encoding="utf-8"))
    # modules declared under #[cfg(test)] are test code, with their subtree
    test_modules: set[str] = set()
    for path, code in raw.items():
        for start, end in top_level_cfg_test_spans(code):
            for decl in MOD_DECL_RE.finditer(code[start:end]):
                child = f"{path}::{decl.group(1)}" if path else decl.group(1)
                test_modules.add(child)

    def is_test(path: str) -> bool:
        return any(path == t or path.startswith(t + "::") for t in test_modules)

    for path, code in raw.items():
        if is_test(path):
            continue
        production = remove_spans(code, top_level_cfg_test_spans(code))
        crate.modules[path] = production
        crate.definitions[path] = set(DEFINITION_RE.findall(production)) | set(
            MACRO_RE.findall(production)
        )
    for module, names in crate.definitions.items():
        for name in names:
            crate.owners.setdefault(name, []).append(module)
    return crate


def resolve(crate: Crate, current: str, path: list[str]) -> str | None:
    """The module file a path names an item of, or None outside the crate."""
    anchor, rest = path[0], path[1:]
    if anchor == "crate":
        base: list[str] = []
    elif anchor == "self":
        base = current.split("::") if current else []
    else:  # super
        base = current.split("::")[:-1] if current else []
        while rest and rest[0] == "super":
            base = base[:-1]
            rest = rest[1:]
    segments = base + rest
    best = None
    for k in range(len(segments), -1, -1):
        candidate = "::".join(segments[:k])
        if candidate in crate.modules:
            best, remainder = candidate, segments[k:]
            break
    if best is None:
        return None
    if remainder and remainder[0] not in crate.definitions.get(best, set()):
        # a facade re-export: find the submodule that defines the name
        prefix = best + "::" if best else ""
        owners = sorted(
            m for m in crate.owners.get(remainder[0], []) if m.startswith(prefix) and m != best
        )
        if len(owners) == 1:
            return owners[0]
    return best


def related(a: str, b: str) -> bool:
    return a == b or a.startswith(b + "::") or b.startswith(a + "::") or not a or not b


def module_graph(crate: Crate) -> dict[str, set[str]]:
    edges: dict[str, set[str]] = {m: set() for m in crate.modules if m}
    for module, code in crate.modules.items():
        if not module:
            continue
        for path in referenced_paths(code):
            target = resolve(crate, module, path)
            if target is not None and not related(module, target):
                edges[module].add(target)
    return edges


def cycles(edges: dict[str, set[str]]) -> list[list[str]]:
    """Strongly connected components with more than one module (Tarjan)."""
    index: dict[str, int] = {}
    low: dict[str, int] = {}
    stack: list[str] = []
    on_stack: set[str] = set()
    found: list[list[str]] = []
    counter = [0]

    def visit(v: str) -> None:
        index[v] = low[v] = counter[0]
        counter[0] += 1
        stack.append(v)
        on_stack.add(v)
        for w in sorted(edges.get(v, ())):
            if w not in index:
                visit(w)
                low[v] = min(low[v], low[w])
            elif w in on_stack:
                low[v] = min(low[v], index[w])
        if low[v] == index[v]:
            component = []
            while True:
                w = stack.pop()
                on_stack.discard(w)
                component.append(w)
                if w == v:
                    break
            if len(component) > 1:
                found.append(sorted(component))

    sys.setrecursionlimit(max(10_000, sys.getrecursionlimit()))
    for v in sorted(edges):
        if v not in index:
            visit(v)
    return sorted(found)


def workspace_crates() -> list[Path]:
    data = tomllib.loads(WORKSPACE_TOML.read_text(encoding="utf-8"))
    members = data.get("workspace", {}).get("members", [])
    return [ROOT / m for m in members if (ROOT / m / "src").is_dir()]


@dataclass(frozen=True)
class Cycle:
    crate: str
    modules: tuple[str, ...]
    links: tuple[str, ...]

    @property
    def key(self) -> str:
        return f"{self.crate}: {', '.join(self.modules)}"


def find_cycles(crate_dirs: list[Path]) -> list[Cycle]:
    found = []
    for crate_dir in crate_dirs:
        crate = load_crate(crate_dir)
        edges = module_graph(crate)
        for component in cycles(edges):
            members = set(component)
            links = tuple(
                f"{a} -> {b}" for a in component for b in sorted(edges[a]) if b in members
            )
            found.append(Cycle(crate.name, tuple(component), links))
    return found


def read_baseline(path: Path) -> set[str]:
    if not path.exists():
        return set()
    return {
        line.strip()
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    }


def check(crate_dirs: list[Path], baseline: Path) -> list[str]:
    """Every cycle the baseline does not list, and every stale baseline line."""
    found = find_cycles(crate_dirs)
    tolerated = read_baseline(baseline)
    problems = [
        f"new cycle {cycle.key} ({'; '.join(cycle.links)})"
        for cycle in found
        if cycle.key not in tolerated
    ]
    current = {cycle.key for cycle in found}
    problems.extend(
        f"stale baseline line `{line}`: no cycle matches it exactly any more — "
        "update the baseline so it lists only live cycles"
        for line in sorted(tolerated - current)
    )
    return problems


def update_baseline(crate_dirs: list[Path], baseline: Path) -> None:
    """Rewrite the cycle lines, keeping the file's comment header."""
    header = []
    if baseline.exists():
        for line in baseline.read_text(encoding="utf-8").splitlines():
            if line.strip() and not line.lstrip().startswith("#"):
                break
            header.append(line)
    lines = sorted(cycle.key for cycle in find_cycles(crate_dirs))
    baseline.parent.mkdir(parents=True, exist_ok=True)
    baseline.write_text("\n".join(header + lines) + "\n", encoding="utf-8")


def main() -> int:
    if "--update-baseline" in sys.argv[1:]:
        update_baseline(workspace_crates(), BASELINE)
        print(f"baseline updated: {BASELINE}")
        return 0
    problems = check(workspace_crates(), BASELINE)
    if problems:
        print("module dependency cycles (QI-BB-013 보완 #2):", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        print(
            "move the shared items to the lower module (or a new one both depend on) "
            "so every edge points one way",
            file=sys.stderr,
        )
        return 1
    print("module graphs acyclic apart from the baselined cycles")
    return 0


if __name__ == "__main__":
    sys.exit(main())
