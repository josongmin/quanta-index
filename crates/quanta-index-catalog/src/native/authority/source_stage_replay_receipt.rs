//! Exact pre-Submit Source stage custody on the existing Catalog connection.

use super::*;
use quanta_index_contract::native_protocol::SourceStageReplayReceiptV1;
use quanta_index_core::index::MemoryLifetime;

const DOMAIN: &[u8] = b"qidx.native.source_stage_replay.v1";
const MAX_PAYLOAD_BYTES: i64 = 8192;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_source_stage_replay_v1 (
    operation BLOB PRIMARY KEY NOT NULL,
    fence INTEGER NOT NULL CHECK(typeof(fence)='integer' AND fence>0),
    correlation BLOB NOT NULL CHECK(typeof(correlation)='blob' AND length(correlation)=32),
    payload BLOB NOT NULL CHECK(typeof(payload)='blob' AND length(payload) BETWEEN 1 AND 8192),
    row_digest BLOB NOT NULL CHECK(typeof(row_digest)='blob' AND length(row_digest)=32),
    FOREIGN KEY(operation) REFERENCES native_ingress_v1(operation) ON DELETE CASCADE
) WITHOUT ROWID;
CREATE TRIGGER IF NOT EXISTS native_source_stage_replay_rotate_v1
AFTER UPDATE OF fence ON native_ingress_v1
WHEN OLD.fence<>NEW.fence
BEGIN
    DELETE FROM native_source_stage_replay_v1 WHERE operation=OLD.operation;
END;
CREATE TRIGGER IF NOT EXISTS native_source_stage_replay_delete_v1
AFTER DELETE ON native_ingress_v1
BEGIN
    DELETE FROM native_source_stage_replay_v1 WHERE operation=OLD.operation;
END;
";

pub(super) const OBJECTS: &[(&str, &str, &str)] = &[(
    "table",
    "native_source_stage_replay_v1",
    "native Source stage replay receipt",
)];
pub(super) const TRIGGERS: &[&str] = &[
    "native_source_stage_replay_rotate_v1",
    "native_source_stage_replay_delete_v1",
];

pub(super) fn is_schema_object(kind: &str, name: &str) -> bool {
    OBJECTS
        .iter()
        .any(|(expected_kind, expected_name, _)| *expected_kind == kind && *expected_name == name)
        || kind == "trigger" && TRIGGERS.contains(&name)
}

pub(super) const fn schema_object_count() -> usize {
    OBJECTS.len() + TRIGGERS.len()
}

pub(super) fn verify_schema(connection: &Connection, path: &Path) -> Result<(), CoreError> {
    verify_schema_with_refusal(connection, path, |label| {
        CoreError::Storage(format!(
            "catalog: {} has an unsupported {label} schema; this build has no migration reader",
            path.display()
        ))
    })
}

pub(super) fn verify_schema_with_refusal(
    connection: &Connection,
    path: &Path,
    refusal: impl Fn(&str) -> CoreError,
) -> Result<(), CoreError> {
    crate::connection::verify_installed_schema_objects_with_refusal(
        connection,
        path,
        SCHEMA,
        OBJECTS,
        |label| refusal(label),
    )?;
    let compact = |sql: &str| sql.split_whitespace().collect::<String>();
    for name in TRIGGERS {
        let prefix = format!("CREATE TRIGGER IF NOT EXISTS {name}");
        let start = SCHEMA.find(prefix.as_str()).ok_or_else(|| {
            CoreError::Storage(format!(
                "catalog: canonical Source stage schema lacks {name}"
            ))
        })?;
        let tail = &SCHEMA[start..];
        let end = tail.find("END;").ok_or_else(|| {
            CoreError::Storage(format!(
                "catalog: canonical Source stage trigger {name} is incomplete"
            ))
        })?;
        let expected =
            tail[..end + 3].replacen("CREATE TRIGGER IF NOT EXISTS", "CREATE TRIGGER", 1);
        let installed: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='trigger' AND name=?1",
                params![*name],
                |row| row.get(0),
            )
            .map_err(|error| engine_error("read native Source stage trigger", path, &error))?;
        if compact(&installed) != compact(&expected) {
            return Err(refusal("native Source stage replay trigger"));
        }
    }
    Ok(())
}

fn ensure_schema(connection: &Connection, path: &Path) -> Result<(), IndexError> {
    connection
        .execute_batch(SCHEMA)
        .map_err(|error| sql(&error))?;
    verify_schema(connection, path).map_err(|error| IndexError::Storage(error.to_string()))
}

