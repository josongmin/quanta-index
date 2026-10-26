//! Durable, fenced genesis custody before the first workspace view exists.

use quanta_index_core::index::ports::ImmutableObjectPort;
use quanta_index_core::index::{
    BootstrapRef, CatalogView, IndexError, IndexProfile, ObjectRef, RegisteredObject, WorkBudget,
    WorkspaceKey,
};
use rusqlite::{Connection, OptionalExtension as _, params};

use super::{
    begin, current_view, decode, digest, encode, ensure_roots, exactly_one, fields_digest, int,
    next_fence, object_install, object_key, register_confirmed, sql, uint, workspace_key,
    write_view,
};

struct StageRow {
    stage: BootstrapRef,
    object: RegisteredObject,
}

fn stage_digest(
    workspace: &[u8],
    epoch: i64,
    fence: i64,
    profile: &[u8],
    payload: &[u8],
) -> Result<Vec<u8>, IndexError> {
    fields_digest(
        b"qidx.native.bootstrap.v1",
        &(workspace, epoch, fence, profile, payload),
    )
}

fn install_digest(
    workspace: &[u8],
    object: &[u8],
    payload: &[u8],
    confirmed: i64,
) -> Result<Vec<u8>, IndexError> {
    fields_digest(
        b"qidx.native.bootstrapobject.v1",
        &(workspace, object, payload, confirmed),
    )
}

