//! Pure, bounded physical inode-capacity accounting for publications.
//!
//! This module performs no filesystem I/O. A service binder must authenticate
//! inode identities and widths, creation outcomes, last-link removal, and the
//! affected parent-directory sync before calling a terminal transition.
//! Unlinked-but-open files and replaced old inodes remain charged until their
//! final physical lifetime is proved over. Boot inventory must include them
//! when observable; this ledger alone cannot discover hidden live inodes.
//! Bytes mean logical regular-file length, not allocated filesystem blocks.
//! The limits do not guarantee free space, process RSS, or I/O headroom.

use std::collections::BTreeMap;
use std::ops::Bound::{Excluded, Unbounded};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use quanta_index_contract::SearchPlaneErrorCodeV2;

use crate::CoreError;

static NEXT_AUTHORITY_ID: AtomicU64 = AtomicU64::new(1);

/// Process-local identity. Tokens are not serialized across a restart; boot
/// inventory creates fresh tokens for every inode in the new authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AuthorityId(u64);

impl AuthorityId {
    fn matches(self, other: Self) -> bool {
        self.0 == other.0
    }
}

fn next_authority_id() -> Result<AuthorityId, CoreError> {
    let mut current = NEXT_AUTHORITY_ID.load(Ordering::Relaxed);
    loop {
        let next = current.checked_add(1).ok_or_else(|| {
            CoreError::InvalidContract(format!(
                "publication capacity authority IDs exhausted at {current}"
            ))
        })?;
        match NEXT_AUTHORITY_ID.compare_exchange_weak(
            current,
            next,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return Ok(AuthorityId(current)),
            Err(observed) => current = observed,
        }
    }
}

/// Physical identity within one boot inventory and filesystem incarnation.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct InodeKey {
    pub device: u64,
    pub inode: u64,
}

/// Opaque authority-bound incarnation of a retained inode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetainedId {
    authority: AuthorityId,
    inode: InodeKey,
    charge_id: u64,
}

impl RetainedId {
    #[must_use]
    pub const fn inode(self) -> InodeKey {
        self.inode
    }
}