fn require_current_stage(
    connection: &Connection,
    ingress: IngressRef,
    must_be_open: bool,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        reads: 3,
        ..WorkUsage::default()
    })?;
    let operation_exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM native_operation_v1 WHERE operation=?1)",
            params![operation_key(ingress.operation)?],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    if operation_exists {
        return Err(IndexError::Conflict);
    }
    let current = current_view(connection, ingress.operation.workspace)?
        .ok_or(IndexError::SnapshotUnavailable)?;
    if current.identity.epoch != ingress.operation.epoch {
        return Err(IndexError::FenceLost);
    }
    if must_be_open {
        require_open_ingress(connection, ingress)
    } else {
        check_ingress(connection, ingress).map(|_| ())
    }
}

fn require_confirmed_roots(
    connection: &Connection,
    receipt: SourceStageReplayReceiptV1,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let mut roots = [None, None, None];
    roots[0] = receipt.owners.root.0;
    if let Some(history) = receipt.context.history {
        roots[1] = Some(history.profile);
        roots[2] = Some(history.root);
    }
    for root in roots.into_iter().flatten() {
        budget.charge(WorkUsage {
            reads: 2,
            bytes: 128,
            ..WorkUsage::default()
        })?;
        if !confirmed_ingress_object(connection, receipt.ingress.operation, root)? {
            return Err(IndexError::InvalidInput(
                "Source stage root is not confirmed by its ingress".into(),
            ));
        }
    }
    Ok(())
}

