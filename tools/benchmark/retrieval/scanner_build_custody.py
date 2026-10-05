"""Candidate local scanner build custody; diagnostic, not remote attestation.

Capture executes only when explicitly called. This module does not run in this
preparation task. Keep its receipts outside the source checkout.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import stat
import subprocess
import sys
from pathlib import Path

try:
    from tools.benchmark.retrieval.scanner_source_identity import (
        CustodyError, capture as source_capture, verify as source_verify,
    )
except ModuleNotFoundError:  # external static candidate before repository integration
    from scanner_source_identity import (
        CustodyError, capture as source_capture, verify as source_verify,
    )


def _canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _file(path: Path) -> str:
    try:
        before = path.lstat()
        if not stat.S_ISREG(before.st_mode):
            raise CustodyError(f"missing, symlinked or non-file artifact: {path}")
        flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
        with os.fdopen(os.open(path, flags), "rb") as stream:
            opened = os.fstat(stream.fileno())
            data = stream.read()
        after = path.lstat()
    except OSError as error:
        raise CustodyError(f"scanner artifact could not be read: {path}") from error
    identity = lambda row: (row.st_dev, row.st_ino, row.st_mode, row.st_size,
                            row.st_mtime_ns, row.st_ctime_ns)
    if identity(before) != identity(opened) or identity(opened) != identity(after):
        raise CustodyError(f"scanner artifact changed while reading: {path}")
    return _sha(data)


def _inside(path: Path, root: Path) -> bool:
    try:
        path.resolve(strict=False).relative_to(root.resolve(strict=False))
        return True
    except ValueError:
        return False


def _input_inventory(roots: dict[str, str]) -> dict:
    if not isinstance(roots, dict) or not roots or any(not isinstance(k, str) for k in roots):
        raise CustodyError("scanner input roots must be a nonempty role map")
    result = {}
    for role, raw in sorted(roots.items()):
        path = Path(raw)
        if not isinstance(raw, str) or not path.is_absolute() or path.is_symlink() or not path.exists():
            raise CustodyError(f"invalid input root: {role}")
        if path.is_file():
            files = [path]
            base = path.parent
        elif path.is_dir():
            files = sorted(p for p in path.rglob("*") if p.is_file() or p.is_symlink())
            base = path
        else:
            raise CustodyError(f"invalid input type: {role}")
        if not files:
            raise CustodyError(f"empty input root: {role}")
        if path.is_dir() and any(p.is_symlink() for p in path.rglob("*")):
            raise CustodyError(f"symlink in scanner input root: {role}")
        result[role] = {
            "root": str(path),
            "files": [
                {"path": p.relative_to(base).as_posix(), "sha256": _file(p)}
                for p in files
            ],
        }
    return result


def _tools(repo: Path, env: dict[str, str]) -> dict:
    tools = {"wrapper": repo / "scripts/cargow", "python": Path(sys.executable)}
    for name in ("cargo", "rustc", "git"):
        found = shutil.which(name, path=env.get("PATH"))
        if found is None:
            raise CustodyError(f"missing build tool: {name}")
        tools[name] = Path(found)
    result = {}
    for name, path in tools.items():
        resolved = path.resolve(strict=True)
        result[name] = {"path": str(path), "realpath": str(resolved), "sha256": _file(resolved)}
    for name, argv in (("cargo", [str(tools["cargo"]), "--version"]),
                       ("rustc", [str(tools["rustc"]), "-Vv"])):
        process = subprocess.run(argv, cwd=repo, env=env, stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, timeout=30, check=False)
        if process.returncode or not process.stdout:
            raise CustodyError(f"tool version probe failed: {name}")
        result[name]["version"] = process.stdout.decode(errors="replace").strip()
    return result


EXECUTION_ENV_KEYS = frozenset({
    "PATH", "HOME", "TMPDIR", "LANG", "LC_ALL", "CARGO_HOME", "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN", "RUSTC", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_INCREMENTAL", "CARGO_BUILD_TARGET", "CARGO_BUILD_RUSTFLAGS",
    "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "RUSTDOCFLAGS", "SDKROOT",
    "MACOSX_DEPLOYMENT_TARGET", "CC", "CXX", "AR",
    "PYTHONNOUSERSITE", "HF_HOME", "HF_HUB_CACHE", "TRANSFORMERS_CACHE",
    "TOKENIZERS_PARALLELISM", "OMP_NUM_THREADS", "OPENBLAS_NUM_THREADS",
    "QUANTA_INDEX_CACHE_ROOT", "QUANTA_INDEX_RESOURCE_WAIT_SECONDS",
    "QUANTA_INDEX_RESOURCE_TIMEOUT_SECONDS",
})


def _effective_env(repo: Path, out: Path, overrides: dict[str, str]) -> dict[str, str]:
    if not isinstance(overrides, dict) or any(
        not isinstance(k, str) or not isinstance(v, str) or k not in EXECUTION_ENV_KEYS
        for k, v in overrides.items()
    ):
        raise CustodyError("execution environment overrides are malformed or unsupported")
    mandatory = {
        "CARGO_TARGET_DIR": str(out / "target"),
        "QUANTA_INDEX_PRESERVE_CARGO_TARGET_DIR": "1",
        "QUANTA_INDEX_SCCACHE": "0",
        "CARGO_NET_OFFLINE": "true",
        "QUANTA_INDEX_RESOURCE_ADMISSION": "auto",
        "QUANTA_INDEX_BUILD_LOGGING": "0",
        "PYTHONPATH": str(repo),
        "PYTHONDONTWRITEBYTECODE": "1",
    }
    if mandatory.keys() & overrides.keys():
        raise CustodyError("fresh target/cache controls cannot be overridden")
    inherited = {key: os.environ[key] for key in sorted(EXECUTION_ENV_KEYS & os.environ.keys())}
    if "PATH" not in inherited or "HOME" not in inherited:
        raise CustodyError("PATH and HOME are required for scanner execution")
    return {**inherited, **overrides, **mandatory}


def _env_fingerprints(env: dict[str, str]) -> dict[str, str]:
    return {key: _sha(value.encode()) for key, value in sorted(env.items())}


def _binary_inventory(out: Path, relpaths: dict[str, str]) -> dict:
    if set(relpaths) != {"runner", "searchd"}:
        raise CustodyError("runner and searchd binary paths required")
    result = {}
    for role, relative in sorted(relpaths.items()):
        if not isinstance(relative, str) or Path(relative).is_absolute() or ".." in Path(relative).parts:
            raise CustodyError("unsafe binary relative path")
        path = out / relative
        if not _inside(path, out):
            raise CustodyError("binary escapes fresh target")
        result[role] = {"path": str(path), "sha256": _file(path)}
    if len({entry["path"] for entry in result.values()}) != 2:
        raise CustodyError("runner and searchd binaries must be distinct files")
    return result


def _capture_inventory(out: Path, relpaths: dict[str, str]) -> dict:
    if set(relpaths) != {"record", "phases", "diagnostic", "pack"}:
        raise CustodyError("record, phases, diagnostic and projected pack required")
    result = {}
    for role, relative in sorted(relpaths.items()):
        if not isinstance(relative, str) or Path(relative).is_absolute() or ".." in Path(relative).parts:
            raise CustodyError("unsafe capture relative path")
        path = out / relative
        if not _inside(path, out):
            raise CustodyError("capture output escapes fresh target")
        result[role] = {"path": str(path), "sha256": _file(path)}
    if len({entry["path"] for entry in result.values()}) != 4:
        raise CustodyError("capture outputs must be distinct files")
    return result


BIN_RELPATHS = {
    "runner": "target/release/quanta-index-retrieval-bench",
    "searchd": "target/release/quanta-index-searchd",
}
CAP_RELPATHS = {
    "record": "capture/strategy-00-fw_strict/record.json",
    "phases": "capture/strategy-00-fw_strict/phase-metrics.json",
    "diagnostic": "capture/strategy-00-fw_strict/retrieval-diagnostic.json",
    "pack": "capture/quanta-pack.json",
}


def _template_context(spec: dict) -> tuple[dict, dict[str, str]]:
    path = Path(spec["run_template"])
    if not path.is_absolute() or path.is_symlink() or not path.is_file():
        raise CustodyError("scanner run template must be an absolute regular file")
    try:
        template = json.loads(path.read_bytes())
    except (OSError, ValueError) as error:
        raise CustodyError("scanner run template is not JSON") from error
    dynamic = {"runner_binary", "searchd_binary", "searchd_expected_sha256", "output_root"}
    required = {
        "spec_version", "repo", "manifest", "suite", "query_pack", "execution_profiles",
        "top_k", "strategies", "routes", "scope", "claims", "embedder",
        "query_repetitions_per_root", "query_warmup_passes", "query_stage_observation",
    }
    optional = {
        "seed", "run_id", "runner_name", "repo_id", "revision_id", "generation",
        "timeout_secs", "io_timeout_secs", "cache_regime", "host_profile",
        "symbol_coverage_policy", "symbol_total_timeout_ms", "code_search_rank_study",
    }
    if (
        not isinstance(template, dict)
        or not required <= set(template) <= required | optional
        or dynamic & template.keys()
        or type(template.get("spec_version")) is not int
        or template["spec_version"] != 2
        or type(template.get("top_k")) is not int
        or not (1 <= template["top_k"] <= 10_000)
        or template.get("scope") != "exploratory"
        or not isinstance(template.get("claims"), dict)
        or set(template["claims"]) != {"quality", "speed", "same_model", "incremental"}
        or any(type(value) is not bool or value for value in template["claims"].values())
        or not isinstance(template.get("execution_profiles"), dict)
        or set(template["execution_profiles"]) != {"quanta"}
        or template.get("routes") != ["lexical"]
        or template.get("strategies") != [{"name": "fixed_window_strict"}]
        or template.get("embedder") != "hash-dev"
        or template.get("query_stage_observation") != "enabled"
        or type(template.get("query_warmup_passes")) is not int
        or template["query_warmup_passes"] < 0
        or type(template.get("query_repetitions_per_root")) is not int
        or template["query_repetitions_per_root"] < 2
        or "receipts" in template
        or "source_closure_reuse" in template
    ):
        raise CustodyError("scanner template must be one exploratory lexical Quanta capture")
    for key in ("seed", "generation", "timeout_secs", "io_timeout_secs", "symbol_total_timeout_ms"):
        if key in template and (type(template[key]) is not int or template[key] < 0):
            raise CustodyError(f"scanner template {key} must be a nonnegative integer")
    for key in ("run_id", "runner_name", "repo_id", "revision_id", "cache_regime", "symbol_coverage_policy"):
        if key in template and (not isinstance(template[key], str) or not template[key]):
            raise CustodyError(f"scanner template {key} must be a nonempty string")
    if "code_search_rank_study" in template:
        study = template["code_search_rank_study"]
        if not isinstance(study, dict) or set(study) != {"max_files", "max_pages", "timeout_ms"} or any(
            type(value) is not int or value <= 0 for value in study.values()
        ):
            raise CustodyError("scanner rank study limits must be positive integers")
    roots = {"run_template": str(path)}
    for role, key in (("corpus", "repo"), ("manifest", "manifest"),
                      ("suite", "suite"), ("query_pack", "query_pack")):
        raw = template.get(key)
        if not isinstance(raw, str) or not Path(raw).is_absolute():
            raise CustodyError(f"scanner template {key} must be absolute")
        roots[role] = raw
    if "host_profile" in template:
        raw = template["host_profile"]
        if not isinstance(raw, str) or not Path(raw).is_absolute():
            raise CustodyError("scanner host profile must be absolute")
        roots["host_profile"] = raw
    return template, roots


def _validate_spec(spec: dict) -> tuple[Path, Path, dict | None, dict, dict[str, str]]:
    if not isinstance(spec, dict) or set(spec) != {
        "schema_version", "repo", "base_git_revision", "overlay", "output_root",
        "run_template", "env_overrides"
    } or type(spec["schema_version"]) is not int or spec["schema_version"] != 2:
        raise CustodyError("scanner build spec schema differs")
    if not all(isinstance(spec[key], str) for key in ("repo", "base_git_revision", "output_root", "run_template")):
        raise CustodyError("scanner build spec path/revision type differs")
    repo = Path(spec["repo"])
    out = Path(spec["output_root"])
    if not repo.is_absolute() or not out.is_absolute() or not repo.is_dir() or _inside(out, repo):
        raise CustodyError("build root must be absolute, existing, and outside checkout")
    if out.is_symlink() or (out.exists() and not out.is_dir()):
        raise CustodyError("scanner output is not a plain directory")
    if spec["overlay"] is not None and not isinstance(spec["overlay"], dict):
        raise CustodyError("scanner overlay must be a mapping or null")
    if not isinstance(spec["env_overrides"], dict):
        raise CustodyError("scanner environment override must be a mapping")
    template, roots = _template_context(spec)
    return repo, out, spec["overlay"], template, roots


def _build_argv(repo: Path) -> list[str]:
    return [str(repo / "scripts/cargow"), "--lane", "scanner-ab-lane", "build",
            "-p", "quanta-index-retrieval-bench", "-p", "quanta-index-searchd-runtime",
            "--bin", "quanta-index-retrieval-bench", "--bin", "quanta-index-searchd",
            "--all-features", "--release", "--locked"]


def _capture_argv(repo: Path, run_spec: Path) -> list[str]:
    return [sys.executable, "-m", "tools.benchmark.retrieval.run", "quanta", "--spec", str(run_spec)]


def _run_spec(template: dict, out: Path, binaries: dict) -> dict:
    return {
        **template,
        "runner_binary": binaries["runner"]["path"],
        "searchd_binary": binaries["searchd"]["path"],
        "searchd_expected_sha256": binaries["searchd"]["sha256"],
        "output_root": str(out / "capture"),
    }


def _assert_run_spec(out: Path, template: dict, binaries: dict) -> None:
    actual = out / "run-spec.json"
    if actual.is_symlink() or not actual.is_file():
        raise CustodyError("generated scanner run spec is missing or symlinked")
    if actual.read_bytes() != _canonical(_run_spec(template, out, binaries)) + b"\n":
        raise CustodyError("run spec differs from frozen template and built binaries")


def capture(spec: dict, receipt_path: Path) -> dict:
    """Run a fresh controlled build and write a bound local receipt once."""
    repo, out, overlay, template, roots = _validate_spec(spec)
    if not receipt_path.is_absolute() or out.exists() or receipt_path.exists() or _inside(receipt_path, repo):
        raise CustodyError("output/receipt must be new and outside checkout")
    if any(_inside(Path(raw), out) or _inside(out, Path(raw)) or _inside(receipt_path, Path(raw))
           for raw in roots.values()):
        raise CustodyError("input and output roots must be disjoint")
    env = _effective_env(repo, out, spec["env_overrides"])
    before_source = source_capture(repo, spec["base_git_revision"], overlay, env=env)
    before_inputs = _input_inventory(roots)
    out.mkdir(parents=True)
    before_tools = _tools(repo, env)
    build_argv = _build_argv(repo)
    process = subprocess.run(build_argv, cwd=repo, env=env,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    (out / "build.stdout").write_bytes(process.stdout)
    (out / "build.stderr").write_bytes(process.stderr)
    after_source = source_capture(repo, spec["base_git_revision"], overlay, env=env)
    after_inputs = _input_inventory(roots)
    after_tools = _tools(repo, env)
    if before_source != after_source or before_inputs != after_inputs or before_tools != after_tools:
        raise CustodyError("source, input or tool drift during build")
    if process.returncode:
        raise CustodyError(f"scanner build failed with exit {process.returncode}; raw output retained")
    binaries = _binary_inventory(out, BIN_RELPATHS)
    run_spec = out / "run-spec.json"
    run_spec.write_bytes(_canonical(_run_spec(template, out, binaries)) + b"\n")
    _assert_run_spec(out, template, binaries)
    capture_argv = _capture_argv(repo, run_spec)
    observed = subprocess.run(capture_argv, cwd=repo, env=env,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    (out / "capture.stdout").write_bytes(observed.stdout)
    (out / "capture.stderr").write_bytes(observed.stderr)
    if (source_capture(repo, spec["base_git_revision"], overlay, env=env) != before_source
        or _input_inventory(roots) != before_inputs
        or _tools(repo, env) != before_tools
        or _binary_inventory(out, BIN_RELPATHS) != binaries):
        raise CustodyError("source, input, tool or binary drift during capture")
    _assert_run_spec(out, template, binaries)
    if observed.returncode:
        raise CustodyError(f"scanner Quanta capture failed with exit {observed.returncode}; raw output retained")
    capture_outputs = _capture_inventory(out, CAP_RELPATHS)
    core = {
        "schema_version": 1,
        "spec": spec,
        "source_identity": before_source,
        "inputs": before_inputs,
        "tools": before_tools,
        "execution_env_sha256": _env_fingerprints(env),
        "build_argv": build_argv,
        "build_exit_code": 0,
        "build_stdout_sha256": _file(out / "build.stdout"),
        "build_stderr_sha256": _file(out / "build.stderr"),
        "binaries": binaries,
        "run_spec_sha256": _file(run_spec),
        "capture_argv": capture_argv,
        "capture_exit_code": observed.returncode,
        "capture_stdout_sha256": _file(out / "capture.stdout"),
        "capture_stderr_sha256": _file(out / "capture.stderr"),
        "capture_outputs": capture_outputs,
    }
    receipt = {**core, "receipt_sha256": _sha(_canonical(core))}
    with receipt_path.open("xb") as stream:
        stream.write(_canonical(receipt) + b"\n")
    return receipt


def verify(receipt: dict) -> dict:
    """Refuse stale source, inputs, toolchain, environment or binary bytes."""
    if not isinstance(receipt, dict) or set(receipt) != {
        "schema_version", "spec", "source_identity", "inputs", "tools", "execution_env_sha256",
        "build_argv", "build_exit_code", "build_stdout_sha256", "build_stderr_sha256",
        "binaries", "run_spec_sha256", "capture_argv", "capture_exit_code",
        "capture_stdout_sha256", "capture_stderr_sha256", "capture_outputs", "receipt_sha256"
    } or type(receipt["schema_version"]) is not int or receipt["schema_version"] != 1 \
      or type(receipt["build_exit_code"]) is not int or receipt["build_exit_code"] != 0 \
      or type(receipt["capture_exit_code"]) is not int or receipt["capture_exit_code"] != 0:
        raise CustodyError("scanner build receipt schema differs")
    core = {k: v for k, v in receipt.items() if k != "receipt_sha256"}
    if _sha(_canonical(core)) != receipt["receipt_sha256"]:
        raise CustodyError("scanner build receipt digest differs")
    spec = receipt["spec"]
    repo, out, overlay, template, roots = _validate_spec(spec)
    if not out.is_dir() or not receipt["source_identity"]:
        raise CustodyError("scanner build output or source identity missing")
    env = _effective_env(repo, out, spec["env_overrides"])
    source_verify(repo, receipt["source_identity"],
                  patch_path=None if overlay is None else Path(overlay["patch_path"]), env=env)
    if receipt["inputs"] != _input_inventory(roots):
        raise CustodyError("scanner run input bytes drifted")
    if receipt["execution_env_sha256"] != _env_fingerprints(env) or receipt["tools"] != _tools(repo, env):
        raise CustodyError("scanner build environment or toolchain drifted")
    if receipt["build_argv"] != _build_argv(repo):
        raise CustodyError("scanner build command differs from canonical owner")
    if receipt["binaries"] != _binary_inventory(out, BIN_RELPATHS):
        raise CustodyError("scanner built binary drifted")
    _assert_run_spec(out, template, receipt["binaries"])
    if receipt["run_spec_sha256"] != _file(out / "run-spec.json"):
        raise CustodyError("scanner run spec digest differs")
    if receipt["capture_outputs"] != _capture_inventory(out, CAP_RELPATHS):
        raise CustodyError("scanner capture output drifted")
    if (receipt["capture_argv"] != _capture_argv(repo, out / "run-spec.json")
        or receipt["capture_stdout_sha256"] != _file(out / "capture.stdout")
        or receipt["capture_stderr_sha256"] != _file(out / "capture.stderr")):
        raise CustodyError("scanner capture command or output log differs")
    if receipt["build_stdout_sha256"] != _file(out / "build.stdout") or receipt["build_stderr_sha256"] != _file(out / "build.stderr"):
        raise CustodyError("scanner build output logs drifted")
    return receipt["binaries"]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("capture", "verify"))
    parser.add_argument("--spec", type=Path)
    parser.add_argument("--receipt", type=Path, required=True)
    args = parser.parse_args()
    if args.mode == "capture":
        if args.spec is None:
            parser.error("capture requires --spec")
        capture(json.loads(args.spec.read_bytes()), args.receipt)
    else:
        verify(json.loads(args.receipt.read_bytes()))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
