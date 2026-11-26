#!/usr/bin/env python3
"""Inventory Sourcegraph syntax and the queries exercised by maintained tests.

`--write` regenerates the reference page. `--check` rejects untracked
coverage gaps. The inventory identifies test source and its runtime scope.
It does not collect test results or establish Native product support.
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
PREDICATE_REGISTRY_RS = REPO_ROOT / "crates/quanta-index-lexical/src/predicate_registry.rs"
CORE_PREDICATE_RS = REPO_ROOT / "crates/quanta-index-core/src/domains/read_view/predicate.rs"
RUNTIME_TESTS = REPO_ROOT / "crates/quanta-index-searchd-runtime/tests"
FRONTDOOR_SCENARIOS_RS = RUNTIME_TESTS / "common/frontdoor_scenarios.rs"
LOWERING_RS = REPO_ROOT / "crates/quanta-index-search-plane/src/lowering.rs"
OUT_MD = REPO_ROOT / "docs/reference/sourcegraph-filter-parity.md"

# Each entry points at a query argument in a maintained pinned old-Q rail.
OLD_Q_QUERIES = {
    "legacy_text_route_hellgate.rs": (r"\.query(?:_text)?\(\s*TextQuerySyntax::Sourcegraph,",),
    "legacy_structural_hellgate.rs": (r"structural_query\(client,",),
    "legacy_rev_at_time_route.rs": (r"expect_query\(\s*&client,", r"\.sourcegraph\("),
    "legacy_dirty_only_route.rs": (r"require_catalog_replay\(",),
    "e2e_exact_count_window.rs": (r"text_query\(client,\s*pin,",),
    "end_to_end.rs": (r"syntax: TextQuerySyntax::Sourcegraph,\s*query_text:",),
    "sdk_frontdoor/search_tests.rs": (r"\.sourcegraph\(",),
    "sdk_frontdoor/failure_tests.rs": (r"\.sourcegraph\(",),
}
OLD_Q_QUERY_TABLES = {
    "legacy_text_route_hellgate.rs": (r"for query in \[(.*?)\n\s*\] \{", r"(?:\A|,)"),
    "legacy_structural_hellgate.rs": (r"for \(query, expected_count\) in \[(.*?)\n\s*\] \{", r"\("),
    "e2e_history_text_predicate.rs": (r"let cases:[^=]+=\s*\[(.*?)\n\s*\];", r"\("),
}
NATIVE_REPOSITORY_QUERIES = RUNTIME_TESTS / (
    "native_index_uds/read_sessions/query/repository_query_contract.rs"
)

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
    "repo:contains.file(...)",
    "repo:has.path(...)",
    "repo:contains.path(...)",
    "repo:has.content(...)",
    "repo:has.commit.after(...)",
    "repo:contains.commit.after(...)",
    "repo:has.meta(...)",
    "repo:has.topic(...)",
    "repo:has.description(...)",
    "file:has.content(...)",
    "file:has.owner(...)",
    "file:has.contributor(...)",
    "file:contains.content(...)",
]

SG_SELECT_SURFACES = [
    "select:file.owners",
]

# Exact gaps left by retired in-process cases. A new gap is an error, and an
# entry is removed when a maintained asserting query covers it.
EXECUTION_UNVERIFIED_WAIVER = {"Before", "Committer", "Content", "Message", "Since"}
RETIRED_SURFACE_WITNESSES = {"repo.contains.file", "file.contains.content"}

# Predicate namespaces and arguments have their own witness table. Quoted text
# can also contain filter-like tokens without selecting a filter.
QUERY_TOKENS = re.compile(
    r""" "(?:\\.|[^"\\])*" | '(?:\\.|[^'\\])*' """
    r""" | (?<![\w./])/(?:\\.|[^/\\\n])*/ """
    r""" | (?P<predicate>\b(?:repo|file|symbol)[.:][a-z][a-z._]*\() """
    r""" | (?P<filter>(?<![\w.])[a-z][a-z._]*)\s*: """
    r""" | (?P<open>\() | (?P<close>\)) """,
    re.X,
)

