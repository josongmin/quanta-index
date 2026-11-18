//! Atomic Catalog import of authenticated protected Native heads.
//!
//! The held previous root supplies exact operation and ledger rows. A route
//! object alone describes expected heads but cannot recreate source events.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_contract::index::{
    NativeProtectedNativeSnapshotV1, NativeProtectedSourceRoutesV1,
};
use quanta_index_core::index::{
    CatalogView, ImmutableOperation, IndexError, IngressRef, MemoryLifetime, OperationKey,
    OperationStatus, ViewRef, WorkBudget, WorkUsage, WorkspaceKey,
};
use rusqlite::{Connection, OptionalExtension as _, params};

use super::{
    EventClaim, EventClaimState, OperationRow, SqliteCatalog, begin, check_ingress,
    confirmed_ingress_object, current_view, decode, digest, encode, ensure_roots, event_identity,
    exactly_one, fields_digest, int, publication_result, read_event_claim, read_operation,
    read_view_row, sql, workspace_key, write_event_claim, write_operation, write_view,
};

struct ExpectedProtectedRowsV1 {
    operations: BTreeMap<OperationKey, (ImmutableOperation, CatalogView)>,
    views: BTreeMap<ViewRef, CatalogView>,
    current: BTreeMap<WorkspaceKey, CatalogView>,
    genesis: BTreeMap<ViewRef, CatalogView>,
}

impl ExpectedProtectedRowsV1 {
    fn from_routes(
        primary: OperationKey,
        routes: &NativeProtectedSourceRoutesV1,
    ) -> Result<Self, IndexError> {
        routes.validate().map_err(|_| IndexError::DigestConflict)?;
        let mut expected = Self {
            operations: BTreeMap::new(),
            views: BTreeMap::new(),
            current: BTreeMap::new(),
            genesis: BTreeMap::new(),
        };
        for head in &routes.heads {
            expected.add_operation(head.source_operation, &head.source_publication)?;
            expected.add_operation(head.operation, &head.publication)?;
            expected.add_current(primary, &head.publication)?;
        }
        for head in &routes.repomap_heads {
            match head.source_operation {
                Some(operation) => {
                    expected.add_operation(operation, &head.source_publication)?;
                }
                None => {
                    expected.add_view(primary, &head.source_publication)?;
                    let _prior = expected.genesis.insert(
                        head.source_publication.identity,
                        head.source_publication.clone(),
                    );
                }
            }
            expected.add_operation(head.operation, &head.publication)?;
            expected.add_current(primary, &head.publication)?;
        }
        if expected.current.is_empty() {
            return Err(IndexError::DigestConflict);
        }
        Ok(expected)
    }

    fn add_operation(
        &mut self,
        operation: ImmutableOperation,
        view: &CatalogView,
    ) -> Result<(), IndexError> {
        if operation.key.workspace != view.identity.workspace
            || operation.key.epoch != view.identity.epoch
        {
            return Err(IndexError::DigestConflict);
        }
        if self
            .operations
            .insert(operation.key, (operation, view.clone()))
            .is_some_and(|prior| prior != (operation, view.clone()))
        {
            return Err(IndexError::DigestConflict);
        }
        Ok(())
    }

    fn add_view(&mut self, primary: OperationKey, view: &CatalogView) -> Result<(), IndexError> {
        if view.identity.workspace == primary.workspace
            || view.identity.digest
                != view
                    .identity_digest()
                    .map_err(|_| IndexError::DigestConflict)?
        {
            return Err(IndexError::DigestConflict);
        }
        if self
            .views
            .insert(view.identity, view.clone())
            .is_some_and(|prior| prior != *view)
        {
            return Err(IndexError::DigestConflict);
        }
        Ok(())
    }

    fn add_current(&mut self, primary: OperationKey, view: &CatalogView) -> Result<(), IndexError> {
        self.add_view(primary, view)?;
        if self
            .current
            .insert(view.identity.workspace, view.clone())
            .is_some_and(|prior| prior != *view)
        {
            return Err(IndexError::DigestConflict);
        }
        Ok(())
    }
}

