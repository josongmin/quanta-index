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
     asserting test: the `ParityScenario` rows in
     `e2e_dual_syntax_lowering_parity.rs` (exact-id / typed-error assertions),
     the DSL bench `SCENARIOS` table, and the pos/neg filter-execution tests in
     `e2e_filter_execution.rs`.

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
PREDICATE_REGISTRY_RS = (
    REPO_ROOT / "crates/quanta-index-lexical/src/predicate_registry.rs"
)
PARITY_RS = (
    REPO_ROOT / "crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs"
)
SCENARIOS_RS = REPO_ROOT / "crates/quanta-index-searchd-harness/src/scenarios.rs"
FILTER_EXEC_RS = REPO_ROOT / "crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs"
FRONTDOOR_SCENARIOS_RS = (
    REPO_ROOT / "crates/quanta-index-searchd-runtime/tests/common/frontdoor_scenarios.rs"
)
LOWERING_RS = REPO_ROOT / "crates/quanta-index-search-plane/src/lowering.rs"
RUNTIME_ROWS_TOML = (
    REPO_ROOT
    / "crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml"
)
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
    "repo:contains.commit.after(...)",
    "repo:has.meta(...)",
    "repo:has.topic(...)",
    "file:has.content(...)",
    "file:has.owner(...)",
    "file:has.contributor(...)",
    "file:contains.content(...)",
]

SG_SELECT_SURFACES = [
    "select:file.owners",
]

EXPLICIT_UNSUPPORTED_COMPARISON_GAPS: tuple[tuple[str, str, str], ...] = ()

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
#
# Now EMPTY: the previously-waived eight (rev/author/committer/message/archived/
# content/context/timeout) gained pos+neg execution tests in
# `e2e_filter_execution.rs`. Keep the mechanism so a future accepted filter
# without a test fails `--check`.
EXECUTION_UNVERIFIED_WAIVER: set[str] = set()

# Match `keyword:` and `keyword(` tokens (filters + predicates) in a query.
TOKEN_RE = re.compile(r"([a-z][a-z._]*[a-z])\s*[:(]")

SURFACE_TOKENS: list[tuple[str, tuple[str, ...]]] = [
    ("rev.at.time", ("rev:at.time(",)),
    (
        "repo.contains.commit.after",
        ("repo:contains.commit.after(", "repo.contains.commit.after("),
    ),
    ("repo.has.commit.after", ("repo:has.commit.after(", "repo.has.commit.after(")),
    ("repo.has.meta", ("repo:has.meta(", "repo.has.meta(")),
    ("repo.has.topic", ("repo:has.topic(", "repo.has.topic(")),
    ("repo.contains.content", ("repo:contains.content(", "repo.contains.content(")),
    ("repo.has.content", ("repo:has.content(", "repo.has.content(")),
    ("repo.has.path", ("repo:has.path(", "repo.has.path(")),
    ("repo.has.file", ("repo:has.file(", "repo.has.file(")),
    ("file.contains.content", ("file:contains.content(", "file.contains.content(")),
    ("file.has.content", ("file:has.content(", "file.has.content(")),
    ("file.contains", ("file:contains(", "file.contains(")),
    ("file.has.owner", ("file:has.owner(", "file.has.owner(")),
    ("file.has.contributor", ("file:has.contributor(", "file.has.contributor(")),
    ("select.file.owners", ("select:file.owners",)),
    ("symbol.has.name", ("symbol:has.name(", "symbol.has.name(")),
]

REQUIRED_SURFACES: tuple[str, ...] = (
    "rev.at.time",
    "repo.has.commit.after",
    "repo.contains.commit.after",
    "repo.has.meta",
    "repo.has.topic",
    "repo.has.file",
    "repo.has.path",
    "repo.has.content",
    "repo.contains.content",
    "file.contains",
    "file.contains.content",
    "file.has.content",
    "file.has.owner",
    "file.has.contributor",
    "select.file.owners",
    "symbol.has.name",
)

