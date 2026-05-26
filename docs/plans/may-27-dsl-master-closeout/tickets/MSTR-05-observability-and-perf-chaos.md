# MSTR-05 Observability and Perf / Chaos

Parent packet: [../README.md](../README.md)

## Objective

Close the bounded-label observability and remaining `E2E-07` performance /
chaos proof against the current source.

## Required Closeout

- normalize `classify_error_metric_name` coverage across live routes
- add bounded metrics proof for structural / bridge / history / runtime paths
- close the remaining perf / chaos rows that still lack current owner proof

## Current Source Truth

Closed in current source:

- `classify_error_metric_name` covers parse / invalid / not-ready / unavailable /
  plan-limit / internal / other under the bounded name taxonomy
- owner-unit proof exists for structural / bridge / history / runtime route
  metrics
- runtime E2E proof exists for hybrid, structural, bridge, history, and runtime
  metadata routes with closed dimensions and no query-text leakage
- history ref-shard, tag-shard, structural shard, and history diff-shard
  unavailable rows now have runtime proof under the bounded unavailable metric
  bucket
- lexical regex timeout now lowers from native and Sourcegraph `timeout:`
  surface into canonical query options, fails closed as `QUERY_TIMEOUT`, and
  records under the bounded plan-limit metric bucket without poisoning the next
  query

Repo-local open residue:

- none under the current packet; wider multi-engine cancellation contract work
  would be a new scope, not an unclosed repo-local proof row

## Guardrails

- keep the existing `MetricSample + Dimensions` contract
- keep dimensions exactly `{ticket_id,wave_id,tenant_id,repo_id,generation_id}`
- no raw query text, file path, or vector payload in metric names or dimensions
