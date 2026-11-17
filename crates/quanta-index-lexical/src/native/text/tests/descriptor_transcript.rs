//! Independent fixed transcript for DirectoryAlias selector DATA.

use super::*;
use quanta_index_core::index::{Digest, ObjectRef, OrdinalMapEdit, WorkLimits};
use std::time::{Duration, Instant};

fn budget() -> WorkBudget {
    WorkBudget::new(
        WorkLimits {
            reads: 1_000,
            writes: 1_000,
            bytes: 1_000_000,
            mutations: 1_000,
        },
        Instant::now() + Duration::from_secs(60),
    )
}

fn selector(data: [u8; 3]) -> Result<OrdinalMapEdit, IndexError> {
    OrdinalMapEdit::unscoped(
        0,
        MapEdit {
            key: b"\x01z".to_vec(),
            expected: None,
            next: Some(MapValue::inline(
                &data,
                Some(ObjectRef(Digest([0xa5; 32]))),
            )?),
        },
    )
}

#[test]
fn directory_descriptor_stamp_binds_fixed_wire_and_same_child_mutations() -> Result<(), IndexError>
{
    let baseline = selector([1, 0, 2])?;
    let encoded = baseline.next()?.ok_or(IndexError::Conflict)?.encoded()?;
    let mut fixed_wire = vec![1, 3, 1, 0, 2, 1];
    fixed_wire.extend_from_slice(&[0xa5; 32]);
    assert_eq!(encoded.as_slice()?, fixed_wire.as_slice());

    let mut stamp = entry_patch::Edits::new();
    let mut work = budget();
    stamp.push_descriptor(&baseline, &mut work)?;
    let golden = stamp.finish();
    assert_eq!(golden.count, 1);
    // SHA-256 of the independently fixed domain, key length/key, CAS flags,
    // 38-byte MapValue wire length, and the exact inline descriptor wire.
    assert_eq!(
        golden.digest,
        Digest([
            0x2a, 0x26, 0x42, 0x3b, 0xc7, 0xd5, 0x77, 0x88, 0x41, 0x0c, 0x9f, 0xd7, 0x1e, 0x06,
            0x2c, 0xc4, 0xcc, 0x62, 0x62, 0x6f, 0xb6, 0x32, 0x37, 0x73, 0xf6, 0x17, 0x88, 0xf6,
            0x91, 0xfb, 0x7e, 0x52,
        ])
    );
    for changed in [[2, 0, 1], [1, 0, 3]] {
        let mut alternate = entry_patch::Edits::new();
        alternate.push_descriptor(&selector(changed)?, &mut budget())?;
        let changed_stamp = alternate.finish();
        assert_eq!(changed_stamp.count, golden.count);
        assert_ne!(changed_stamp.digest, golden.digest);
    }
    Ok(())
}
