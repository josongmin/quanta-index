use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn branch(edge: u8) -> Branch {
    Branch::indirect(ObjectRef(quanta_index_core::index::Digest([edge; 32])))
}

#[test]
fn sparse_branches_preserve_nibble_order_across_reverse_insert_remove_and_replace() -> TestResult {
    let mut work = budget()?;
    let tracker = array_scope(MemoryLifetime::Retained, None, &mut work);
    let mut branches = Branches::new();
    for edge in (0_u8..16).rev() {
        check!(
            branches
                .replace(usize::from(edge), Some(branch(edge)), &mut work)?
                .is_none()
        );
    }
    check_eq!(branches.len(), 16);
    for (edge, value) in branches.iter() {
        check!(value == &branch(u8::try_from(edge)?));
    }
    for edge in [0, 15, 7, 8] {
        check!(
            branches
                .replace(usize::from(edge), None, &mut work)?
                .as_ref()
                == Some(&branch(edge))
        );
        check!(branches.get(usize::from(edge))?.is_none());
    }
    let keys = branches.iter().map(|(edge, _)| edge).collect::<Vec<_>>();
    check_eq!(keys, vec![1, 2, 3, 4, 5, 6, 9, 10, 11, 12, 13, 14]);
    let _old = branches.replace(4, Some(branch(42)), &mut work)?;
    check!(branches.get(4)? == Some(&branch(42)));
    let before = branches.len();
    check!(matches!(
        branches.replace(16, Some(branch(16)), &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(branches.len(), before);
    check!(branches.get(4)? == Some(&branch(42)));
    check!(branches.into_child(4)?.as_ref() == Some(&branch(42)));
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn sparse_branch_growth_refusal_preserves_old_backing_and_last_drop_refunds() -> TestResult {
    let mut work = budget()?;
    let two = std::mem::size_of::<Branch>()
        .checked_mul(2)
        .ok_or(IndexError::ResourceLimited)?;
    let tracker = array_scope(
        MemoryLifetime::Temporary,
        Some(u64::try_from(two)?),
        &mut work,
    );
    let mut branches = Branches::new();
    let _old = branches.replace(15, Some(branch(15)), &mut work)?;
    let held = tracker.live.load(Ordering::Relaxed);
    check_eq!(held, u64::try_from(std::mem::size_of::<Branch>())?);
    check!(matches!(
        branches.replace(0, Some(branch(0)), &mut work),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(tracker.refused.load(Ordering::Relaxed), 1);
    check_eq!(tracker.live.load(Ordering::Relaxed), held);
    check_eq!(branches.len(), 1);
    check!(branches.get(15)? == Some(&branch(15)) && branches.get(0)?.is_none());
    let _old = branches.replace(15, None, &mut work)?;
    check_eq!(branches.len(), 0);
    // Removal does not release an allocation that is still held by the vector.
    check_eq!(tracker.live.load(Ordering::Relaxed), held);
    drop(branches);
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn terminal_frames_have_no_sibling_allocation_and_small_fixed_layout() -> TestResult {
    let mut work = budget()?;
    let tracker = array_scope(MemoryLifetime::Temporary, None, &mut work);
    let scalar = MapValue::inline(b"fixed", [])?;
    let node = Node::leaf(&[], scalar, &work)?;
    let frame = BootstrapFrame::new(MapBuffer::new(0), Some(scalar));
    check_eq!(node.branch_count(), 0);
    check_eq!(frame.branches.capacity_bytes()?, 0);
    check_eq!(node.branches.capacity_bytes()?, 0);
    check_eq!(tracker.refused.load(Ordering::Relaxed), 0);
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);
    // Prevent reintroducing one worst-case16-slot array in every leaf/frame.
    check!(std::mem::size_of::<BootstrapFrame>() <= 512);
    check!(std::mem::size_of::<EditFrame>() <= 512);
    let decoded = Branches::for_decode(0x8001, &work);
    check!(matches!(decoded, Err(IndexError::ResourceLimited)));
    check_eq!(tracker.refused.load(Ordering::Relaxed), 1);
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn sparse_branch_work_refusal_precedes_backing_birth() -> TestResult {
    let mut work = WorkBudget::new(
        quanta_index_core::index::WorkLimits {
            reads: 100,
            writes: 100,
            bytes: 0,
            mutations: 100,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    );
    let tracker = array_scope(MemoryLifetime::Temporary, None, &mut work);
    let mut branches = Branches::new();
    check!(matches!(
        branches.replace(0, Some(branch(0)), &mut work),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(branches.len(), 0);
    check_eq!(branches.capacity_bytes()?, 0);
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);
    // The work byte check must fail before trying the deliberately denied allocation.
    check_eq!(tracker.refused.load(Ordering::Relaxed), 0);
    Ok(())
}
