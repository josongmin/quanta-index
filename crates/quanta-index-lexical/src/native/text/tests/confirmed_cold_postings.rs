//! Attempt-local dense-root memo: exact content, Original custody, and fences.

use super::*;
use crate::native::text::dense_posting::{wire, write};

fn attempt() -> Result<AttemptRef, IndexError> {
    Ok(test_attempt(
        visitor_fixture()?.ticket.view.identity.workspace,
    ))
}

fn rows(count: u8) -> Vec<wire::PostingRow> {
    (1..=count)
        .map(|id| wire::PostingRow {
            owner: [id; 32],
            witness: [41; 32],
        })
        .collect()
}

fn finished_draft(
    rows: &[wire::PostingRow],
    prior: Option<wire::Directory>,
    work: &mut WorkBudget,
) -> Result<write::Draft, IndexError> {
    let objects = TestObjects::default();
    let maps = TestMaps::default();
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut context = AttemptContext {
        attempt: attempt()?,
        objects: &objects,
        maps: &maps,
        custody: &custody,
        scratch: &scratch,
        budget: work,
    };
    let mut draft = write::Draft::new(
        prior,
        u64::try_from(rows.len()).map_err(|_error| IndexError::ResourceLimited)?,
        context.budget,
    )?;
    for row in rows {
        draft.push(*row, &mut context)?;
    }
    draft.finish(&mut context)?;
    Ok(draft)
}

fn prior_directory(work: &WorkBudget) -> Result<wire::Directory, IndexError> {
    let rows = rows(9);
    let (_page, fence) = wire::page_payload(&rows, work)?;
    let payload = wire::directory_payload(&[fence], work)?.bytes;
    let directory_ref = reference(90);
    let child = reference(91);
    let bytes = payload
        .len()
        .checked_add(16 + 32)
        .ok_or(IndexError::ResourceLimited)?;
    let stored = StoredObject::new(
        RegisteredObject::unscoped(
            directory_ref,
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            vec![child],
        ),
        payload,
        None,
    );
    wire::decode_directory(stored, directory_ref, work)
}

#[test]
fn confirmed_cold_roots_optional_original_custody_and_attempt_fence() -> Result<(), IndexError> {
    let attempt = attempt()?;
    let unscoped = budget()?;
    check!(write::ConfirmedColdRoots::try_new(attempt, &unscoped)?.is_none());
    check_eq!(unscoped.first_denial(), None);

    let (refused, refused_memory) = funded_token_budget(1)?;
    check!(write::ConfirmedColdRoots::try_new(attempt, &refused)?.is_none());
    check_eq!(refused.first_denial(), None);
    check_eq!(refused_memory.live.load(Ordering::Relaxed), 0);
    check!(!refused_memory.invalid_release.load(Ordering::Relaxed));

    let (funded, memory) = funded_token_budget(64 * 1024)?;
    let mut memo = write::ConfirmedColdRoots::try_new(attempt, &funded)?
        .ok_or_else(|| IndexError::BackendViolation("funded cold-root memo absent".into()))?;
    check!(memory.live.load(Ordering::Relaxed) > 0);
    check_eq!(funded.first_denial(), None);

    let mut first = [0_u8; 32];
    first[..2].copy_from_slice(&[17, 29]);
    first[2] = 1;
    let mut colliding = first;
    colliding[2] = 2;
    let first_root = reference(71);
    let second_root = reference(72);
    memo.remember(first, first_root, attempt, &funded)?;
    check_eq!(memo.get(first, attempt, &funded)?, Some(first_root));
    // Same direct-map prefix is not evidence for a different complete key.
    check_eq!(memo.get(colliding, attempt, &funded)?, None);
    memo.remember(colliding, second_root, attempt, &funded)?;
    check_eq!(memo.get(colliding, attempt, &funded)?, Some(second_root));
    check!(memo.get(first, attempt, &funded)? != Some(second_root));

    let mut foreign = attempt;
    foreign.fence = foreign
        .fence
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    check!(matches!(
        memo.get(colliding, foreign, &funded),
        Err(IndexError::FenceLost)
    ));
    check!(matches!(
        memo.remember(colliding, first_root, foreign, &funded),
        Err(IndexError::FenceLost)
    ));
    check_eq!(memo.get(colliding, attempt, &funded)?, Some(second_root));
    drop(memo);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn dense_cold_key_binds_all_rows_and_rejects_unfinished_or_prior_drafts() -> Result<(), IndexError>
{
    let mut work = budget()?;
    let base = rows(9);
    let baseline = finished_draft(&base, None, &mut work)?.cold_key(&mut work)?;
    // Independent SHA-256 transcript for the fixed policy domain, page width,
    // row count, and nine owner/witness pairs.
    check_eq!(
        baseline,
        [
            0x6a, 0x20, 0xaf, 0xc9, 0x6d, 0x55, 0xb9, 0xd1, 0x4d, 0x87, 0x1c, 0x44, 0x78, 0x87,
            0x47, 0x6e, 0x45, 0xa5, 0xa5, 0xf4, 0xf6, 0x33, 0x9e, 0x73, 0xda, 0x36, 0xf2, 0x82,
            0xac, 0x8d, 0xd5, 0x16,
        ]
    );
    check_eq!(
        finished_draft(&base, None, &mut work)?.cold_key(&mut work)?,
        baseline
    );

    let mut changed_owner = base.clone();
    changed_owner[4].owner[31] = 6;
    check!(finished_draft(&changed_owner, None, &mut work)?.cold_key(&mut work)? != baseline);

    let mut changed_witness = base.clone();
    changed_witness[4].witness[31] ^= 1;
    check!(finished_draft(&changed_witness, None, &mut work)?.cold_key(&mut work)? != baseline);
    check!(finished_draft(&rows(10), None, &mut work)?.cold_key(&mut work)? != baseline);

    let unfinished = write::Draft::new(None, 9, &work)?;
    check!(unfinished.cold_key(&mut work).is_err());

    let prior = prior_directory(&work)?;
    let prior_draft = finished_draft(&base, Some(prior), &mut work)?;
    check!(prior_draft.cold_key(&mut work).is_err());
    Ok(())
}
