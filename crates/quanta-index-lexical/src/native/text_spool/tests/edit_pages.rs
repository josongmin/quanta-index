use super::*;
use quanta_index_core::index::MapValue;

fn owner_key(index: u16) -> Vec<u8> {
    let mut key = vec![0; 30];
    key.extend_from_slice(&index.to_be_bytes());
    key
}

fn executions(spool: &TextSpool, queries: &[&str]) -> Result<i64, IndexError> {
    let db = spool.db.scoped()?;
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
                root,
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
    let posting_sql = format!("INSERT INTO posting(term,k,p,n) VALUES {suffix}");
    let run_count = |statement_sql: &str| -> Result<i32, IndexError> {
        let db = spool.db.scoped()?;
        let statement = db
            .prepare_cached(statement_sql)
            .map_err(|error| sql(&error))?;
        Ok(statement.get_status(rusqlite::StatementStatus::Run))
    };
    check_eq!(run_count(&inventory_sql)?, 0);
    check_eq!(run_count(&posting_sql)?, 0);
    spool.inventory_batch(&inventories, &mut work)?;
    spool.posting_batch(&postings, &mut work)?;
    check_eq!(run_count(&inventory_sql)?, 1);
    check_eq!(run_count(&posting_sql)?, 1);
    let mut inventory = spool.inventory_cursor(32)?;
    for root in 0..32_u32 {
        for surface in [0_u8, 1_u8] {
            let row = inventory
                .next_edit(&mut work)?
                .ok_or(IndexError::ResourceLimited)?;
            check_eq!(row.root, root);
            check_eq!(row.edit.key.as_slice(), &[surface, b'a']);
            check_eq!(row.edit.expected, None);
            check_eq!(row.edit.next, Some(reference.into()));
        }
    }
    check!(inventory.next_edit(&mut work)?.is_none());
    for surface in [0_u8, 1_u8] {
        let term = [surface, b'a'];
        let mut rows = spool.posting_root_cursor(&term);
        for root in 0..32_u32 {
            let row = rows
                .next_edit(&mut work)?
                .ok_or(IndexError::ResourceLimited)?;
            check_eq!(
                row.edit.key.as_slice(),
                owner_key(u16::try_from(root).map_err(|_error| IndexError::ResourceLimited)?)
            );
            check_eq!(row.edit.next, Some(reference.into()));
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
        spool.inventory_batch(&[(0, term_edit.clone())], &mut setup)?;
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
    for kind in 0..5 {
        let queries: &[&str] = match kind {
            0 => &[INVENTORY_FIRST_SQL, INVENTORY_NEXT_SQL],
            1 => &[INVENTORY_ROOT_FIRST_SQL, INVENTORY_ROOT_NEXT_SQL],
            2 | 3 => &[POSTING_ROOT_FIRST_SQL, POSTING_ROOT_NEXT_SQL],
            _ => &[TOP_NEXT_SQL],
        };
        let before_executions = executions(&spool, queries)?;
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
        let mut cursor: Box<dyn MapEditCursor + '_> = match kind {
            0 => Box::new(spool.inventory_cursor(1)?),
            1 => Box::new(spool.inventory_root_cursor(0)?),
            2 => Box::new(spool.posting_cursor(spool.posting_term_page(None, &mut scan)?)),
            3 => Box::new(spool.posting_root_cursor(b"\0bulk")),
            _ => Box::new(spool.top_cursor()),
        };
        let mut observed = Vec::new();
        while let Some(row) = cursor.next_edit(&mut scan)? {
            observed.push((
                row.root,
                row.edit.key.clone(),
                row.edit.expected,
                row.edit.next,
            ));
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
        let calls = executions(&spool, queries)?
            .checked_sub(before_executions)
            .ok_or(IndexError::ResourceLimited)?;
        // Independent SQLite execution counters prove fewer real seeks, not
        // merely a change in the WorkUsage fee calculation.
        check!(
            if kind == 4 {
                (4..=8).contains(&calls)
            } else {
                (2..=4).contains(&calls)
            },
            "kind={kind},calls={calls}"
        );
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
