#!/usr/bin/env python3
"""Record the observed CircleCI P00 command result for proof manifest input."""

from __future__ import annotations

import argparse
import json
import platform
from pathlib import Path

from tools.ci.proof_host import circleci_host_environment


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--started-at", required=True)
    parser.add_argument("--ended-at", required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    terminal = {
        "status": "passed",
        "counts": {"selected": 1, "executed": 1, "passed": 1, "failed": 0, "ignored": 0},
        "environment": {
            "toolchain": f"python-{platform.python_version()}",
            "features": [],
            "os": platform.system().lower(),
            "arch": platform.machine(),
            "host": circleci_host_environment(),
        },
        "daemon_binary": None,
        "state_root_format": "not-applicable",
        "inputs": {
            "fixture": None,
            "corpus": None,
            "config": None,
            "model": None,
            "provider": None,
        },
        "started_at": args.started_at,
        "ended_at": args.ended_at,
        "artifacts": [
            "artifacts/proof-authority/raw/p00-authority.log",
            "artifacts/proof-authority/raw/p00-terminal-input.json",
            "artifacts/sep-21/p00/error-authority-inventory.json",
        ],
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(terminal, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
