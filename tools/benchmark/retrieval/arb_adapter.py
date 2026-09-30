"""ARB natural-language input adapter (``arb-nl-adapter-v1``).

Deterministic, preregistered, gold-blind per-task transformation of one Agent
Retrieval Bench (ARB) sample's structured ``query`` into a short plain-text
query that the bench runner's ``natural_language`` input policy always accepts
(``tools.benchmark.retrieval.query_plan`` / ``benchmarks/retrieval/src/query_plan.rs``,
``NlPlanConfig::default()``: at most 32 distinct tokens, each at most 96 chars).

The frozen rule is ``RULE_TEXT`` below; ``RULE_SHA256`` binds it. Any change to
the rule, the field orders, or the constants is a new adapter version.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import re
import sys
from pathlib import Path
from typing import Any

import regex

from tools.benchmark.retrieval.query_plan import (
    DEFAULT_NL_CONFIG,
    MAX_TOKEN_BYTES,
    QueryPlanError,
    _is_token_char,
    plan_lexical_request,
    tokenize_nl,
)

ADAPTER_VERSION = "arb-nl-adapter-v1"

RULE_TEXT = """\
arb-nl-adapter-v1 rule (frozen).

Input projection (gold-blindness is structural):
  The adapter copies ONLY these sample keys into a projected dict and every
  later step sees nothing else: id, task_type, repo, base_commit, query.
  It never reads sample["gold"] (root_cause_files, related_tests,
  supporting_files, files, given_files, fix_commit, ...), gold_blocks,
  gold_spans, hard_negative_files, metadata, audit, candidate_corpus or
  query_provenance. For edit2ripple the anchor file is query["anchor_file"],
  which is part of the official query text, so gold.given_files is not read.

Original query identity:
  original_query_sha256 = sha256(utf-8(json.dumps(query, ensure_ascii=False,
  sort_keys=True))) == sha256 of ARB baseline.query_text_for_eval(sample).

Step 1 - field walk. Query fields are visited in a fixed per-task order;
  fields present in the query but not listed are visited afterwards in sorted
  key order; listed fields absent from the query are skipped:
    trace2code:      failure_excerpt, command
    code2test:       pr_title, pr_body, implementation_files, changed_file_summary
    edit2ripple:     anchor_file, anchor_diff
    comment2context: given_file, path, comment
    any other task:  (no listed fields; sorted keys only)
  Value flattening: str as-is; list/tuple elements in order (recursively);
  dict values in sorted-key order (recursively; keys are not emitted);
  bool/int/float as json.dumps(value); None emits nothing.
  Each emitted string is a separate segment.

Step 2 - tokenize every segment with query_plan.tokenize_nl (NFC; maximal runs
  of Unicode Alphabetic/Number/Mark or "_" joined by "-", "_", ".", "/";
  case preserved), concatenating segment token lists in walk order.

Step 3 - per raw token, in order:
  0. strip leading and trailing "-", "." and "/" characters (never "_", which
     is a lexical token character); "deprecated." -> "deprecated",
     "./logger_test.go" -> "logger_test.go". This does not change which
     lexical index terms the token matches, since "-", "." and "/" are not
     term characters.
  a. if len(token) > 96 chars: split it on every joiner character "-_./";
     each non-empty piece replaces it; a piece still > 96 chars is dropped
     (dropped.too_long_piece). Pieces are not stripped again (splitting on
     every joiner leaves none at their edges).
  b. drop if len < 2 chars (dropped.too_short).
  c. drop pure numbers: the token contains at least one Unicode Number char and
     consists only of Unicode Number chars and joiners "-_./"
     (dropped.pure_number), e.g. 325, 1.19, 2021-01-02.
  d. drop hex hashes: the token fully matches [0-9A-Fa-f]{7,} and contains at
     least one ASCII digit (dropped.hex_hash).
  e. drop tokens the plan would skip or refuse as lexical terms: no run of
     token characters (dropped.no_index_term), or some run of token characters
     longer than MAX_TOKEN_BYTES=256 UTF-8 bytes (dropped.index_term_too_long).

Step 4 - dedupe by exact string, keeping the first occurrence
  (dropped.duplicate).