fn decode_operation_rows_v1(
    snapshot: &NativeProtectedNativeSnapshotV1,
    expected: &ExpectedProtectedRowsV1,
) -> Result<BTreeMap<OperationKey, OperationRow>, IndexError> {
    let mut rows = BTreeMap::new();
    for bytes in &snapshot.operation_rows {
        let row: OperationRow = decode(bytes)?;
        if encode(&row)? != *bytes
            || !expected.current.contains_key(&row.immutable.key.workspace)
            || row.fence == 0
            || !matches!(
                &row.status,
                OperationStatus::Published(_) | OperationStatus::Refused(_)
            )
        {
            return Err(IndexError::DigestConflict);
        }
        publication_result::validate_operation_result(&row)?;
        if rows.insert(row.immutable.key, row).is_some() {
            return Err(IndexError::DigestConflict);
        }
    }
    for (key, (operation, view)) in &expected.operations {
        let row = rows.get(key).ok_or(IndexError::DigestConflict)?;
        if row.immutable != *operation
            || row.status != OperationStatus::Published(view.identity)
            || row.published_view.as_ref() != Some(view)
        {
            return Err(IndexError::DigestConflict);
        }
    }
    Ok(rows)
}

fn validate_claims_v1(
    snapshot: &NativeProtectedNativeSnapshotV1,
    operations: &BTreeMap<OperationKey, OperationRow>,
) -> Result<Vec<EventClaim>, IndexError> {
    let mut claims = Vec::with_capacity(snapshot.event_claims.len());
    let mut seen = BTreeSet::new();
    for bytes in &snapshot.event_claims {
        let claim: EventClaim = decode(bytes)?;
        let owner = operations
            .get(&claim.operation)
            .ok_or(IndexError::DigestConflict)?;
        if encode(&claim)? != *bytes
            || !seen.insert(event_identity(claim.operation, claim.event)?)
            || !owner.events.contains(&claim.event)
            || !matches!(
                (&owner.status, claim.state),
                (OperationStatus::Published(_), EventClaimState::Committed)
                    | (OperationStatus::Refused(_), EventClaimState::Refused)
            )
        {
            return Err(IndexError::DigestConflict);
        }
        claims.push(claim);
    }
    for row in operations
        .values()
        .filter(|row| matches!(&row.status, OperationStatus::Published(_)))
    {
        for event in &row.events {
            if !seen.contains(&event_identity(row.immutable.key, *event)?) {
                return Err(IndexError::DigestConflict);
            }
        }
    }
    Ok(claims)
}

fn count_workspace_v1(
    tx: &Connection,
    table: &'static str,
    workspace: WorkspaceKey,
) -> Result<u64, IndexError> {
    let sql_v1 = match table {
        "native_view_v1" => "SELECT count(*) FROM native_view_v1 WHERE workspace=?1",
        "native_operation_v1" => "SELECT count(*) FROM native_operation_v1 WHERE workspace=?1",
        "native_frontier_v1" => "SELECT count(*) FROM native_frontier_v1 WHERE workspace=?1",
        "native_source_authority_v1" => {
            "SELECT count(*) FROM native_source_authority_v1 WHERE workspace=?1"
        }
        "native_event_claim_v1" => {
            "SELECT count(*) FROM native_event_claim_v1 c JOIN native_operation_v1 o ON c.operation=o.operation WHERE o.workspace=?1"
        }
        _ => {
            return Err(IndexError::InvalidInput(
                "unknown protected workspace table".into(),
            ));
        }
    };
    let count: i64 = tx
        .query_row(sql_v1, params![workspace_key(workspace)?], |row| row.get(0))
        .map_err(|error| sql(&error))?;
    u64::try_from(count).map_err(|_| IndexError::Storage("negative protected row count".into()))
}

