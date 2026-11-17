use super::*;
use quanta_index_core::index::MapValue;

fn owner_key(index: u16) -> Vec<u8> {
    let mut key = vec![0; 30];
    key.extend_from_slice(&index.to_be_bytes());
    key
}

fn executions(spool: &TextSpool, queries: &[&str], budget: &WorkBudget) -> Result<i64, IndexError> {
    let db = spool.db.scoped(budget)?;
    queries.iter().try_fold(0_i64, |total, query| {
        let statement = db.prepare_cached(query).map_err(|error| sql(&error))?;
        total
            .checked_add(i64::from(
                statement.get_status(rusqlite::StatementStatus::Run),
            ))
            .ok_or(IndexError::ResourceLimited)
    })
}

#[test]
fn mixed_owner_edits_use_one_real_insert_per_64_row_packet() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let reference = ObjectRef(quanta_index_core::index::Digest([23; 32]));
    let mut inventories = Vec::new();
    let mut postings = Vec::new();
    for root in 0..32_u32 {
        let owner = owner_key(u16::try_from(root).map_err(|_error| IndexError::ResourceLimited)?);
        for surface in [0_u8, 1_u8] {
            inventories.push((
                InventoryPartition::Ready(root),
                MapEdit {
                    key: vec![surface, b'a'],
                    expected: None,
                    next: Some(reference.into()),
                },
            ));
            postings.push((
                vec![surface, b'a'],
                MapEdit {
                    key: owner.clone(),
                    expected: None,
                    next: Some(reference.into()),
                },
            ));
        }
    }
    let suffix = std::iter::repeat_n("(?,?,?,?)", 64)
        .collect::<Vec<_>>()
        .join(",");
    let inventory_sql = format!("INSERT INTO inventory(root,k,p,n) VALUES {suffix}");
    let run_count = |statement_sql: &str, budget: &WorkBudget| -> Result<i32, IndexError> {
        let db = spool.db.scoped(budget)?;
        let statement = db
            .prepare_cached(statement_sql)
            .map_err(|error| sql(&error))?;
        Ok(statement.get_status(rusqlite::StatementStatus::Run))
    };
    check_eq!(run_count(&inventory_sql, &work)?, 0);
    spool.inventory_batch(&inventories, &mut work)?;
    spool.posting_batch(&postings, &mut work)?;
    check_eq!(run_count(&inventory_sql, &work)?, 1);
    let terms = spool.posting_term_page(None, &mut work)?;
    let packets: i64 = spool
        .db
        .scoped(&work)?
        .query_row(
            "SELECT COUNT(*) FROM posting_packet WHERE phase=0 AND run=0",
            [],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    check_eq!(packets, 1);
    let mut inventory = spool.inventory_cursor(32)?;
    for root in 0..32_u32 {
        for surface in [0_u8, 1_u8] {
            let row = inventory
                .next_edit(&mut work)?
                .ok_or(IndexError::ResourceLimited)?;
            check_eq!(row.root(), root);
            check_eq!(row.key(), &[surface, b'a']);
            check_eq!(row.expected()?, None);
            check_eq!(row.next()?, Some(reference.into()));
        }
    }
    check!(inventory.next_edit(&mut work)?.is_none());
    let mut roots = spool.posting_root_cursors(&terms, &[false, false], &mut work)?;
    for (slot, surface) in [0_u8, 1_u8].into_iter().enumerate() {
        let term = terms
            .get(slot)
            .ok_or(IndexError::ResourceLimited)?
            .as_slice();
        let expected_term = [surface, b'a'];
        check_eq!(term, expected_term.as_slice());
        let mut rows = roots.cursor(slot, term)?;
        for root in 0..32_u32 {
            let row = rows
                .next_edit(&mut work)?
                .ok_or(IndexError::ResourceLimited)?;
            check_eq!(
                row.key(),
                owner_key(u16::try_from(root).map_err(|_error| IndexError::ResourceLimited)?)
            );
            check_eq!(row.next()?, Some(reference.into()));
        }
        check!(rows.next_edit(&mut work)?.is_none());
    }
    Ok(())
}

