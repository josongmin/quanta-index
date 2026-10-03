"""Materialize pinned CodeSearchNet judged snippets into an isolated diagnostic corpus.

This produces one synthetic file per judged source span. It does not infer
negative labels for unjudged snippets or certify the source repositories'
licenses. Partial fetches remain visible in the ledger and never become a
qualified benchmark suite.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import time
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import quote, unquote, urlsplit
from urllib.request import Request, urlopen

try:
    from tools.benchmark import evidence
    from tools.benchmark.retrieval import codesearchnet_qrels
except ModuleNotFoundError:  # direct script invocation
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from tools.benchmark import evidence
    from tools.benchmark.retrieval import codesearchnet_qrels


MAX_FILE_BYTES = 8 * 1024 * 1024
FETCH_TIMEOUT_SECONDS = 12
WORKERS = 8
RETRYABLE_STATUS = frozenset((429, 502, 503, 504))
EXTENSIONS = {
    "go": ".go",
    "java": ".java",
    "javascript": ".js",
    "php": ".php",
    "python": ".py",
    "ruby": ".rb",
}


class MaterializationError(ValueError):
    """The source or output does not satisfy the diagnostic corpus contract."""


def _digest(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def _source_location(url: str) -> dict:
    repository, source_sha40 = codesearchnet_qrels._source_url(url)
    parsed = urlsplit(url)
    path = unquote(parsed.path.split("/", 5)[5])
    span = codesearchnet_qrels.LINE_RE.fullmatch(parsed.fragment)
    assert span is not None  # _source_url validated the fragment.
    source_url = f"https://raw.githubusercontent.com/{repository}/{source_sha40}/" + quote(
        path, safe="/"
    )
    return {
        "github_url": url,
        "repository": repository,
        "source_sha40": source_sha40,
        "source_path": path,
        "source_url": source_url,
        "start_line": int(span.group(1)),
        "end_line": int(span.group(2) or span.group(1)),
    }


def _extract_span(raw: bytes, start_line: int, end_line: int) -> bytes:
    line_count = raw.count(b"\n") + int(bool(raw) and not raw.endswith(b"\n"))
    if start_line < 1 or end_line < start_line or end_line > line_count:
        raise MaterializationError(f"line span {start_line}-{end_line} exceeds {line_count} lines")
    starts = [0]
    starts.extend(index + 1 for index, byte in enumerate(raw) if byte == 10)
    start_byte = starts[start_line - 1]
    end_byte = starts[end_line] if end_line < len(starts) else len(raw)
    return raw[start_byte:end_byte]


def _http_fetch(url: str) -> dict:
    """Fetch at most one bounded source file; a failed HTTP response is data."""
    for attempt in (1, 2):
        request = Request(url, headers={"User-Agent": "quanta-codesearchnet-audit"})
        try:
            with urlopen(request, timeout=FETCH_TIMEOUT_SECONDS) as response:
                declared = response.headers.get("Content-Length")
                if declared is not None and int(declared) > MAX_FILE_BYTES:
                    return {
                        "status": "source_oversize",
                        "http_status": response.status,
                        "attempts": attempt,
                    }
                data = response.read(MAX_FILE_BYTES + 1)
                if len(data) > MAX_FILE_BYTES:
                    return {
                        "status": "source_oversize",
                        "http_status": response.status,
                        "attempts": attempt,
                    }
                return {
                    "status": "fetched",
                    "http_status": response.status,
                    "attempts": attempt,
                    "data": data,
                    "response_url": response.geturl(),
                }
        except HTTPError as error:
            if error.code in RETRYABLE_STATUS and attempt == 1:
                time.sleep(0.25)
                continue
            return {"status": "http_error", "http_status": error.code, "attempts": attempt}
        except (OSError, URLError, TimeoutError) as error:
            if attempt == 1:
                time.sleep(0.25)
                continue
            return {
                "status": "transport_error",
                "error_type": type(error).__name__,
                "attempts": attempt,
            }
    raise AssertionError("unreachable retry loop")


def _check_root(output_root: Path) -> None:
    if not output_root.is_absolute():
        raise MaterializationError("output root must be absolute")
    if not output_root.parent.is_dir():
        raise MaterializationError("output parent directory does not exist")
    if output_root.resolve(strict=False).is_relative_to(codesearchnet_qrels.TOOL_CHECKOUT):
        raise MaterializationError("output root must be outside the tool checkout")
    if output_root.exists() or output_root.is_symlink():
        raise MaterializationError("output root must be new")


def _write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, sort_keys=True, ensure_ascii=False, allow_nan=False) + "\n")


def materialize(qrels: list[dict], output_root: Path, fetcher=_http_fetch) -> dict:
    """Build a diagnostic corpus and complete ledger from prevalidated qrels.

    Callers that publish upstream provenance must validate the pinned CSV first.
    The injectable fetcher is only for local tests and makes no source claim.
    """
    _check_root(output_root)
    locations = {}
    languages_by_url: dict[str, set[str]] = defaultdict(set)
    for qrel in qrels:
        url = qrel["github_url"]
        location = _source_location(url)
        language = qrel["language"]
        if language not in EXTENSIONS:
            raise MaterializationError(f"unsupported CodeSearchNet language {language!r}")
        locations[url] = location
        languages_by_url[url].add(language)
    file_urls = {location["source_url"] for location in locations.values()}
    output_root.mkdir(mode=0o700)
    source_dir = output_root / "sources"
    snippet_dir = output_root / "snippets"
    source_dir.mkdir()
    snippet_dir.mkdir()
    source_results = {}
    with ThreadPoolExecutor(max_workers=WORKERS) as executor:
        futures = {executor.submit(fetcher, url): url for url in sorted(file_urls)}
        for future in as_completed(futures):
            url = futures[future]
            try:
                result = future.result()
            except Exception as error:
                result = {"status": "fetcher_error", "error_type": type(error).__name__}
            if result.get("status") == "fetched":
                data = result.pop("data")
                if type(data) is not bytes or len(data) > MAX_FILE_BYTES:
                    result = {"status": "invalid_fetcher_data"}
                else:
                    relative_path = f"sources/{_digest(url.encode())}.blob"
                    (output_root / relative_path).write_bytes(data)
                    result.update(
                        {
                            "relative_path": relative_path,
                            "sha256": _digest(data),
                            "bytes": len(data),
                        }
                    )
            source_results[url] = result
    snippets = []
    for url, location in sorted(locations.items()):
        source = source_results[location["source_url"]]
        entry = {**location, "languages": sorted(languages_by_url[url])}
        if source["status"] != "fetched":
            entry.update({"status": "source_unavailable", "source_status": source["status"]})
        else:
            raw = (output_root / source["relative_path"]).read_bytes()
            try:
                snippet = _extract_span(raw, location["start_line"], location["end_line"])
            except MaterializationError:
                entry.update({"status": "invalid_line_span", "source_sha256": source["sha256"]})
            else:
                entry.update({"status": "admitted", "source_sha256": source["sha256"]})
                entry["snippet_sha256"] = _digest(snippet)
                entry["snippet_bytes"] = len(snippet)
                entry["relative_paths"] = {}
                for language in sorted(languages_by_url[url]):
                    language_dir = snippet_dir / language
                    language_dir.mkdir(exist_ok=True)
                    relative_path = (
                        f"snippets/{language}/{_digest(url.encode())}{EXTENSIONS[language]}"
                    )
                    (output_root / relative_path).write_bytes(snippet)
                    entry["relative_paths"][language] = relative_path
        snippets.append(entry)
    by_url = {item["github_url"]: item for item in snippets}
    qrel_rows = []
    task_status: dict[tuple[str, str], list[bool]] = defaultdict(list)
    for qrel in qrels:
        location = by_url[qrel["github_url"]]
        admitted = location["status"] == "admitted"
        task_status[(qrel["language"], qrel["query"])].append(admitted)
        qrel_rows.append(
            {
                **qrel,
                "materialization_status": location["status"],
                "snippet_path": location.get("relative_paths", {}).get(qrel["language"]),
            }
        )
    coverage = {}
    for language in sorted(EXTENSIONS):
        rows = [row for row in qrel_rows if row["language"] == language]
        tasks = [statuses for (lang, _), statuses in task_status.items() if lang == language]
        coverage[language] = {
            "qrels_total": len(rows),
            "qrels_admitted": sum(row["materialization_status"] == "admitted" for row in rows),
            "tasks_total": len(tasks),
            "tasks_complete": sum(all(statuses) for statuses in tasks),
        }
    manifest = {
        "kind": "codesearchnet_snippet_materialization_diagnostic_v1",
        "qualification": "diagnostic_unqualified",
        "unit": "one_commit_pinned_source_span_per_synthetic_file",
        "source_attestation": "raw_github_response_sha256_recorded_git_tree_blob_not_attested",
        "license_status": "original_repository_file_licenses_unverified",
        "source_fetch_count": len(file_urls),
        "source_fetch_statuses": dict(Counter(row["status"] for row in source_results.values())),
        "span_count": len(snippets),
        "span_statuses": dict(Counter(row["status"] for row in snippets)),
        "qrel_count": len(qrels),
        "task_count": len(task_status),
        "coverage": coverage,
        "complete_materialization": all(row["status"] == "admitted" for row in snippets),
        "qualified_for_product_comparison": False,
    }
    _write_json(output_root / "manifest.json", manifest)
    _write_json(output_root / "source-fetches.json", source_results)
    _write_json(output_root / "spans.json", snippets)
    _write_json(output_root / "qrels.json", qrel_rows)
    return manifest


def capture(csv_path: Path, output_root: Path) -> dict:
    raw = evidence.read_control(csv_path)
    seed = codesearchnet_qrels.diagnostic_seed(raw)
    manifest = materialize(seed["qrels"], output_root)
    manifest["upstream_csv"] = seed["source"]
    _write_json(output_root / "manifest.json", manifest)
    return manifest


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--csv", type=Path, required=True)
    parser.add_argument("--output-root", type=Path, required=True)
    args = parser.parse_args(argv)
    manifest = capture(args.csv, args.output_root)
    print(json.dumps(manifest, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