Step 5 - if more than 32 distinct tokens remain, select 32 deterministically:
  identifier/path-like tokens first (contain "_", ".", "/", or an internal
  lower->upper case change, i.e. a Unicode Ll char immediately followed by a
  Unicode Lu char), in first-occurrence order; then every other token in
  first-occurrence order; keep the first 32 of that preference list
  (dropped.over_limit). The kept tokens are emitted in their original
  first-occurrence order.

Step 6 - adapted_text = kept tokens joined by single U+0020 spaces.
  Zero kept tokens is a typed refusal (ArbAdapterRefusal, a ValueError).
  Self-check (refusal on failure): tokenize_nl(adapted_text) == kept tokens and
  query_plan.plan_lexical_request("natural_language", adapted_text) succeeds.
"""

__doc__ = (__doc__ or "") + "\n" + RULE_TEXT

RULE_SHA256 = hashlib.sha256(RULE_TEXT.encode("utf-8")).hexdigest()

ALLOWED_SAMPLE_KEYS = ("id", "task_type", "repo", "base_commit", "query")

TASK_FIELD_ORDER: dict[str, tuple[str, ...]] = {
    "trace2code": ("failure_excerpt", "command"),
    "code2test": ("pr_title", "pr_body", "implementation_files", "changed_file_summary"),
    "edit2ripple": ("anchor_file", "anchor_diff"),
    "comment2context": ("given_file", "path", "comment"),
}

MAX_TOKENS = DEFAULT_NL_CONFIG["max_tokens"]
MAX_TOKEN_CHARS = DEFAULT_NL_CONFIG["max_token_chars"]
MIN_TOKEN_CHARS = 2
JOINERS = "-_./"

_JOINER_SPLIT = re.compile(r"[-_./]")
_EDGE_STRIP = "-./"
_PURE_NUMBER = regex.compile(r"\A(?=.*\p{Number})[\p{Number}\-_./]+\Z")
_HEX_HASH = re.compile(r"\A(?=.*[0-9])[0-9A-Fa-f]{7,}\Z")
_CASE_CHANGE = regex.compile(r"\p{Ll}\p{Lu}")

DROP_REASONS = (
    "too_long_piece",
    "too_short",
    "pure_number",
    "hex_hash",
    "no_index_term",
    "index_term_too_long",
    "duplicate",
    "over_limit",
)


class ArbAdapterRefusal(ValueError):
    """Typed refusal: the sample cannot be adapted into an accepted NL plan."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code


def official_query_text(query: Any) -> str:
    """ARB ``baseline.query_text_for_eval`` bytes for a query dict."""
    return json.dumps(query or {}, ensure_ascii=False, sort_keys=True)


def project_input(sample: dict[str, Any]) -> dict[str, Any]:
    """Copy only the allowed keys; the transformation never sees anything else."""
    if not isinstance(sample, dict):
        raise ArbAdapterRefusal("ARB_ADAPTER_INVALID_SAMPLE", "sample must be a JSON object")
    projected = {key: copy.deepcopy(sample.get(key)) for key in ALLOWED_SAMPLE_KEYS}
    if not isinstance(projected["query"], dict):
        raise ArbAdapterRefusal("ARB_ADAPTER_INVALID_SAMPLE", "sample.query must be a JSON object")
    return projected


def field_walk_order(task_type: Any, query: dict[str, Any]) -> list[str]:
    listed = TASK_FIELD_ORDER.get(str(task_type), ())
    order = [field for field in listed if field in query]
    order.extend(sorted(key for key in query if key not in listed))
    return order


def _flatten(value: Any) -> list[str]:
    if value is None:
        return []
    if isinstance(value, str):
        return [value]
    if isinstance(value, bool | int | float):
        return [json.dumps(value)]
    if isinstance(value, list | tuple):
        out: list[str] = []
        for item in value:
            out.extend(_flatten(item))
        return out
    if isinstance(value, dict):
        out = []
        for key in sorted(value):
            out.extend(_flatten(value[key]))
        return out
    raise ArbAdapterRefusal(
        "ARB_ADAPTER_INVALID_SAMPLE", f"unsupported query value type {type(value).__name__}"
    )


