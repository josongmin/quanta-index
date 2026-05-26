# MSTR-05 Observability and Perf / Chaos

Parent packet: [../README.md](../README.md)

## Objective

Close the bounded-label observability and remaining `E2E-07` performance /
chaos proof against the current source.

## Required Closeout

- normalize `classify_error_metric_name` coverage across live routes
- add bounded metrics proof for structural / bridge / history / runtime paths
- close the remaining perf / chaos rows that still lack current owner proof

## Guardrails

- keep the existing `MetricSample + Dimensions` contract
- keep dimensions exactly `{ticket_id,wave_id,tenant_id,repo_id,generation_id}`
- no raw query text, file path, or vector payload in metric names or dimensions