fn validate_existing_v1(
    tx: &Connection,
    expected: &ExpectedProtectedRowsV1,
    operations: &BTreeMap<OperationKey, OperationRow>,
    claims: &[EventClaim],
    snapshot: &NativeProtectedNativeSnapshotV1,
) -> Result<bool, IndexError> {
    let mut existing = 0;
    for workspace in expected.current.keys() {
        if current_view(tx, *workspace)?.is_some() {
            existing += 1;
        }
    }
    if existing == 0 {
        for workspace in expected.current.keys() {
            if count_workspace_v1(tx, "native_view_v1", *workspace)? != 0
                || count_workspace_v1(tx, "native_operation_v1", *workspace)? != 0
                || count_workspace_v1(tx, "native_frontier_v1", *workspace)? != 0
                || count_workspace_v1(tx, "native_source_authority_v1", *workspace)? != 0
            {
                return Err(IndexError::Conflict);
            }
        }
        return Ok(false);
    }
    if existing != expected.current.len() {
        return Err(IndexError::Conflict);
    }
    for (workspace, terminal) in &expected.current {
        if current_view(tx, *workspace)?.as_ref() != Some(terminal) {
            return Err(IndexError::Conflict);
        }
        let count = |table| count_workspace_v1(tx, table, *workspace);
        if count("native_view_v1")?
            != expected
                .views
                .values()
                .filter(|view| view.identity.workspace == *workspace)
                .count() as u64
            || count("native_operation_v1")?
                != operations
                    .values()
                    .filter(|row| row.immutable.key.workspace == *workspace)
                    .count() as u64
            || count("native_event_claim_v1")?
                != claims
                    .iter()
                    .filter(|claim| claim.operation.workspace == *workspace)
                    .count() as u64
            || count("native_frontier_v1")?
                != snapshot
                    .frontiers
                    .iter()
                    .filter(|row| row.workspace == *workspace)
                    .count() as u64
            || count("native_source_authority_v1")?
                != snapshot
                    .source_authorities
                    .iter()
                    .filter(|row| row.workspace == *workspace)
                    .count() as u64
        {
            return Err(IndexError::Conflict);
        }
    }
    for (identity, view) in &expected.views {
        if read_view_row(tx, *identity)? != *view {
            return Err(IndexError::DigestConflict);
        }
    }
    for (key, row) in operations {
        if read_operation(tx, *key)?.as_ref() != Some(row) {
            return Err(IndexError::DigestConflict);
        }
    }
    for claim in claims {
        if read_event_claim(tx, claim.operation, claim.event)?.as_ref() != Some(claim) {
            return Err(IndexError::DigestConflict);
        }
    }
    for row in &snapshot.frontiers {
        let workspace = workspace_key(row.workspace)?;
        let producer = row.producer.0.to_vec();
        let stream = row.stream.0.to_vec();
        let actual: Option<(i64, i64, Vec<u8>)> = tx.query_row(
            "SELECT stream_epoch,sequence,row_digest FROM native_frontier_v1 WHERE workspace=?1 AND producer=?2 AND stream=?3",
            params![&workspace, &producer, &stream],
            |record| Ok((record.get(0)?, record.get(1)?, record.get(2)?)),
        ).optional().map_err(|error| sql(&error))?;
        let epoch = int(row.stream_epoch)?;
        let sequence = int(row.sequence)?;
        if actual
            != Some((
                epoch,
                sequence,
                fields_digest(
                    b"qidx.native.frontier.v1",
                    &(&workspace, &producer, &stream, epoch, sequence),
                )?,
            ))
        {
            return Err(IndexError::DigestConflict);
        }
    }
    for row in &snapshot.source_authorities {
        let workspace = workspace_key(row.workspace)?;
        let producer = row.producer.0.to_vec();
        let actual: Option<(i64, Vec<u8>)> = tx.query_row(
            "SELECT source_epoch,row_digest FROM native_source_authority_v1 WHERE workspace=?1 AND workspace_epoch=?2 AND producer=?3",
            params![&workspace, int(row.workspace_epoch)?, &producer],
            |record| Ok((record.get(0)?, record.get(1)?)),
        ).optional().map_err(|error| sql(&error))?;
        let source_epoch = int(row.source_epoch)?;
        if actual
            != Some((
                source_epoch,
                fields_digest(
                    b"qidx.native.sourceauthority.v1",
                    &(
                        &workspace,
                        int(row.workspace_epoch)?,
                        &producer,
                        source_epoch,
                    ),
                )?,
            ))
        {
            return Err(IndexError::DigestConflict);
        }
    }
    Ok(true)
}

