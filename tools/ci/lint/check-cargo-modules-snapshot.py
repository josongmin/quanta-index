#!/usr/bin/env python3
"""Snapshot the module tree of each workspace crate.

Complements `check-public-api.py`:

  * `cargo public-api` answers "what items are reachable from lib.rs"
  * `cargo modules structure --lib` answers "what is the internal library mod tree"

The pair locks both the *external* shape and the *internal* organization so
that silent module renames, deletions, or relocations always appear as a
reviewable diff in the same PR that performs them.

Usage:
    check-cargo-modules-snapshot.py                   # CI: fail on drift
    check-cargo-modules-snapshot.py --update-baseline # accept the new tree

Requires:
    cargo install cargo-modules --locked
"""

from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from workspace_metadata import library_target_name, load_workspace_members

ROOT = Path(__file__).resolve().parents[3]
BASELINE_DIR = ROOT / "tools" / "ci" / "lint" / "baselines" / "cargo-modules"

# Contract, core and the publication kernel are structurally load-bearing;
# adapters can evolve their module tree freely. Extend this list deliberately.
GUARDED_CRATES: list[str] = [
    "quanta-index-contract",
    "quanta-index-core",
    "quanta-index-kernel",
]


def default_cache_root() -> Path:
    override = os.environ.get("QUANTA_INDEX_CACHE_ROOT")
    if override:
        return Path(override).expanduser()

    if platform.system() == "Darwin":
        return Path.home() / "Library" / "Caches" / "quanta-index"

    base = os.environ.get("XDG_CACHE_HOME")
    if base:
        return Path(base).expanduser() / "quanta-index"
    return Path.home() / ".cache" / "quanta-index"


def cargo_env(default_lane: str) -> dict[str, str]:
    env = os.environ.copy()
    env.pop("CARGO_BUILD_BUILD_DIR", None)
    cache_root = default_cache_root()
    env.setdefault("QUANTA_INDEX_CACHE_ROOT", str(cache_root))
    env.setdefault("QUANTA_INDEX_REPO_ROOT", str(ROOT))
    env.setdefault("QUANTA_INDEX_BUILD_LANE", default_lane)
    env.setdefault(
        "CARGO_TARGET_DIR",
        str(
            cache_root
            / "target"
            / hashlib.sha256(str(ROOT.resolve()).encode()).hexdigest()[:16]
            / env["QUANTA_INDEX_BUILD_LANE"]
        ),
    )
    return env


def validate_workspace_dependencies() -> dict[str, dict]:
    # cargo-modules can emit a partial tree when Cargo cannot resolve the
    # workspace. Reject that state before comparing or writing any baseline.
    return load_workspace_members(ROOT, cargo_env("cargo-modules-lane"))


def child_check_diagnostics(status_file: Path) -> str:
    diagnostics: list[str] = []
    for path in sorted(status_file.parent.glob(f"{status_file.name}.*.stdout")):
        # With --message-format=json, build-script compiler errors arrive on stdout.
        with path.open(encoding="utf-8", errors="replace") as stream:
            for line in stream:
                if '"compiler-message"' not in line:
                    continue
                try:
                    record = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if not isinstance(record, dict) or record.get("reason") != "compiler-message":
                    continue
                message = record.get("message")
                if isinstance(message, dict) and message.get("level") == "error":
                    detail = message.get("rendered") or message.get("message")
                    if isinstance(detail, str) and detail.strip():
                        diagnostics.append(detail.rstrip())
    for path in sorted(status_file.parent.glob(f"{status_file.name}.*.stderr")):
        detail = path.read_text(encoding="utf-8", errors="replace").strip()
        if detail:
            diagnostics.append(detail)
    return "\n".join(diagnostics)


