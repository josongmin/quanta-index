"""Freeze per-repository code-search candidate manifests; never issue admission.

The paired runner accepts one clean Git checkout/commit per capture. This tool
therefore emits one RB-00 manifest per repository, not a synthetic mixed repo.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

MAX_FILE_BYTES = 1024 * 1024
EXOTIC_LINE_BREAKS = frozenset("\v\f\x1c\x1d\x1e\x85\u2028\u2029")
EXTENSIONS = {
    "rust": frozenset({".rs"}),
    "python": frozenset({".py"}),
    "go": frozenset({".go"}),
    "typescript": frozenset({".ts", ".tsx"}),
    "javascript": frozenset({".js", ".jsx", ".mjs", ".cjs"}),
}
REPOSITORY_ROOT = Path(__file__).resolve().parents[3]


def require_external_path(path: Path, label: str) -> None:
    if path.resolve().is_relative_to(REPOSITORY_ROOT):
        raise ValueError(f"{label} must stay outside the quanta-index checkout")


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def git(root: Path, *args: str) -> bytes:
    return subprocess.check_output(["git", "-C", str(root), *args])


def canonical_json(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, indent=2, ensure_ascii=False) + "\n").encode()


def freeze_one(entry: dict, checkouts: Path) -> tuple[dict, dict]:
    name = entry["name"]
    language = entry["language"]
    commit = entry["revision"]
    prefix = entry["benchmark_root"]
    if not name.isascii() or not name.replace("-", "").isalnum():
        raise ValueError(f"unsafe repository name: {name}")
    if language not in EXTENSIONS:
        raise ValueError(f"unsupported language: {language}")
    if len(commit) != 40 or any(c not in "0123456789abcdef" for c in commit):
        raise ValueError(f"invalid revision: {name}")
    if (
        not isinstance(prefix, str)
        or prefix.startswith("/")
        or "\\" in prefix
        or (prefix and any(p in {"", ".", ".."} for p in prefix.split("/")))
    ):
        raise ValueError(f"invalid benchmark root: {name}")
    root = checkouts / name
    if (
        not root.is_dir()
        or root.is_symlink()
        or Path(git(root, "rev-parse", "--show-toplevel").decode().strip()) != root.resolve()
    ):
        raise ValueError(f"checkout root mismatch: {name}")
    if git(root, "rev-parse", "HEAD").decode().strip() != commit:
        raise ValueError(f"checkout revision mismatch: {name}")
    if git(root, "status", "--porcelain=v1", "--untracked-files=all"):
        raise ValueError(f"dirty checkout: {name}")

    paths = git(root, "ls-files", "-z").split(b"\0")
    files = []
    exclusions: dict[str, int] = {}
    excluded_files = []
    license_sources = []
    for raw_path in paths:
        if not raw_path:
            continue
        path = raw_path.decode("utf-8", "strict")
        if (
            path.startswith("/")
            or "\\" in path
            or any(p in {"", ".", ".."} for p in path.split("/"))
        ):
            raise ValueError(f"noncanonical tracked path: {name}/{path}")
        license_name = path.lower()
        if "/" not in path and any(
            license_name == stem or license_name.startswith(stem + separator)
            for stem in ("license", "licence", "copying", "unlicense")
            for separator in (".", "-", "_")
        ):
            license_path = root / path
            if not license_path.is_file() or license_path.is_symlink():
                raise ValueError(f"non-regular license source: {name}/{path}")
            license_sources.append({"path": path, "sha256": sha(license_path.read_bytes())})
        reason = None
        if prefix and not (path == prefix or path.startswith(prefix + "/")):
            reason = "outside_benchmark_root"
        elif Path(path).suffix.lower() not in EXTENSIONS[language]:
            reason = "non_code_extension"
        else:
            absolute = root / path
            if absolute.is_symlink() or not absolute.is_file():
                reason = "non_regular_or_symlink"
            elif absolute.stat().st_size == 0 or absolute.stat().st_size > MAX_FILE_BYTES:
                reason = "empty_or_oversize"
            else:
                data = absolute.read_bytes()
                try:
                    content = data.decode("utf-8", "strict")
                except UnicodeDecodeError:
                    reason = "non_utf8"
                else:
                    if "\0" in content or any(c in content for c in EXOTIC_LINE_BREAKS):
                        reason = "binary_or_exotic_line_break"
                    else:
                        files.append({"path": path, "file_sha256": sha(data)})
        if reason:
            exclusions[reason] = exclusions.get(reason, 0) + 1
            excluded_files.append({"path": path, "reason": reason})
    if not files:
        raise ValueError(f"no admitted code files: {name}")
    if not license_sources:
        raise ValueError(f"no root license source: {name}")
    files.sort(key=lambda row: row["path"])
    license_sources.sort(key=lambda row: row["path"])
    excluded_files.sort(key=lambda row: row["path"])
    manifest = {"repository_commit": commit, "files": files}
    summary = {
        "name": name,
        "language": language,
        "source_url": entry["url"],
        "revision": commit,
        "benchmark_root": prefix,
        "upstream_semble_benchmark_overlap": entry["upstream_semble_benchmark_overlap"],
        "file_count": len(files),
        "excluded_tracked_file_counts": dict(sorted(exclusions.items())),
        "excluded_tracked_files": excluded_files,
        "license_source_files_not_approval": license_sources,
        "manifest_sha256": sha(canonical_json(manifest)),
    }
    return manifest, summary


def freeze_set(spec: dict, checkouts: Path) -> tuple[dict[str, dict], dict]:
    if set(spec) != {"source_revision", "repositories"} or not isinstance(
        spec["repositories"], list
    ):
        raise ValueError("invalid corpus-set spec")
    seen: set[str] = set()
    manifests: dict[str, dict] = {}
    summaries = []
    for entry in spec["repositories"]:
        if set(entry) != {
            "name",
            "language",
            "url",
            "revision",
            "benchmark_root",
            "upstream_semble_benchmark_overlap",
        }:
            raise ValueError("invalid repository entry")
        if type(entry["upstream_semble_benchmark_overlap"]) is not bool:
            raise ValueError("overlap must be boolean")
        if entry["name"] in seen:
            raise ValueError("duplicate repository name")
        seen.add(entry["name"])
        manifest, summary = freeze_one(entry, checkouts)
        manifests[entry["name"]] = manifest
        summaries.append(summary)
    if not summaries:
        raise ValueError("empty repository set")
    return manifests, {
        "status": "candidate_not_admitted_no_gold_no_pair",
        "source_revision": spec["source_revision"],
        "repositories": summaries,
        "repository_count": len(summaries),
        "file_count": sum(row["file_count"] for row in summaries),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--spec", type=Path, required=True)
    parser.add_argument("--checkouts", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    require_external_path(args.spec, "corpus-set spec")
    require_external_path(args.checkouts, "corpus checkouts")
    require_external_path(args.out, "corpus artifacts")
    if args.out.exists() and any(args.out.iterdir()):
        raise ValueError("output directory must be absent or empty")
    spec = json.loads(args.spec.read_text(encoding="utf-8"))
    manifests, summary = freeze_set(spec, args.checkouts.resolve())
    args.out.mkdir(parents=True, exist_ok=True)
    manifest_dir = args.out / "manifests"
    manifest_dir.mkdir()
    for name, manifest in manifests.items():
        (manifest_dir / f"{name}.json").write_bytes(canonical_json(manifest))
    (args.out / "corpus-set.json").write_bytes(canonical_json(summary))
    print(
        json.dumps(
            {
                "repository_count": summary["repository_count"],
                "file_count": summary["file_count"],
                "corpus_set_sha256": sha(canonical_json(summary)),
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