def _index_term_status(token: str) -> str | None:
    """None when every token-char run is an index term; else the drop reason."""
    saw = False
    run_bytes = 0
    for ch in token + " ":
        if _is_token_char(ch):
            run_bytes += len(ch.encode("utf-8"))
            continue
        if run_bytes:
            if run_bytes > MAX_TOKEN_BYTES:
                return "index_term_too_long"
            saw = True
            run_bytes = 0
    return None if saw else "no_index_term"


def is_identifier_like(token: str) -> bool:
    return "_" in token or "." in token or "/" in token or _CASE_CHANGE.search(token) is not None


def _transform(projected: dict[str, Any]) -> dict[str, Any]:
    query = projected["query"]
    task_type = projected["task_type"]
    order = field_walk_order(task_type, query)
    dropped = dict.fromkeys(DROP_REASONS, 0)
    split_tokens = 0
    raw_tokens: list[str] = []
    for field in order:
        for segment in _flatten(query[field]):
            raw_tokens.extend(tokenize_nl(segment))

    candidates: list[str] = []
    for raw_token in raw_tokens:
        token = raw_token.strip(_EDGE_STRIP)
        pieces = [token]
        if len(token) > MAX_TOKEN_CHARS:
            split_tokens += 1
            pieces = []
            for piece in _JOINER_SPLIT.split(token):
                if not piece:
                    continue
                if len(piece) > MAX_TOKEN_CHARS:
                    dropped["too_long_piece"] += 1
                    continue
                pieces.append(piece)
        for piece in pieces:
            if len(piece) < MIN_TOKEN_CHARS:
                dropped["too_short"] += 1
                continue
            if _PURE_NUMBER.fullmatch(piece):
                dropped["pure_number"] += 1
                continue
            if _HEX_HASH.fullmatch(piece):
                dropped["hex_hash"] += 1
                continue
            reason = _index_term_status(piece)
            if reason is not None:
                dropped[reason] += 1
                continue
            candidates.append(piece)

    distinct: list[str] = []
    seen: set[str] = set()
    for token in candidates:
        if token in seen:
            dropped["duplicate"] += 1
            continue
        seen.add(token)
        distinct.append(token)

    if len(distinct) > MAX_TOKENS:
        preference = [t for t in distinct if is_identifier_like(t)] + [
            t for t in distinct if not is_identifier_like(t)
        ]
        keep = set(preference[:MAX_TOKENS])
        dropped["over_limit"] = len(distinct) - MAX_TOKENS
        tokens = [t for t in distinct if t in keep]
    else:
        tokens = distinct

    if not tokens:
        raise ArbAdapterRefusal(
            "ARB_ADAPTER_EMPTY", "no plan-acceptable token remains after adaptation"
        )
    adapted_text = " ".join(tokens)
    if tokenize_nl(adapted_text) != tokens:
        raise ArbAdapterRefusal(
            "ARB_ADAPTER_SELF_CHECK", "adapted text does not re-tokenize to the kept tokens"
        )
    try:
        plan_lexical_request("natural_language", adapted_text)
    except QueryPlanError as error:
        raise ArbAdapterRefusal("ARB_ADAPTER_PLAN_REFUSED", str(error)) from error

    return {
        "adapter": ADAPTER_VERSION,
        "rule_sha256": RULE_SHA256,
        "sample_id": projected["id"],
        "task_type": task_type,
        "repo": projected["repo"],
        "base_commit": projected["base_commit"],
        "original_query_sha256": hashlib.sha256(
            official_query_text(query).encode("utf-8")
        ).hexdigest(),
        "adapted_text": adapted_text,
        "adapted_text_sha256": hashlib.sha256(adapted_text.encode("utf-8")).hexdigest(),
        "tokens": tokens,
        "token_count": len(tokens),
        "raw_token_count": len(raw_tokens),
        "dropped": {**dropped, "split_over_96_chars": split_tokens},
        "fields_used": [f"query.{field}" for field in order],
    }


def adapt(sample: dict[str, Any]) -> dict[str, Any]:
    """Adapt one ARB sample. Reads only ``ALLOWED_SAMPLE_KEYS`` (via projection)."""
    return _transform(project_input(sample))