UNSUPPORTED_SURFACE_TOKENS: list[tuple[str, tuple[str, ...]]] = []

DEMOTED_STRUCTURAL_OWNER_TESTS: dict[str, tuple[str, ...]] = {
    "sg_structural.direct_phrase_lexical_sibling": (
        "sourcegraph_structural_route_rewrites_single_pattern_body_into_structural_leaf",
    ),
    "sg_structural.direct_regex_lexical_sibling": (
        "sourcegraph_structural_route_rewrites_regex_body_into_structural_leaf",
    ),
    "sg_structural.file_contains_predicate_sibling": (
        "sourcegraph_structural_route_rejects_file_contains_predicate_sibling",
        "sourcegraph_structural_route_rejects_file_contains_predicate_sibling_under_or",
        "sourcegraph_structural_route_rejects_file_contains_predicate_sibling_under_and_not",
    ),
    "sg_structural.file_has_content_predicate_sibling": (
        "sourcegraph_structural_route_rejects_file_has_content_predicate_sibling",
        "sourcegraph_structural_route_rejects_file_has_content_predicate_sibling_under_or",
        "sourcegraph_structural_route_rejects_file_has_content_predicate_sibling_under_and_not",
    ),
    "sg_structural.symbol_has_name_predicate_sibling": (
        "sourcegraph_structural_route_rejects_non_repo_predicate_sibling",
        "sourcegraph_structural_route_rejects_non_repo_predicate_sibling_under_or",
        "sourcegraph_structural_route_rejects_non_repo_predicate_sibling_under_and_not",
    ),
}

DEMOTED_STRUCTURAL_NOTES: dict[str, str] = {
    "sg_structural.direct_phrase_lexical_sibling": "quoted SG token is structural body syntax, not a distinct lexical sibling surface",
    "sg_structural.direct_regex_lexical_sibling": "/.../ SG token is structural regex body syntax, not a distinct lexical sibling surface",
    "sg_structural.file_contains_predicate_sibling": "mixed SG structural boolean cells reject non-repo `file.contains(...)` predicate siblings",
    "sg_structural.file_has_content_predicate_sibling": "mixed SG structural boolean cells reject non-repo `file.has.content(...)` predicate siblings",
    "sg_structural.symbol_has_name_predicate_sibling": "mixed SG structural boolean cells reject non-repo `symbol.has.name(...)` predicate siblings",
}


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


def decode_literal_query(raw: str) -> str:
    return raw.replace('\\"', '"').replace("\\\\", "\\")


def surface_ids_in(query: str) -> set[str]:
    found: set[str] = set()
    for surface, tokens in SURFACE_TOKENS:
        if any(token in query for token in tokens):
            found.add(surface)
    return found


def demoted_structural_surface_ids_in(query: str) -> set[str]:
    found: set[str] = set()
    if "patterntype:structural" not in query:
        return found
    if "file:contains(path:" in query or "file:contains(file:" in query:
        found.add("sg_structural.file_contains_predicate_sibling")
    if "file:has.content(path:" in query or "file:has.content(file:" in query:
        found.add("sg_structural.file_has_content_predicate_sibling")
    if "symbol:has.name(" in query:
        found.add("sg_structural.symbol_has_name_predicate_sibling")
    return found


def unsupported_surface_ids_in(query: str) -> set[str]:
    found: set[str] = set()
    for surface, tokens in UNSUPPORTED_SURFACE_TOKENS:
        if any(token in query for token in tokens):
            found.add(surface)
    return found


