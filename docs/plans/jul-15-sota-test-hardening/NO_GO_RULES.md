# No-Go Rules

- Do not call a broad E2E, full-corpus, or TSan green result owner-local proof.
- Do not accept mock-only durability proof; crash consistency requires actual
  child termination at the claimed boundary.
- Do not accept random tests without recorded seed, shrink/minimization path,
  and reproduction command.
- Do not use a coverage percentage as a correctness completion condition.
- Do not promote a mutation baseline update and behavior change together
  without an independently reviewed survivor explanation.
- Do not retry a flaky rail into green. Preserve the failing receipt and assign
  owner, cadence, and expiry for any temporary exception.
- Do not add `#[ignore]` without an entry in the ignored-test policy including
  owner, cadence, expiry, and reason.
- Do not let nightly-only evidence close a P0 regression that can run in the PR
  budget.
- Do not claim SDK black-box conformance when a test calls internal control APIs.
- Do not claim cross-repo ingress, live-provider behavior, or production-scale
  performance from repo-local Linux CI.
- Do not update an authoritative catalog, scenario fixture, baseline, or receipt
  schema without its guard and a negative test.
- Do not collapse `implemented`, `in-flight`, `planned`, and `external` status.