fn read_stage(
    connection: &Connection,
    workspace: WorkspaceKey,
) -> Result<Option<StageRow>, IndexError> {
    let key = workspace_key(workspace)?;
    let lengths: Option<(i64, i64, i64)> = connection
        .query_row(
            "SELECT length(profile),length(payload),length(row_digest) FROM native_bootstrap_v1 WHERE workspace=?1",
            params![&key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((profile_len, payload_len, digest_len)) = lengths else {
        return Ok(None);
    };
    if profile_len != 32
        || !(1..=object_install::MAX_METADATA_BYTES_SQL).contains(&payload_len)
        || digest_len != 32
    {
        return Err(IndexError::Storage("bootstrap row shape mismatch".into()));
    }
    let (epoch, fence, profile, payload, checksum): (i64, i64, Vec<u8>, Vec<u8>, Vec<u8>) = connection
        .query_row(
            "SELECT epoch,fence,profile,payload,row_digest FROM native_bootstrap_v1 WHERE workspace=?1",
            params![&key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .map_err(|error| sql(&error))?;
    if profile.len() != 32
        || payload.len()
            != usize::try_from(payload_len).map_err(|_error| IndexError::ResourceLimited)?
        || checksum != stage_digest(&key, epoch, fence, &profile, &payload)?
    {
        return Err(IndexError::Storage("bootstrap row digest mismatch".into()));
    }
    let object: RegisteredObject = decode(&payload)?;
    let stage = BootstrapRef {
        workspace,
        epoch: uint(epoch)?,
        fence: uint(fence)?,
        profile: ObjectRef(quanta_index_core::index::Digest(
            <[u8; 32]>::try_from(profile.as_slice())
                .map_err(|_error| IndexError::Storage("bootstrap profile shape mismatch".into()))?,
        )),
    };
    if stage.fence == 0 || object.reference != stage.profile || !object.children.is_empty() {
        return Err(IndexError::Storage(
            "bootstrap profile binding mismatch".into(),
        ));
    }
    Ok(Some(StageRow { stage, object }))
}

fn require_stage(connection: &Connection, stage: BootstrapRef) -> Result<StageRow, IndexError> {
    let row = read_stage(connection, stage.workspace)?.ok_or(IndexError::FenceLost)?;
    if row.stage != stage {
        return Err(IndexError::FenceLost);
    }
    Ok(row)
}

fn read_install(
    connection: &Connection,
    stage: BootstrapRef,
) -> Result<Option<(RegisteredObject, bool)>, IndexError> {
    let key = workspace_key(stage.workspace)?;
    let lengths: Option<(i64, i64, i64)> = connection
        .query_row(
            "SELECT length(object),length(payload),length(row_digest) FROM native_bootstrap_object_v1 WHERE workspace=?1",
            params![&key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((object_len, payload_len, digest_len)) = lengths else {
        return Ok(None);
    };
    if object_len != 32
        || !(1..=object_install::MAX_METADATA_BYTES_SQL).contains(&payload_len)
        || digest_len != 32
    {
        return Err(IndexError::Storage(
            "bootstrap object row shape mismatch".into(),
        ));
    }
    let (object, payload, confirmed, checksum): (Vec<u8>, Vec<u8>, i64, Vec<u8>) = connection
        .query_row(
            "SELECT object,payload,confirmed,row_digest FROM native_bootstrap_object_v1 WHERE workspace=?1",
            params![&key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|error| sql(&error))?;
    if object.len() != 32
        || payload.len()
            != usize::try_from(payload_len).map_err(|_error| IndexError::ResourceLimited)?
        || !matches!(confirmed, 0 | 1)
        || checksum != install_digest(&key, &object, &payload, confirmed)?
    {
        return Err(IndexError::Storage(
            "bootstrap object row digest mismatch".into(),
        ));
    }
    let registration: RegisteredObject = decode(&payload)?;
    if registration.reference != stage.profile || object != object_key(stage.profile) {
        return Err(IndexError::Storage(
            "bootstrap object identity mismatch".into(),
        ));
    }
    Ok(Some((registration, confirmed == 1)))
}

fn require_object_match(
    expected: &RegisteredObject,
    actual: &RegisteredObject,
) -> Result<(), IndexError> {
    if actual.reference != expected.reference || !actual.children.is_empty() {
        return Err(IndexError::BackendViolation(
            "bootstrap object differs from frozen profile".into(),
        ));
    }
    if actual != expected {
        return Err(IndexError::CorruptObject(
            "bootstrap metadata changed for one digest".into(),
        ));
    }
    Ok(())
}

pub(super) fn begin_bootstrap(
    connection: &mut Connection,
    workspace: WorkspaceKey,
    epoch: u64,
    identity: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<BootstrapRef, IndexError> {
    budget.checkpoint()?;
    let payload = encode(&IndexProfile {
        instances: Vec::new(),
    })?;
    let object = object_install::checked_object(&identity.identify(&payload, &[], budget)?)?;
    if !object.children.is_empty() {
        return Err(IndexError::BackendViolation(
            "genesis identity has children".into(),
        ));
    }
    budget.checkpoint()?;
    let tx = begin(connection)?;
    budget.checkpoint()?;
    if current_view(&tx, workspace)?.is_some() {
        return Err(IndexError::Conflict);
    }
    if let Some(prior) = read_stage(&tx, workspace)? {
        if prior.stage.epoch != epoch || prior.stage.profile != object.reference {
            return Err(IndexError::Conflict);
        }
        require_object_match(&prior.object, &object)?;
        budget.checkpoint()?;
        return Ok(prior.stage);
    }
    let fence = next_fence(&tx)?;
    let stage = BootstrapRef {
        workspace,
        epoch,
        fence,
        profile: object.reference,
    };
    // A recycled profile digest may already be Retiring. Cancel that fence
    // before publishing a new root; Reclaiming/Draining refuse adoption.
    budget.charge(quanta_index_core::index::WorkUsage {
        reads: 3,
        writes: 2,
        mutations: 2,
        ..quanta_index_core::index::WorkUsage::default()
    })?;
    super::reclaim::adopt_if_known(&tx, object.reference)?;
    let key = workspace_key(workspace)?;
    let epoch = int(epoch)?;
    let fence = int(fence)?;
    let profile = object_key(object.reference);
    let encoded = encode(&object)?;
    let changed = tx.execute(
        "INSERT INTO native_bootstrap_v1(workspace,epoch,fence,profile,payload,row_digest) VALUES (?1,?2,?3,?4,?5,?6)",
        params![&key, epoch, fence, &profile, &encoded,
            stage_digest(&key, epoch, fence, &profile, &encoded)?],
    ).map_err(|error| sql(&error))?;
    exactly_one(changed, "begin bootstrap")?;
    budget.checkpoint()?;
    tx.commit().map_err(|error| sql(&error))?;
    Ok(stage)
}

pub(super) fn reserve_bootstrap(
    connection: &mut Connection,
    stage: BootstrapRef,
    object: &RegisteredObject,
) -> Result<(), IndexError> {
    let tx = begin(connection)?;
    super::reclaim::require_unbound(&tx)?;
    let result = reserve_bootstrap_tx(&tx, stage, object);
    result?;
    tx.commit().map_err(|error| sql(&error))
}

pub(super) fn reserve_bootstrap_tx(
    tx: &rusqlite::Transaction<'_>,
    stage: BootstrapRef,
    object: &RegisteredObject,
) -> Result<(), IndexError> {
    let row = require_stage(tx, stage)?;
    require_object_match(&row.object, object)?;
    if let Some((prior, _confirmed)) = read_install(tx, stage)? {
        require_object_match(&row.object, &prior)?;
    }
    object_install::reserve(tx, object)?;
    if read_install(tx, stage)?.is_none() {
        let key = workspace_key(stage.workspace)?;
        let object_key = object_key(object.reference);
        let payload = encode(object)?;
        let changed = tx.execute(
            "INSERT INTO native_bootstrap_object_v1(workspace,object,payload,confirmed,row_digest) VALUES (?1,?2,?3,0,?4)",
            params![&key, &object_key, &payload, install_digest(&key, &object_key, &payload, 0)?],
        ).map_err(|error| sql(&error))?;
        exactly_one(changed, "reserve bootstrap object")?;
    }
    Ok(())
}

pub(super) fn confirm_bootstrap(
    connection: &mut Connection,
    stage: BootstrapRef,
    object: &RegisteredObject,
) -> Result<(), IndexError> {
    let tx = begin(connection)?;
    super::reclaim::require_unbound(&tx)?;
    let result = confirm_bootstrap_tx(&tx, stage, object);
    result?;
    tx.commit().map_err(|error| sql(&error))
}

pub(super) fn confirm_bootstrap_tx(
    tx: &Connection,
    stage: BootstrapRef,
    object: &RegisteredObject,
) -> Result<(), IndexError> {
    let row = require_stage(tx, stage)?;
    require_object_match(&row.object, object)?;
    let (prior, _confirmed) = read_install(tx, stage)?.ok_or_else(|| {
        IndexError::BackendViolation("bootstrap confirmation lacks reservation".into())
    })?;
    require_object_match(&prior, object)?;
    register_confirmed(tx, std::slice::from_ref(object))?;
    let key = workspace_key(stage.workspace)?;
    let object_key = object_key(object.reference);
    let payload = encode(object)?;
    let changed = tx
        .execute(
            "UPDATE native_bootstrap_object_v1 SET confirmed=1,row_digest=?2 WHERE workspace=?1",
            params![&key, install_digest(&key, &object_key, &payload, 1)?],
        )
        .map_err(|error| sql(&error))?;
    exactly_one(changed, "confirm bootstrap object")?;
    Ok(())
}

pub(super) fn publish_bootstrap(
    connection: &mut Connection,
    stage: BootstrapRef,
    genesis: &CatalogView,
    budget: &WorkBudget,
) -> Result<quanta_index_core::index::ViewRef, IndexError> {
    budget.checkpoint()?;
    let tx = begin(connection)?;
    budget.checkpoint()?;
    let row = require_stage(&tx, stage)?;
    if current_view(&tx, stage.workspace)?.is_some() {
        return Err(IndexError::Conflict);
    }
    if genesis.identity.workspace != stage.workspace
        || genesis.identity.epoch != stage.epoch
        || genesis.identity.sequence != 0
        || genesis.profile != stage.profile
        || genesis.source.owners.0.is_some()
        || genesis.source.paths.0.is_some()
        || genesis.source.frontiers.0.is_some()
        || genesis.projections.0.is_some()
        || genesis.members.0.is_some()
        || genesis.coverage.0.is_some()
    {
        return Err(IndexError::BackendViolation(
            "genesis differs from frozen bootstrap".into(),
        ));
    }
    let (installed, confirmed) = read_install(&tx, stage)?
        .ok_or_else(|| IndexError::BackendViolation("bootstrap profile was not reserved".into()))?;
    require_object_match(&row.object, &installed)?;
    if !confirmed || !object_install::confirmed_matches(&tx, &installed)? {
        return Err(IndexError::BackendViolation(
            "bootstrap profile was not confirmed".into(),
        ));
    }
    budget.checkpoint()?;
    ensure_roots(&tx, genesis)?;
    write_view(&tx, genesis)?;
    let payload = encode(genesis)?;
    let key = workspace_key(stage.workspace)?;
    let changed = tx.execute(
        "INSERT INTO native_workspace_v1(workspace,epoch,sequence,payload,row_digest) VALUES (?1,?2,0,?3,?4)",
        params![&key, int(stage.epoch)?, &payload, digest(b"qidx.native.view.v1", &payload).to_vec()],
    ).map_err(|error| sql(&error))?;
    exactly_one(changed, "publish bootstrap workspace")?;
    let changed = tx
        .execute(
            "DELETE FROM native_bootstrap_object_v1 WHERE workspace=?1",
            params![&key],
        )
        .map_err(|error| sql(&error))?;
    exactly_one(changed, "transfer bootstrap object custody")?;
    let changed = tx
        .execute(
            "DELETE FROM native_bootstrap_v1 WHERE workspace=?1",
            params![&key],
        )
        .map_err(|error| sql(&error))?;
    exactly_one(changed, "transfer bootstrap authority")?;
    // The view root was written in this transaction; no ownership gap exists.
    budget.checkpoint()?;
    tx.commit().map_err(|error| sql(&error))?;
    Ok(genesis.identity)
}

/// Explicit fenced abandonment only. No wall-clock stage expiry is inferred.
pub(super) fn abort_bootstrap(
    connection: &mut Connection,
    stage: BootstrapRef,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    budget.checkpoint()?;
    let tx = begin(connection)?;
    budget.checkpoint()?;
    let _stage = require_stage(&tx, stage)?;
    let installed = read_install(&tx, stage)?;
    budget.charge(quanta_index_core::index::WorkUsage {
        reads: 2,
        writes: if installed.is_some() { 3 } else { 1 },
        mutations: if installed.is_some() { 3 } else { 1 },
        ..quanta_index_core::index::WorkUsage::default()
    })?;
    let key = workspace_key(stage.workspace)?;
    if let Some((object, _confirmed)) = installed {
        exactly_one(
            tx.execute(
                "DELETE FROM native_bootstrap_object_v1 WHERE workspace=?1",
                params![&key],
            )
            .map_err(|error| sql(&error))?,
            "abort bootstrap object",
        )?;
        super::reclaim::release(&tx, object.reference)?;
    }
    exactly_one(
        tx.execute(
            "DELETE FROM native_bootstrap_v1 WHERE workspace=?1 AND fence=?2",
            params![&key, int(stage.fence)?],
        )
        .map_err(|error| sql(&error))?,
        "abort bootstrap stage",
    )?;
    budget.checkpoint()?;
    tx.commit().map_err(|error| sql(&error))
}
