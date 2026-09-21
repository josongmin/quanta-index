#!/usr/bin/env python3
"""Structural guard for the S21-05 read-view cutover (SEP-21 P04).

Two invariants, checked statically so they cannot regress silently:

1. Zero ambient lookup after view acquisition: a query route may reach a
   backend only through the view's accessors. The routes directory must
   not name the dispatcher's registries, openers, the RepoMap acquire
   port, or the ledger; those are acquisition-time collaborators that
   live in ``read_view/`` only.
2. Zero V1 live path: the production sources must not name the retired
   ``QueryReadViewV1`` / ``ReadIdentityV1`` / ``RepoMapQueryPort`` /
   ``read_query_snapshot`` ambient surfaces anywhere in the workspace.
"""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
ROUTES = ROOT / "crates/quanta-index-search-plane/src/query_dispatcher/routes"
SRC = ROOT / "crates"

ROUTE_FORBIDDEN = (
    "self.repo_map_snapshots",
    "self.snapshots",
    "self.lex_opener",
    "self.sem_opener",
    "self.ledger",
    "self.history_text",
)

WORKSPACE_FORBIDDEN = (
    "QueryReadViewV1",
    "ReadIdentityV1",
    "RepoMapQueryPort",
    "read_query_snapshot",
)


def fail(messages: list[str]) -> int:
    for message in messages:
        print(f"check-read-view-ambient-lookup: {message}", file=sys.stderr)
    print(
        f"check-read-view-ambient-lookup: {len(messages)} violation(s); "
        "routes must execute only through the read view and the V1 "
        "ambient surfaces must stay deleted",
        file=sys.stderr,
    )
    return 1


def main() -> int:
    violations: list[str] = []
    for path in sorted(ROUTES.rglob("*.rs")):
        text = path.read_text(encoding="utf-8")
        for token in ROUTE_FORBIDDEN:
            if token in text:
                violations.append(f"{path.relative_to(ROOT)} names {token}")
    for path in sorted(SRC.glob("*/src/**/*.rs")):
        text = path.read_text(encoding="utf-8")
        for token in WORKSPACE_FORBIDDEN:
            if token in text:
                violations.append(f"{path.relative_to(ROOT)} names retired surface {token}")
    if violations:
        return fail(violations)
    print("check-read-view-ambient-lookup: ok (routes view-only, V1 surfaces absent)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
