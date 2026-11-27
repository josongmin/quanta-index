# Product-owned SQLite Unix syscall custody

## Protected Catalog reads on Linux

`with_protected_read` opens the main image and optional WAL relative to a
caller-held directory FD. It validates the preflight identities, takes an
exclusive OFD lock across SQLite's main-file locking bytes, and holds the lock
through SQLite close and input revalidation. The named VFS exposes only the
pinned image descriptors. WAL index recovery uses SQLite heap memory. The VFS
refuses source changes, SHM files, and extension loading.

The Linux Catalog path calls this API under the Runtime's StateRoot lease and
FD admission. The standalone oracle links registry `libsqlite3-sys 0.39.1`.
The application still selects its SQLite supplier through the root Cargo lock.

## Experimental Unix mutation hooks

The standalone oracle links registry `libsqlite3-sys 0.39.1` and installs
`open`/`close` callbacks through SQLite's public Unix VFS v3 syscall API. An
admitted open runs under the product FD ledger lock, calls an authority's
rooted opener, and records the real `fstat` device/inode. A physical close
runs under the same lock. Its named VFS wrapper captures FDs opened by each
lower `xOpen` and releases their logical owners when the corresponding lower
`xClose` succeeds, including a failed `xOpen` that leaves `pMethods` set.
For WAL SHM, the first `xShmMap` captures the physical FD; further connections
to the same rooted DB inode retain distinct ledger owners. Successful
`xShmUnmap` or final `xClose` releases each owner after the lower call.
Successful `xShmLock` calls maintain per-connection shared/exclusive masks.
An active lock or failed lower unmap prevents immediate refund; a failed
lower close quarantines the charge.
Physical and logical retirement together issue a one-shot receipt. A failed
lower `xClose` retains the charge for reconciliation.

Installation is process-global. The `ExclusiveProcessPermit` constructor is
unsafe because code cannot discover other SQLite users or prove that no prior
syscall override exists. The caller must launch a dedicated SQLite process,
install before any SQLite connection or concurrent worker, and classify all
open paths. The installer rejects concurrent/repeated attempts; a rejected
setter restores both original callbacks or aborts if restoration fails.

The mutation-hook experiment has no production caller. Root token and charge
admission, SHM growth quota and failure recovery,
retained ledger memory grant, Linux deployment quota, and multi-root isolation
remain release gates. The separate registry oracle exercises this crate with
a held root directory FD and synthetic charges.

The named VFS now requests an exact-layout retained grant before allocating a
fixed file-slot and SHM-node arrays and the VFS struct. Each file slot contains its method table
and at most eight `xOpen` FD tokens. A full file table refuses before lower
`xOpen`; token overflow closes the newly opened FD under ledger custody and
releases its initial owner. An uncertain close quarantines the charge. Nested
`xOpen` refuses before lower SQLite or the owner table. A failed lower `xClose`
quarantines its slot and charges for the process lifetime. There is no
per-open heap allocation in this wrapper.
Early refusal clears `sqlite3_file.pMethods`, so SQLite cannot call `xClose`
on stale file methods.
Failure after the process-global owner table is published rejects reinstall
and retains its grant until process exit.

The registry oracle uses a synthetic retained grant. An optional Runtime
adapter now holds real `MemoryLease` values from `MemoryScope` for the VFS
backing and its returned Arc. No production call site passes that grant to
this wrapper or activates the process-global hook. The FD ledger and
authority's own allocations need separate grants. This oracle does not prove
the Runtime admission contract. Before Runtime use, verify real reservation
composition, allocation failure, reconciliation, Linux quota, and concurrent
process behavior. Thread-local bookkeeping is two small cells per SQLite
thread; the process thread count must also have a bound.

Registry WAL coverage includes simultaneous reader and writer owners,
successful lock/unlock tracking, automatic last-owner SHM retirement, and a
denied SHM open followed by a successful retry. A failure after the SHM FD is
opened but before `xShmMap` completes, injected lower `xShmUnmap` failure,
inode collisions, and multi-root behavior remain unqualified.
