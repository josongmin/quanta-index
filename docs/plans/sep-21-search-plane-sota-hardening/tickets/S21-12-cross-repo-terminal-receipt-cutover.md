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

The existing cross-repo recipe now has an optional typed caller/kernel archive
under `QUANTA_P11_R5_EVIDENCE_ROOT`. Semantica nextest passes through CLI-owned
immutable completion custody; both required caller features are selected by
resolver/list/run, and the kernel retains its own required feature. Quanta18 and
Semantica25 owner tests passed on the integrated owned source. Actual clean-pair
build/test and daemon custody are `NOT_RUN`; this is a `runner-candidate-only`
component, not a staged P11 operational result. [OCT-04 I0-03](../../oct-4-parallel-closure/tickets/INDEX.md#o4-i0-03)
owns source/candidate/main and execution checkpoints. A subsequent source epoch
rechecks affected proof; frozen5796 SDK bytes do not imply current-pair binary parity.

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
The three operational recipes now use `tools/ci/operational_proof.py` and the
typed `operational-action` result path. The producer, manifest schema/checker
and aggregate bind pre/action/post execution, independent actor sources,
source pair, immutable prerequisites, daemon, host and target/config identity.
Source archives cannot hide dirty actor or contract code. Owner fixtures do
not qualify a Linux action.
The runner rejects inadmissible pre-state before invoking the action and checks
operational prerequisite target/config continuity before any actor. The manifest
checker also enforces that continuity across immutable dependency edges.

The registry entries remain staged. Concrete deployment/activation/
restore-forward adapters, independent observer contracts and authorized Linux
target/state/retention/rollback inputs are still missing. Recipes refuse before
mutation or output when staged; a shell exit zero or caller-written success
JSON cannot promote them. See the [operational contract](../../../operator/p11-operational-proof.md).
[S21-13](S21-13-release-evidence-and-sota-qualification.md) owns the final graph
and four separate verdicts.

## Stop conditions

A producer that has not adopted the terminal schema, a missing exact checkout,
unstable dependency roots, absent built binary/host, or unapproved deployment/
rollback leaves its dependent proof `BLOCKED` or `NOT_RUN`. Keep unaffected
owner-local work separate. Do not synthesize identity-only ACKs or receipts to
bypass external closure.