SURFACE_TOKENS: list[tuple[str, tuple[str, ...]]] = [
    ("rev.at.time", ("rev:at.time(",)),
    (
        "repo.contains.commit.after",
        ("repo:contains.commit.after(", "repo.contains.commit.after("),
    ),
    ("repo.has.commit.after", ("repo:has.commit.after(", "repo.has.commit.after(")),
    ("repo.has.meta", ("repo:has.meta(", "repo.has.meta(")),
    ("repo.has.topic", ("repo:has.topic(", "repo.has.topic(")),
    (
        "repo.has.description",
        ("repo:has.description(", "repo.has.description("),
    ),
    ("repo.contains.content", ("repo:contains.content(", "repo.contains.content(")),
    ("repo.has.content", ("repo:has.content(", "repo.has.content(")),
    ("repo.contains.path", ("repo:contains.path(", "repo.contains.path(")),
    ("repo.contains.file", ("repo:contains.file(", "repo.contains.file(")),
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
    "repo.has.meta.regex.key_only",
    "repo.has.meta.regex.key_exact_value",
    "repo.has.meta.regex.exact_key_value",
    "repo.has.meta.regex.pair",
    "repo.has.topic",
    "repo.has.description",
    "repo.contains.file",
    "repo.contains.path",
    "repo.has.file",
    "repo.has.path",
    "repo.has.content",
    "repo.contains.content",
    "file.contains",
    "file.contains.content",
    "file.has.content",
    "file.has.owner",
    "file.has.contributor",
    "file.has.contributor.regex",
    "file.has.contributor.regex.name",
    "file.has.contributor.regex.email",
    "select.file.owners",
    "symbol.has.name",
    "sg_structural.file_contains_predicate_sibling",
    "sg_structural.file_has_content_predicate_sibling",
    "sg_structural.symbol_has_name_predicate_sibling",
)

DEMOTED_STRUCTURAL_OWNER_TESTS: dict[str, tuple[str, ...]] = {
    "sg_structural.direct_phrase_lexical_sibling": (
        "sourcegraph_structural_route_rewrites_single_pattern_body_into_structural_leaf",
    ),
    "sg_structural.direct_regex_lexical_sibling": (
        "sourcegraph_structural_route_rewrites_regex_body_into_structural_leaf",
    ),
}

