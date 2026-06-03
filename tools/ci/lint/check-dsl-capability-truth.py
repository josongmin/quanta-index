#!/usr/bin/env python3
"""check-dsl-capability-truth.py — fail-closed DSL capability drift gate (ADV-04).

Code owns the executable DSL capability; this checker fails closed when the
hand-maintained docs drift from the code-owned sources, and when a widening lane
is labelled "advanced"/"SOTA" without benchmark/shadow evidence.

Three guards, deliberately distinct:

  1. predicate subset — DOCS-vs-CODE parity. The executable predicate names in
     `PREDICATE_REGISTRY` (crates/quanta-index-lexical/src/predicate_registry.rs)
     must exactly match the names documented in the capability matrix.

  2. SG structural legality — CODE-vs-FROZEN-SNAPSHOT, like the public-api
     baselines. `structural_leaf_verdict`
     (crates/quanta-index-search-plane/src/lowering.rs) MUST be a *flat constant
     table*: one match, no `if`/`else`/nested `match`, exactly one
     `StructuralLeafVerdict` token per arm. The checker fails closed if the
     function is not that shape (a guarded or branching arm means a leaf's
     verdict is no longer a constant the snapshot can freeze — evolve the
     snapshot model + EXPECTED_STRUCTURAL_VERDICTS *with parity proof* before
     the checker can read it). When the shape is canonical, the extracted
     leaf→verdict map must equal `EXPECTED_STRUCTURAL_VERDICTS`.

  3. advanced-claim gate — a ticket whose `Status` claims `advanced`/`SOTA`
     must carry a benchmark/shadow evidence *section* (a heading), not a word.

Checker, not generator: never rewrites docs, only fails closed. Blind-safe — if
it can extract nothing it refuses to pass.

Usage:
    check-dsl-capability-truth.py     # CI / pre-commit: exit 1 on drift
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]

PREDICATE_REGISTRY_RS = (
    ROOT / "crates" / "quanta-index-lexical" / "src" / "predicate_registry.rs"
)
LOWERING_RS = ROOT / "crates" / "quanta-index-search-plane" / "src" / "lowering.rs"
CAPABILITY_MATRIX_MD = (
    ROOT / "docs" / "plans" / "may-25-lexical-enhancement" / "lexical-capability-matrix.md"
)
ADV_TICKETS_DIR = ROOT / "docs" / "plans" / "jun-2-dsl-advanced" / "tickets"

# Frozen SG structural leaf-kind verdict map. Every LqLeaf variant the route
# handles appears here with its exact verdict. Changing a cell or adding a
# variant requires bumping this snapshot *and* attaching parity proof.
EXPECTED_STRUCTURAL_VERDICTS = {
    "Keyword": "PreserveLexical",
    "RawString": "PreserveLexical",
    "Phrase": "LowerPhraseBody",
    "Regex": "LowerRegexBody",
    "StructuralBlock": "TypedFail",
    "Predicate": "PreserveLexical",
}

# A lane claims "advanced" by saying so on its Status line; the anchor keeps the
# trigger off the README/admission-bar prose that *defines* the bar (scanning
# whole bodies would false-positive on that meta-discussion). Tolerates leading
# whitespace and markdown emphasis (`**Status:** advanced`).
ADVANCED_CLAIM_RE = re.compile(
    r"^\s*\**\s*Status:?\**.*\b(advanced|sota)\b", re.MULTILINE | re.IGNORECASE
)
# Evidence must be a real section heading, not a bare word buried in prose.
BENCHMARK_SECTION_RE = re.compile(
    r"^#{1,6}\s.*\b(benchmark|shadow)\b", re.MULTILINE | re.IGNORECASE
)

# Tokenizer that recognises Rust string/char/raw-string literals so comment
# stripping never eats a `//` or `/*` that lives inside a literal, and so a
# literal `}` cannot truncate a function slice.
_RUST_TOKEN_RE = re.compile(
    r"""
      (?P<rawstr> r (?P<hashes>\#*) " (?: (?! "(?P=hashes) ) . )* " (?P=hashes) )  # r"..." / r#"..."#
    | (?P<string> " (?: \\. | [^"\\] )* " )             # "..."
    | (?P<char>   ' (?: \\. | [^'\\] ) ' )              # 'a' / '\n'  (not lifetimes)
    | (?P<line>   // [^\n]* )
    | (?P<block>  /\* .*? \*/ )
    """,
    re.S | re.X,
)


def strip_rust_comments(src: str) -> str:
    """Remove `//` and `/* */` comments while preserving string/char literals.

    Capability is owned by live code, never by a commented-out line. The
    tokenizer matches literals first so a `//`/`/*` inside a string is kept and
    a comment is replaced by a space (block comments collapse to one space,
    which is harmless for the token/line counting this checker does).
    """

    def repl(m: re.Match) -> str:
        return " " if m.lastgroup in ("line", "block") else m.group(0)

    return _RUST_TOKEN_RE.sub(repl, src)


# --- code-owned source extraction -------------------------------------------


def extract_registry_predicates(rust_src: str) -> set:
    """The executable predicate names enumerated in `PREDICATE_REGISTRY`."""
    return set(re.findall(r'PredicateSpec\s*\{\s*name:\s*"([^"]+)"', strip_rust_comments(rust_src)))


def _slice_fn(rust_src: str, signature: str) -> str:
    """Return a comment-free body of a top-level fn, signature to next `\\n}`.

    Comments are stripped *before* slicing so a `}` inside a comment cannot
    truncate the slice. Valid only for flat, single-block top-level fns (all
    this checker slices); not a general Rust parser.
    """
    clean = strip_rust_comments(rust_src)
    start = clean.find(signature)
    if start == -1:
        return ""
    end = clean.find("\n}", start)
    return clean[start:] if end == -1 else clean[start:end]


def structural_matrix_shape_violations(rust_src: str) -> list:
    """`structural_leaf_verdict` must be a flat constant table; else fail closed.

    A guarded/branching/multi-verdict arm means a leaf's verdict is no longer a
    constant the frozen snapshot can represent, so the checker refuses to read
    it rather than guess (which is how a guarded widening would otherwise slip
    through — round-2 audit finding). Catching this here makes the simple
    leaf→verdict extraction below provably unambiguous.
    """
    body = _slice_fn(rust_src, "fn structural_leaf_verdict")
    if not body:
        return ["structural_leaf_verdict not found — checker blind, fail closed"]
    violations = []
    if re.search(r"\bif\b", body) or re.search(r"\belse\b", body):
        violations.append(
            "structural_leaf_verdict contains a guard/branch (`if`/`else`): the SG "
            "legality matrix must be a flat constant table. A name/value-guarded "
            "verdict cannot be frozen by EXPECTED_STRUCTURAL_VERDICTS — evolve the "
            "snapshot model and attach parity proof."
        )
    if len(re.findall(r"\bmatch\b", body)) != 1:
        violations.append(
            "structural_leaf_verdict must contain exactly one (flat) `match`; "
            "nested/zero match is non-canonical."
        )
    arm_count = body.count("=>")
    verdict_count = len(re.findall(r"StructuralLeafVerdict::\w+", body))
    if arm_count == 0:
        violations.append("structural_leaf_verdict has no match arms — checker blind")
    elif verdict_count != arm_count:
        violations.append(
            f"structural_leaf_verdict arm/verdict mismatch (arms={arm_count}, "
            f"verdicts={verdict_count}): each arm must yield exactly one verdict "
            f"(a helper-delegated or multi-verdict arm is not freezable)."
        )
    return violations


def extract_structural_verdicts(rust_src: str) -> dict:
    """Map each `LqLeaf::<Kind>` to its `StructuralLeafVerdict::<Verdict>`.

    Precondition: the function is a flat constant table (enforced by
    `structural_matrix_shape_violations`). Under that precondition each arm has
    exactly one verdict, so walking tokens in source order and binding each
    accumulated leaf to the next verdict is unambiguous. First-write-wins
    (`setdefault`) is used as belt-and-braces so a stray rebinding can never
    overwrite an earlier one.
    """
    body = _slice_fn(rust_src, "fn structural_leaf_verdict")
    tokens = [
        (m.start(), "leaf", m.group(1)) for m in re.finditer(r"LqLeaf::(\w+)", body)
    ] + [
        (m.start(), "verdict", m.group(1))
        for m in re.finditer(r"StructuralLeafVerdict::(\w+)", body)
    ]
    tokens.sort(key=lambda t: t[0])
    verdicts: dict = {}
    pending: list = []
    for _pos, kind, val in tokens:
        if kind == "leaf":
            pending.append(val)
        else:
            for leaf in pending:
                verdicts.setdefault(leaf, val)
            pending = []
    return verdicts


# --- doc-side extraction -----------------------------------------------------


def documented_predicates(matrix_md: str) -> set:
    """Predicate names documented in the capability matrix (`Predicate name(...)`)."""
    return set(re.findall(r"Predicate\s+([A-Za-z][\w.]*)\(", matrix_md))


# --- checks ------------------------------------------------------------------


def check_predicate_parity(code_names: set, doc_names: set) -> list:
    """Code-owned predicate subset must exactly match the documented subset."""
    violations = []
    for missing in sorted(code_names - doc_names):
        violations.append(
            f"predicate `{missing}` is in PREDICATE_REGISTRY but not documented "
            f"in the capability matrix"
        )
    for extra in sorted(doc_names - code_names):
        violations.append(
            f"predicate `{extra}` is documented as executable but is not in "
            f"PREDICATE_REGISTRY (docs-only widening is forbidden)"
        )
    return violations


def check_structural_legality(verdicts: dict) -> list:
    """The full SG structural verdict map must equal the frozen snapshot."""
    violations = []
    for leaf in sorted(set(verdicts) | set(EXPECTED_STRUCTURAL_VERDICTS)):
        got = verdicts.get(leaf)
        want = EXPECTED_STRUCTURAL_VERDICTS.get(leaf)
        if got == want:
            continue
        if want is None:
            violations.append(
                f"SG structural leaf `{leaf}` has verdict `{got}` but is not in the "
                f"frozen snapshot (new/un-triaged leaf — add it with parity proof)"
            )
        elif got is None:
            violations.append(
                f"SG structural leaf `{leaf}` (frozen `{want}`) is missing from "
                f"structural_leaf_verdict — silent narrowing/extraction loss"
            )
        else:
            violations.append(
                f"SG structural leaf `{leaf}` drifted: frozen `{want}` -> code `{got}`. "
                f"A widened/narrowed verdict must travel with parity proof and an "
                f"updated EXPECTED_STRUCTURAL_VERDICTS."
            )
    return violations


def advanced_claim_violations(ticket_name: str, text: str) -> list:
    """A lane whose Status claims `advanced`/`SOTA` must carry a benchmark/shadow section."""
    if not ADVANCED_CLAIM_RE.search(text):
        return []
    if BENCHMARK_SECTION_RE.search(text):
        return []
    return [
        f"{ticket_name}: Status claims `advanced`/`SOTA` but the ticket has no "
        f"benchmark/shadow evidence *section* (a `## Benchmark`/`## Shadow` "
        f"heading; see README §9.2 + §10 template)"
    ]


def main() -> int:
    violations = []

    registry_src = PREDICATE_REGISTRY_RS.read_text(encoding="utf-8")
    lowering_src = LOWERING_RS.read_text(encoding="utf-8")
    matrix_md = CAPABILITY_MATRIX_MD.read_text(encoding="utf-8")

    code_predicates = extract_registry_predicates(registry_src)
    if not code_predicates:
        violations.append(
            "could not extract any predicate from PREDICATE_REGISTRY — the "
            "checker is blind, refusing to pass (fail-closed)"
        )
    doc_predicates = documented_predicates(matrix_md)
    violations += check_predicate_parity(code_predicates, doc_predicates)

    # Shape FIRST: only read the verdict map if the matrix is a flat table.
    verdicts: dict = {}
    shape_violations = structural_matrix_shape_violations(lowering_src)
    violations += shape_violations
    if not shape_violations:
        verdicts = extract_structural_verdicts(lowering_src)
        if not verdicts:
            violations.append(
                "could not extract any leaf verdict from structural_leaf_verdict — "
                "the checker is blind, refusing to pass"
            )
        violations += check_structural_legality(verdicts)

    for ticket in sorted(ADV_TICKETS_DIR.glob("ADV-*.md")):
        violations += advanced_claim_violations(
            ticket.name, ticket.read_text(encoding="utf-8")
        )

    if violations:
        sys.stderr.write("\nDSL capability truth drift detected:\n")
        for v in violations:
            sys.stderr.write(f"  - {v}\n")
        sys.stderr.write(
            "\nCode owns capability; update the docs / frozen snapshot to match "
            "the code-owned sources (or revert the code change). See ADV-04.\n"
        )
        return 1

    print(
        "dsl-capability-truth: in sync "
        f"(predicates={sorted(code_predicates)}, "
        f"sg_verdicts={dict(sorted(verdicts.items()))})."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