def record_query(
    query: str,
    outcome: str,
    test_name: str,
    keyword_evidence: dict[str, Evidence],
    surface_evidence: dict[str, Evidence],
    demoted_evidence: dict[str, Evidence],
    unsupported_evidence: dict[str, Evidence],
) -> None:
    for kw in keywords_in(query):
        ev = keyword_evidence.setdefault(kw, Evidence())
        ev.outcomes.add(outcome)
        ev.tests.add(test_name)
        ev.samples += 1
    for surface in surface_ids_in(query):
        ev = surface_evidence.setdefault(surface, Evidence())
        ev.outcomes.add(outcome)
        ev.tests.add(test_name)
        ev.samples += 1
    for surface in demoted_structural_surface_ids_in(query):
        ev = demoted_evidence.setdefault(surface, Evidence())
        ev.outcomes.add(outcome)
        ev.tests.add(test_name)
        ev.samples += 1
    for surface in unsupported_surface_ids_in(query):
        ev = unsupported_evidence.setdefault(surface, Evidence())
        ev.outcomes.add(outcome)
        ev.tests.add(test_name)
        ev.samples += 1


def scan_parity(
    keyword_evidence: dict[str, Evidence],
    surface_evidence: dict[str, Evidence],
    demoted_evidence: dict[str, Evidence],
    unsupported_evidence: dict[str, Evidence],
) -> int:
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
            record_query(
                query,
                label,
                "e2e_dual_parity",
                keyword_evidence,
                surface_evidence,
                demoted_evidence,
                unsupported_evidence,
            )
    return rows


def scan_bench(
    keyword_evidence: dict[str, Evidence],
    surface_evidence: dict[str, Evidence],
    demoted_evidence: dict[str, Evidence],
    unsupported_evidence: dict[str, Evidence],
) -> int:
    body = read(SCENARIOS_RS)
    rows = 0
    for block in body.split("DslBenchScenario {")[1:]:
        # `query: QuerySpec::Literal("...")` holds the static query string;
        # `QuerySpec::Generated(...)` adversarial rows are runtime-built (no
        # literal to scan) and carry no filter keyword, so they're skipped.
        query = re.search(r'QuerySpec::Literal\(\s*"(.*?)"\s*\)', block)
        shape = re.search(r"ResultShape::([A-Za-z]+)", block)
        if query is None:
            continue
        rows += 1
        label = SHAPE_LABEL.get(shape.group(1), shape.group(1).lower()) if shape else "candidates"
        decoded = decode_literal_query(query.group(1))
        record_query(
            decoded,
            label,
            "dsl_bench",
            keyword_evidence,
            surface_evidence,
            demoted_evidence,
            unsupported_evidence,
        )
    return rows


def scan_filter_exec(
    keyword_evidence: dict[str, Evidence],
    surface_evidence: dict[str, Evidence],
    demoted_evidence: dict[str, Evidence],
    unsupported_evidence: dict[str, Evidence],
) -> int:
    if not FILTER_EXEC_RS.exists():
        return 0
    body = read(FILTER_EXEC_RS)
    # Lexical queries pass the literal directly to `query_text(...)`; history
    # queries route through a helper but always carry the `type:commit` /
    # `type:diff` discriminator, so match those literals wherever they appear.
    queries = re.findall(r'query_text\(\s*TextQuerySyntax::\w+,\s*"(.*?)"', body)
    queries += re.findall(
        r'query_text_with_pin\(\s*TextQuerySyntax::\w+,\s*"(.*?)"', body
    )
    queries += re.findall(r'"(type:(?:commit|diff)[^"]*)"', body)
    for query in queries:
        decoded = decode_literal_query(query)
        record_query(
            decoded,
            "executed",
            "e2e_filter_exec",
            keyword_evidence,
            surface_evidence,
            demoted_evidence,
            unsupported_evidence,
        )
    return len(queries)


def scan_frontdoor_scenarios(
    keyword_evidence: dict[str, Evidence],
    surface_evidence: dict[str, Evidence],
    demoted_evidence: dict[str, Evidence],
    unsupported_evidence: dict[str, Evidence],
) -> int:
    if not FRONTDOOR_SCENARIOS_RS.exists():
        return 0
    body = read(FRONTDOOR_SCENARIOS_RS)
    queries = re.findall(r'query_text:\s*"(.*?)"', body)
    for query in queries:
        decoded = decode_literal_query(query)
        record_query(
            decoded,
            "frontdoor",
            "frontdoor_scenarios",
            keyword_evidence,
            surface_evidence,
            demoted_evidence,
            unsupported_evidence,
        )
    return len(queries)


