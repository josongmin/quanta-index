# S21-12 — Exact-pair terminal receipt and cutover

Status: `ACTIVE — protocol and operational qualification remain staged`.

Depends on S21-02/04/07/11. The completed resolver, V2 receipt, replay and
activation decisions are in the [SEP-21 registry](../../../adr/SEP-21-DECISION-REGISTRY.md)
and [SEP-27-005](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md).
[R5/R6](FINAL-RESIDUAL-EXECUTION-PLAN.md) own remaining implementation order;
[the residual ledger](CURRENT-RESIDUAL-2026-09-26.md) owns status. Quanta-local
CAS behavior is not Semantica compatibility or a release receipt.

## Exact-pair entry and terminal chain

Freeze both repositories' HEAD/dirty digest, resolved dependency package roots
and locks, contract/SDK versions, public receipt fields, release daemon binary,
features/toolchain and target Linux host. Semantica must build against the same
coherent Quanta contract and SDK tree selected by the daemon. Recheck the pair
before qualification; a historical dirty producer snapshot is insufficient.

```text
producer source → prepared payload digest → canonical request/body digest
→ operation-journal terminal receipt → sealed candidate commitment
→ activation identity/epoch and prior-head CAS (if requested)
→ attested daemon and contract/SDK source identity → exact replay receipt
```

Each step binds its preceding identity. An identity-only ACK, optional digest
added to a V1 receipt or self-reported success cannot close this chain. Producer
preparation manifest, SDK request context, daemon publish/activate and Semantica
closeout must agree on the mandatory V2 commitment. Keep a single breaking
cutover; retired wire/receipt forms refuse before storage/provider mutation.

## Remaining acceptance

- Bind canonical resolver mapping and nested lock into typed paired receipt
  evidence. Run actual fresh producer/daemon builds and the selected positive/
  negative public SDK/daemon tests on one frozen pair, including publish-only,
  publish+activate, restart/query and exact ACK replay. Archive the terminal
  build/test result, not only binary hashes or a mapping log.
- Compare exact payload, operation key/body, terminal sequence, sealed manifest/
  candidate, source-bundle and authority digests, activation prior/new head,
  epoch and CAS result. Replay the original terminal receipt without rebuilding,
  embedding or reactivating. Retention/window and rollback scope stay explicit.
- Refuse same identity/different payload, same generation/different candidate,
  missing/zero/reordered commitments, dirty or wrong dependency tree, wrong
  binary/source/features/host, unsupported legacy producer, stale prior head,
  wrong transition and replay divergence. Check old producer/new daemon and new
  producer/old daemon as typed incompatibility with zero mutation. Query-only
  and mutation-capable SDK profiles need separate compile/run evidence.
- Observe actual deploy, activation and restore-forward rollback on the
  authorized root/host after exact-pair protocol proof. Deployment freezes while
  cutover qualification is open. Do not infer an action receipt from a prior
  stage or a disposable state fixture.

| Proof ID | Required result | Prerequisite |
| --- | --- | --- |
| `p11-cross-repo-cutover` | Exact-pair protocol/wire/replay on attested daemon | P03/P02B/P06/P10; current registry additionally owns the complete DAG |
| `p11-deployment` | Installed binary/config/root observation | Cross-repo protocol |
| `p11-activation` | Serving generation/query observation | Deployment |
| `p11-rollback` | Actual restore-forward drill and receipt | Activation plus P10 state proof |

Run the registered cross-repo command through the
[execution entrypoint](prompts/README.md) only after required inputs are frozen.
The three operational recipe names are staged registry entries, not working
commands. They require independent typed action producers and manifests before
promotion. [S21-13](S21-13-release-evidence-and-sota-qualification.md) owns the
final graph and four separate verdicts.

## Stop conditions

A producer that has not adopted the terminal schema, a missing exact checkout,
unstable dependency roots, absent built binary/host, or unapproved deployment/
rollback leaves its dependent proof `BLOCKED` or `NOT_RUN`. Keep unaffected
owner-local work separate. Do not synthesize identity-only ACKs or receipts to
bypass external closure.