DEMOTED_STRUCTURAL_NOTES: dict[str, str] = {
    "sg_structural.direct_phrase_lexical_sibling": "quoted SG structural token already communicates structural context and lowers as structural body syntax, not a distinct lexical sibling surface",
    "sg_structural.direct_regex_lexical_sibling": "/.../ SG structural token lowers as structural regex body syntax on this route, not a distinct lexical sibling surface",
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
    keywords = set()
    predicate_depth = 0
    for token in QUERY_TOKENS.finditer(query):
        if token.group("predicate") or (predicate_depth and token.group("open")):
            predicate_depth += 1
        elif predicate_depth and token.group("close"):
            predicate_depth -= 1
        elif not predicate_depth and token.group("filter"):
            keywords.add(token.group("filter"))
    return keywords


def decode_literal_query(raw: str) -> str:
    return raw.replace('\\"', '"').replace("\\\\", "\\")


_RUST_STRING = r'r(?P<hashes>\#*)"(?P<raw>.*?)"(?P=hashes)|"(?P<cooked>(?:\\.|[^"\\])*)"'
_RUST_TOKENS = re.compile(_RUST_STRING + r"|(?P<comment>//[^\n]*|/\*.*?\*/)", re.S)


def strip_rust_comments(body: str) -> str:
    return _RUST_TOKENS.sub(
        lambda match: " " if match.group("comment") is not None else match.group(), body
    )


def rust_string_literals(body: str, prefix_pattern: str) -> list[str]:
    prefix = re.compile(prefix_pattern + r"\s*\Z")
    queries = []
    body = strip_rust_comments(body)
    for match in _RUST_TOKENS.finditer(body):
        if not prefix.search(body, 0, match.start()):
            continue
        if match.group("raw") is not None:
            queries.append(match.group("raw"))
        elif match.group("cooked") is not None:
            queries.append(decode_literal_query(match.group("cooked")))
    return queries


def surface_ids_in(query: str) -> set[str]:
    found: set[str] = set()
    for surface, tokens in SURFACE_TOKENS:
        if any(token in query for token in tokens):
            found.add(surface)
    found.update(repo_meta_regex_surface_ids_in(query))
    if "patterntype:structural" in query:
        if "file:contains(path:" in query or "file:contains(file:" in query:
            found.add("sg_structural.file_contains_predicate_sibling")
        if "file:has.content(path:" in query or "file:has.content(file:" in query:
            found.add("sg_structural.file_has_content_predicate_sibling")
        if "symbol:has.name(" in query:
            found.add("sg_structural.symbol_has_name_predicate_sibling")
    if re.search(r"file[:.]has\.contributor\(/", query):
        found.add("file.has.contributor.regex")
        if "@" in query:
            found.add("file.has.contributor.regex.email")
        else:
            found.add("file.has.contributor.regex.name")
    return found


def is_slash_delimited_regex_token(token: str) -> bool:
    token = token.strip()
    return len(token) >= 2 and token.startswith("/") and token.endswith("/")


def repo_meta_regex_surface_ids_in(query: str) -> set[str]:
    found: set[str] = set()
    for arg in re.findall(r"repo[:.]has\.meta\(([^)]*)\)", query):
        value = arg.strip()
        if not value:
            continue
        if ":" not in value:
            if is_slash_delimited_regex_token(value):
                found.add("repo.has.meta.regex.key_only")
            continue
        key_token, value_token = value.split(":", 1)
        key_token = key_token.strip()
        value_token = value_token.strip()
        key_is_regex = is_slash_delimited_regex_token(key_token)
        value_is_regex = is_slash_delimited_regex_token(value_token)
        if key_is_regex and not value_token:
            found.add("repo.has.meta.regex.key_only")
            continue
        if key_is_regex and value_is_regex:
            found.add("repo.has.meta.regex.pair")
        elif key_is_regex:
            found.add("repo.has.meta.regex.key_exact_value")
        elif value_is_regex:
            found.add("repo.has.meta.regex.exact_key_value")
    return found


def record_query(
    query: str,
    outcome: str,
    test_name: str,
    keyword_evidence: dict[str, Evidence],
    surface_evidence: dict[str, Evidence],
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


def scan_old_q_queries(
    keyword_evidence: dict[str, Evidence],
    surface_evidence: dict[str, Evidence],
) -> int:
    rows = 0
    for relative in dict.fromkeys((*OLD_Q_QUERIES, *OLD_Q_QUERY_TABLES)):
        body = read(RUNTIME_TESTS / relative)
        queries = [
            query
            for prefix in OLD_Q_QUERIES.get(relative, ())
            for query in rust_string_literals(body, prefix)
        ]
        if relative in OLD_Q_QUERY_TABLES:
            table_pattern, prefix = OLD_Q_QUERY_TABLES[relative]
            for table in re.findall(table_pattern, strip_rust_comments(body), re.S):
                queries.extend(rust_string_literals(table, prefix))
        if not queries:
            raise ValueError(f"{relative}: no maintained old-Q query arguments")
        rows += len(queries)
        for query in queries:
            record_query(
                query,
                "old_q_assertion",
                Path(relative).stem,
                keyword_evidence,
                surface_evidence,
            )
    return rows


def scan_native_repository_queries(
    keyword_evidence: dict[str, Evidence],
    surface_evidence: dict[str, Evidence],
) -> int:
    body = strip_rust_comments(read(NATIVE_REPOSITORY_QUERIES))
    cases = (
        (
            "repository_path_aliases_and_scalar_file_matchers_gate_the_source_repository",
            "native_assertion",
        ),
        (
            "legacy_repo_metadata_and_repo_surface_are_explicitly_unavailable_on_native_text",
            "native_refusal",
        ),
    )
    count = 0
    for name, outcome in cases:
        case = re.search(rf"fn {name}\(\).*?\n\}}", body, re.S)
        if case is None:
            raise ValueError(f"missing Native repository case {name}")
        queries = [
            query
            for table in re.findall(r"for query in \[(.*?)\n\s*\] \{", case.group(), re.S)
            for query in rust_string_literals(table, r"(?:\A|,)")
        ]
        if not queries:
            raise ValueError(f"{name}: no Native repository query arguments")
        count += len(queries)
        for query in queries:
            record_query(
                query,
                outcome,
                name,
                keyword_evidence,
                surface_evidence,
            )
    return count


def scan_frontdoor_scenarios(
    keyword_evidence: dict[str, Evidence],
    surface_evidence: dict[str, Evidence],
) -> int:
    body = strip_rust_comments(read(FRONTDOOR_SCENARIOS_RS))
    queries = []
    for rail in ("SDK", "IPC"):
        table = re.search(rf"{rail}_FRONTDOOR_SCENARIOS[^=]*=\s*&\[(.*?)\n\];", body, re.S)
        if table is None:
            raise ValueError(f"missing {rail} front-door scenario table")
        rows = rust_string_literals(table.group(1), r"query_text:")
        if not rows:
            raise ValueError(f"{rail} front-door scenario table has no queries")
        queries.extend(rows)
    for query in queries:
        record_query(
            query,
            "old_q_assertion",
            "frontdoor_scenarios",
            keyword_evidence,
            surface_evidence,
        )
    return len(queries)


def scan_lowering_owner_local_demotions(
    demoted_evidence: dict[str, Evidence],
) -> int:
    body = strip_rust_comments(read(LOWERING_RS))
    hits = 0
    for surface, test_names in DEMOTED_STRUCTURAL_OWNER_TESTS.items():
        for test_name in test_names:
            if re.search(rf"\bfn {test_name}\(", body):
                ev = demoted_evidence.setdefault(surface, Evidence())
                ev.outcomes.add("owner_local")
                ev.tests.add(test_name)
                ev.samples += 1
                hits += 1
    return hits


def _typed_names(core_body: str, type_name: str) -> tuple[set[str], dict[str, str]]:
    start = core_body.find(f"impl {type_name} {{")
    end = core_body.find("\n}\n", start)
    if start < 0 or end < 0:
        raise ValueError(f"cannot locate {type_name} implementation")
    implementation = core_body[start:end]
    all_match = re.search(
        r"pub const ALL: \[Self; (\d+)\] = \[(.*?)\n    \];", implementation, re.S
    )
    name_match = re.search(
        r"pub const fn name\(self\).*?match self \{(.*?)\n        \}", implementation, re.S
    )
    if not all_match or not name_match:
        raise ValueError(f"cannot locate {type_name} ALL/name tables")
    all_body, names_body = all_match.group(2), name_match.group(1)
    if "/*" in all_body + names_body:
        raise ValueError(f"{type_name} table contains unsupported block comments")
    variants = re.findall(r"^\s*Self::(\w+),\s*$", all_body, re.M)
    names = re.findall(r'^\s*Self::(\w+)\s*=>\s*"([^"]+)",\s*$', names_body, re.M)
    if (
        len(variants) != int(all_match.group(1))
        or len(variants) != len(set(variants))
        or len(names) != names_body.count("=>")
        or len(names) != len({variant for variant, _ in names})
        or set(variants) != {variant for variant, _ in names}
        or len(names) != len({name for _, name in names})
    ):
        raise ValueError(f"{type_name} ALL/name tables are incomplete or ambiguous")
    return set(variants), dict(names)


def _registry_variants(registry_body: str, table: str, row: str, enum: str, field: str) -> set[str]:
    match = re.search(
        rf"pub\(crate\) const {table}: &\[{row}\] = &\[(.*?)\n\];", registry_body, re.S
    )
    if not match or "/*" in match.group(1):
        raise ValueError(f"cannot read active {table} rows")
    body = match.group(1)
    rows = re.findall(rf"^\s*{row}\s*\{{\s*$", body, re.M)
    variants = re.findall(rf"^\s*{field}:\s*{enum}::(\w+),\s*$", body, re.M)
    if not variants or len(rows) != len(variants) or len(variants) != len(set(variants)):
        raise ValueError(f"{table} rows are missing or ambiguous")
    return set(variants)


def _predicate_families(core_body: str, variants: set[str]) -> dict[str, set[str]]:
    match = re.search(
        r"pub const fn family\(self\).*?match self \{(.*?)\n        \}", core_body, re.S
    )
    if not match or "/*" in match.group(1):
        raise ValueError("cannot locate predicate family table")
    body = match.group(1)
    arm = re.compile(
        r"((?:\s*(?:\|\s*)?Self::\w+)+)\s*=>\s*"
        r"(?:LexicalPredicateFamilyV1::(ContentOrRepo|Symbol)|"
        r"\{\s*LexicalPredicateFamilyV1::(ContentOrRepo|Symbol)\s*\})\s*,?"
    )
    families: dict[str, set[str]] = {"ContentOrRepo": set(), "Symbol": set()}
    seen: set[str] = set()
    end = 0
    for row in arm.finditer(body):
        if body[end : row.start()].strip():
            raise ValueError("predicate family table contains unsupported syntax")
        names = re.findall(r"Self::(\w+)", row.group(1))
        if len(names) != len(set(names)) or seen.intersection(names):
            raise ValueError("predicate family table contains duplicate variants")
        family = row.group(2) or row.group(3)
        families[family].update(names)
        seen.update(names)
        end = row.end()
    if body[end:].strip() or seen != variants:
        raise ValueError("predicate family table is incomplete or ambiguous")
    return families


def ours_predicates() -> tuple[list[str], list[str], list[str]]:
    core_body = read(CORE_PREDICATE_RS)
    registry_body = read(PREDICATE_REGISTRY_RS)
    canonical_variants, canonical_names = _typed_names(core_body, "LexicalPredicateV1")
    alias_variants, alias_names = _typed_names(core_body, "LexicalPredicateAliasV1")
    registry_variants = _registry_variants(
        registry_body, "PREDICATE_REGISTRY", "PredicateSpec", "LexicalPredicateV1", "predicate"
    )
    registered_aliases = _registry_variants(
        registry_body, "PREDICATE_ALIASES", "PredicateAliasSpec", "LexicalPredicateAliasV1", "alias"
    )
    families = _predicate_families(core_body, canonical_variants)
    if registry_variants != families["ContentOrRepo"]:
        raise ValueError("predicate registry disagrees with the content/repo family")
    if registry_variants - canonical_variants or registered_aliases != alias_variants:
        raise ValueError("predicate registry disagrees with core typed names")
    if set(canonical_names.values()) & set(alias_names.values()):
        raise ValueError("canonical predicate and alias names overlap")
    return (
        sorted(canonical_names[variant] for variant in registry_variants),
        sorted(alias_names[variant] for variant in registered_aliases),
        sorted(canonical_names[variant] for variant in families["Symbol"]),
    )


def supported_select_surfaces() -> set[str]:
    translator_body = read(TRANSLATOR_RS)
    return set(re.findall(r'"([^"]+)"\s*=>\s*LqSelect::', translator_body))


def build_report(
    predicate_inventory: tuple[list[str], list[str], list[str]],
    variants: list[str],
    refused: set[str],
    keyword_evidence: dict[str, Evidence],
    surface_evidence: dict[str, Evidence],
    demoted_evidence: dict[str, Evidence],
    old_q_rows: int,
    frontdoor_rows: int,
    native_rows: int,
    owner_local_demotion_hits: int,
) -> tuple[str, list[str], list[str]]:
    lines: list[str] = []
    untested: list[str] = []
    unverified_surfaces: list[str] = []

    lines.append("# Sourcegraph filter parity — test inventory")
    lines.append("")
    lines.append(
        "Generated by `tools/benchmark/sourcegraph_parity.py` from the bridge and "
        f"maintained test sources. It lists {old_q_rows} pinned old-Q query arguments, "
        f"{frontdoor_rows} SDK/IPC front-door rows and "
        f"{native_rows} Native repository query arguments. It also lists "
        f"{owner_local_demotion_hits} structural lowering tests. "
        "The front-door rows also use pinned old-Q. "
        "This inventory records source coverage. Native product support requires "
        "its own execution results."
    )
    lines.append("")
    lines.append("## Accepted SgFilter surface — disposition")
    lines.append("")
    lines.append("| SgFilter | keyword | translate | query witnesses (source / scope) |")
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
            evid = f"{tests} → {outs} ({ev.samples})"
        elif kw in refused:
            evid = "— (refusal asserted at bridge unit level)"
        elif variant in EXECUTION_UNVERIFIED_WAIVER:
            evid = "Retired witness; current execution unverified"
            untested.append(f"{variant} ({kw})")
        else:
            evid = "❌ ACCEPTED BUT UNVERIFIED — not waived"
            untested.append(f"{variant} ({kw})")
        lines.append(f"| `{variant}` | `{kw}:` | {disp} | {evid} |")
    lines.append("")

    lines.append("## Canonical predicate / alias surface — query witnesses")
    lines.append("")
    lines.append("| surface id | evidence (tests / outcomes) |")
    lines.append("| --- | --- |")
    for surface in REQUIRED_SURFACES:
        ev = surface_evidence.get(surface)
        if ev and ev.outcomes:
            tests = ",".join(sorted(ev.tests))
            outs = ",".join(sorted(ev.outcomes))
            evid = f"{tests} → {outs} ({ev.samples})"
        else:
            evid = "❌ ACCEPTED/SHIPPED SURFACE BUT UNVERIFIED"
            if surface in RETIRED_SURFACE_WITNESSES:
                evid = "Retired witness; current execution unverified"
            else:
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
            if surface in RETIRED_SURFACE_WITNESSES:
                evid = "Retired witness; current execution unverified"
            else:
                unverified_surfaces.append(surface)
        lines.append(f"| `{surface}` | {evid} | {note} |")
    lines.append("")

    lines.append("## Predicate coverage vs Sourcegraph")
    lines.append("")
    canonical, aliases, route_owned = predicate_inventory
    supported_surface = set(canonical) | set(aliases) | set(route_owned)
    lines.append(
        "**Canonical predicate registry (`PREDICATE_REGISTRY`):** "
        + (", ".join(f"`{p}`" for p in canonical) or "none")
    )
    lines.append("")
    lines.append(
        "**Sourcegraph-only bridge aliases:** " + (", ".join(f"`{p}`" for p in aliases) or "none")
    )
    lines.append("")
    lines.append(
        "**Separate symbol predicate routes:** "
        + (", ".join(f"`{p}`" for p in route_owned) or "none")
    )
    lines.append("")
    lines.append("**Sourcegraph (docs, external reference — not verified here):**")
    for pred in SG_PREDICATES:
        have = pred.split("(")[0].replace(":", ".") in supported_surface
        mark = "registered" if have else "unregistered"
        lines.append(f"- `{pred}` — {mark}")
    lines.append("")
    lines.append(
        f"The registry names {len(canonical)} canonical predicates and "
        f"{len(aliases)} aliases. Consult the query witnesses above for coverage."
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
            lines.append(f"- `{select_surface}` — pinned old-Q query witness")
            continue
        if implemented:
            lines.append(f"- `{select_surface}` — ⚠ unverified")
            if surface_id not in RETIRED_SURFACE_WITNESSES:
                unverified_surfaces.append(surface_id)
            continue
        lines.append(f"- `{select_surface}` — ❌ lack")
    lines.append("")

    return "\n".join(lines).rstrip("\n") + "\n", untested, unverified_surfaces


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
    try:
        old_q_rows = scan_old_q_queries(keyword_evidence, surface_evidence)
        frontdoor_rows = scan_frontdoor_scenarios(keyword_evidence, surface_evidence)
        native_rows = scan_native_repository_queries(keyword_evidence, surface_evidence)
        owner_local_demotion_hits = scan_lowering_owner_local_demotions(demoted_evidence)
    except (OSError, ValueError) as error:
        print(f"FAIL: query witness inventory: {error}", file=sys.stderr)
        return 1

    missing_map = [v for v in variants if v not in VARIANT_KEYWORD]
    try:
        predicate_inventory = ours_predicates()
    except (OSError, ValueError) as error:
        print(f"FAIL: typed predicate inventory: {error}", file=sys.stderr)
        return 1
    report, untested_filters, unverified_surfaces = build_report(
        predicate_inventory,
        variants,
        refused,
        keyword_evidence,
        surface_evidence,
        demoted_evidence,
        old_q_rows,
        frontdoor_rows,
        native_rows,
        owner_local_demotion_hits,
    )

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
        stale_waivers = sorted(
            EXECUTION_UNVERIFIED_WAIVER - {item.split()[0] for item in untested_filters}
            | {
                surface
                for surface in RETIRED_SURFACE_WITNESSES
                if surface not in REQUIRED_SURFACES or surface in surface_evidence
            }
        )
        if stale_waivers:
            print(
                f"FAIL: stale query witness exceptions: {', '.join(stale_waivers)}", file=sys.stderr
            )
            problems += len(stale_waivers)
        waived = [
            item for item in untested_filters if item.split(" ")[0] in EXECUTION_UNVERIFIED_WAIVER
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
            f"— query witnesses inventoried, {len(waived)} filter and "
            f"{len(RETIRED_SURFACE_WITNESSES)} predicate witness gaps tracked."
        )

    if args.write:
        OUT_MD.write_text(report, encoding="utf-8")
        print(f"wrote {OUT_MD.relative_to(REPO_ROOT)} ({len(variants)} filters)")
    elif not args.check:
        print(report)
    return 0


if __name__ == "__main__":
    sys.exit(main())