impl SqliteCatalog {
    /// Import exact Catalog history from a separately authenticated held Native root.
    /// This transaction does not issue a Source ACK or consume the staging ingress.
    pub fn restore_protected_native_heads_v1(
        &self,
        primary: OperationKey,
        ingress: IngressRef,
        snapshot: &NativeProtectedNativeSnapshotV1,
        source_routes: &NativeProtectedSourceRoutesV1,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        snapshot
            .validate_shape_v1()
            .map_err(|_| IndexError::DigestConflict)?;
        let bytes = snapshot
            .encoded_payload_len_v1()
            .map_err(|_| IndexError::ResourceLimited)?;
        let _memory = budget
            .reserve_memory(
                u64::try_from(bytes)
                    .map_err(|_| IndexError::ResourceLimited)?
                    .checked_mul(3)
                    .ok_or(IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?
            .ok_or(IndexError::ResourceLimited)?;
        let row_count = snapshot
            .operation_rows
            .len()
            .checked_add(snapshot.genesis_views.len())
            .and_then(|count| count.checked_add(snapshot.frontiers.len()))
            .and_then(|count| count.checked_add(snapshot.source_authorities.len()))
            .and_then(|count| count.checked_add(snapshot.event_claims.len()))
            .and_then(|count| count.checked_add(snapshot.roots.len()))
            .ok_or(IndexError::ResourceLimited)?;
        let row_count = u64::try_from(row_count).map_err(|_| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            reads: row_count
                .checked_mul(8)
                .and_then(|value| value.checked_add(64))
                .ok_or(IndexError::ResourceLimited)?,
            writes: row_count
                .checked_mul(4)
                .and_then(|value| value.checked_add(64))
                .ok_or(IndexError::ResourceLimited)?,
            mutations: row_count
                .checked_mul(4)
                .and_then(|value| value.checked_add(64))
                .ok_or(IndexError::ResourceLimited)?,
            bytes: u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let origin = source_routes
            .native_origin
            .as_ref()
            .ok_or(IndexError::DigestConflict)?;
        if origin.source_root_nonce != snapshot.source_root_nonce
            || origin.source_routes != snapshot.source_routes
            || origin.snapshot
                != snapshot
                    .object_ref_v1()
                    .map_err(|_| IndexError::DigestConflict)?
            || ingress.operation.workspace != primary.workspace
            || ingress.operation.epoch != primary.epoch
            || ingress.operation == primary
        {
            return Err(IndexError::DigestConflict);
        }
        let expected = ExpectedProtectedRowsV1::from_routes(primary, source_routes)?;
        let operations = decode_operation_rows_v1(snapshot, &expected)?;
        let claims = validate_claims_v1(snapshot, &operations)?;
        let mut frontier_keys = BTreeSet::new();
        for row in &snapshot.frontiers {
            if !expected.current.contains_key(&row.workspace)
                || !frontier_keys.insert((row.workspace, row.producer, row.stream))
            {
                return Err(IndexError::DigestConflict);
            }
        }
        let mut authority_keys = BTreeSet::new();
        for row in &snapshot.source_authorities {
            if !expected.current.contains_key(&row.workspace)
                || !authority_keys.insert((row.workspace, row.workspace_epoch, row.producer))
            {
                return Err(IndexError::DigestConflict);
            }
        }
        let genesis = snapshot
            .genesis_views
            .iter()
            .map(|view| (view.identity, view.clone()))
            .collect::<BTreeMap<_, _>>();
        if genesis != expected.genesis || snapshot.genesis_views.len() != genesis.len() {
            return Err(IndexError::DigestConflict);
        }
        let mut guard = self.lock_native_budget(budget)?;
        let tx = begin(&mut guard)?;
        if check_ingress(&tx, ingress)?.sealed.is_some()
            || read_operation(&tx, ingress.operation)?.is_some()
        {
            return Err(IndexError::Conflict);
        }
        let primary_row = read_operation(&tx, primary)?.ok_or(IndexError::SnapshotUnavailable)?;
        if !matches!(&primary_row.status, OperationStatus::Published(_)) {
            return Err(IndexError::Conflict);
        }
        {
            let mut statement = tx
                .prepare("SELECT workspace FROM native_workspace_v1 ORDER BY workspace")
                .map_err(|error| sql(&error))?;
            let workspaces = statement
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .map_err(|error| sql(&error))?;
            for workspace in workspaces {
                let workspace = workspace.map_err(|error| sql(&error))?;
                if workspace != workspace_key(primary.workspace)?
                    && !expected
                        .current
                        .keys()
                        .any(|key| workspace_key(*key).ok().as_ref() == Some(&workspace))
                {
                    return Err(IndexError::Conflict);
                }
            }
        }
        let new_route = source_routes
            .object_ref()
            .map_err(|_| IndexError::DigestConflict)?;
        let mut required_roots = BTreeSet::new();
        for view in expected.views.values() {
            required_roots.extend(super::distinct_roots(view));
        }
        required_roots.extend(
            expected
                .operations
                .values()
                .map(|(operation, _)| operation.request),
        );
        for root in required_roots {
            if !confirmed_ingress_object(&tx, ingress.operation, root)? {
                return Err(IndexError::DigestConflict);
            }
        }
        if !confirmed_ingress_object(&tx, ingress.operation, origin.snapshot)?
            || !confirmed_ingress_object(&tx, ingress.operation, new_route)?
        {
            return Err(IndexError::DigestConflict);
        }
        for root in &snapshot.roots {
            if !confirmed_ingress_object(&tx, ingress.operation, *root)? {
                return Err(IndexError::DigestConflict);
            }
        }
        let already = validate_existing_v1(&tx, &expected, &operations, &claims, snapshot)?;
        if !already {
            for view in expected.views.values() {
                ensure_roots(&tx, view)?;
                write_view(&tx, view)?;
            }
            for (workspace, terminal) in &expected.current {
                let payload = encode(terminal)?;
                exactly_one(
                    tx.execute(
                        "INSERT INTO native_workspace_v1(workspace,epoch,sequence,payload,row_digest) VALUES (?1,?2,?3,?4,?5)",
                        params![workspace_key(*workspace)?, int(terminal.identity.epoch)?, int(terminal.identity.sequence)?, &payload, digest(b"qidx.native.view.v1", &payload).to_vec()],
                    ).map_err(|error| sql(&error))?,
                    "import protected workspace",
                )?;
            }
            for row in operations.values() {
                write_operation(&tx, row, true)?;
            }
            for claim in &claims {
                write_event_claim(&tx, *claim, true)?;
            }
            for row in &snapshot.frontiers {
                let workspace = workspace_key(row.workspace)?;
                let epoch = int(row.stream_epoch)?;
                let sequence = int(row.sequence)?;
                let producer = row.producer.0.to_vec();
                let stream = row.stream.0.to_vec();
                exactly_one(tx.execute(
                    "INSERT INTO native_frontier_v1(workspace,producer,stream,stream_epoch,sequence,row_digest) VALUES (?1,?2,?3,?4,?5,?6)",
                    params![&workspace, &producer, &stream, epoch, sequence,
                        fields_digest(b"qidx.native.frontier.v1", &(&workspace, &producer, &stream, epoch, sequence))?],
                ).map_err(|error| sql(&error))?, "import protected frontier")?;
            }
            for row in &snapshot.source_authorities {
                let workspace = workspace_key(row.workspace)?;
                let workspace_epoch = int(row.workspace_epoch)?;
                let source_epoch = int(row.source_epoch)?;
                let producer = row.producer.0.to_vec();
                exactly_one(tx.execute(
                    "INSERT INTO native_source_authority_v1(workspace,workspace_epoch,producer,source_epoch,row_digest) VALUES (?1,?2,?3,?4,?5)",
                    params![&workspace, workspace_epoch, &producer, source_epoch,
                        fields_digest(b"qidx.native.sourceauthority.v1", &(&workspace, workspace_epoch, &producer, source_epoch))?],
                ).map_err(|error| sql(&error))?, "import protected source authority")?;
            }
        }
        let _verified_route = super::boot_manifest::validate_protected_routes(
            &tx,
            primary,
            source_routes
                .object_ref()
                .map_err(|_| IndexError::DigestConflict)?,
            source_routes,
            true,
            Some(ingress),
            budget,
        )?;
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        Ok(())
    }
}