def render_module_tree(package: str, library_name: str) -> str:
    # The snapshot is compared byte-for-byte against a plain-text baseline, so
    # the tool must never colour its output. cargo-modules honours NO_COLOR; a
    # shell that exports CLICOLOR_FORCE (or a tool version that colours when
    # piped) would otherwise turn a one-module change into a whole-tree diff.
    env = cargo_env("cargo-modules-lane")
    env["NO_COLOR"] = "1"
    env.pop("CLICOLOR_FORCE", None)
    env.pop("FORCE_COLOR", None)
    cargo_home_bin = Path(env.get("CARGO_HOME", str(Path.home() / ".cargo"))).expanduser() / "bin"
    tool_path = os.pathsep.join((env.get("PATH", ""), str(cargo_home_bin)))
    cargo_modules = shutil.which("cargo-modules", path=tool_path)
    cargo = shutil.which("cargo", path=tool_path)
    if cargo_modules is None or cargo is None:
        raise RuntimeError("cargo modules or cargo executable is unavailable")
    # Invoking `cargo modules` via Cargo overwrites CARGO, hiding the child
    # process from our shim. The direct binary honors CARGO. cargo-modules may
    # return a valid-looking tree with exit 0 even when its child check fails.
    cmd = [cargo_modules, "modules", "structure", "--lib", "--package", package, "--no-fns"]
    with tempfile.TemporaryDirectory(prefix="quanta-cargo-modules-") as temporary:
        shim = Path(temporary) / "cargo"
        status_file = Path(temporary) / "check-status"
        shim.write_text(
            "#!/bin/sh\n"
            'if [ "${1:-}" != "check" ]; then\n'
            '  exec "$QUANTA_INDEX_MODULES_REAL_CARGO" "$@"\n'
            "fi\n"
            'log="$QUANTA_INDEX_MODULES_CHECK_STATUS.$$"\n'
            '"$QUANTA_INDEX_MODULES_REAL_CARGO" "$@" > "$log.stdout" 2> "$log.stderr"\n'
            "status=$?\n"
            'if ! cat "$log.stdout" || ! cat "$log.stderr" >&2; then\n'
            "  status=1\n"
            "fi\n"
            'if [ "$status" -eq 0 ]; then rm -f "$log.stdout" "$log.stderr"; fi\n'
            'printf "%s\\n" "$status" >> "$QUANTA_INDEX_MODULES_CHECK_STATUS" || exit 1\n'
            'exit "$status"\n',
            encoding="utf-8",
        )
        shim.chmod(0o700)
        env["CARGO"] = str(shim)
        env["QUANTA_INDEX_MODULES_REAL_CARGO"] = cargo
        env["QUANTA_INDEX_MODULES_CHECK_STATUS"] = str(status_file)
        result = subprocess.run(
            cmd,
            cwd=ROOT,
            env=env,
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            diagnostics = "\n".join(
                part
                for part in (result.stderr.rstrip(), child_check_diagnostics(status_file))
                if part
            )
            raise RuntimeError(f"cargo modules structure failed for {package}: {diagnostics}")
        try:
            statuses = status_file.read_text(encoding="ascii").splitlines()
        except (FileNotFoundError, UnicodeError) as error:
            raise RuntimeError(
                f"cargo modules did not prove child cargo check for {package}"
            ) from error
        if not statuses or any(re.fullmatch(r"[0-9]+", status) is None for status in statuses):
            raise RuntimeError(
                f"cargo modules returned invalid child cargo check status for {package}"
            )
        failures = [status for status in statuses if int(status) != 0]
        if failures:
            summary = (
                f"cargo modules child cargo check failed for {package}: exit {', '.join(failures)}"
            )
            diagnostics = child_check_diagnostics(status_file)
            raise RuntimeError(f"{summary}\n{diagnostics}" if diagnostics else summary)
    body = "\n".join(line.rstrip() for line in result.stdout.splitlines())
    expected_root = f"crate {library_name}"
    if next((line for line in body.splitlines() if line.strip()), None) != expected_root:
        raise RuntimeError(f"cargo modules returned missing or wrong crate root for {package}")
    return body + "\n"


def baseline_path(package: str) -> Path:
    return BASELINE_DIR / f"{package}.txt"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--update-baseline", action="store_true")
    parser.add_argument("--packages", nargs="+", default=GUARDED_CRATES)
    return parser.parse_args()


def main() -> int:
    args = parse_args()

    # A removed protected crate is a structural change, not a passing snapshot.
    present = validate_workspace_dependencies()
    packages = [p for p in args.packages if p in present]
    missing = [p for p in args.packages if p not in present]
    for p in missing:
        print(
            f"{p}: protected crate is absent from workspace.members; update the guarded contract explicitly",
            file=sys.stderr,
        )

    if missing:
        return 1

    # All selected producers must succeed before an update can touch even the
    # first baseline. Otherwise a later failure leaves a mixed snapshot set.
    rendered = {
        pkg: render_module_tree(pkg, library_target_name(pkg, present[pkg])) for pkg in packages
    }
    if args.update_baseline:
        BASELINE_DIR.mkdir(parents=True, exist_ok=True)
        for pkg, current in rendered.items():
            path = baseline_path(pkg)
            path.write_text(current, encoding="utf-8")
            print(f"baseline updated: {path}")
        return 0

    bad = False
    for pkg in packages:
        current = rendered[pkg]
        path = baseline_path(pkg)

        if not path.exists():
            print(
                f"{pkg}: no baseline at {path}. Seed with --update-baseline.",
                file=sys.stderr,
            )
            bad = True
            continue

        previous = path.read_text(encoding="utf-8")
        if current == previous:
            print(f"{pkg}: module tree unchanged.")
            continue

        bad = True
        diff = difflib.unified_diff(
            previous.splitlines(keepends=True),
            current.splitlines(keepends=True),
            fromfile=str(path),
            tofile=f"{pkg} (current)",
        )
        sys.stderr.write(f"\n{pkg}: module tree DRIFTED\n")
        sys.stderr.writelines(diff)

    if bad:
        sys.stderr.write(
            "\nA guarded crate's module tree changed. If intentional, re-run "
            "with --update-baseline and commit the new baseline.\n"
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