def scan_runtime_rows(
    keyword_evidence: dict[str, Evidence],
    surface_evidence: dict[str, Evidence],
    demoted_evidence: dict[str, Evidence],
    unsupported_evidence: dict[str, Evidence],
) -> int:
    if not RUNTIME_ROWS_TOML.exists():
        return 0
    body = read(RUNTIME_ROWS_TOML)
    queries = re.findall(r'^query = "(.*)"$', body, re.MULTILINE)
    for query in queries:
        decoded = decode_literal_query(query)
        record_query(
            decoded,
            "runtime_row",
            "runtime_rows",
            keyword_evidence,
            surface_evidence,
            demoted_evidence,
            unsupported_evidence,
        )
    return len(queries)


def scan_lowering_owner_local_demotions(
    demoted_evidence: dict[str, Evidence],
) -> int:
    body = read(LOWERING_RS)
    hits = 0
    for surface, test_names in DEMOTED_STRUCTURAL_OWNER_TESTS.items():
        for test_name in test_names:
            if test_name in body:
                ev = demoted_evidence.setdefault(surface, Evidence())
                ev.outcomes.add("owner_local")
                ev.tests.add(test_name)
                ev.samples += 1
                hits += 1
    return hits


def ours_predicates() -> tuple[list[str], list[str], list[str]]:
    registry_body = read(PREDICATE_REGISTRY_RS)
    translator_body = read(TRANSLATOR_RS) + read(SYNTAX_RS)
    canonical = set(
        re.findall(r'PredicateSpec\s*\{\s*name:\s*"([^"]+)"', registry_body)
    )
    aliases = set(re.findall(r'alias:\s*"([^"]+)"', registry_body))
    aliases.update(
        re.findall(
            r'"(repo\.has\.path|file\.contains\.content|repo\.contains\.content)"',
            translator_body,
        )
    )
    route_owned = ["symbol.has.name"]
    return sorted(canonical), sorted(aliases), route_owned


def supported_select_surfaces() -> set[str]:
    translator_body = read(TRANSLATOR_RS)
    return set(re.findall(r'"([^"]+)"\s*=>\s*LqSelect::', translator_body))