fn read_row(
    connection: &Connection,
    operation: OperationKey,
    correlation: quanta_index_core::index::Digest,
    budget: &mut WorkBudget,
) -> Result<Option<SourceStageReplayReceiptV1>, IndexError> {
    let key = operation_key(operation)?;
    let _metadata_memory = budget.reserve_memory(256, MemoryLifetime::Temporary)?;
    budget.charge(WorkUsage {
        reads: 1,
        bytes: 128,
        ..WorkUsage::default()
    })?;
    let metadata: Option<(i64, Option<Vec<u8>>, Option<i64>, Option<i64>)> = connection
        .query_row(
            "SELECT fence,
                    CASE WHEN typeof(correlation)='blob' AND length(correlation)=32 THEN correlation END,
                    CASE WHEN typeof(payload)='blob' AND length(payload) BETWEEN 1 AND ?2 THEN length(payload) END,
                    CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN length(row_digest) END
             FROM native_source_stage_replay_v1 WHERE operation=?1",
            params![&key, MAX_PAYLOAD_BYTES],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let (fence, row_correlation, payload_len) = match metadata {
        None => return Ok(None),
        Some((fence, Some(row_correlation), Some(payload_len), Some(32))) if fence > 0 => {
            (fence, row_correlation, payload_len)
        }
        Some(_) => {
            return Err(IndexError::Storage(
                "Source stage replay row shape invalid".into(),
            ));
        }
    };
    if row_correlation.as_slice() != &correlation.0[..] {
        return Ok(None);
    }
    let payload_bytes = u64::try_from(payload_len).map_err(|_| IndexError::ResourceLimited)?;
    let _payload_memory = budget.reserve_memory(
        payload_bytes
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(256))
            .ok_or(IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    budget.charge(WorkUsage {
        reads: 1,
        bytes: payload_bytes
            .checked_add(32)
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let (payload, row_digest): (Vec<u8>, Vec<u8>) = connection
        .query_row(
            "SELECT payload,row_digest FROM native_source_stage_replay_v1
             WHERE operation=?1 AND fence=?2 AND correlation=?3",
            params![&key, fence, &row_correlation],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?
        .ok_or(IndexError::FenceLost)?;
    if payload.len() != usize::try_from(payload_len).map_err(|_| IndexError::ResourceLimited)?
        || row_digest.len() != 32
    {
        return Err(IndexError::Storage(
            "Source stage replay row changed shape".into(),
        ));
    }
    check_digest(DOMAIN, &payload, &row_digest)?;
    let receipt: SourceStageReplayReceiptV1 = decode(&payload)?;
    if encode(&receipt)? != payload
        || receipt.ingress.operation != operation
        || int(receipt.ingress.fence)? != fence
        || receipt.context.correlation != correlation
    {
        return Err(IndexError::Storage(
            "Source stage replay row identity mismatch".into(),
        ));
    }
    require_current_stage(connection, receipt.ingress, false, budget)?;
    require_confirmed_roots(connection, receipt, budget)?;
    Ok(Some(receipt))
}

pub(super) fn persist(
    catalog: &SqliteCatalog,
    receipt: SourceStageReplayReceiptV1,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let _payload_memory =
        budget.reserve_memory(MAX_PAYLOAD_BYTES as u64, MemoryLifetime::Temporary)?;
    let payload = encode(&receipt)?;
    let payload_bytes = u64::try_from(payload.len()).map_err(|_| IndexError::ResourceLimited)?;
    if payload.is_empty() || payload_bytes > u64::try_from(MAX_PAYLOAD_BYTES).unwrap() {
        return Err(IndexError::ResourceLimited);
    }
    budget.charge(WorkUsage {
        reads: 1,
        writes: 1,
        bytes: payload_bytes
            .checked_add(256)
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let mut guard = catalog.lock_native_budget(budget)?;
    let tx = begin(&mut guard)?;
    let install_schema = !catalog
        .native_source_stage_schema
        .load(std::sync::atomic::Ordering::Acquire);
    if install_schema {
        ensure_schema(&tx, catalog.path())?;
    }
    require_current_stage(&tx, receipt.ingress, true, budget)?;
    require_confirmed_roots(&tx, receipt, budget)?;
    let key = operation_key(receipt.ingress.operation)?;
    let checksum = digest(DOMAIN, &payload);
    let inserted = tx.execute(
        "INSERT OR IGNORE INTO native_source_stage_replay_v1(operation,fence,correlation,payload,row_digest)
         VALUES (?1,?2,?3,?4,?5)",
        params![&key, int(receipt.ingress.fence)?, &receipt.context.correlation.0[..], &payload, &checksum[..]],
    ).map_err(|error| sql(&error))?;
    at_most_one(inserted, "persist Source stage replay receipt")?;
    if inserted == 0
        && read_row(
            &tx,
            receipt.ingress.operation,
            receipt.context.correlation,
            budget,
        )? != Some(receipt)
    {
        return Err(IndexError::DigestConflict);
    }
    budget.checkpoint()?;
    tx.commit().map_err(|error| sql(&error))?;
    if install_schema {
        catalog
            .native_source_stage_schema
            .store(true, std::sync::atomic::Ordering::Release);
    }
    Ok(())
}

pub(super) fn read(
    catalog: &SqliteCatalog,
    operation: OperationKey,
    correlation: quanta_index_core::index::Digest,
    budget: &mut WorkBudget,
) -> Result<Option<SourceStageReplayReceiptV1>, IndexError> {
    budget.checkpoint()?;
    if !catalog
        .native_source_stage_schema
        .load(std::sync::atomic::Ordering::Acquire)
    {
        return Ok(None);
    }
    let mut guard = catalog.lock_native_budget(budget)?;
    let tx = begin(&mut guard)?;
    let receipt = read_row(&tx, operation, correlation, budget)?;
    budget.checkpoint()?;
    tx.commit().map_err(|error| sql(&error))?;
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingress_rotation_and_deletion_invalidate_source_stage_receipts() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(super::super::SCHEMA).unwrap();
        connection.execute_batch(SCHEMA).unwrap();
        verify_schema(&connection, Path::new(":memory:")).unwrap();
        let inserted = connection
            .execute(
                "INSERT INTO native_ingress_v1(operation,workspace,epoch,fence,payload,row_digest)
             VALUES (?1,?2,0,1,?3,?4)",
                params![
                    &b"operation"[..],
                    &b"workspace"[..],
                    &b"ingress"[..],
                    vec![1_u8; 32]
                ],
            )
            .unwrap();
        assert_eq!(inserted, 1);
        let insert_receipt = || {
            let inserted = connection.execute(
                "INSERT INTO native_source_stage_replay_v1(operation,fence,correlation,payload,row_digest)
                 VALUES (?1,(SELECT fence FROM native_ingress_v1 WHERE operation=?1),?2,?3,?4)",
                params![&b"operation"[..], vec![2_u8; 32], &b"receipt"[..], vec![3_u8; 32]],
            ).unwrap();
            assert_eq!(inserted, 1);
        };
        let count = || -> i64 {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM native_source_stage_replay_v1",
                    [],
                    |row| row.get(0),
                )
                .unwrap()
        };
        insert_receipt();
        assert_eq!(count(), 1);
        let rotated = connection
            .execute(
                "UPDATE native_ingress_v1 SET fence=2 WHERE operation=?1",
                params![&b"operation"[..]],
            )
            .unwrap();
        assert_eq!(rotated, 1);
        assert_eq!(count(), 0);
        insert_receipt();
        let deleted = connection
            .execute(
                "DELETE FROM native_ingress_v1 WHERE operation=?1",
                params![&b"operation"[..]],
            )
            .unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(count(), 0);
        connection
            .execute_batch(
                "DROP TRIGGER native_source_stage_replay_rotate_v1;
             CREATE TRIGGER native_source_stage_replay_rotate_v1
             AFTER UPDATE OF fence ON native_ingress_v1 BEGIN SELECT 1; END;",
            )
            .unwrap();
        assert!(verify_schema(&connection, Path::new(":memory:")).is_err());
    }
}