/// Opaque process-local identity of a pending output within one authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingId {
    authority: AuthorityId,
    sequence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapacitySnapshot {
    pub used_bytes: u64,
    pub used_files: u64,
    pub retained_inodes: u64,
    pub active_allowances: u64,
    pub active_tickets: u64,
    pub uncertain_pending: u64,
    pub poisoned: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingCharge {
    pub ticket_id: PendingId,
    pub reserved_bytes: u64,
    pub kind: PendingChargeKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingChargeKind {
    Create,
    Resize {
        base: RetainedId,
        base_bytes: u64,
        max_new_bytes: u64,
    },
}

struct Retained {
    bytes: u64,
    charge_id: u64,
    allowance_id: Option<u64>,
    busy_resize: Option<u64>,
}

#[derive(Clone, Copy)]
struct AllowanceState {
    free_bytes: u64,
    free_files: u64,
    children: u64,
    retained: u64,
    closing: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TicketPhase {
    Reserved,
    MutationStarted,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TicketKind {
    Create,
    Resize {
        inode: InodeKey,
        base_charge_id: u64,
        base_bytes: u64,
        max_new_bytes: u64,
    },
}

#[derive(Clone, Copy)]
struct TicketState {
    allowance_id: u64,
    reserved_bytes: u64,
    phase: TicketPhase,
    orphaned: bool,
    kind: TicketKind,
}

struct State {
    authority: AuthorityId,
    max_bytes: u64,
    max_files: u64,
    max_pending: u64,
    used_bytes: u64,
    used_files: u64,
    uncertain_pending: u64,
    active_allowances: u64,
    next_id: u64,
    sealed: bool,
    poison: Option<String>,
    // Retained <= max_files; tickets <= max_pending. Active allowances <=
    // max_pending. A closed allowance survives only while one of its bounded
    // tickets or retained inodes still refers to it, so the map is bounded by
    // max_files + 2*max_pending without scanning all retained roots on Drop.
    retained: BTreeMap<InodeKey, Retained>,
    allowances: BTreeMap<u64, AllowanceState>,
    tickets: BTreeMap<u64, TicketState>,
}

fn capacity(detail: &str) -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
        message: format!("publication physical capacity: {detail}"),
    }
}

fn invalid(state: &mut State, detail: &str) -> CoreError {
    state.poison = Some(detail.to_owned());
    CoreError::Storage(format!("publication physical capacity poisoned: {detail}"))
}

fn healthy(state: &State) -> Result<(), CoreError> {
    state.poison.as_ref().map_or(Ok(()), |reason| {
        Err(CoreError::Storage(format!(
            "publication physical capacity is poisoned: {reason}"
        )))
    })
}

fn lock(shared: &Arc<Mutex<State>>) -> Result<MutexGuard<'_, State>, CoreError> {
    shared.lock().map_err(|error| {
        CoreError::Storage(format!(
            "publication physical capacity mutex is poisoned: {error}"
        ))
    })
}

fn next_id(state: &mut State) -> Result<u64, CoreError> {
    let id = state.next_id;
    let Some(next) = id.checked_add(1) else {
        return Err(invalid(state, "identity sequence overflow"));
    };
    state.next_id = next;
    Ok(id)
}

fn room(state: &State, bytes: u64, files: u64) -> Result<(u64, u64), CoreError> {
    let used_bytes = state
        .used_bytes
        .checked_add(bytes)
        .ok_or_else(|| capacity("byte charge overflow"))?;
    let used_files = state
        .used_files
        .checked_add(files)
        .ok_or_else(|| capacity("file charge overflow"))?;
    if used_bytes > state.max_bytes || used_files > state.max_files {
        return Err(capacity("configured byte or unique-file ceiling exceeded"));
    }
    Ok((used_bytes, used_files))
}

fn release_child(state: &mut State, ticket: &TicketState) -> Result<(), CoreError> {
    let Some(allowance) = state.allowances.get(&ticket.allowance_id) else {
        return Err(invalid(state, "child refers to missing parent allowance"));
    };
    if allowance.children == 0 {
        return Err(invalid(state, "parent child count underflow"));
    }
    let Some(children) = allowance.children.checked_sub(1) else {
        return Err(invalid(state, "parent child count underflow"));
    };
    let closing = allowance.closing;
    let adjusted = if closing {
        state.used_bytes.checked_sub(ticket.reserved_bytes).zip(
            state
                .used_files
                .checked_sub(u64::from(matches!(ticket.kind, TicketKind::Create))),
        )
    } else {
        allowance.free_bytes.checked_add(ticket.reserved_bytes).zip(
            allowance
                .free_files
                .checked_add(u64::from(matches!(ticket.kind, TicketKind::Create))),
        )
    };
    let Some((bytes, files)) = adjusted else {
        return Err(invalid(state, "child refund arithmetic failed"));
    };
    let Some(allowance) = state.allowances.get_mut(&ticket.allowance_id) else {
        return Err(invalid(state, "child parent disappeared"));
    };
    allowance.children = children;
    if closing {
        state.used_bytes = bytes;
        state.used_files = files;
    } else {
        allowance.free_bytes = bytes;
        allowance.free_files = files;
    }
    prune_allowance(state, ticket.allowance_id);
    Ok(())
}

fn commit_started(
    state: &mut State,
    ticket_id: u64,
    inode: InodeKey,
    actual_bytes: u64,
) -> Result<RetainedId, CoreError> {
    let Some(ticket) = state.tickets.get(&ticket_id) else {
        return Err(invalid(state, "commit ticket missing"));
    };
    if ticket.kind != TicketKind::Create || ticket.phase != TicketPhase::MutationStarted {
        return Err(invalid(state, "commit before creation"));
    }
    if actual_bytes > ticket.reserved_bytes {
        return Err(invalid(state, "actual output exceeds admitted upper bound"));
    }
    if state.retained.contains_key(&inode) {
        return Err(invalid(state, "committed inode already retained"));
    }
    let allowance_id = ticket.allowance_id;
    let Some(slack) = ticket.reserved_bytes.checked_sub(actual_bytes) else {
        return Err(invalid(state, "committed byte difference underflow"));
    };
    let Some(parent) = state.allowances.get(&allowance_id) else {
        return Err(invalid(state, "commit parent missing"));
    };
    let Some(children) = parent.children.checked_sub(1) else {
        return Err(invalid(state, "commit counts invalid"));
    };
    let Some(retained_count) = parent.retained.checked_add(1) else {
        return Err(invalid(state, "commit retained count overflow"));
    };
    let Some(pending_count) = state.uncertain_pending.checked_sub(1) else {
        return Err(invalid(state, "commit pending count underflow"));
    };
    let closing = parent.closing;
    let adjusted_bytes = if closing {
        state.used_bytes.checked_sub(slack)
    } else {
        parent.free_bytes.checked_add(slack)
    };
    let Some(adjusted_bytes) = adjusted_bytes else {
        return Err(invalid(state, "commit slack arithmetic failed"));
    };
    let id = next_id(state)?;
    let Some(ticket) = state.tickets.remove(&ticket_id) else {
        return Err(invalid(state, "commit ticket disappeared"));
    };
    let Some(parent) = state.allowances.get_mut(&ticket.allowance_id) else {
        return Err(invalid(state, "commit parent disappeared"));
    };
    parent.children = children;
    parent.retained = retained_count;
    if closing {
        state.used_bytes = adjusted_bytes;
    } else {
        parent.free_bytes = adjusted_bytes;
    }
    state.uncertain_pending = pending_count;
    let _old = state.retained.insert(
        inode,
        Retained {
            bytes: actual_bytes,
            charge_id: id,
            allowance_id: Some(ticket.allowance_id),
            busy_resize: None,
        },
    );
    Ok(RetainedId {
        authority: state.authority,
        inode,
        charge_id: id,
    })
}

fn resize_source(
    state: &mut State,
    ticket_id: u64,
) -> Result<(TicketState, InodeKey, u64, u64), CoreError> {
    let Some(ticket) = state.tickets.get(&ticket_id).copied() else {
        return Err(invalid(state, "resize ticket missing"));
    };
    let TicketKind::Resize {
        inode,
        base_charge_id,
        base_bytes,
        max_new_bytes,
    } = ticket.kind
    else {
        return Err(invalid(state, "create ticket used for resize"));
    };
    let Some(retained) = state.retained.get(&inode) else {
        return Err(invalid(state, "resize source inode missing"));
    };
    if retained.charge_id != base_charge_id
        || retained.bytes != base_bytes
        || retained.busy_resize != Some(ticket_id)
    {
        return Err(invalid(state, "resize source identity or width changed"));
    }
    Ok((ticket, inode, base_bytes, max_new_bytes))
}

fn settle_resize_started(
    state: &mut State,
    ticket_id: u64,
    inode: InodeKey,
    actual_bytes: u64,
    require_orphan: bool,
) -> Result<RetainedId, CoreError> {
    let (ticket, expected_inode, base_bytes, max_new_bytes) = resize_source(state, ticket_id)?;
    if ticket.phase != TicketPhase::MutationStarted || ticket.orphaned != require_orphan {
        return Err(invalid(
            state,
            "resize settlement ownership or phase differs",
        ));
    }
    // A failed or partial shrink may retain the already charged base width.
    // Only growth beyond both the base and admitted output needs more capacity.
    if inode != expected_inode || actual_bytes > max_new_bytes.max(base_bytes) {
        return Err(invalid(
            state,
            "resize inode or final width exceeds admitted bound",
        ));
    }
    let actual_growth = actual_bytes.saturating_sub(base_bytes);
    let Some(slack) = ticket.reserved_bytes.checked_sub(actual_growth) else {
        return Err(invalid(state, "resize growth exceeds held reservation"));
    };
    let shrink = base_bytes.saturating_sub(actual_bytes);
    let Some(new_parent) = state.allowances.get(&ticket.allowance_id).copied() else {
        return Err(invalid(state, "resize parent missing"));
    };
    let Some(children) = new_parent.children.checked_sub(1) else {
        return Err(invalid(state, "resize child count underflow"));
    };
    let adjusted_free = if new_parent.closing {
        new_parent.free_bytes
    } else {
        new_parent
            .free_bytes
            .checked_add(slack)
            .ok_or_else(|| invalid(state, "resize parent refund overflow"))?
    };
    let Some(retained) = state.retained.get(&inode) else {
        return Err(invalid(state, "resize retained inode disappeared"));
    };
    let old_owner = retained.allowance_id;
    // The same live parent still owns both the inode and its growth. A
    // resize must not detach that custody or refund a shrink globally while
    // the parent is promised as a reusable, already charged allowance.
    let keep_owner = old_owner == Some(ticket.allowance_id) && !new_parent.closing;
    let adjusted_free = if keep_owner {
        adjusted_free
            .checked_add(shrink)
            .ok_or_else(|| invalid(state, "same-parent shrink refund overflow"))?
    } else {
        adjusted_free
    };
    let old_retained = if let Some(old_id) = old_owner {
        let Some(old) = state.allowances.get(&old_id) else {
            return Err(invalid(state, "resize old parent missing"));
        };
        Some(
            old.retained
                .checked_sub(1)
                .ok_or_else(|| invalid(state, "resize old retained underflow"))?,
        )
    } else {
        None
    };
    let Some(pending) = state.uncertain_pending.checked_sub(1) else {
        return Err(invalid(state, "resize pending count underflow"));
    };
    let total_refund = (if keep_owner { 0 } else { shrink })
        .checked_add(if new_parent.closing { slack } else { 0 })
        .ok_or_else(|| invalid(state, "resize total refund overflow"))?;
    let Some(used_bytes) = state.used_bytes.checked_sub(total_refund) else {
        return Err(invalid(state, "resize global byte underflow"));
    };
    let charge_id = next_id(state)?;
    let Some(_ticket) = state.tickets.remove(&ticket_id) else {
        return Err(invalid(state, "resize ticket disappeared"));
    };
    let Some(retained) = state.retained.get_mut(&inode) else {
        return Err(invalid(
            state,
            "resize retained disappeared after validation",
        ));
    };
    retained.bytes = actual_bytes;
    retained.charge_id = charge_id;
    retained.allowance_id = keep_owner.then_some(ticket.allowance_id);
    retained.busy_resize = None;
    let Some(parent) = state.allowances.get_mut(&ticket.allowance_id) else {
        return Err(invalid(state, "resize parent disappeared after validation"));
    };
    parent.children = children;
    parent.free_bytes = adjusted_free;
    if !keep_owner && let (Some(old_id), Some(old_count)) = (old_owner, old_retained) {
        let Some(old) = state.allowances.get_mut(&old_id) else {
            return Err(invalid(
                state,
                "resize old parent disappeared after validation",
            ));
        };
        old.retained = old_count;
    }
    state.used_bytes = used_bytes;
    state.uncertain_pending = pending;
    prune_allowance(state, ticket.allowance_id);
    if let Some(old_id) = old_owner {
        prune_allowance(state, old_id);
    }
    Ok(RetainedId {
        authority: state.authority,
        inode,
        charge_id,
    })
}

fn remove_resize_started(
    state: &mut State,
    ticket_id: u64,
    require_orphan: bool,
) -> Result<(), CoreError> {
    let (ticket, inode, base_bytes, _max_new_bytes) = resize_source(state, ticket_id)?;
    if ticket.phase != TicketPhase::MutationStarted || ticket.orphaned != require_orphan {
        return Err(invalid(
            state,
            "resize final removal ownership or phase differs",
        ));
    }
    let Some(old_owner) = state.retained.get(&inode).map(|row| row.allowance_id) else {
        return Err(invalid(state, "resize final removal source missing"));
    };
    let mut parents = BTreeMap::<u64, AllowanceState>::new();
    for id in [old_owner, Some(ticket.allowance_id)].into_iter().flatten() {
        let Some(parent) = state.allowances.get(&id).copied() else {
            return Err(invalid(state, "resize final removal parent missing"));
        };
        let _prior = parents.insert(id, parent);
    }
    let mut global_bytes = state.used_bytes;
    let mut global_files = state.used_files;
    if let Some(id) = old_owner {
        let Some(parent) = parents.get_mut(&id) else {
            return Err(invalid(state, "resize old parent update missing"));
        };
        parent.retained = parent
            .retained
            .checked_sub(1)
            .ok_or_else(|| invalid(state, "resize old retained count underflow"))?;
        if parent.closing {
            global_bytes = global_bytes
                .checked_sub(base_bytes)
                .ok_or_else(|| invalid(state, "resize old global byte underflow"))?;
            global_files = global_files
                .checked_sub(1)
                .ok_or_else(|| invalid(state, "resize old global file underflow"))?;
        } else {
            parent.free_bytes = parent
                .free_bytes
                .checked_add(base_bytes)
                .ok_or_else(|| invalid(state, "resize old parent byte overflow"))?;
            parent.free_files = parent
                .free_files
                .checked_add(1)
                .ok_or_else(|| invalid(state, "resize old parent file overflow"))?;
        }
    } else {
        global_bytes = global_bytes
            .checked_sub(base_bytes)
            .ok_or_else(|| invalid(state, "resize ownerless byte underflow"))?;
        global_files = global_files
            .checked_sub(1)
            .ok_or_else(|| invalid(state, "resize ownerless file underflow"))?;
    }
    let Some(parent) = parents.get_mut(&ticket.allowance_id) else {
        return Err(invalid(state, "resize new parent update missing"));
    };
    parent.children = parent
        .children
        .checked_sub(1)
        .ok_or_else(|| invalid(state, "resize new child count underflow"))?;
    if parent.closing {
        global_bytes = global_bytes
            .checked_sub(ticket.reserved_bytes)
            .ok_or_else(|| invalid(state, "resize growth global refund underflow"))?;
    } else {
        parent.free_bytes = parent
            .free_bytes
            .checked_add(ticket.reserved_bytes)
            .ok_or_else(|| invalid(state, "resize growth parent refund overflow"))?;
    }
    let pending = state
        .uncertain_pending
        .checked_sub(1)
        .ok_or_else(|| invalid(state, "resize final removal pending underflow"))?;
    let Some(_ticket) = state.tickets.remove(&ticket_id) else {
        return Err(invalid(state, "resize final removal ticket disappeared"));
    };
    let Some(_retained) = state.retained.remove(&inode) else {
        return Err(invalid(state, "resize final removal inode disappeared"));
    };
    for (id, adjusted) in parents {
        let Some(parent) = state.allowances.get_mut(&id) else {
            return Err(invalid(state, "resize final removal parent disappeared"));
        };
        *parent = adjusted;
        prune_allowance(state, id);
    }
    state.used_bytes = global_bytes;
    state.used_files = global_files;
    state.uncertain_pending = pending;
    Ok(())
}

fn prune_allowance(state: &mut State, id: u64) {
    if state
        .allowances
        .get(&id)
        .is_some_and(|entry| entry.closing && entry.children == 0 && entry.retained == 0)
    {
        let _removed = state.allowances.remove(&id);
    }
}

/// Boot-only inventory recorder. It deliberately cannot create a live permit.
pub struct PublicationCapacityLedger {
    state: State,
}

impl PublicationCapacityLedger {
    pub fn new(max_bytes: u64, max_files: u64, max_pending: u64) -> Result<Self, CoreError> {
        if max_bytes == 0 || max_files == 0 || max_pending == 0 {
            return Err(CoreError::InvalidContract(
                "publication capacity ceilings must be positive".into(),
            ));
        }
        let _pending_limit = usize::try_from(max_pending).map_err(|error| {
            CoreError::InvalidContract(format!(
                "publication pending entry ceiling does not fit this host: {error}"
            ))
        })?;
        let authority = next_authority_id()?;
        Ok(Self {
            state: State {
                authority,
                max_bytes,
                max_files,
                max_pending,
                used_bytes: 0,
                used_files: 0,
                uncertain_pending: 0,
                active_allowances: 0,
                next_id: 1,
                sealed: false,
                poison: None,
                retained: BTreeMap::new(),
                allowances: BTreeMap::new(),
                tickets: BTreeMap::new(),
            },
        })
    }

    /// Record one observed regular file; hardlinked paths share one charge.
    pub fn seed_inode(&mut self, inode: InodeKey, bytes: u64) -> Result<RetainedId, CoreError> {
        let state = &mut self.state;
        healthy(state)?;
        if let Some(existing) = state.retained.get(&inode) {
            if existing.bytes != bytes {
                return Err(invalid(state, "same boot inode has conflicting widths"));
            }
            return Ok(RetainedId {
                authority: state.authority,
                inode,
                charge_id: existing.charge_id,
            });
        }
        let (used_bytes, used_files) = match room(state, bytes, 1) {
            Ok(charged) => charged,
            Err(error) => {
                state.poison = Some("boot inventory exceeded configured capacity".into());
                return Err(error);
            }
        };
        let id = next_id(state)?;
        let _old = state.retained.insert(
            inode,
            Retained {
                bytes,
                charge_id: id,
                allowance_id: None,
                busy_resize: None,
            },
        );
        state.used_bytes = used_bytes;
        state.used_files = used_files;
        Ok(RetainedId {
            authority: state.authority,
            inode,
            charge_id: id,
        })
    }

    pub fn seal_boot(mut self) -> Result<PublicationCapacity, CoreError> {
        healthy(&self.state)?;
        self.state.sealed = true;
        Ok(PublicationCapacity {
            shared: Arc::new(Mutex::new(self.state)),
        })
    }
}

/// Cloneable service authority after the exact boot inventory was sealed.
#[derive(Clone)]
pub struct PublicationCapacity {
    shared: Arc<Mutex<State>>,
}

impl PublicationCapacity {
    /// Look up a charged inode after the binder authenticates its opened FD.
    /// No path or filesystem identity is inferred by this pure ledger.
    pub fn retained_inode(
        &self,
        inode: InodeKey,
        observed_bytes: u64,
    ) -> Result<Option<RetainedId>, CoreError> {
        let mut state = lock(&self.shared)?;
        healthy(&state)?;
        let result = if let Some(retained) = state.retained.get(&inode) {
            if retained.busy_resize.is_some() {
                return Err(capacity("retained inode has an active resize"));
            }
            if retained.bytes != observed_bytes {
                return Err(invalid(&mut state, "retained lookup width differs"));
            }
            Some(RetainedId {
                authority: state.authority,
                inode,
                charge_id: retained.charge_id,
            })
        } else {
            None
        };
        drop(state);
        Ok(result)
    }

    pub fn reserve_publication(
        &self,
        max_bytes: u64,
        max_files: u64,
    ) -> Result<PublicationAllowance, CoreError> {
        let mut state = lock(&self.shared)?;
        healthy(&state)?;
        if !state.sealed {
            return Err(invalid(&mut state, "publication before boot seal"));
        }
        let (used_bytes, used_files) = room(&state, max_bytes, max_files)?;
        if state.active_allowances >= state.max_pending {
            return Err(capacity("active allowance entry ceiling exceeded"));
        }
        let active_allowances = state
            .active_allowances
            .checked_add(1)
            .ok_or_else(|| invalid(&mut state, "active allowance count overflow"))?;
        let id = next_id(&mut state)?;
        let _old = state.allowances.insert(
            id,
            AllowanceState {
                free_bytes: max_bytes,
                free_files: max_files,
                children: 0,
                retained: 0,
                closing: false,
            },
        );
        state.used_bytes = used_bytes;
        state.used_files = used_files;
        state.active_allowances = active_allowances;
        drop(state);
        Ok(PublicationAllowance {
            shared: Arc::clone(&self.shared),
            id,
        })
    }

    /// Binder calls only after all hardlinks and live owned handles have been
    /// released, and the affected parent directory has synced. A path unlink
    /// alone does not prove the physical inode's last reference is gone.
    pub fn confirmed_last_reference_removed_and_synced(
        &self,
        id: RetainedId,
        observed_bytes: u64,
    ) -> Result<(), CoreError> {
        let mut state = lock(&self.shared)?;
        healthy(&state)?;
        if !id.authority.matches(state.authority) {
            return Err(CoreError::InvalidContract(
                "retained inode belongs to another capacity authority".into(),
            ));
        }
        let Some(retained) = state.retained.get(&id.inode) else {
            return Err(invalid(&mut state, "deletion of unknown inode"));
        };
        if retained.busy_resize.is_some() {
            return Err(capacity("retained inode has an active resize"));
        }
        if retained.charge_id != id.charge_id || retained.bytes != observed_bytes {
            return Err(invalid(&mut state, "stale or conflicting inode deletion"));
        }
        let owner = retained.allowance_id;
        let owner_adjustment = if let Some(owner) = owner {
            let Some(allowance) = state.allowances.get(&owner) else {
                return Err(invalid(&mut state, "retained inode lost its allowance"));
            };
            let Some(remaining) = allowance.retained.checked_sub(1) else {
                return Err(invalid(&mut state, "retained owner count underflow"));
            };
            let adjusted_free = if allowance.closing {
                Some((allowance.free_bytes, allowance.free_files))
            } else {
                allowance
                    .free_bytes
                    .checked_add(observed_bytes)
                    .zip(allowance.free_files.checked_add(1))
            };
            let Some((free_bytes, free_files)) = adjusted_free else {
                return Err(invalid(&mut state, "retained owner refund overflow"));
            };
            Some((owner, remaining, allowance.closing, free_bytes, free_files))
        } else {
            None
        };
        let release_global = owner_adjustment
            .as_ref()
            .is_none_or(|(_, _, closing, _, _)| *closing);
        let adjusted_global = if release_global {
            state
                .used_bytes
                .checked_sub(observed_bytes)
                .zip(state.used_files.checked_sub(1))
        } else {
            Some((state.used_bytes, state.used_files))
        };
        let Some((used_bytes, used_files)) = adjusted_global else {
            return Err(invalid(&mut state, "retained capacity underflow"));
        };
        let _removed = state.retained.remove(&id.inode);
        if let Some((owner, remaining, closing, free_bytes, free_files)) = owner_adjustment {
            let Some(allowance) = state.allowances.get_mut(&owner) else {
                return Err(invalid(&mut state, "retained owner disappeared"));
            };
            allowance.retained = remaining;
            if !closing {
                allowance.free_bytes = free_bytes;
                allowance.free_files = free_files;
            }
            prune_allowance(&mut state, owner);
        }
        state.used_bytes = used_bytes;
        state.used_files = used_files;
        drop(state);
        Ok(())
    }

    pub fn confirmed_pending_absent_and_synced(
        &self,
        ticket_id: PendingId,
    ) -> Result<(), CoreError> {
        let mut state = lock(&self.shared)?;
        healthy(&state)?;
        if !ticket_id.authority.matches(state.authority) {
            return Err(CoreError::InvalidContract(
                "pending output belongs to another capacity authority".into(),
            ));
        }
        let Some(ticket) = state.tickets.get(&ticket_id.sequence) else {
            return Err(invalid(&mut state, "unknown pending ticket"));
        };
        if ticket.kind != TicketKind::Create
            || ticket.phase != TicketPhase::MutationStarted
            || !ticket.orphaned
        {
            return Err(invalid(&mut state, "pending absence out of order"));
        }
        let Some(pending) = state.uncertain_pending.checked_sub(1) else {
            return Err(invalid(&mut state, "pending absence count underflow"));
        };
        let Some(ticket) = state.tickets.remove(&ticket_id.sequence) else {
            return Err(invalid(&mut state, "pending ticket disappeared"));
        };
        state.uncertain_pending = pending;
        release_child(&mut state, &ticket)?;
        drop(state);
        Ok(())
    }

    /// Settle a dropped creation-started ticket after the binder proves the
    /// exact durable inode and width. This never creates a second charge.
    pub fn confirmed_pending_committed_and_synced(
        &self,
        ticket_id: PendingId,
        inode: InodeKey,
        actual_bytes: u64,
    ) -> Result<RetainedId, CoreError> {
        let mut state = lock(&self.shared)?;
        healthy(&state)?;
        if !ticket_id.authority.matches(state.authority) {
            return Err(CoreError::InvalidContract(
                "pending output belongs to another capacity authority".into(),
            ));
        }
        if !state
            .tickets
            .get(&ticket_id.sequence)
            .is_some_and(|ticket| {
                ticket.kind == TicketKind::Create
                    && ticket.phase == TicketPhase::MutationStarted
                    && ticket.orphaned
            })
        {
            return Err(invalid(
                &mut state,
                "pending commit requires dropped created ticket",
            ));
        }
        let retained = commit_started(&mut state, ticket_id.sequence, inode, actual_bytes)?;
        drop(state);
        Ok(retained)
    }

    /// Reconcile one dropped resize only after an anchored fstat and parent
    /// sync prove the durable final width of the original inode.
    pub fn confirmed_pending_resize_settled_and_synced(
        &self,
        ticket_id: PendingId,
        inode: InodeKey,
        actual_bytes: u64,
    ) -> Result<RetainedId, CoreError> {
        let mut state = lock(&self.shared)?;
        healthy(&state)?;
        if !ticket_id.authority.matches(state.authority) {
            return Err(CoreError::InvalidContract(
                "pending resize belongs to another capacity authority".into(),
            ));
        }
        let retained =
            settle_resize_started(&mut state, ticket_id.sequence, inode, actual_bytes, true)?;
        drop(state);
        Ok(retained)
    }

    /// Reconcile a failed resize only when every link and live owned handle
    /// of the original inode is gone and the affected parents are synced.
    pub fn confirmed_pending_resize_last_reference_removed_and_synced(
        &self,
        ticket_id: PendingId,
    ) -> Result<(), CoreError> {
        let mut state = lock(&self.shared)?;
        healthy(&state)?;
        if !ticket_id.authority.matches(state.authority) {
            return Err(CoreError::InvalidContract(
                "pending resize belongs to another capacity authority".into(),
            ));
        }
        remove_resize_started(&mut state, ticket_id.sequence, true)?;
        drop(state);
        Ok(())
    }

    /// Return at most `limit` uncertain charges; traversal is bounded by the
    /// explicit ticket ceiling, and no corpus-sized snapshot is allocated.
    pub fn pending_batch(
        &self,
        after: Option<PendingId>,
        limit: usize,
    ) -> Result<Vec<PendingCharge>, CoreError> {
        let state = lock(&self.shared)?;
        healthy(&state)?;
        if after.is_some_and(|cursor| !cursor.authority.matches(state.authority)) {
            return Err(CoreError::InvalidContract(
                "pending cursor belongs to another capacity authority".into(),
            ));
        }
        if limit == 0 {
            return Err(CoreError::InvalidContract(
                "pending batch limit must be positive".into(),
            ));
        }
        let cap = usize::try_from(state.max_pending)
            .map_err(|error| capacity(&format!("pending batch limit conversion: {error}")))?;
        let mut batch = Vec::new();
        let range = state.tickets.range((
            after.map_or(Unbounded, |cursor| Excluded(cursor.sequence)),
            Unbounded,
        ));
        for (id, ticket) in range {
            if ticket.phase == TicketPhase::MutationStarted && ticket.orphaned {
                let kind = match ticket.kind {
                    TicketKind::Create => PendingChargeKind::Create,
                    TicketKind::Resize {
                        inode,
                        base_charge_id,
                        base_bytes,
                        max_new_bytes,
                    } => PendingChargeKind::Resize {
                        base: RetainedId {
                            authority: state.authority,
                            inode,
                            charge_id: base_charge_id,
                        },
                        base_bytes,
                        max_new_bytes,
                    },
                };
                batch.push(PendingCharge {
                    ticket_id: PendingId {
                        authority: state.authority,
                        sequence: *id,
                    },
                    reserved_bytes: ticket.reserved_bytes,
                    kind,
                });
                if batch.len() == limit.min(cap) {
                    break;
                }
            }
        }
        drop(state);
        Ok(batch)
    }

    pub fn snapshot(&self) -> Result<CapacitySnapshot, CoreError> {
        let state = lock(&self.shared)?;
        let snapshot = CapacitySnapshot {
            used_bytes: state.used_bytes,
            used_files: state.used_files,
            retained_inodes: u64::try_from(state.retained.len())
                .map_err(|error| capacity(&format!("retained count conversion: {error}")))?,
            active_allowances: state.active_allowances,
            active_tickets: u64::try_from(state.tickets.len())
                .map_err(|error| capacity(&format!("ticket count conversion: {error}")))?,
            uncertain_pending: state.uncertain_pending,
            poisoned: state.poison.is_some(),
        };
        drop(state);
        Ok(snapshot)
    }
}

/// Parent charge. Dropping it refunds only unassigned capacity; committed or
/// uncertain children remain charged through their physical lifetime.
pub struct PublicationAllowance {
    shared: Arc<Mutex<State>>,
    id: u64,
}

impl PublicationAllowance {
    /// Unassigned capacity held by this parent, excluding live children/inodes.
    pub fn available_capacity(&self) -> Result<(u64, u64), CoreError> {
        let state = lock(&self.shared)?;
        healthy(&state)?;
        let parent = state.allowances.get(&self.id).ok_or_else(|| {
            CoreError::InvalidContract("allowance availability parent is missing".into())
        })?;
        if parent.closing {
            return Err(capacity("allowance availability parent is closed"));
        }
        let available = (parent.free_bytes, parent.free_files);
        drop(state);
        Ok(available)
    }

    /// Atomically restore an explicit unassigned reserve, or return its excess.
    /// Committed, started and retained child charges are never resized/refunded.
    /// Denial leaves both the parent and global counters unchanged.
    pub fn resize_available(&self, bytes: u64, files: u64) -> Result<(), CoreError> {
        self.resize_available_inner(bytes, files, false)
    }

    /// Restore as much unassigned capacity as the shared cap permits, returning
    /// excess above the target. A partial refill never prevents cleanup from
    /// using the remaining held window. Live child charges remain untouched.
    pub fn refill_available_up_to(&self, bytes: u64, files: u64) -> Result<(), CoreError> {
        self.resize_available_inner(bytes, files, true)
    }

    fn resize_available_inner(
        &self,
        bytes: u64,
        files: u64,
        partial: bool,
    ) -> Result<(), CoreError> {
        let mut state = lock(&self.shared)?;
        healthy(&state)?;
        let parent = state
            .allowances
            .get(&self.id)
            .ok_or_else(|| CoreError::InvalidContract("reserve resize parent is missing".into()))?;
        if parent.closing {
            return Err(capacity("reserve resize parent is closed"));
        }
        let (bytes, files) = if partial {
            let byte_room = state
                .max_bytes
                .checked_sub(state.used_bytes)
                .ok_or_else(|| capacity("reserve refill byte cap invariant"))?;
            let file_room = state
                .max_files
                .checked_sub(state.used_files)
                .ok_or_else(|| capacity("reserve refill file cap invariant"))?;
            (
                bytes.min(
                    parent
                        .free_bytes
                        .checked_add(byte_room)
                        .ok_or_else(|| capacity("reserve refill byte overflow"))?,
                ),
                files.min(
                    parent
                        .free_files
                        .checked_add(file_room)
                        .ok_or_else(|| capacity("reserve refill file overflow"))?,
                ),
            )
        } else {
            (bytes, files)
        };
        let used_bytes = state
            .used_bytes
            .checked_sub(parent.free_bytes)
            .and_then(|used| used.checked_add(bytes))
            .ok_or_else(|| capacity("reserve resize byte overflow"))?;
        let used_files = state
            .used_files
            .checked_sub(parent.free_files)
            .and_then(|used| used.checked_add(files))
            .ok_or_else(|| capacity("reserve resize file overflow"))?;
        if used_bytes > state.max_bytes || used_files > state.max_files {
            return Err(capacity("reserve resize exceeds shared capacity"));
        }
        let parent = state.allowances.get_mut(&self.id).ok_or_else(|| {
            CoreError::InvalidContract("reserve resize parent disappeared".into())
        })?;
        parent.free_bytes = bytes;
        parent.free_files = files;
        state.used_bytes = used_bytes;
        state.used_files = used_files;
        drop(state);
        Ok(())
    }

    /// Authenticate a borrowed allowance before a filesystem binder subdivides
    /// it. Equal ceilings or local sequence numbers do not establish ownership.
    #[must_use]
    pub fn belongs_to(&self, authority: &PublicationCapacity) -> bool {
        Arc::ptr_eq(&self.shared, &authority.shared)
    }

    /// Atomically add capacity before a mutable file's next growth reservation.
    ///
    /// This is a global charge, not a child ticket;
    /// `reserve_growth` then subdivides it without charging global capacity
    /// again. A refused top-up leaves every ledger field unchanged.
    pub fn top_up(&self, additional_bytes: u64, additional_files: u64) -> Result<(), CoreError> {
        let mut state = lock(&self.shared)?;
        healthy(&state)?;
        let Some(parent) = state.allowances.get(&self.id) else {
            return Err(invalid(&mut state, "top-up parent is missing"));
        };
        if parent.closing {
            return Err(capacity("top-up parent already closed"));
        }
        if additional_bytes == 0 && additional_files == 0 {
            return Ok(());
        }
        let free_bytes = parent
            .free_bytes
            .checked_add(additional_bytes)
            .ok_or_else(|| capacity("top-up parent byte overflow"))?;
        let free_files = parent
            .free_files
            .checked_add(additional_files)
            .ok_or_else(|| capacity("top-up parent file overflow"))?;
        let (used_bytes, used_files) = room(&state, additional_bytes, additional_files)?;
        let Some(parent) = state.allowances.get_mut(&self.id) else {
            return Err(invalid(&mut state, "top-up parent disappeared"));
        };
        parent.free_bytes = free_bytes;
        parent.free_files = free_files;
        state.used_bytes = used_bytes;
        state.used_files = used_files;
        drop(state);
        Ok(())
    }

    pub fn reserve_output(&self, max_bytes: u64) -> Result<OutputTicket, CoreError> {
        if max_bytes == 0 {
            return Err(CoreError::InvalidContract(
                "output byte bound must be positive".into(),
            ));
        }
        let mut state = lock(&self.shared)?;
        healthy(&state)?;
        let Some(parent) = state.allowances.get(&self.id) else {
            return Err(invalid(&mut state, "output parent is missing"));
        };
        if parent.closing {
            return Err(invalid(&mut state, "output parent already closed"));
        }
        if parent.free_bytes < max_bytes || parent.free_files == 0 {
            return Err(capacity("output exceeds held publication allowance"));
        }
        if u64::try_from(state.tickets.len())
            .map_err(|error| capacity(&format!("ticket count conversion: {error}")))?
            >= state.max_pending
        {
            return Err(capacity("pending output ticket ceiling exceeded"));
        }
        let Some(free_bytes) = parent.free_bytes.checked_sub(max_bytes) else {
            return Err(invalid(&mut state, "output free-byte underflow"));
        };
        let Some(free_files) = parent.free_files.checked_sub(1) else {
            return Err(invalid(&mut state, "output free-file underflow"));
        };
        let Some(children) = parent.children.checked_add(1) else {
            return Err(invalid(&mut state, "parent child count overflow"));
        };
        let id = next_id(&mut state)?;
        let Some(parent) = state.allowances.get_mut(&self.id) else {
            return Err(invalid(&mut state, "output parent disappeared"));
        };
        parent.free_bytes = free_bytes;
        parent.free_files = free_files;
        parent.children = children;
        let _old = state.tickets.insert(
            id,
            TicketState {
                allowance_id: self.id,
                reserved_bytes: max_bytes,
                phase: TicketPhase::Reserved,
                orphaned: false,
                kind: TicketKind::Create,
            },
        );
        let authority = state.authority;
        drop(state);
        Ok(OutputTicket {
            shared: Arc::clone(&self.shared),
            authority,
            id,
            terminal: false,
        })
    }

    /// Hold only the maximum positive growth of an existing retained inode.
    /// `max_new_bytes` is the admitted final end offset/truncate width before
    /// each modifying syscall, not the sum of payload chunks. A shrink to zero
    /// holds zero growth bytes and no new file slot.
    pub fn reserve_resize(
        &self,
        base: RetainedId,
        observed_old_bytes: u64,
        max_new_bytes: u64,
    ) -> Result<ResizeTicket, CoreError> {
        let mut state = lock(&self.shared)?;
        healthy(&state)?;
        if !base.authority.matches(state.authority) {
            return Err(CoreError::InvalidContract(
                "resize inode belongs to another capacity authority".into(),
            ));
        }
        let Some(retained) = state.retained.get(&base.inode) else {
            return Err(invalid(&mut state, "resize inode absent from inventory"));
        };
        if retained.busy_resize.is_some() {
            return Err(capacity("retained inode already has a resize ticket"));
        }
        if retained.charge_id != base.charge_id {
            // Another writer may have completed a resize since this caller
            // observed the old token. No mutation has been admitted here.
            return Err(CoreError::InvalidContract(
                "stale retained resize identity".into(),
            ));
        }
        if retained.bytes != observed_old_bytes {
            return Err(invalid(&mut state, "resize source width differs"));
        }
        let Some(parent) = state.allowances.get(&self.id) else {
            return Err(invalid(&mut state, "resize parent is missing"));
        };
        if parent.closing {
            return Err(invalid(&mut state, "resize parent already closed"));
        }
        let held_growth = max_new_bytes.saturating_sub(observed_old_bytes);
        let Some(free_bytes) = parent.free_bytes.checked_sub(held_growth) else {
            return Err(capacity("resize growth exceeds held publication allowance"));
        };
        if u64::try_from(state.tickets.len())
            .map_err(|error| capacity(&format!("ticket count conversion: {error}")))?
            >= state.max_pending
        {
            return Err(capacity("pending output ticket ceiling exceeded"));
        }
        let Some(children) = parent.children.checked_add(1) else {
            return Err(invalid(&mut state, "resize parent child count overflow"));
        };
        let id = next_id(&mut state)?;
        let Some(parent) = state.allowances.get_mut(&self.id) else {
            return Err(invalid(&mut state, "resize parent disappeared"));
        };
        parent.children = children;
        parent.free_bytes = free_bytes;
        let Some(retained) = state.retained.get_mut(&base.inode) else {
            return Err(invalid(&mut state, "resize inode disappeared"));
        };
        retained.busy_resize = Some(id);
        let _prior = state.tickets.insert(
            id,
            TicketState {
                allowance_id: self.id,
                reserved_bytes: held_growth,
                phase: TicketPhase::Reserved,
                orphaned: false,
                kind: TicketKind::Resize {
                    inode: base.inode,
                    base_charge_id: base.charge_id,
                    base_bytes: observed_old_bytes,
                    max_new_bytes,
                },
            },
        );
        let authority = state.authority;
        drop(state);
        Ok(ResizeTicket {
            shared: Arc::clone(&self.shared),
            authority,
            id,
            terminal: false,
        })
    }
}

impl Drop for PublicationAllowance {
    fn drop(&mut self) {
        let Ok(mut state) = self.shared.lock() else {
            return;
        };
        if state.poison.is_some() {
            return;
        }
        let Some(parent) = state.allowances.get_mut(&self.id) else {
            state.poison = Some("dropped allowance missing from ledger".into());
            return;
        };
        if parent.closing {
            state.poison = Some("allowance closed twice".into());
            return;
        }
        let free_bytes = parent.free_bytes;
        let free_files = parent.free_files;
        let Some(active_allowances) = state.active_allowances.checked_sub(1) else {
            state.poison = Some("active allowance count underflow".into());
            return;
        };
        if let (Some(used_bytes), Some(used_files)) = (
            state.used_bytes.checked_sub(free_bytes),
            state.used_files.checked_sub(free_files),
        ) {
            state.used_bytes = used_bytes;
            state.used_files = used_files;
            state.active_allowances = active_allowances;
            if let Some(parent) = state.allowances.get_mut(&self.id) {
                parent.free_bytes = 0;
                parent.free_files = 0;
                parent.closing = true;
            }
            prune_allowance(&mut state, self.id);
        } else {
            state.poison = Some("allowance drop refund underflow".into());
        }
    }
}

/// One provisional physical file, not a second global reservation.
pub struct OutputTicket {
    shared: Arc<Mutex<State>>,
    authority: AuthorityId,
    id: u64,
    terminal: bool,
}

impl OutputTicket {
    /// Check a pre-reserved child before handing it to a physical file binder.
    /// A mismatched legitimate ticket is a caller error, not ledger corruption.
    pub fn authenticate_reserved_for_parent(
        &self,
        parent: &PublicationAllowance,
        max_bytes: u64,
    ) -> Result<(), CoreError> {
        if !Arc::ptr_eq(&self.shared, &parent.shared) {
            return Err(CoreError::InvalidContract(
                "reserved output belongs to another authority".into(),
            ));
        }
        let state = lock(&self.shared)?;
        healthy(&state)?;
        let Some(ticket) = state.tickets.get(&self.id) else {
            return Err(CoreError::InvalidContract(
                "reserved output ticket is absent".into(),
            ));
        };
        if self.terminal
            || ticket.allowance_id != parent.id
            || ticket.reserved_bytes != max_bytes
            || ticket.phase != TicketPhase::Reserved
            || ticket.orphaned
            || ticket.kind != TicketKind::Create
            || state
                .allowances
                .get(&parent.id)
                .is_none_or(|allowance| allowance.closing)
        {
            return Err(CoreError::InvalidContract(
                "output ticket is not reserved under the supplied parent and bound".into(),
            ));
        }
        drop(state);
        Ok(())
    }

    #[must_use]
    pub fn id(&self) -> PendingId {
        PendingId {
            authority: self.authority,
            sequence: self.id,
        }
    }

    /// Must precede the first create syscall, even if that syscall may fail.
    pub fn creation_started(&self) -> Result<(), CoreError> {
        let mut state = lock(&self.shared)?;
        healthy(&state)?;
        let Some(ticket) = state.tickets.get(&self.id) else {
            return Err(invalid(&mut state, "creation ticket missing"));
        };
        if self.terminal
            || ticket.kind != TicketKind::Create
            || ticket.phase != TicketPhase::Reserved
        {
            return Err(invalid(&mut state, "creation started out of order"));
        }
        let pending = state
            .uncertain_pending
            .checked_add(1)
            .ok_or_else(|| invalid(&mut state, "pending count overflow"))?;
        let Some(ticket) = state.tickets.get_mut(&self.id) else {
            return Err(invalid(&mut state, "creation ticket disappeared"));
        };
        ticket.phase = TicketPhase::MutationStarted;
        state.uncertain_pending = pending;
        drop(state);
        Ok(())
    }

    /// Reserve a larger upper bound before the next write to this same file.
    /// This subdivides the already charged parent allowance: it neither adds
    /// a global charge nor consumes another unique-file slot. A covered bound
    /// is idempotent. The parent must remain open for any growth request.
    pub fn reserve_growth(&self, new_max_bytes: u64) -> Result<(), CoreError> {
        if new_max_bytes == 0 {
            return Err(CoreError::InvalidContract(
                "output growth bound must be positive".into(),
            ));
        }
        let mut state = lock(&self.shared)?;
        healthy(&state)?;
        let Some(ticket) = state.tickets.get(&self.id) else {
            return Err(invalid(&mut state, "growth ticket missing"));
        };
        if self.terminal || ticket.kind != TicketKind::Create || ticket.orphaned {
            return Err(invalid(
                &mut state,
                "growth after terminal or orphan transition",
            ));
        }
        let allowance_id = ticket.allowance_id;
        let current_max_bytes = ticket.reserved_bytes;
        let Some(parent) = state.allowances.get(&allowance_id) else {
            return Err(invalid(&mut state, "growth parent missing"));
        };
        if parent.closing {
            return Err(capacity(
                "publication allowance closed before output growth",
            ));
        }
        if new_max_bytes <= current_max_bytes {
            return Ok(());
        }
        let Some(delta) = new_max_bytes.checked_sub(current_max_bytes) else {
            return Err(invalid(&mut state, "output growth delta underflow"));
        };
        let Some(free_bytes) = parent.free_bytes.checked_sub(delta) else {
            return Err(capacity("output growth exceeds held publication allowance"));
        };
        let Some(parent) = state.allowances.get_mut(&allowance_id) else {
            return Err(invalid(&mut state, "growth parent disappeared"));
        };
        parent.free_bytes = free_bytes;
        let Some(ticket) = state.tickets.get_mut(&self.id) else {
            return Err(invalid(&mut state, "growth ticket disappeared"));
        };
        ticket.reserved_bytes = new_max_bytes;
        drop(state);
        Ok(())
    }

    /// Actual durable output must be within the upper bound and a new inode.
    pub fn committed(
        &mut self,
        inode: InodeKey,
        actual_bytes: u64,
    ) -> Result<RetainedId, CoreError> {
        let mut state = lock(&self.shared)?;
        healthy(&state)?;
        if self.terminal {
            return Err(invalid(&mut state, "commit out of order"));
        }
        let retained = commit_started(&mut state, self.id, inode, actual_bytes)?;
        self.terminal = true;
        drop(state);
        Ok(retained)
    }

    /// Reuse is valid only before a create could have left an orphan.
    pub fn verified_reuse(
        &mut self,
        inode: InodeKey,
        actual_bytes: u64,
    ) -> Result<RetainedId, CoreError> {
        let mut state = lock(&self.shared)?;
        healthy(&state)?;
        let Some(ticket) = state.tickets.get(&self.id) else {
            return Err(invalid(&mut state, "reuse ticket missing"));
        };
        if self.terminal
            || ticket.kind != TicketKind::Create
            || ticket.phase != TicketPhase::Reserved
        {
            return Err(invalid(
                &mut state,
                "reuse after creation or terminal state",
            ));
        }
        let Some(retained) = state.retained.get(&inode) else {
            return Err(invalid(&mut state, "reuse inode absent from inventory"));
        };
        if retained.busy_resize.is_some() {
            return Err(capacity("reused inode has an active resize"));
        }
        if actual_bytes > ticket.reserved_bytes {
            return Err(invalid(
                &mut state,
                "reused output exceeds admitted child bound",
            ));
        }
        if retained.bytes != actual_bytes {
            return Err(invalid(&mut state, "reuse inode width differs"));
        }
        let id = RetainedId {
            authority: state.authority,
            inode,
            charge_id: retained.charge_id,
        };
        let Some(ticket) = state.tickets.remove(&self.id) else {
            return Err(invalid(&mut state, "reuse ticket disappeared"));
        };
        release_child(&mut state, &ticket)?;
        self.terminal = true;
        drop(state);
        Ok(id)
    }

    /// Binder has proved an attempted creation left no inode and synced the
    /// parent. This is the only refund for a creation-started ticket.
    pub fn confirmed_absent_and_synced(&mut self) -> Result<(), CoreError> {
        let mut state = lock(&self.shared)?;
        healthy(&state)?;
        let Some(ticket) = state.tickets.get(&self.id) else {
            return Err(invalid(&mut state, "absence ticket missing"));
        };
        if self.terminal
            || ticket.kind != TicketKind::Create
            || ticket.phase != TicketPhase::MutationStarted
        {
            return Err(invalid(&mut state, "absence confirmation out of order"));
        }
        let Some(pending) = state.uncertain_pending.checked_sub(1) else {
            return Err(invalid(&mut state, "absence pending count underflow"));
        };
        let Some(ticket) = state.tickets.remove(&self.id) else {
            return Err(invalid(&mut state, "absence ticket disappeared"));
        };
        state.uncertain_pending = pending;
        release_child(&mut state, &ticket)?;
        self.terminal = true;
        drop(state);
        Ok(())
    }
}

impl Drop for OutputTicket {
    fn drop(&mut self) {
        if self.terminal {
            return;
        }
        let Ok(mut state) = self.shared.lock() else {
            return;
        };
        if state.poison.is_some() {
            return;
        }
        let Some(ticket) = state.tickets.get(&self.id) else {
            state.poison = Some("dropped output ticket missing from ledger".into());
            return;
        };
        if ticket.phase == TicketPhase::MutationStarted {
            let Some(ticket) = state.tickets.get_mut(&self.id) else {
                state.poison = Some("started resize ticket disappeared on drop".into());
                return;
            };
            ticket.orphaned = true;
            return;
        }
        if let Some(ticket) = state.tickets.remove(&self.id)
            && let Err(error) = release_child(&mut state, &ticket)
        {
            state.poison = Some(error.to_string());
        }
    }
}

/// Exclusive provisional growth of one already retained physical inode.
///
/// A service binder must authenticate every filesystem outcome; this handle
/// only accounts declared identities and widths. A dropped started mutation
/// keeps its base inode and growth charge busy until explicit reconciliation.
pub struct ResizeTicket {
    shared: Arc<Mutex<State>>,
    authority: AuthorityId,
    id: u64,
    terminal: bool,
}

impl ResizeTicket {
    #[must_use]
    pub fn id(&self) -> PendingId {
        PendingId {
            authority: self.authority,
            sequence: self.id,
        }
    }

    /// Must precede the first in-place write or truncate syscall.
    pub fn mutation_started(&self) -> Result<(), CoreError> {
        let mut state = lock(&self.shared)?;
        healthy(&state)?;
        let (ticket, _, _, _) = resize_source(&mut state, self.id)?;
        if self.terminal || ticket.orphaned || ticket.phase != TicketPhase::Reserved {
            return Err(invalid(&mut state, "resize mutation started out of order"));
        }
        let pending = state
            .uncertain_pending
            .checked_add(1)
            .ok_or_else(|| invalid(&mut state, "resize pending count overflow"))?;
        let Some(ticket) = state.tickets.get_mut(&self.id) else {
            return Err(invalid(&mut state, "resize ticket disappeared"));
        };
        ticket.phase = TicketPhase::MutationStarted;
        state.uncertain_pending = pending;
        drop(state);
        Ok(())
    }

    /// Raise the admitted final file-width bound before writing past it.
    /// The extra growth is subdivided from the same still-live parent.
    pub fn reserve_growth(&self, new_max_new_bytes: u64) -> Result<(), CoreError> {
        let mut state = lock(&self.shared)?;
        healthy(&state)?;
        let (ticket, _, base_bytes, current_max) = resize_source(&mut state, self.id)?;
        if self.terminal || ticket.orphaned {
            return Err(invalid(
                &mut state,
                "resize growth after terminal or orphan",
            ));
        }
        let Some(parent) = state.allowances.get(&ticket.allowance_id) else {
            return Err(invalid(&mut state, "resize growth parent missing"));
        };
        if parent.closing {
            return Err(capacity("resize growth parent already closed"));
        }
        if new_max_new_bytes <= current_max {
            return Ok(());
        }
        let new_growth = new_max_new_bytes.saturating_sub(base_bytes);
        let Some(extra) = new_growth.checked_sub(ticket.reserved_bytes) else {
            return Err(invalid(&mut state, "resize growth reservation regressed"));
        };
        let Some(free_bytes) = parent.free_bytes.checked_sub(extra) else {
            return Err(capacity("resize growth exceeds held publication allowance"));
        };
        let TicketKind::Resize {
            inode,
            base_charge_id,
            ..
        } = ticket.kind
        else {
            return Err(invalid(&mut state, "resize kind changed"));
        };
        let Some(parent) = state.allowances.get_mut(&ticket.allowance_id) else {
            return Err(invalid(&mut state, "resize growth parent disappeared"));
        };
        parent.free_bytes = free_bytes;
        let Some(ticket) = state.tickets.get_mut(&self.id) else {
            return Err(invalid(&mut state, "resize growth ticket disappeared"));
        };
        ticket.reserved_bytes = new_growth;
        ticket.kind = TicketKind::Resize {
            inode,
            base_charge_id,
            base_bytes,
            max_new_bytes: new_max_new_bytes,
        };
        drop(state);
        Ok(())
    }

    /// Settle the same durable inode after its final width and parent sync
    /// have been authenticated. Returns a fresh ID; the base ID is stale.
    pub fn settled(&mut self, inode: InodeKey, actual_bytes: u64) -> Result<RetainedId, CoreError> {
        let mut state = lock(&self.shared)?;
        healthy(&state)?;
        if self.terminal {
            return Err(invalid(&mut state, "resize settled twice"));
        }
        let retained = settle_resize_started(&mut state, self.id, inode, actual_bytes, false)?;
        self.terminal = true;
        drop(state);
        Ok(retained)
    }

    /// Settle a failed mutation only after all links and live owned handles
    /// of its original inode have gone and the affected parents are synced.
    pub fn confirmed_last_reference_removed_and_synced(&mut self) -> Result<(), CoreError> {
        let mut state = lock(&self.shared)?;
        healthy(&state)?;
        if self.terminal {
            return Err(invalid(&mut state, "resize final removal repeated"));
        }
        remove_resize_started(&mut state, self.id, false)?;
        self.terminal = true;
        drop(state);
        Ok(())
    }
}

impl Drop for ResizeTicket {
    fn drop(&mut self) {
        if self.terminal {
            return;
        }
        let Ok(mut state) = self.shared.lock() else {
            return;
        };
        if state.poison.is_some() {
            return;
        }
        let Ok((ticket, inode, _, _)) = resize_source(&mut state, self.id) else {
            return;
        };
        if ticket.phase == TicketPhase::MutationStarted {
            if let Some(ticket) = state.tickets.get_mut(&self.id) {
                ticket.orphaned = true;
            }
            return;
        }
        if let Err(error) = release_child(&mut state, &ticket) {
            state.poison = Some(error.to_string());
            return;
        }
        let Some(retained) = state.retained.get_mut(&inode) else {
            state.poison = Some("reserved resize inode disappeared on drop".into());
            return;
        };
        retained.busy_resize = None;
        if state.tickets.remove(&self.id).is_none() {
            state.poison = Some("reserved resize ticket disappeared on drop".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(CoreError::InvalidContract(format!(
                    "condition failed: {}",
                    stringify!($condition)
                )));
            }
        }};
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) if !(*left == *right) => {
                    return Err(CoreError::InvalidContract(format!(
                        "values differ: {} and {}; left: {left:?}, right: {right:?}",
                        stringify!($left),
                        stringify!($right)
                    )));
                }
                _ => {}
            }
        }};
    }

    macro_rules! check_ne {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) if !(*left != *right) => {
                    return Err(CoreError::InvalidContract(format!(
                        "values unexpectedly equal: {} and {}; left: {left:?}, right: {right:?}",
                        stringify!($left),
                        stringify!($right)
                    )));
                }
                _ => {}
            }
        }};
    }

    fn key(inode: u64) -> InodeKey {
        InodeKey { device: 7, inode }
    }

    #[test]
    fn boot_hardlink_dedup_and_capacity_are_exact() -> Result<(), CoreError> {
        let mut boot = PublicationCapacityLedger::new(11, 2, 2)?;
        let first = boot.seed_inode(key(1), 5)?;
        check_eq!(boot.seed_inode(key(1), 5)?, first);
        let _second = boot.seed_inode(key(2), 6)?;
        let authority = boot.seal_boot()?;
        check_eq!(authority.snapshot()?.used_bytes, 11);
        check_eq!(authority.snapshot()?.used_files, 2);
        let before = authority.snapshot()?;
        check!(matches!(
            authority.reserve_publication(1, 1),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                ..
            })
        ));
        check_eq!(authority.snapshot()?, before);
        Ok(())
    }

    #[test]
    fn reserved_output_authentication_rejects_foreign_parent_and_bound_without_poison()
    -> Result<(), CoreError> {
        let authority = PublicationCapacityLedger::new(32, 4, 4)?.seal_boot()?;
        let parent = authority.reserve_publication(8, 1)?;
        let other_parent = authority.reserve_publication(8, 1)?;
        let foreign = PublicationCapacityLedger::new(32, 4, 4)?.seal_boot()?;
        let foreign_parent = foreign.reserve_publication(8, 1)?;
        let ticket = parent.reserve_output(5)?;
        check!(matches!(
            ticket.authenticate_reserved_for_parent(&other_parent, 5),
            Err(CoreError::InvalidContract(_))
        ));
        check!(matches!(
            ticket.authenticate_reserved_for_parent(&foreign_parent, 5),
            Err(CoreError::InvalidContract(_))
        ));
        check!(matches!(
            ticket.authenticate_reserved_for_parent(&parent, 4),
            Err(CoreError::InvalidContract(_))
        ));
        ticket.authenticate_reserved_for_parent(&parent, 5)?;
        check!(!authority.snapshot()?.poisoned);
        check!(!foreign.snapshot()?.poisoned);
        drop(ticket);
        drop(parent);
        drop(other_parent);
        drop(foreign_parent);
        check_eq!(authority.snapshot()?.used_bytes, 0);
        check_eq!(foreign.snapshot()?.used_bytes, 0);
        Ok(())
    }

    #[test]
    fn one_parent_charge_is_subdivided_and_retained_until_last_link() -> Result<(), CoreError> {
        let boot = PublicationCapacityLedger::new(20, 2, 2)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(20, 2)?;
        let mut first = parent.reserve_output(12)?;
        let mut second = parent.reserve_output(8)?;
        check_eq!(authority.snapshot()?.used_bytes, 20);
        check_eq!(authority.snapshot()?.used_files, 2);
        first.creation_started()?;
        let retained = first.committed(key(1), 9)?;
        second.creation_started()?;
        let _other = second.committed(key(2), 7)?;
        drop(parent);
        check_eq!(authority.snapshot()?.used_bytes, 16);
        check_eq!(authority.snapshot()?.used_files, 2);
        authority.confirmed_last_reference_removed_and_synced(retained, 9)?;
        check_eq!(authority.snapshot()?.used_bytes, 7);
        check_eq!(authority.snapshot()?.used_files, 1);
        Ok(())
    }

    #[test]
    fn precreate_drop_refunds_but_started_drop_remains_pending() -> Result<(), CoreError> {
        let boot = PublicationCapacityLedger::new(10, 1, 1)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(10, 1)?;
        drop(parent.reserve_output(10)?);
        check_eq!(authority.snapshot()?.active_tickets, 0);
        let child = parent.reserve_output(10)?;
        child.creation_started()?;
        let pending_id = child.id();
        drop(child);
        drop(parent);
        check_eq!(
            authority.pending_batch(None, 1)?,
            vec![PendingCharge {
                ticket_id: pending_id,
                reserved_bytes: 10,
                kind: PendingChargeKind::Create,
            }]
        );
        check!(matches!(
            authority.reserve_publication(1, 1),
            Err(CoreError::Typed { .. })
        ));
        authority.confirmed_pending_absent_and_synced(pending_id)?;
        check_eq!(authority.snapshot()?.used_bytes, 0);
        check_eq!(authority.snapshot()?.used_files, 0);
        Ok(())
    }

    #[test]
    fn verified_reuse_refunds_provisional_without_double_charge() -> Result<(), CoreError> {
        let mut boot = PublicationCapacityLedger::new(10, 2, 1)?;
        let existing = boot.seed_inode(key(4), 4)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(6, 1)?;
        let mut child = parent.reserve_output(6)?;
        check_eq!(child.verified_reuse(key(4), 4)?, existing);
        drop(parent);
        check_eq!(authority.snapshot()?.used_bytes, 4);
        check_eq!(authority.snapshot()?.used_files, 1);
        Ok(())
    }

    #[test]
    fn conflicting_width_and_repeated_release_poison_closed() -> Result<(), CoreError> {
        let mut boot = PublicationCapacityLedger::new(10, 2, 1)?;
        let retained = boot.seed_inode(key(1), 3)?;
        let authority = boot.seal_boot()?;
        authority.confirmed_last_reference_removed_and_synced(retained, 3)?;
        check!(
            authority
                .confirmed_last_reference_removed_and_synced(retained, 3)
                .is_err()
        );
        check!(authority.snapshot()?.poisoned);
        check!(authority.reserve_publication(1, 1).is_err());
        Ok(())
    }

    #[test]
    fn zero_byte_files_use_file_slots_until_exact_release() -> Result<(), CoreError> {
        let mut boot = PublicationCapacityLedger::new(1, 2, 1)?;
        let boot_file = boot.seed_inode(key(1), 0)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(1, 1)?;
        let mut child = parent.reserve_output(1)?;
        child.creation_started()?;
        let new_file = child.committed(key(2), 0)?;
        drop(parent);
        check_eq!(authority.snapshot()?.used_bytes, 0);
        check_eq!(authority.snapshot()?.used_files, 2);
        check!(matches!(
            authority.reserve_publication(1, 1),
            Err(CoreError::Typed { .. })
        ));
        authority.confirmed_last_reference_removed_and_synced(boot_file, 0)?;
        authority.confirmed_last_reference_removed_and_synced(new_file, 0)?;
        check_eq!(authority.snapshot()?.used_files, 0);
        Ok(())
    }

    #[test]
    fn dropped_parent_and_child_keep_pending_charge_for_verified_commit() -> Result<(), CoreError> {
        let boot = PublicationCapacityLedger::new(5, 1, 1)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(5, 1)?;
        let child = parent.reserve_output(5)?;
        child.creation_started()?;
        let ticket_id = child.id();
        drop(parent);
        drop(child);
        check_eq!(authority.snapshot()?.used_bytes, 5);
        check_eq!(authority.snapshot()?.uncertain_pending, 1);
        let retained = authority.confirmed_pending_committed_and_synced(ticket_id, key(9), 3)?;
        check_eq!(authority.snapshot()?.used_bytes, 3);
        check_eq!(authority.snapshot()?.used_files, 1);
        check_eq!(authority.snapshot()?.uncertain_pending, 0);
        authority.confirmed_last_reference_removed_and_synced(retained, 3)?;
        check_eq!(authority.snapshot()?.used_files, 0);
        Ok(())
    }

    #[test]
    fn allowance_entries_and_tickets_have_independent_explicit_caps() -> Result<(), CoreError> {
        let boot = PublicationCapacityLedger::new(10, 2, 1)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(2, 1)?;
        check!(matches!(
            authority.reserve_publication(2, 1),
            Err(CoreError::Typed { .. })
        ));
        let first = parent.reserve_output(1)?;
        check!(matches!(
            parent.reserve_output(1),
            Err(CoreError::Typed { .. })
        ));
        drop(first);
        drop(parent);
        let _next = authority.reserve_publication(2, 1)?;
        Ok(())
    }

    #[test]
    fn conflicting_boot_hardlink_width_poison_refuses_exposure() -> Result<(), CoreError> {
        let mut boot = PublicationCapacityLedger::new(10, 2, 1)?;
        let _first = boot.seed_inode(key(3), 2)?;
        check!(boot.seed_inode(key(3), 3).is_err());
        check!(boot.seal_boot().is_err());
        Ok(())
    }

    #[test]
    fn boot_over_capacity_cannot_seal_incomplete_inventory() -> Result<(), CoreError> {
        for (max_bytes, max_files) in [(1, 2), (2, 1)] {
            let mut boot = PublicationCapacityLedger::new(max_bytes, max_files, 1)?;
            let _first = boot.seed_inode(key(1), 1)?;
            check!(matches!(
                boot.seed_inode(key(2), 1),
                Err(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                    ..
                })
            ));
            check!(boot.seal_boot().is_err());
        }
        Ok(())
    }

    #[test]
    fn foreign_retained_id_cannot_release_matching_inode_and_sequence() -> Result<(), CoreError> {
        let mut first_boot = PublicationCapacityLedger::new(5, 1, 1)?;
        let first_id = first_boot.seed_inode(key(1), 3)?;
        let _first = first_boot.seal_boot()?;
        let mut second_boot = PublicationCapacityLedger::new(5, 1, 1)?;
        let second_id = second_boot.seed_inode(key(1), 3)?;
        check_eq!(first_id.charge_id, second_id.charge_id);
        check_ne!(first_id.authority, second_id.authority);
        let second = second_boot.seal_boot()?;
        check!(matches!(
            second.confirmed_last_reference_removed_and_synced(first_id, 3),
            Err(CoreError::InvalidContract(_))
        ));
        check_eq!(second.snapshot()?.used_bytes, 3);
        check!(!second.snapshot()?.poisoned);
        second.confirmed_last_reference_removed_and_synced(second_id, 3)?;
        Ok(())
    }

    #[test]
    fn foreign_pending_id_and_cursor_cannot_settle_matching_sequence() -> Result<(), CoreError> {
        let first_boot = PublicationCapacityLedger::new(5, 1, 1)?;
        let first = first_boot.seal_boot()?;
        let first_parent = first.reserve_publication(5, 1)?;
        let first_child = first_parent.reserve_output(5)?;
        first_child.creation_started()?;
        let first_id = first_child.id();
        drop(first_child);
        drop(first_parent);

        let second_boot = PublicationCapacityLedger::new(5, 1, 1)?;
        let second = second_boot.seal_boot()?;
        let second_parent = second.reserve_publication(5, 1)?;
        let second_child = second_parent.reserve_output(5)?;
        second_child.creation_started()?;
        let second_id = second_child.id();
        drop(second_child);
        drop(second_parent);

        check_eq!(first_id.sequence, second_id.sequence);
        check_ne!(first_id.authority, second_id.authority);
        check!(matches!(
            second.pending_batch(Some(first_id), 1),
            Err(CoreError::InvalidContract(_))
        ));
        check!(matches!(
            second.confirmed_pending_absent_and_synced(first_id),
            Err(CoreError::InvalidContract(_))
        ));
        check!(matches!(
            second.confirmed_pending_committed_and_synced(first_id, key(9), 3),
            Err(CoreError::InvalidContract(_))
        ));
        check_eq!(second.snapshot()?.used_bytes, 5);
        check_eq!(second.snapshot()?.uncertain_pending, 1);
        second.confirmed_pending_absent_and_synced(second_id)?;
        check_eq!(second.snapshot()?.used_bytes, 0);
        Ok(())
    }

    #[test]
    fn repeated_creation_signal_keeps_charge_and_poison_refuses_new_work() -> Result<(), CoreError>
    {
        let boot = PublicationCapacityLedger::new(4, 1, 1)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(4, 1)?;
        let child = parent.reserve_output(4)?;
        child.creation_started()?;
        check!(child.creation_started().is_err());
        drop(child);
        drop(parent);
        let snapshot = authority.snapshot()?;
        check_eq!(snapshot.used_bytes, 4);
        check_eq!(snapshot.used_files, 1);
        check!(snapshot.poisoned);
        check!(authority.reserve_publication(1, 1).is_err());
        Ok(())
    }

    #[test]
    fn closed_retained_parent_does_not_consume_active_allowance_slot() -> Result<(), CoreError> {
        let boot = PublicationCapacityLedger::new(20, 2, 1)?;
        let authority = boot.seal_boot()?;
        let first_parent = authority.reserve_publication(10, 1)?;
        let mut first_child = first_parent.reserve_output(10)?;
        first_child.creation_started()?;
        let first_file = first_child.committed(key(1), 7)?;
        drop(first_parent);
        check_eq!(authority.snapshot()?.active_allowances, 0);
        check_eq!(authority.snapshot()?.used_bytes, 7);
        let second_parent = authority.reserve_publication(10, 1)?;
        check_eq!(authority.snapshot()?.used_bytes, 17);
        check_eq!(authority.snapshot()?.used_files, 2);
        drop(second_parent);
        check_eq!(authority.snapshot()?.used_bytes, 7);
        check_eq!(authority.snapshot()?.used_files, 1);
        authority.confirmed_last_reference_removed_and_synced(first_file, 7)?;
        check_eq!(authority.snapshot()?.used_files, 0);
        Ok(())
    }

    #[test]
    fn live_started_ticket_is_not_a_recovery_pending_entry() -> Result<(), CoreError> {
        let boot = PublicationCapacityLedger::new(5, 1, 1)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(5, 1)?;
        let child = parent.reserve_output(5)?;
        child.creation_started()?;
        check!(authority.pending_batch(None, 1)?.is_empty());
        let ticket_id = child.id();
        drop(child);
        check_eq!(authority.pending_batch(None, 1)?.len(), 1);
        authority.confirmed_pending_absent_and_synced(ticket_id)?;
        drop(parent);
        check_eq!(authority.snapshot()?.used_files, 0);
        Ok(())
    }

    #[test]
    fn recovery_settlement_of_live_ticket_poison_refuses_double_owner() -> Result<(), CoreError> {
        let boot = PublicationCapacityLedger::new(5, 1, 1)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(5, 1)?;
        let child = parent.reserve_output(5)?;
        child.creation_started()?;
        check!(
            authority
                .confirmed_pending_absent_and_synced(child.id())
                .is_err()
        );
        check!(authority.snapshot()?.poisoned);
        drop(child);
        drop(parent);
        check_eq!(authority.snapshot()?.used_bytes, 5);
        Ok(())
    }

    #[test]
    fn live_file_grows_within_one_parent_charge_and_commits_actual_width() -> Result<(), CoreError>
    {
        let boot = PublicationCapacityLedger::new(10, 1, 1)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(10, 1)?;
        let mut child = parent.reserve_output(2)?;
        child.creation_started()?;
        child.reserve_growth(6)?;
        child.reserve_growth(5)?;
        check_eq!(authority.snapshot()?.used_bytes, 10);
        check_eq!(authority.snapshot()?.used_files, 1);
        let retained = child.committed(key(17), 5)?;
        drop(parent);
        check_eq!(authority.snapshot()?.used_bytes, 5);
        check_eq!(authority.snapshot()?.used_files, 1);
        authority.confirmed_last_reference_removed_and_synced(retained, 5)?;
        Ok(())
    }

    #[test]
    fn denied_growth_keeps_prior_written_charge_and_ticket_bound() -> Result<(), CoreError> {
        let boot = PublicationCapacityLedger::new(10, 2, 2)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(10, 2)?;
        let mut written = parent.reserve_output(2)?;
        written.creation_started()?;
        let sibling = parent.reserve_output(6)?;
        let before = authority.snapshot()?;
        check!(matches!(
            written.reserve_growth(7),
            Err(CoreError::Typed { .. })
        ));
        check!(matches!(
            written.reserve_growth(u64::MAX),
            Err(CoreError::Typed { .. })
        ));
        check!(matches!(
            written.reserve_growth(0),
            Err(CoreError::InvalidContract(_))
        ));
        check_eq!(authority.snapshot()?, before);
        // Prior two-byte output remains owned; a refused growth never refunds
        // it or contaminates the successful commit path.
        let retained = written.committed(key(21), 2)?;
        drop(sibling);
        drop(parent);
        check_eq!(authority.snapshot()?.used_bytes, 2);
        check_eq!(authority.snapshot()?.used_files, 1);
        authority.confirmed_last_reference_removed_and_synced(retained, 2)?;
        Ok(())
    }

    #[test]
    fn growth_after_parent_close_is_typed_refusal_without_changing_pending() -> Result<(), CoreError>
    {
        let boot = PublicationCapacityLedger::new(10, 1, 1)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(10, 1)?;
        let mut child = parent.reserve_output(2)?;
        child.creation_started()?;
        drop(parent);
        let before = authority.snapshot()?;
        check!(matches!(
            child.reserve_growth(3),
            Err(CoreError::Typed { .. })
        ));
        check_eq!(authority.snapshot()?, before);
        child.confirmed_absent_and_synced()?;
        check_eq!(authority.snapshot()?.used_files, 0);
        Ok(())
    }

    #[test]
    fn resize_growth_and_shrink_keep_only_final_inode_width() -> Result<(), CoreError> {
        let boot = PublicationCapacityLedger::new(39, 2, 2)?;
        let authority = boot.seal_boot()?;
        let parent_a = authority.reserve_publication(20, 1)?;
        let mut create = parent_a.reserve_output(10)?;
        create.creation_started()?;
        let original = create.committed(key(31), 10)?;
        let parent_b = authority.reserve_publication(15, 1)?;
        let mut grow = parent_b.reserve_resize(original, 10, 17)?;
        grow.mutation_started()?;
        let grown = grow.settled(key(31), 16)?;
        check_eq!(authority.snapshot()?.used_bytes, 35);
        check_eq!(authority.snapshot()?.used_files, 2);
        drop(parent_a);
        check_eq!(authority.snapshot()?.used_bytes, 25);
        drop(parent_b);
        check_eq!(authority.snapshot()?.used_bytes, 16);
        check_eq!(authority.snapshot()?.used_files, 1);
        let parent_c = authority.reserve_publication(4, 0)?;
        let mut shrink = parent_c.reserve_resize(grown, 16, 9)?;
        shrink.mutation_started()?;
        let reduced = shrink.settled(key(31), 9)?;
        check_eq!(authority.snapshot()?.used_bytes, 13);
        drop(parent_c);
        check_eq!(authority.snapshot()?.used_bytes, 9);
        check_eq!(authority.retained_inode(key(31), 9)?, Some(reduced));
        check!(
            authority
                .confirmed_last_reference_removed_and_synced(grown, 16)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn closed_resize_parent_refunds_slack_without_releasing_base() -> Result<(), CoreError> {
        let boot = PublicationCapacityLedger::new(35, 2, 2)?;
        let authority = boot.seal_boot()?;
        let parent_a = authority.reserve_publication(20, 1)?;
        let mut create = parent_a.reserve_output(10)?;
        create.creation_started()?;
        let base = create.committed(key(32), 10)?;
        let parent_b = authority.reserve_publication(15, 1)?;
        let mut resize = parent_b.reserve_resize(base, 10, 17)?;
        resize.mutation_started()?;
        drop(parent_b);
        check_eq!(authority.snapshot()?.used_bytes, 27);
        let final_id = resize.settled(key(32), 16)?;
        check_eq!(authority.snapshot()?.used_bytes, 26);
        drop(parent_a);
        check_eq!(authority.snapshot()?.used_bytes, 16);
        authority.confirmed_last_reference_removed_and_synced(final_id, 16)?;
        check_eq!(authority.snapshot()?.used_files, 0);
        Ok(())
    }

    #[test]
    fn at_file_cap_accepts_zero_file_growth_and_zero_allowance_shrink() -> Result<(), CoreError> {
        let mut boot = PublicationCapacityLedger::new(17, 1, 2)?;
        let base = boot.seed_inode(key(33), 10)?;
        let authority = boot.seal_boot()?;
        let grow_parent = authority.reserve_publication(7, 0)?;
        let mut grow = grow_parent.reserve_resize(base, 10, 17)?;
        grow.mutation_started()?;
        let grown = grow.settled(key(33), 16)?;
        drop(grow_parent);
        check_eq!(authority.snapshot()?.used_bytes, 16);
        check_eq!(authority.snapshot()?.used_files, 1);
        let shrink_parent = authority.reserve_publication(0, 0)?;
        let mut shrink = shrink_parent.reserve_resize(grown, 16, 0)?;
        shrink.mutation_started()?;
        let zero = shrink.settled(key(33), 0)?;
        drop(shrink_parent);
        check_eq!(authority.snapshot()?.used_bytes, 0);
        check_eq!(authority.snapshot()?.used_files, 1);
        authority.confirmed_last_reference_removed_and_synced(zero, 0)?;
        Ok(())
    }

    #[test]
    fn failed_shrink_reconciles_unchanged_inode_after_parent_drop() -> Result<(), CoreError> {
        let mut boot = PublicationCapacityLedger::new(10, 1, 1)?;
        let base = boot.seed_inode(key(42), 10)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(0, 0)?;
        let shrink = parent.reserve_resize(base, 10, 0)?;
        shrink.mutation_started()?;
        let pending = shrink.id();
        drop(shrink);
        drop(parent);
        check_eq!(authority.pending_batch(None, 1)?.len(), 1);
        let fresh = authority.confirmed_pending_resize_settled_and_synced(pending, key(42), 10)?;
        check!(fresh != base);
        check_eq!(authority.retained_inode(key(42), 10)?, Some(fresh));
        let snapshot = authority.snapshot()?;
        check_eq!(snapshot.used_bytes, 10);
        check_eq!(snapshot.used_files, 1);
        check_eq!(snapshot.uncertain_pending, 0);
        check!(!snapshot.poisoned);
        authority.confirmed_last_reference_removed_and_synced(fresh, 10)?;
        Ok(())
    }

    #[test]
    fn partial_shrink_settles_live_ticket_at_actual_width() -> Result<(), CoreError> {
        let mut boot = PublicationCapacityLedger::new(10, 1, 1)?;
        let base = boot.seed_inode(key(43), 10)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(0, 0)?;
        let mut shrink = parent.reserve_resize(base, 10, 0)?;
        shrink.mutation_started()?;
        let fresh = shrink.settled(key(43), 6)?;
        drop(parent);
        check!(fresh != base);
        check_eq!(authority.retained_inode(key(43), 6)?, Some(fresh));
        let snapshot = authority.snapshot()?;
        check_eq!(snapshot.used_bytes, 6);
        check_eq!(snapshot.used_files, 1);
        check_eq!(snapshot.uncertain_pending, 0);
        check!(!snapshot.poisoned);
        authority.confirmed_last_reference_removed_and_synced(fresh, 6)?;
        Ok(())
    }

    #[test]
    fn resize_settlement_above_base_and_growth_bound_stays_poisoned() -> Result<(), CoreError> {
        let mut boot = PublicationCapacityLedger::new(12, 1, 1)?;
        let base = boot.seed_inode(key(44), 10)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(2, 0)?;
        let mut growth = parent.reserve_resize(base, 10, 12)?;
        growth.mutation_started()?;
        check!(matches!(
            growth.settled(key(44), 13),
            Err(CoreError::Storage(_))
        ));
        let snapshot = authority.snapshot()?;
        check_eq!(snapshot.used_bytes, 12);
        check_eq!(snapshot.used_files, 1);
        check_eq!(snapshot.uncertain_pending, 1);
        check!(snapshot.poisoned);
        Ok(())
    }

    #[test]
    fn resize_busy_stale_and_prestart_drop_are_bounded() -> Result<(), CoreError> {
        let mut boot = PublicationCapacityLedger::new(20, 2, 1)?;
        let base = boot.seed_inode(key(34), 10)?;
        let authority = boot.seal_boot()?;
        check_eq!(authority.retained_inode(key(34), 10)?, Some(base));
        check_eq!(authority.retained_inode(key(35), 1)?, None);
        let parent = authority.reserve_publication(7, 0)?;
        let before = authority.snapshot()?;
        let ticket = parent.reserve_resize(base, 10, 17)?;
        check_eq!(authority.snapshot()?.active_tickets, 1);
        check!(parent.reserve_resize(base, 10, 17).is_err());
        check!(authority.retained_inode(key(34), 10).is_err());
        check!(
            authority
                .confirmed_last_reference_removed_and_synced(base, 10)
                .is_err()
        );
        check!(parent.reserve_output(1).is_err());
        drop(ticket);
        check_eq!(authority.snapshot()?, before);
        check_eq!(authority.retained_inode(key(34), 10)?, Some(base));
        drop(parent);
        Ok(())
    }

    #[test]
    fn in_flight_inode_width_is_busy_before_any_width_mismatch_poison() -> Result<(), CoreError> {
        let mut boot = PublicationCapacityLedger::new(40, 2, 3)?;
        let base = boot.seed_inode(key(45), 10)?;
        let authority = boot.seal_boot()?;
        let owner = authority.reserve_publication(7, 0)?;
        let mut resize = owner.reserve_resize(base, 10, 17)?;
        resize.mutation_started()?;
        // A concurrent binder has authenticated the same opened inode after
        // the first writer grew it to 16, before durable resize settlement.
        let contender = authority.reserve_publication(16, 1)?;
        let mut create = contender.reserve_output(8)?;
        let before = authority.snapshot()?;
        for refused in [
            authority.retained_inode(key(45), 16).map(|_| ()),
            contender.reserve_resize(base, 16, 17).map(|_| ()),
            create.verified_reuse(key(45), 16).map(|_| ()),
        ] {
            check!(matches!(
                refused,
                Err(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                    ..
                })
            ));
            check_eq!(authority.snapshot()?, before);
            check!(!authority.snapshot()?.poisoned);
        }
        let fresh = resize.settled(key(45), 16)?;
        check_ne!(fresh, base);
        drop(create);
        drop(contender);
        drop(owner);
        check_eq!(authority.retained_inode(key(45), 16)?, Some(fresh));
        check_eq!(authority.snapshot()?.used_bytes, 16);
        Ok(())
    }

    #[test]
    fn orphaned_resize_reconciles_exact_width_or_final_removal() -> Result<(), CoreError> {
        let mut boot = PublicationCapacityLedger::new(20, 1, 1)?;
        let base = boot.seed_inode(key(36), 10)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(7, 0)?;
        let ticket = parent.reserve_resize(base, 10, 17)?;
        ticket.mutation_started()?;
        let id = ticket.id();
        check!(authority.pending_batch(None, 1)?.is_empty());
        drop(ticket);
        check_eq!(
            authority.pending_batch(None, 1)?,
            vec![PendingCharge {
                ticket_id: id,
                reserved_bytes: 7,
                kind: PendingChargeKind::Resize {
                    base,
                    base_bytes: 10,
                    max_new_bytes: 17
                },
            }]
        );
        drop(parent);
        let fresh = authority.confirmed_pending_resize_settled_and_synced(id, key(36), 16)?;
        check_eq!(authority.snapshot()?.used_bytes, 16);
        check_eq!(authority.retained_inode(key(36), 16)?, Some(fresh));
        check!(
            authority
                .confirmed_pending_resize_settled_and_synced(id, key(36), 16)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn failed_resize_final_removal_refunds_base_and_growth_once() -> Result<(), CoreError> {
        let mut boot = PublicationCapacityLedger::new(20, 1, 1)?;
        let base = boot.seed_inode(key(37), 10)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(7, 0)?;
        let ticket = parent.reserve_resize(base, 10, 17)?;
        ticket.mutation_started()?;
        let id = ticket.id();
        drop(ticket);
        drop(parent);
        check_eq!(authority.snapshot()?.used_bytes, 17);
        authority.confirmed_pending_resize_last_reference_removed_and_synced(id)?;
        check_eq!(authority.snapshot()?.used_bytes, 0);
        check_eq!(authority.snapshot()?.used_files, 0);
        check_eq!(authority.retained_inode(key(37), 10)?, None);
        Ok(())
    }

    #[test]
    fn final_removal_refunds_each_parent_lifetime_without_double_charge() -> Result<(), CoreError> {
        #[derive(Clone, Copy)]
        enum ParentRelation {
            Same,
            Distinct,
        }
        struct Case {
            relation: ParentRelation,
            close_old: bool,
            close_new: bool,
            orphan: bool,
            before: (u64, u64),
            after: (u64, u64),
        }
        // Each pair is (charged bytes, charged file slots). All cases begin
        // with A20/1 creating X10; a distinct B, when present, holds 15/1 and
        // its resize ticket holds 7 additional bytes before final removal.
        let cases = [
            Case {
                relation: ParentRelation::Same,
                close_old: false,
                close_new: false,
                orphan: false,
                before: (20, 1),
                after: (20, 1),
            },
            Case {
                relation: ParentRelation::Same,
                close_old: true,
                close_new: true,
                orphan: false,
                before: (17, 1),
                after: (0, 0),
            },
            Case {
                relation: ParentRelation::Same,
                close_old: true,
                close_new: true,
                orphan: true,
                before: (17, 1),
                after: (0, 0),
            },
            Case {
                relation: ParentRelation::Distinct,
                close_old: false,
                close_new: false,
                orphan: false,
                before: (35, 2),
                after: (35, 2),
            },
            Case {
                relation: ParentRelation::Distinct,
                close_old: true,
                close_new: false,
                orphan: false,
                before: (25, 2),
                after: (15, 1),
            },
            Case {
                relation: ParentRelation::Distinct,
                close_old: false,
                close_new: true,
                orphan: false,
                before: (27, 1),
                after: (20, 1),
            },
            Case {
                relation: ParentRelation::Distinct,
                close_old: true,
                close_new: true,
                orphan: true,
                before: (17, 1),
                after: (0, 0),
            },
            Case {
                relation: ParentRelation::Distinct,
                close_old: false,
                close_new: true,
                orphan: true,
                before: (27, 1),
                after: (20, 1),
            },
        ];
        for case in cases {
            let boot = PublicationCapacityLedger::new(40, 2, 2)?;
            let authority = boot.seal_boot()?;
            let mut old_parent = Some(authority.reserve_publication(20, 1)?);
            let old = old_parent
                .as_ref()
                .ok_or_else(|| CoreError::InvalidContract("missing old parent".into()))?;
            let mut created = old.reserve_output(10)?;
            created.creation_started()?;
            let base = created.committed(key(46), 10)?;
            let mut new_parent = if matches!(case.relation, ParentRelation::Same) {
                None
            } else {
                Some(authority.reserve_publication(15, 1)?)
            };
            let resize_owner = if matches!(case.relation, ParentRelation::Same) {
                old
            } else {
                new_parent
                    .as_ref()
                    .ok_or_else(|| CoreError::InvalidContract("missing new parent".into()))?
            };
            let mut resize = resize_owner.reserve_resize(base, 10, 17)?;
            resize.mutation_started()?;
            if case.close_old {
                drop(old_parent.take());
            }
            if case.close_new && !matches!(case.relation, ParentRelation::Same) {
                drop(new_parent.take());
            }
            let before = authority.snapshot()?;
            check_eq!((before.used_bytes, before.used_files), case.before);
            if case.orphan {
                let id = resize.id();
                drop(resize);
                authority.confirmed_pending_resize_last_reference_removed_and_synced(id)?;
            } else {
                resize.confirmed_last_reference_removed_and_synced()?;
            }
            let after = authority.snapshot()?;
            check_eq!((after.used_bytes, after.used_files), case.after);
            check_eq!(after.retained_inodes, 0);
            check!(!after.poisoned);
            drop(old_parent);
            drop(new_parent);
            let final_usage = authority.snapshot()?;
            check_eq!((final_usage.used_bytes, final_usage.used_files), (0, 0));
        }
        Ok(())
    }

    #[test]
    fn same_parent_resize_and_monotonic_prewrite_growth() -> Result<(), CoreError> {
        let boot = PublicationCapacityLedger::new(20, 2, 2)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(20, 2)?;
        let mut create = parent.reserve_output(10)?;
        create.creation_started()?;
        let base = create.committed(key(38), 10)?;
        let mut resize = parent.reserve_resize(base, 10, 12)?;
        resize.mutation_started()?;
        resize.reserve_growth(17)?;
        let before_refusal = authority.snapshot()?;
        check!(matches!(
            resize.reserve_growth(21),
            Err(CoreError::Typed { .. })
        ));
        check_eq!(authority.snapshot()?, before_refusal);
        resize.reserve_growth(16)?;
        // A lower request must not refund the byte already held for width 17.
        // One file slot remains, so this rejection proves the byte bound.
        check!(matches!(
            parent.reserve_output(4),
            Err(CoreError::Typed { .. })
        ));
        check_eq!(authority.snapshot()?, before_refusal);
        let current = resize.settled(key(38), 17)?;
        drop(parent);
        check_eq!(authority.snapshot()?.used_bytes, 17);
        check_eq!(authority.snapshot()?.used_files, 1);
        authority.confirmed_last_reference_removed_and_synced(current, 17)?;
        Ok(())
    }

    #[test]
    fn same_live_parent_keeps_resize_and_orphan_shrink_budget_reusable() -> Result<(), CoreError> {
        let authority = PublicationCapacityLedger::new(20, 1, 2)?.seal_boot()?;
        let parent = authority.reserve_publication(20, 1)?;
        let mut create = parent.reserve_output(8)?;
        create.creation_started()?;
        let base = create.committed(key(48), 8)?;
        let mut grow = parent.reserve_resize(base, 8, 12)?;
        grow.mutation_started()?;
        let grown = grow.settled(key(48), 10)?;
        check_eq!(authority.snapshot()?.used_bytes, 20);
        check_eq!(authority.snapshot()?.used_files, 1);
        // Recovery of an interrupted shrink follows the same ownership rule.
        let shrink = parent.reserve_resize(grown, 10, 4)?;
        shrink.mutation_started()?;
        let pending = shrink.id();
        drop(shrink);
        let reduced = authority.confirmed_pending_resize_settled_and_synced(pending, key(48), 6)?;
        check_eq!(authority.snapshot()?.used_bytes, 20);
        check_eq!(authority.snapshot()?.used_files, 1);
        check_eq!(authority.snapshot()?.uncertain_pending, 0);
        authority.confirmed_last_reference_removed_and_synced(reduced, 6)?;
        check_eq!(authority.snapshot()?.used_bytes, 20);
        check_eq!(authority.snapshot()?.used_files, 1);
        // The held parent can fund its full original output again. Global
        // capacity is unchanged until its final owner ends.
        let mut replacement = parent.reserve_output(20)?;
        replacement.creation_started()?;
        let full = replacement.committed(key(49), 20)?;
        drop(parent);
        check_eq!(authority.snapshot()?.used_bytes, 20);
        check_eq!(authority.snapshot()?.used_files, 1);
        authority.confirmed_last_reference_removed_and_synced(full, 20)?;
        check_eq!(authority.snapshot()?.used_bytes, 0);
        check_eq!(authority.snapshot()?.used_files, 0);
        Ok(())
    }

    #[test]
    fn foreign_resize_identity_and_conflicting_lookup_do_not_mint_tokens() -> Result<(), CoreError>
    {
        let mut first_boot = PublicationCapacityLedger::new(10, 1, 1)?;
        let foreign = first_boot.seed_inode(key(39), 3)?;
        let _first = first_boot.seal_boot()?;
        let mut second_boot = PublicationCapacityLedger::new(10, 1, 1)?;
        let local = second_boot.seed_inode(key(39), 3)?;
        let second = second_boot.seal_boot()?;
        let parent = second.reserve_publication(3, 0)?;
        let before = second.snapshot()?;
        check!(matches!(
            parent.reserve_resize(foreign, 3, 6),
            Err(CoreError::InvalidContract(_))
        ));
        check_eq!(second.snapshot()?, before);
        check_eq!(second.retained_inode(key(39), 3)?, Some(local));
        check!(second.retained_inode(key(39), 4).is_err());
        check!(second.snapshot()?.poisoned);
        Ok(())
    }

    #[test]
    fn settled_resize_stale_preflight_does_not_poison_the_fresh_inode() -> Result<(), CoreError> {
        let mut boot = PublicationCapacityLedger::new(30, 1, 3)?;
        let stale = boot.seed_inode(key(47), 10)?;
        let authority = boot.seal_boot()?;
        let first = authority.reserve_publication(7, 0)?;
        let contender = authority.reserve_publication(1, 0)?;
        let mut resize = first.reserve_resize(stale, 10, 17)?;
        resize.mutation_started()?;
        let fresh = resize.settled(key(47), 16)?;
        check_ne!(fresh, stale);

        let before = authority.snapshot()?;
        check!(matches!(
            contender.reserve_resize(stale, 16, 17),
            Err(CoreError::InvalidContract(_))
        ));
        check_eq!(authority.snapshot()?, before);
        check!(!authority.snapshot()?.poisoned);
        check_eq!(authority.retained_inode(key(47), 16)?, Some(fresh));

        let mut retry = contender.reserve_resize(fresh, 16, 17)?;
        retry.mutation_started()?;
        let final_inode = retry.settled(key(47), 17)?;
        drop(first);
        drop(contender);
        check_eq!(authority.snapshot()?.used_bytes, 17);
        check_eq!(authority.snapshot()?.used_files, 1);
        authority.confirmed_last_reference_removed_and_synced(final_inode, 17)?;
        check_eq!(authority.snapshot()?.used_bytes, 0);
        check_eq!(authority.snapshot()?.used_files, 0);
        Ok(())
    }

    #[test]
    fn current_resize_identity_with_unaccounted_width_still_poisons() -> Result<(), CoreError> {
        let mut boot = PublicationCapacityLedger::new(20, 1, 1)?;
        let current = boot.seed_inode(key(48), 10)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(2, 0)?;
        check!(matches!(
            parent.reserve_resize(current, 11, 12),
            Err(CoreError::Storage(_))
        ));
        let after = authority.snapshot()?;
        check!(after.poisoned);
        check_eq!(after.used_bytes, 12);
        check_eq!(after.used_files, 1);
        check_eq!(after.active_tickets, 0);
        Ok(())
    }

    #[test]
    fn create_recovery_cannot_consume_resize_pending_or_inverse() -> Result<(), CoreError> {
        let mut boot = PublicationCapacityLedger::new(15, 2, 2)?;
        let base = boot.seed_inode(key(40), 5)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(10, 1)?;
        let resize = parent.reserve_resize(base, 5, 7)?;
        resize.mutation_started()?;
        let resize_id = resize.id();
        drop(resize);
        check!(
            authority
                .confirmed_pending_absent_and_synced(resize_id)
                .is_err()
        );
        check!(authority.snapshot()?.poisoned);

        let second_boot = PublicationCapacityLedger::new(10, 1, 1)?;
        let second = second_boot.seal_boot()?;
        let parent = second.reserve_publication(10, 1)?;
        let child = parent.reserve_output(5)?;
        child.creation_started()?;
        let create_id = child.id();
        drop(child);
        check!(
            second
                .confirmed_pending_resize_settled_and_synced(create_id, key(41), 5)
                .is_err()
        );
        check!(second.snapshot()?.poisoned);
        Ok(())
    }

    #[test]
    fn resize_tickets_share_the_bounded_pending_slot_with_creates() -> Result<(), CoreError> {
        let mut boot = PublicationCapacityLedger::new(20, 3, 1)?;
        let first = boot.seed_inode(key(42), 5)?;
        let second = boot.seed_inode(key(43), 5)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(10, 1)?;
        let held = parent.reserve_resize(first, 5, 7)?;
        let before = authority.snapshot()?;
        check!(matches!(
            parent.reserve_resize(second, 5, 7),
            Err(CoreError::Typed { .. })
        ));
        check!(matches!(
            parent.reserve_output(1),
            Err(CoreError::Typed { .. })
        ));
        check_eq!(authority.snapshot()?, before);
        drop(held);
        let again = parent.reserve_resize(second, 5, 7)?;
        drop(again);
        drop(parent);
        check_eq!(authority.snapshot()?.active_tickets, 0);
        Ok(())
    }

    #[test]
    fn parent_top_up_is_atomic_and_child_growth_does_not_double_charge() -> Result<(), CoreError> {
        let boot = PublicationCapacityLedger::new(10, 2, 2)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(2, 1)?;
        let child = parent.reserve_output(2)?;
        child.creation_started()?;
        parent.top_up(4, 0)?;
        check_eq!(authority.snapshot()?.used_bytes, 6);
        child.reserve_growth(6)?;
        check_eq!(authority.snapshot()?.used_bytes, 6);
        let before_refusal = authority.snapshot()?;
        check!(matches!(parent.top_up(5, 0), Err(CoreError::Typed { .. })));
        check_eq!(authority.snapshot()?, before_refusal);
        drop(child);
        drop(parent);
        check_eq!(authority.snapshot()?.used_bytes, 6);
        check_eq!(authority.snapshot()?.uncertain_pending, 1);
        Ok(())
    }

    #[test]
    fn borrowed_allowance_authenticates_authority_even_with_equal_counters() -> Result<(), CoreError>
    {
        let first = PublicationCapacityLedger::new(8, 2, 2)?.seal_boot()?;
        let second = PublicationCapacityLedger::new(8, 2, 2)?.seal_boot()?;
        let parent = first.reserve_publication(4, 1)?;
        let foreign = second.reserve_publication(4, 1)?;
        check_eq!(first.snapshot()?, second.snapshot()?);
        check!(parent.belongs_to(&first));
        let same_authority = first.clone();
        drop(first);
        check!(parent.belongs_to(&same_authority));
        check!(!parent.belongs_to(&second));
        check!(!foreign.belongs_to(&same_authority));
        Ok(())
    }
    #[test]
    fn reserve_resize_is_atomic_and_returns_only_unassigned_excess() -> Result<(), CoreError> {
        let authority = PublicationCapacityLedger::new(10, 2, 8)?.seal_boot()?;
        let maintenance = authority.reserve_publication(8, 1)?;
        let foreground = authority.reserve_publication(2, 1)?;
        let mut child = maintenance.reserve_output(4)?;
        child.creation_started()?;
        let retained = child.committed(key(91), 2)?;
        check_eq!(maintenance.available_capacity()?, (6, 0));
        let before = authority.snapshot()?;
        check!(matches!(
            maintenance.resize_available(8, 1),
            Err(CoreError::Typed { .. })
        ));
        check_eq!(authority.snapshot()?, before);
        check_eq!(maintenance.available_capacity()?, (6, 0));
        drop(foreground);
        maintenance.resize_available(8, 1)?;
        check_eq!(maintenance.available_capacity()?, (8, 1));
        check_eq!(authority.snapshot()?.used_bytes, 10);
        check_eq!(authority.snapshot()?.used_files, 2);
        authority.confirmed_last_reference_removed_and_synced(retained, 2)?;
        check_eq!(maintenance.available_capacity()?, (10, 2));
        maintenance.resize_available(8, 1)?;
        check_eq!(authority.snapshot()?.used_bytes, 8);
        check_eq!(authority.snapshot()?.used_files, 1);
        drop(maintenance);
        check_eq!(authority.snapshot()?.used_bytes, 0);
        check_eq!(authority.snapshot()?.used_files, 0);
        Ok(())
    }

    #[test]
    fn reserve_partial_refill_respects_both_caps_and_returns_cleanup_excess()
    -> Result<(), CoreError> {
        let authority = PublicationCapacityLedger::new(10, 3, 8)?.seal_boot()?;
        let maintenance = authority.reserve_publication(6, 2)?;
        let foreground = authority.reserve_publication(4, 1)?;
        let mut child = maintenance.reserve_output(4)?;
        child.creation_started()?;
        let retained = child.committed(key(92), 4)?;
        let full = authority.snapshot()?;
        maintenance.refill_available_up_to(6, 2)?;
        check_eq!(authority.snapshot()?, full);
        check_eq!(maintenance.available_capacity()?, (2, 1));
        foreground.resize_available(2, 1)?;
        maintenance.refill_available_up_to(6, 2)?;
        check_eq!(maintenance.available_capacity()?, (4, 1));
        check_eq!(authority.snapshot()?.used_bytes, 10);
        check_eq!(authority.snapshot()?.used_files, 3);
        drop(foreground);
        maintenance.refill_available_up_to(6, 2)?;
        check_eq!(maintenance.available_capacity()?, (6, 2));
        authority.confirmed_last_reference_removed_and_synced(retained, 4)?;
        check_eq!(maintenance.available_capacity()?, (10, 3));
        maintenance.refill_available_up_to(6, 2)?;
        check_eq!(authority.snapshot()?.used_bytes, 6);
        check_eq!(authority.snapshot()?.used_files, 2);
        drop(maintenance);
        check_eq!(authority.snapshot()?.used_bytes, 0);
        check_eq!(authority.snapshot()?.used_files, 0);
        Ok(())
    }

    #[test]
    fn parent_top_up_cannot_mask_poison_even_without_new_charge() -> Result<(), CoreError> {
        let mut boot = PublicationCapacityLedger::new(10, 2, 2)?;
        let retained = boot.seed_inode(key(1), 2)?;
        let authority = boot.seal_boot()?;
        let parent = authority.reserve_publication(1, 0)?;
        check!(matches!(
            parent.reserve_resize(retained, 1, 3),
            Err(CoreError::Storage(_))
        ));
        check!(matches!(parent.top_up(0, 0), Err(CoreError::Storage(_))));
        Ok(())
    }

    #[test]
    fn concurrent_parent_top_ups_share_one_global_ceiling() -> Result<(), CoreError> {
        let boot = PublicationCapacityLedger::new(10, 2, 2)?;
        let authority = boot.seal_boot()?;
        let parent = Arc::new(authority.reserve_publication(2, 0)?);
        let barrier = Arc::new(std::sync::Barrier::new(4));
        let mut joins = Vec::new();
        for _ in 0..4 {
            let parent = Arc::clone(&parent);
            let barrier = Arc::clone(&barrier);
            joins.push(std::thread::spawn(move || {
                let _arrival = barrier.wait();
                parent.top_up(3, 0)
            }));
        }
        let mut admitted = 0u64;
        let mut refused = 0u64;
        for join in joins {
            let outcome = join.join().map_err(|_panic| {
                CoreError::InvalidContract("top-up test worker panicked".into())
            })?;
            match outcome {
                Ok(()) => {
                    admitted = admitted.checked_add(1).ok_or_else(|| {
                        CoreError::InvalidContract("admission counter overflow".into())
                    })?;
                }
                Err(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                    ..
                }) => {
                    refused = refused.checked_add(1).ok_or_else(|| {
                        CoreError::InvalidContract("refusal counter overflow".into())
                    })?;
                }
                Err(error) => return Err(error),
            }
        }
        check_eq!(admitted, 2);
        check_eq!(refused, 2);
        check_eq!(authority.snapshot()?.used_bytes, 8);
        drop(parent);
        check_eq!(authority.snapshot()?.used_bytes, 0);
        Ok(())
    }
}
