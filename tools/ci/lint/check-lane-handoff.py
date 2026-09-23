#!/usr/bin/env python3
"""Validate a SEP-21 lane handoff against Git and immutable proof authority."""

from __future__ import annotations

import argparse
import importlib.util
import json
import sys
from pathlib import Path
from types import ModuleType
from typing import Any

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python 3.10 compatibility
    import tomli as tomllib


ROOT = Path(__file__).resolve().parents[3]
PROOF_CHECKER_PATH = ROOT / "tools/ci/lint/check-proof-authority.py"
CHAIN_VALIDATOR_PATH = ROOT / "tools/ci/lint/handoff_validation.py"


def _load_module(name: str, path: Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise ValueError(f"cannot load module: {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


CHAIN_VALIDATOR = _load_module("quanta_handoff_validation", CHAIN_VALIDATOR_PATH)


def _read_json(path: Path) -> Any:
    return CHAIN_VALIDATOR._read_json(path)


def validate_handoff(
    payload: Any,
    *,
    handoff_path: Path,
    root: Path,
    require_result_head: bool = False,
) -> list[str]:
    checker = _load_module(
        "quanta_check_proof_authority_handoff",
        root / PROOF_CHECKER_PATH.relative_to(ROOT),
    )
    return CHAIN_VALIDATOR.validate_handoff(
        payload,
        handoff_path=handoff_path,
        root=root,
        proof_checker=checker,
        require_result_head=require_result_head,
    )


def validate_product_handoff_directory(*, directory: Path, root: Path) -> list[str]:
    """Validate each historical receipt, then its fixed fork/join/serial graph."""
    payloads: list[dict[str, Any]] = []
    errors: list[str] = []
    for lane in CHAIN_VALIDATOR.PRODUCT_LANES:
        path = directory / f"{lane}.json"
        try:
            payload = _read_json(path)
        except (OSError, ValueError, json.JSONDecodeError) as error:
            errors.append(f"{path}: unreadable handoff: {error}")
            continue
        findings = validate_handoff(
            payload, handoff_path=path, root=root, require_result_head=False
        )
        errors.extend(f"{path}: {finding}" for finding in findings)
        if isinstance(payload, dict):
            payloads.append(payload)
    if len(payloads) == len(CHAIN_VALIDATOR.PRODUCT_LANES):
        errors.extend(CHAIN_VALIDATOR.validate_product_handoff_chain(payloads))
    return errors


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("handoff", type=Path)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--require-result-head", action="store_true")
    parser.add_argument("--product-chain", action="store_true", help="handoff is a directory")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    root = args.root.resolve()
    handoff_path = args.handoff if args.handoff.is_absolute() else root / args.handoff
    try:
        if args.product_chain:
            if args.require_result_head:
                raise ValueError("--product-chain is historical and cannot require result HEAD")
            errors = validate_product_handoff_directory(directory=handoff_path, root=root)
        else:
            payload = _read_json(handoff_path)
            errors = validate_handoff(
                payload,
                handoff_path=handoff_path,
                root=root,
                require_result_head=args.require_result_head,
            )
    except (OSError, ValueError, json.JSONDecodeError, tomllib.TOMLDecodeError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2
    if errors:
        for error in errors:
            print(f"ERROR: {handoff_path}: {error}", file=sys.stderr)
        return 1
    print(f"OK: {handoff_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
