# L3 follow-up code audit

State: **VERIFIED for the two repairs and selected lexical tests only**.
HEAD: `98601a66d8cab9c86232b3e62ce490c8b43b71b6`, shared dirty `main`.
Selected source/input SHA256: `45f9f778344090b611e2393f554c9fbbd8376e703d12873379865069de4a922e`.
Receipt SHA256: `becb54bb15f38dfb527c337d9634c64c7f11317fa008a1d315b5351d64170d35` in `L3_FOLLOWUP.source.json`.

## Reproduced defects and repairs

1. An already cancelled/expired request could allocate the fruit buffer or build
   the query weight before checking interruption. A one-byte collection limit
   masked cancellation with a resource-budget error. `search_with_probe` now
   checks interruption first. The regression requires typed cancellation/deadline
   with zero weight/scorer opens and zero resource work/peak/resident bytes at
   both tight and ample byte limits.
2. Numeric order fields accepted their first value even when a malformed index
   document held multiple `start_line`/`end_line` values. Both fields now require
   exactly one value. Real malformed Tantivy fixtures assert Storage refusal
   without advancing in both ranked and grouped collectors. This is stored-index
   integrity hardening, not a demonstrated normal-ingestion failure.

Owned changes in this continuation are confined to `budgeted_search.rs`,
`ranked_page.rs` and `ranked_page_tests.rs`. Other shared edits were preserved.
The cancellation tick comment now states scorer-action granularity; no inner
engine wall-clock bound is asserted.

## Verification

Environment: `QUANTA_INDEX_RESOURCE_ADMISSION=1 CARGO_BUILD_JOBS=2`.

```sh
./scripts/cargow --lane test-fast-lane test -p quanta-index-lexical --lib --test execution_budget --test ranked_pages --test l3_exact_source --test cancellation_inside_search --locked
```

Result: **188 passed, 0 failed, 0 ignored**: library 170,
`cancellation_inside_search` 3, `execution_budget` 4, `l3_exact_source` 6,
`ranked_pages` 5. Run: `audit2-related-5`. Source baseline:
last snapshot completed while the admission marker was still absent. Relevant source was stable from that baseline before
Cargo execution through the final snapshot. Front-door/configuration inputs
were stable from command launch, including queue admission. Any earlier changes
while queued are retained in the receipt and are not execution-window drift.
Raw log SHA256: `0282b35d203cadb001d0f02e2779079552eb4a8e2586b6f9b1ebf116f6cd1332`.

Formatting for the three owned files and `git diff --check` are **VERIFIED**.
The receipt includes complete dirty/source inventories, dependency closure,
environment/toolchain identity, exact commands, five binary digests and archived
raw outputs. Red reproductions are `audit2-cancel-red-2` and `audit2-lines-red`;
each executed one failing test before its repair. Earlier compile failures and
source-drifted passing runs remain historical diagnostics and are not counted.

SDK/CLI on the concurrently changed contract, full repository CI, installed
daemon E2E, request-wide memory/RSS and ranking/latency performance are **NOT_RUN**.
The previous 373-test result belongs to an earlier snapshot; its original
handoff and receipt are archived under `l3-proof/followup/prior-L3_HANDOFF.*`.
No inter-task messages were read or sent. No commit/push/reset was performed.
