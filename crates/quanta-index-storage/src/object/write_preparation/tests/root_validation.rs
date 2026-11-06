//! Managed scalar and batch writes recheck their held namespace before physical IO.

use super::*;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

fn require(condition: bool, failure: &'static str) -> Result<(), IndexError> {
    if condition {
        Ok(())
    } else {
        Err(IndexError::BackendViolation(failure.into()))
    }
}

#[test]
fn managed_writes_refuse_changed_root_and_objects_before_physical_io()
-> Result<(), Box<dyn std::error::Error>> {
    for count in [1, 3] {
        for change_root in [false, true] {
            for change in ["mode", "rename", "symlink"] {
                let directory = tempfile::tempdir()?;
                let root = directory.path().join("store");
                let scratch = Arc::new(FileScratchSpace::open_standalone(
                    directory.path().join("scratch"),
                    1024 * 1024,
                )?);
                let authority: Arc<dyn ManagedObjectAuthority> = Arc::new(TraceAuthority {
                    fail_confirmation: false,
                    inputs: count,
                });
                let observer = Arc::new(ObjectIoObservation::default());
                let store = FileObjectStore::open_managed_with_scratch(&root, scratch, authority)?
                    .with_io_observation(Arc::clone(&observer));
                let mut work = WorkBudget::with_request(
                    WorkLimits {
                        reads: 100,
                        writes: 100,
                        bytes: 1_000_000,
                        mutations: 100,
                    },
                    RequestBudgetV1::for_duration(Duration::from_secs(30)),
                );
                let payload = b"root-validation";
                let profile = store.identify(payload, &[], &mut work)?.reference;
                let workspace = WorkspaceKey {
                    tenant: TenantId([1; 16]),
                    collection: CollectionId([2; 16]),
                    worktree: WorktreeId([3; 16]),
                    slot: SlotId([4; 16]),
                };
                let scope = if count == 1 {
                    ObjectInstallScope::Bootstrap(BootstrapRef {
                        workspace,
                        epoch: 1,
                        fence: 1,
                        profile,
                    })
                } else {
                    ObjectInstallScope::Ingress(IngressRef {
                        operation: OperationKey {
                            workspace,
                            epoch: 1,
                            producer: ProducerId([5; 16]),
                            stream: StreamId([6; 16]),
                            stream_epoch: 1,
                            sequence: 2,
                        },
                        fence: 1,
                    })
                };
                let sink = ManagedSink(scope);
                let inputs =
                    [payload.as_slice(), b"second", b"third"].map(|payload| ObjectBatchInput {
                        payload: payload.to_vec(),
                        children: Vec::new(),
                    });
                let install = |budget: &mut WorkBudget| -> Result<(), IndexError> {
                    if count == 1 {
                        store.install(payload, &[], &sink, budget).map(|_object| ())
                    } else {
                        store
                            .install_batch(&inputs, &sink, budget)
                            .map(|_objects| ())
                    }
                };

                let changed = if change_root {
                    root.clone()
                } else {
                    root.join("objects")
                };
                let moved = if change_root {
                    directory.path().join("store-moved")
                } else {
                    root.join("objects-moved")
                };
                if change == "mode" {
                    fs::set_permissions(&changed, fs::Permissions::from_mode(0o755))?;
                } else {
                    fs::rename(&changed, &moved)?;
                    if change == "symlink" {
                        symlink(&moved, &changed)?;
                    }
                }

                let before = observer.snapshot()?;
                let refusal = install(&mut work);
                require(
                    matches!(refusal, Err(IndexError::Storage(_))),
                    "changed managed root or objects name was not refused as storage error",
                )?;
                let delta = observer.snapshot()?.checked_since(&before)?;
                require(
                    delta.reserve_calls == 1,
                    "managed reserve was not reached once",
                )?;
                require(
                    delta.confirm_calls == 0,
                    "invalid name reached confirmation",
                )?;
                require(delta.write_calls == 0, "invalid name reached object write")?;
                require(delta.file_sync_calls == 0, "invalid name reached file sync")?;
                require(
                    delta.directory_sync_calls == 0,
                    "invalid name reached directory sync",
                )?;
                require(delta.barrier_calls == 0, "invalid name reached barrier")?;
                let real_objects = if change_root && change != "mode" {
                    moved.join("objects")
                } else if !change_root && change != "mode" {
                    moved.clone()
                } else {
                    root.join("objects")
                };
                require(
                    fs::read_dir(&real_objects)?.count() == 0,
                    "invalid name created an object entry",
                )?;

                if change == "mode" {
                    fs::set_permissions(&changed, fs::Permissions::from_mode(0o700))?;
                } else {
                    if change == "symlink" {
                        fs::remove_file(&changed)?;
                    }
                    fs::rename(&moved, &changed)?;
                }
                install(&mut work)?;
                require(
                    fs::read_dir(root.join("objects"))?.count() > 0,
                    "restored managed name did not install an object",
                )?;
            }
        }
    }
    Ok(())
}