#[test]
fn all_edit_cursors_keep_129_row_oracle_with_bounded_read_bytes() -> Result<(), IndexError> {
    let mut setup = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut setup)?;
    let reference = ObjectRef(quanta_index_core::index::Digest([11; 32]));
    let mut term_rows = Vec::new();
    let mut owner_rows = Vec::new();
    let mut owner_top = Vec::new();
    for index in 0_u16..129 {
        let term = format!("\0key-{index:03}").into_bytes();
        let owner = owner_key(index);
        let term_edit = MapEdit {
            key: term.clone(),
            expected: None,
            next: Some(reference.into()),
        };
        let owner_edit = MapEdit {
            key: owner.clone(),
            expected: None,
            next: Some(reference.into()),
        };
        let value = MapValue::inline(&index.to_be_bytes(), [reference; 5])?;
        spool.inventory_batch(
            &[(InventoryPartition::Ready(0), term_edit.clone())],
            &mut setup,
        )?;
        spool.posting_batch(&[(b"\0bulk".to_vec(), owner_edit)], &mut setup)?;
        spool.top_batch(
            &[
                (0, term_edit),
                (
                    1,
                    MapEdit {
                        key: owner.clone(),
                        expected: None,
                        next: Some(value),
                    },
                ),
            ],
            &mut setup,
        )?;
        term_rows.push((0, term, None, Some(reference.into())));
        owner_rows.push((0, owner.clone(), None, Some(reference.into())));
        owner_top.push((1, owner, None, Some(value)));
    }
    let _sealed = spool.posting_term_page(None, &mut setup)?;
    for kind in 0..5 {
        let queries: &[&str] = match kind {
            0 => &[INVENTORY_FIRST_SQL, INVENTORY_NEXT_SQL],
            1 => &[INVENTORY_ROOT_FIRST_SQL, INVENTORY_ROOT_NEXT_SQL],
            2 | 3 => &[],
            _ => &[TOP_NEXT_SQL],
        };
        let before_executions = executions(&spool, queries, &setup)?;
        let mut scan = WorkBudget::new(
            WorkLimits {
                reads: 512,
                writes: 0,
                bytes: if kind == 4 { 256 * 1024 } else { 128 * 1024 },
                mutations: 0,
            },
            Instant::now()
                .checked_add(Duration::from_secs(10))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let root_terms = if kind == 3 {
            Some(spool.posting_term_page(None, &mut scan)?)
        } else {
            None
        };
        let mut roots = root_terms
            .as_ref()
            .map(|terms| spool.posting_root_cursors(terms, &[false], &mut scan))
            .transpose()?;
        let mut cursor: Box<dyn MapEditCursor + '_> = match kind {
            0 => Box::new(spool.inventory_cursor(1)?),
            1 => Box::new(spool.inventory_root_cursor(0)?),
            2 => Box::new(spool.posting_cursor(spool.posting_term_page(None, &mut scan)?)),
            3 => Box::new(
                roots
                    .as_mut()
                    .ok_or(IndexError::ResourceLimited)?
                    .cursor(0, b"\0bulk")?,
            ),
            _ => Box::new(spool.top_cursor()),
        };
        let mut observed = Vec::new();
        while let Some(row) = cursor.next_edit(&mut scan)? {
            observed.push((row.root(), row.key().to_vec(), row.expected()?, row.next()?));
        }
        let expected = match kind {
            0 | 1 => term_rows.clone(),
            2 | 3 => owner_rows.clone(),
            _ => term_rows
                .iter()
                .cloned()
                .chain(owner_top.iter().cloned())
                .collect(),
        };
        check_eq!(observed, expected);
        let calls = executions(&spool, queries, &setup)?
            .checked_sub(before_executions)
            .ok_or(IndexError::ResourceLimited)?;
        // The old SQL seek count applies only to inventory and top. Posting
        // rows now share checked packet pages; the fixed owner oracle above
        // still exercises both posting cursors under the original read cap.
        if kind == 2 || kind == 3 {
            check_eq!(calls, 0);
        } else {
            check!(
                if kind == 4 {
                    (4..=8).contains(&calls)
                } else {
                    (2..=4).contains(&calls)
                },
                "kind={kind},calls={calls}"
            );
        }
        let finished = scan.usage();
        check!(cursor.next_edit(&mut scan)?.is_none());
        check!(cursor.next_edit(&mut scan)?.is_none());
        check_eq!(scan.usage(), finished);
        scan.request().cancel_handle().cancel();
        check!(matches!(
            cursor.next_edit(&mut scan),
            Err(IndexError::Cancelled)
        ));
    }
    Ok(())
}