def _sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def freeze(sample_paths: list[Path], repo: str | None, out_dir: Path) -> dict[str, Any]:
    if out_dir.exists():
        raise FileExistsError(f"refusing existing output directory: {out_dir}")
    rows: list[dict[str, Any]] = []
    source_digests: list[dict[str, str]] = []
    seen_ids: set[str] = set()
    for path in sample_paths:
        # Hash and parse the same bytes: no second read can differ.
        raw = path.read_bytes()
        source_digests.append({"path": str(path), "sha256": hashlib.sha256(raw).hexdigest()})
        for line_no, line in enumerate(raw.decode("utf-8").split("\n"), start=1):
            if not line.strip():
                continue
            sample = json.loads(line)
            if not isinstance(sample, dict):
                raise ValueError(f"{path}:{line_no}: sample row is not a JSON object")
            if repo is not None and sample.get("repo") != repo:
                continue
            for key in ("id", "repo", "base_commit"):
                if not isinstance(sample.get(key), str) or not sample[key]:
                    raise ValueError(f"{path}:{line_no}: sample {key} must be a nonempty string")
            if sample["id"] in seen_ids:
                raise ValueError(f"{path}:{line_no}: duplicate sample id {sample['id']}")
            seen_ids.add(sample["id"])
            source = {"source_file": str(path), "source_line": line_no}
            try:
                row = {"status": "adapted", **adapt(sample), **source}
                row["plan_accepted"] = True
            except ArbAdapterRefusal as refusal:
                row = {
                    "status": "refused",
                    "adapter": ADAPTER_VERSION,
                    "sample_id": sample.get("id"),
                    "task_type": sample.get("task_type"),
                    "refusal_code": refusal.code,
                    "refusal": str(refusal),
                    "plan_accepted": False,
                    **source,
                }
            rows.append(row)
    counts: dict[str, Any] = {"total": len(rows), "adapted": 0, "refused": 0, "by_task": {}}
    for row in rows:
        counts[row["status"]] += 1
        task = counts["by_task"].setdefault(str(row["task_type"]), {"adapted": 0, "refused": 0})
        task[row["status"]] += 1
    module_dir = Path(__file__).resolve().parent
    manifest = {
        "adapter": ADAPTER_VERSION,
        "rule_sha256": RULE_SHA256,
        "rule_text": RULE_TEXT,
        "module_sha256": _sha256_file(Path(__file__).resolve()),
        "query_plan_module_sha256": _sha256_file(module_dir / "query_plan.py"),
        "nl_config": dict(DEFAULT_NL_CONFIG),
        "repo_filter": repo,
        "source_samples": source_digests,
        "counts": counts,
    }
    # Write into a sibling staging directory and publish it by rename, so a
    # failed write never leaves a partial freeze behind.
    staging = out_dir.with_name(out_dir.name + ".staging")
    if staging.exists():
        raise FileExistsError(f"refusing existing staging directory: {staging}")
    staging.mkdir(parents=True)
    with (staging / "adapted.jsonl").open("w", encoding="utf-8") as handle:
        for row in rows:
            handle.write(json.dumps(row, ensure_ascii=False, sort_keys=True) + "\n")
    manifest["adapted_jsonl_sha256"] = _sha256_file(staging / "adapted.jsonl")
    (staging / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    staging.rename(out_dir)
    return manifest


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="python -m tools.benchmark.retrieval.arb_adapter")
    sub = parser.add_subparsers(dest="command", required=True)
    freeze_parser = sub.add_parser("freeze", help="adapt ARB samples into a new directory")
    freeze_parser.add_argument("--samples", nargs="+", required=True, type=Path)
    freeze_parser.add_argument("--repo", default=None, help="keep only samples of this repo")
    freeze_parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args(argv)
    try:
        manifest = freeze(args.samples, args.repo, args.out)
    except FileExistsError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    print(json.dumps(manifest["counts"], sort_keys=True))
    return 0 if manifest["counts"]["refused"] == 0 else 3


if __name__ == "__main__":
    raise SystemExit(main())
