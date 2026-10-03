# Daemon composition library

Application assembly, daemon configuration, supervision, and request
routing. The concrete executable target is in quanta-index-searchd-runtime.

Start with [public exports](src/lib.rs), [configuration](src/app/config.rs),
[supervision](src/app/supervisor.rs), and [IPC dispatch](src/app/ipc_dispatcher.rs).
Process and state-cutover rules are in the [SEP-21-004 ADR](../../docs/adr/SEP-21-004-process-supervision-state-cutover-and-proof.md).
