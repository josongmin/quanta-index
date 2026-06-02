#!/usr/bin/env python3
"""Generate (and verify) the Sourcegraph-filter execution-coverage matrix.

The feature comparison against Sourcegraph must be grounded in *execution
evidence*, not in the accepted-syntax surface. A filter that the `lq-bridge`
parser accepts is not "supported" until a test actually runs it through the
engine and asserts the result. This tool closes that gap by deriving the matrix
straight from code:

  1. the accepted surface — `SgFilter` variants in `lq-bridge/src/syntax.rs`;
  2. the refusals — translator arms that return a typed `BridgeError`
     (`lq-bridge/src/translator.rs`) plus its disposition comment table;
  3. the execution evidence — every filter keyword actually exercised by an
     asserting test: the 57 `ParityScenario` rows in
     `e2e_dual_syntax_lowering_parity.rs` (exact-id / typed-error assertions)
     and the DSL bench `SCENARIOS` table.

`--write` regenerates `tools/benchmark/SOURCEGRAPH_PARITY.md`. `--check` fails
if any accepted `SgFilter` variant is neither execution-verified nor explicitly
refused (i.e. "accepted but silently untested") — a CI-able guard against the
comparison drifting back into parse-only claims.
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
SYNTAX_RS = REPO_ROOT / "crates/quanta-index-lq-bridge/src/syntax.rs"
TRANSLATOR_RS = REPO_ROOT / "crates/quanta-index-lq-bridge/src/translator.rs"
PARITY_RS = (
    REPO_ROOT / "crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs"
)
SCENARIOS_RS = REPO_ROOT / "crates/quanta-index-searchd-harness/src/scenarios.rs"
OUT_MD = REPO_ROOT / "tools/benchmark/SOURCEGRAPH_PARITY.md"

# The one hand-maintained contract: SgFilter enum variant -> its query keyword.
# `--check` flags any enum variant missing here, so drift is caught.
VARIANT_KEYWORD = {
    "Repo": "repo",
    "File": "file",
    "Path": "path",
    "Lang": "lang",
    "Rev": "rev",
    "Author": "author",
    "Committer": "committer",
    "Message": "message",
    "Type": "type",
    "Case": "case",
    "Select": "select",
    "Count": "count",
    "Patterntype": "patterntype",
    "Dirty": "dirty",
    "Changed": "changed",
    "Stale": "stale",
    "Snapshot": "snapshot",
    "MetaOwner": "meta.owner",
    "MetaService": "meta.service",
    "MetaLayer": "meta.layer",
    "MetaSurface": "meta.surface",
    "Affected": "affected",
    "InvalidatedBy": "invalidated_by",
    "Fork": "fork",
    "Archived": "archived",
    "Content": "content",
    "Visibility": "visibility",
    "Context": "context",
    "Index": "index",
    "Boost": "boost",
    "Timeout": "timeout",
    "Before": "before",
    "After": "after",
    "Since": "since",
    "Until": "until",
    "DiffAdded": "diff.added",
    "DiffRemoved": "diff.removed",
    "DiffTouched": "diff.touched",
}

# Sourcegraph predicate surface (Sourcegraph docs — external reference, not
# verified from this repo). Used only for the predicate-gap section.
SG_PREDICATES = [
    "repo:has.file(...)",
    "repo:has.path(...)",
    "repo:has.content(...)",
    "repo:has.commit.after(...)",
    "repo:has.description(...)",
    "repo:has.tag(...)",
    "repo:has.meta(...)",
    "file:has.content(...)",
    "file:has.owner(...)",
    "file:has.contributor(...)",
    "file:contains.content(...)",
]

OUTCOME_LABEL = {
    "Candidates": "candidates",
    "HistoryCommits": "commits",
    "HistoryDiffPaths": "diff_paths",
    "TypedError": "typed_error",
    "ExpectedFailing": "expected_failing",
}

# Canonical labels for the bench `ResultShape` enum (kept in sync with OUTCOME_LABEL).
SHAPE_LABEL = {
    "Candidates": "candidates",
    "Commits": "commits",
    "DiffPaths": "diff_paths",
    "TypedError": "typed_error",
    "Empty": "empty",
}

# Filters that parse + translate (adopted -> LqFilter) but have no test asserting
# their execution effect yet. Tracked gap, NOT a silent pass: `--check` allows
# exactly these and fails on any *new* unverified filter. Remove an entry here
# when an asserting test lands for it.
EXECUTION_UNVERIFIED_WAIVER = {
    "Rev",
    "Author",
    "Committer",
    "Message",
    "Archived",
    "Content",
    "Context",
    "Timeout",
}

# Match `keyword:` and `keyword(` tokens (filters + predicates) in a query.
TOKEN_RE = re.compile(r"([a-z][a-z._]*[a-z])\s*[:(]")


@dataclass
class Evidence:
    outcomes: set[str] = field(default_factory=set)
    tests: set[str] = field(default_factory=set)
    samples: int = 0


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def accepted_variants() -> list[str]:
    body = read(SYNTAX_RS)
    block = re.search(r"pub enum SgFilter\s*\{(.*?)\n\}", body, re.DOTALL)
    if block is None:
        raise RuntimeError("could not locate `pub enum SgFilter`")
    return re.findall(r"^\s+([A-Z][A-Za-z]+)\b", block.group(1), re.MULTILINE)


def refused_keywords() -> set[str]:
    body = read(TRANSLATOR_RS)
    refused: set[str] = set()
    # Code arms: `SgFilter::Variant(...) => Err(...)`.
    for variant in re.findall(r"SgFilter::([A-Z][A-Za-z]+)\b[^=\n]*=>\s*Err", body):
        if variant in VARIANT_KEYWORD:
            refused.add(VARIANT_KEYWORD[variant])
    # Disposition comment table rows marked `refused`.
    for line in body.splitlines():
        if line.lstrip().startswith("//!") and "| refused" in line:
            for kw in re.findall(r"`([a-z][a-z._]*):`", line):
                refused.add(kw)
    return refused


def keywords_in(query: str) -> set[str]:
    return set(TOKEN_RE.findall(query))


def scan_parity(evidence: dict[str, Evidence]) -> int:
    body = read(PARITY_RS)
    rows = 0
    for block in body.split("ParityScenario {")[1:]:
        ids = re.findall(r'(?:sg_query|lq_query):\s*r?#?"(.*?)"#?', block)
        outcome = re.search(r"ExpectedOutcome::([A-Za-z]+)", block)
        if not ids or outcome is None:
            continue
        rows += 1
        label = OUTCOME_LABEL.get(outcome.group(1), outcome.group(1))
        for query in ids:
            for kw in keywords_in(query):
                ev = evidence.setdefault(kw, Evidence())
                ev.outcomes.add(label)
                ev.tests.add("e2e_dual_parity")
                ev.samples += 1
    return rows


def scan_bench(evidence: dict[str, Evidence]) -> int:
    body = read(SCENARIOS_RS)
    rows = 0
    for block in body.split("DslBenchScenario {")[1:]:
        query = re.search(r'query_text:\s*"(.*?)",', block)
        shape = re.search(r"ResultShape::([A-Za-z]+)", block)
        if query is None:
            continue
        rows += 1
        label = SHAPE_LABEL.get(shape.group(1), shape.group(1).lower()) if shape else "candidates"
        decoded = query.group(1).replace('\\"', '"').replace("\\\\", "\\")
        for kw in keywords_in(decoded):
            ev = evidence.setdefault(kw, Evidence())
            ev.outcomes.add(label)
            ev.tests.add("dsl_bench")
            ev.samples += 1
    return rows


def ours_predicates() -> list[str]:
    body = read(TRANSLATOR_RS) + read(SYNTAX_RS)
    found = sorted(set(re.findall(r'"((?:repo|file)[.:]?has\.[a-z]+|file\.contains)"', body)))
    return found


def build_report(
    variants: list[str],
    refused: set[str],
    evidence: dict[str, Evidence],
    parity_rows: int,
    bench_rows: int,
) -> tuple[str, list[str]]:
    lines: list[str] = []
    untested: list[str] = []

    lines.append("# Sourcegraph filter parity — execution coverage")
    lines.append("")
    lines.append(
        "Generated by `tools/benchmark/sourcegraph_parity.py` from code: the "
        "`SgFilter` accepted surface, the `lq-bridge` translator refusals, and "
        "the filter keywords exercised by asserting tests "
        f"({parity_rows} `e2e_dual` parity rows + {bench_rows} DSL bench rows). "
        "Do not hand-edit; run `--write` to regenerate."
    )
    lines.append("")
    lines.append("## Accepted SgFilter surface — disposition")
    lines.append("")
    lines.append("| SgFilter | keyword | translate | execution evidence (tests / outcomes) |")
    lines.append("| --- | --- | --- | --- |")
    for variant in variants:
        kw = VARIANT_KEYWORD.get(variant, "?")
        ev = evidence.get(kw)
        if kw in refused:
            disp = "**refused** (typed BridgeError)"
        else:
            disp = "adopted"
        if ev and ev.outcomes:
            tests = ",".join(sorted(ev.tests))
            outs = ",".join(sorted(ev.outcomes))
            evid = f"✅ {tests} → {outs} ({ev.samples})"
        elif kw in refused:
            evid = "— (refusal asserted at bridge unit level)"
        elif variant in EXECUTION_UNVERIFIED_WAIVER:
            evid = "⚠️ accepted; execution untested (tracked gap)"
            untested.append(f"{variant} ({kw})")
        else:
            evid = "❌ ACCEPTED BUT UNVERIFIED — not waived"
            untested.append(f"{variant} ({kw})")
        lines.append(f"| `{variant}` | `{kw}:` | {disp} | {evid} |")
    lines.append("")

    lines.append("## Refused / unsupported (fail-closed, by design)")
    lines.append("")
    if refused:
        lines.append(", ".join(f"`{kw}:`" for kw in sorted(refused)) + " → typed `BridgeError`.")
    else:
        lines.append("(none detected)")
    lines.append("")

    lines.append("## Predicate coverage vs Sourcegraph")
    lines.append("")
    ours = ours_predicates()
    lines.append("**Ours (code-grounded):** " + (", ".join(f"`{p}`" for p in ours) or "none"))
    lines.append("")
    lines.append("**Sourcegraph (docs, external reference — not verified here):**")
    for pred in SG_PREDICATES:
        have = any(pred.split("(")[0].replace(":", ".").endswith(o.replace(":", ".")) for o in ours)
        mark = "✅ have" if have else "❌ lack"
        lines.append(f"- `{pred}` — {mark}")
    lines.append("")
    lines.append(
        "> Predicate surface is our clearest gap vs Sourcegraph: we ship "
        f"{len(ours)} predicate(s); the rest are typed-refused or unparsed."
    )
    lines.append("")

    return "\n".join(lines) + "\n", untested


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--write", action="store_true", help="regenerate the markdown report")
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit 1 if any accepted filter lacks execution evidence and is not refused",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    variants = accepted_variants()
    refused = refused_keywords()
    evidence: dict[str, Evidence] = {}
    parity_rows = scan_parity(evidence)
    bench_rows = scan_bench(evidence)

    missing_map = [v for v in variants if v not in VARIANT_KEYWORD]
    report, untested = build_report(variants, refused, evidence, parity_rows, bench_rows)

    if args.write:
        OUT_MD.write_text(report, encoding="utf-8")
        print(f"wrote {OUT_MD.relative_to(REPO_ROOT)} ({len(variants)} filters)")

    if args.check:
        problems = 0
        if missing_map:
            print(
                f"FAIL: SgFilter variants missing from VARIANT_KEYWORD map: {missing_map}",
                file=sys.stderr,
            )
            problems += len(missing_map)
        unwaived = [
            item for item in untested if item.split(" ")[0] not in EXECUTION_UNVERIFIED_WAIVER
        ]
        if unwaived:
            print(
                "FAIL: new accepted filter(s) with no execution-asserting test "
                "(add a test or waive explicitly):",
                file=sys.stderr,
            )
            for item in unwaived:
                print(f"  - {item}", file=sys.stderr)
            problems += len(unwaived)
        waived = [item for item in untested if item.split(" ")[0] in EXECUTION_UNVERIFIED_WAIVER]
        if waived:
            print(
                f"note: {len(waived)} filter(s) waived as known execution-untested gaps: "
                + ", ".join(waived)
            )
        if problems:
            print(f"\n{problems} parity-coverage gap(s).", file=sys.stderr)
            return 1
        print(
            f"OK: {len(variants)} filters — execution-verified or typed-refused or waived "
            f"({len(waived)} waived gaps tracked)."
        )
        return 0

    if not args.write:
        print(report)
    return 0


if __name__ == "__main__":
    sys.exit(main())