def build_report(
    variants: list[str],
    refused: set[str],
    keyword_evidence: dict[str, Evidence],
    surface_evidence: dict[str, Evidence],
    demoted_evidence: dict[str, Evidence],
    unsupported_evidence: dict[str, Evidence],
    parity_rows: int,
    bench_rows: int,
    exec_rows: int,
    frontdoor_rows: int,
    runtime_rows: int,
    owner_local_demotion_hits: int,
) -> tuple[str, list[str], list[str]]:
    lines: list[str] = []
    untested: list[str] = []
    unverified_surfaces: list[str] = []

    lines.append("# Sourcegraph filter parity — execution coverage")
    lines.append("")
    lines.append(
        "Generated by `tools/benchmark/sourcegraph_parity.py` from code: the "
        "`SgFilter` accepted surface, the `lq-bridge` translator refusals, and "
        "the filter keywords exercised by asserting tests "
        f"({parity_rows} `e2e_dual` parity rows + {bench_rows} DSL bench rows + "
        f"{exec_rows} `e2e_filter_execution` queries + {frontdoor_rows} shared "
        f"front-door queries + {runtime_rows} runtime rows + "
        f"{owner_local_demotion_hits} owner-local structural demotion witnesses). "
        "Do not hand-edit; run `--write` to regenerate."
    )
    lines.append("")
    lines.append("## Accepted SgFilter surface — disposition")
    lines.append("")
    lines.append("| SgFilter | keyword | translate | execution evidence (tests / outcomes) |")
    lines.append("| --- | --- | --- | --- |")
    for variant in variants:
        kw = VARIANT_KEYWORD.get(variant, "?")
        ev = keyword_evidence.get(kw)
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

    lines.append("## Canonical predicate / alias surface — execution evidence")
    lines.append("")
    lines.append("| surface id | evidence (tests / outcomes) |")
    lines.append("| --- | --- |")
    for surface in REQUIRED_SURFACES:
        ev = surface_evidence.get(surface)
        if ev and ev.outcomes:
            tests = ",".join(sorted(ev.tests))
            outs = ",".join(sorted(ev.outcomes))
            evid = f"✅ {tests} → {outs} ({ev.samples})"
        else:
            evid = "❌ ACCEPTED/SHIPPED SURFACE BUT UNVERIFIED"
            unverified_surfaces.append(surface)
        lines.append(f"| `{surface}` | {evid} |")
    lines.append("")

    lines.append("## Refused / unsupported (fail-closed, by design)")
    lines.append("")
    if refused:
        lines.append(", ".join(f"`{kw}:`" for kw in sorted(refused)) + " → typed `BridgeError`.")
    else:
        lines.append("(none detected)")
    lines.append("")

    lines.append("## Explicit unsupported structural surfaces")
    lines.append("")
    lines.append("| surface id | evidence | note |")
    lines.append("| --- | --- | --- |")
    for surface, note in DEMOTED_STRUCTURAL_NOTES.items():
        ev = demoted_evidence.get(surface)
        if ev and ev.outcomes:
            tests = ",".join(sorted(ev.tests))
            outs = ",".join(sorted(ev.outcomes))
            evid = f"✅ {tests} → {outs} ({ev.samples})"
        else:
            evid = "❌ DEMOTION SURFACE LACKS OWNER/RUNTIME EVIDENCE"
            unverified_surfaces.append(surface)
        lines.append(f"| `{surface}` | {evid} | {note} |")
    lines.append("")

    lines.append("## Predicate coverage vs Sourcegraph")
    lines.append("")
    canonical, aliases, route_owned = ours_predicates()
    supported_surface = set(canonical) | set(aliases) | set(route_owned)
    lines.append(
        "**Canonical executable predicates (`PREDICATE_REGISTRY` SSOT):** "
        + (", ".join(f"`{p}`" for p in canonical) or "none")
    )
    lines.append("")
    lines.append(
        "**Sourcegraph-only bridge aliases:** "
        + (", ".join(f"`{p}`" for p in aliases) or "none")
    )
    lines.append("")
    lines.append(
        "**Shipped non-registry predicate routes:** "
        + (", ".join(f"`{p}`" for p in route_owned) or "none")
    )
    lines.append("")
    lines.append("**Sourcegraph (docs, external reference — not verified here):**")
    for pred in SG_PREDICATES:
        have = pred.split("(")[0].replace(":", ".") in supported_surface
        mark = "✅ have" if have else "❌ lack"
        lines.append(f"- `{pred}` — {mark}")
    lines.append("")
    lines.append(
        "> Predicate surface is our clearest gap vs Sourcegraph: the canonical "
        f"engine inventory is {len(canonical)} predicate(s) and the bridge adds "
        f"{len(aliases)} Sourcegraph-only alias(es); the rest are "
        "typed-refused or unparsed."
    )
    lines.append("")

    lines.append("## Sourcegraph select surface")
    lines.append("")
    for select_surface in SG_SELECT_SURFACES:
        surface_id = select_surface.replace(":", ".", 1)
        translator_surface = select_surface.split(":", 1)[1]
        implemented = translator_surface in supported_select_surfaces()
        ev = surface_evidence.get(surface_id)
        if implemented and ev and ev.outcomes:
            lines.append(f"- `{select_surface}` — ✅ have")
            continue
        if implemented:
            lines.append(f"- `{select_surface}` — ⚠ unverified")
            unverified_surfaces.append(surface_id)
            continue
        lines.append(f"- `{select_surface}` — ❌ lack")
    lines.append("")

    if EXPLICIT_UNSUPPORTED_COMPARISON_GAPS:
        lines.append("## Explicit unsupported authority-backed comparison gaps")
        lines.append("")
        lines.append("| surface id | Sourcegraph surface | evidence | reason |")
        lines.append("| --- | --- | --- | --- |")
        for surface_id, display, reason in EXPLICIT_UNSUPPORTED_COMPARISON_GAPS:
            ev = unsupported_evidence.get(surface_id)
            if ev and ev.outcomes:
                tests = ",".join(sorted(ev.tests))
                outs = ",".join(sorted(ev.outcomes))
                evid = f"✅ {tests} → {outs} ({ev.samples})"
            else:
                evid = "❌ EXPLICIT UNSUPPORTED GAP LACKS TYPED-FAIL EVIDENCE"
                unverified_surfaces.append(surface_id)
            lines.append(f"| `{surface_id}` | `{display}` | {evid} | {reason} |")
        lines.append("")

    return "\n".join(lines) + "\n", untested, unverified_surfaces


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
    keyword_evidence: dict[str, Evidence] = {}
    surface_evidence: dict[str, Evidence] = {}
    demoted_evidence: dict[str, Evidence] = {}
    unsupported_evidence: dict[str, Evidence] = {}
    parity_rows = scan_parity(
        keyword_evidence, surface_evidence, demoted_evidence, unsupported_evidence
    )
    bench_rows = scan_bench(
        keyword_evidence, surface_evidence, demoted_evidence, unsupported_evidence
    )
    exec_rows = scan_filter_exec(
        keyword_evidence, surface_evidence, demoted_evidence, unsupported_evidence
    )
    frontdoor_rows = scan_frontdoor_scenarios(
        keyword_evidence, surface_evidence, demoted_evidence, unsupported_evidence
    )
    runtime_rows = scan_runtime_rows(
        keyword_evidence, surface_evidence, demoted_evidence, unsupported_evidence
    )
    owner_local_demotion_hits = scan_lowering_owner_local_demotions(demoted_evidence)

    missing_map = [v for v in variants if v not in VARIANT_KEYWORD]
    report, untested_filters, unverified_surfaces = build_report(
        variants,
        refused,
        keyword_evidence,
        surface_evidence,
        demoted_evidence,
        unsupported_evidence,
        parity_rows,
        bench_rows,
        exec_rows,
        frontdoor_rows,
        runtime_rows,
        owner_local_demotion_hits,
    )

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
            item
            for item in untested_filters
            if item.split(" ")[0] not in EXECUTION_UNVERIFIED_WAIVER
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
        if unverified_surfaces:
            print(
                "FAIL: accepted/shipped or explicit-unsupported surface(s) lack canonical evidence:",
                file=sys.stderr,
            )
            for surface in unverified_surfaces:
                print(f"  - {surface}", file=sys.stderr)
            problems += len(unverified_surfaces)
        canonical, aliases, route_owned = ours_predicates()
        supported_surface = set(canonical) | set(aliases) | set(route_owned)
        implemented_unsupported = sorted(
            surface_id
            for surface_id, _display, _reason in EXPLICIT_UNSUPPORTED_COMPARISON_GAPS
            if surface_id in supported_surface
        )
        if implemented_unsupported:
            print(
                "FAIL: explicit unsupported comparison gap now appears implemented; "
                "promote the verdict and add canonical execution proof:",
                file=sys.stderr,
            )
            for surface in implemented_unsupported:
                print(f"  - {surface}", file=sys.stderr)
            problems += len(implemented_unsupported)
        waived = [
            item
            for item in untested_filters
            if item.split(" ")[0] in EXECUTION_UNVERIFIED_WAIVER
        ]
        if waived:
            print(
                f"note: {len(waived)} filter(s) waived as known execution-untested gaps: "
                + ", ".join(waived)
            )
        if problems:
            print(f"\n{problems} parity-coverage gap(s).", file=sys.stderr)
            return 1
        print(
            f"OK: {len(variants)} filters and {len(REQUIRED_SURFACES)} required surfaces "
            f"— execution-verified or typed-refused or waived ({len(waived)} waived filter gap(s) tracked)."
        )
        return 0

    if not args.write:
        print(report)
    return 0


if __name__ == "__main__":
    sys.exit(main())
