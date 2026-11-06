//! Independent bounds and byte goldens for allocation-free registration checks.

use quanta_index_core::index::{Digest, IndexError, ObjectRef, RegisteredObject};
use sha2::{Digest as _, Sha256};

use super::super::{object_install, registration_codec};

fn reference(byte: u8) -> ObjectRef {
    ObjectRef(Digest([byte; 32]))
}

fn object(bytes: u64, children: Vec<ObjectRef>) -> RegisteredObject {
    RegisteredObject::unscoped(reference(0x11), bytes, children)
}

fn check(condition: bool) -> Result<(), IndexError> {
    if condition {
        Ok(())
    } else {
        Err(IndexError::BackendViolation(
            "checked registration golden differs".into(),
        ))
    }
}

#[test]
fn checked_registration_refuses_exact_shape_boundaries() -> Result<(), IndexError> {
    let invalid = [
        object(0, vec![]),
        object(16_908_305, vec![]),
        object(1, vec![reference(0x22); 4_097]),
    ];
    for input in invalid {
        check(matches!(
            object_install::checked_object(&input),
            Err(IndexError::ResourceLimited)
        ))?;
    }
    let self_edge = object(1, vec![reference(0x22), reference(0x11)]);
    check(matches!(
        object_install::checked_object(&self_edge),
        Err(IndexError::InvalidInput(_))
    ))
}

#[test]
fn checked_registration_preserves_order_and_duplicate_children() -> Result<(), IndexError> {
    let input = object(
        16_908_304,
        vec![reference(0x44), reference(0x22), reference(0x44)],
    );
    let checked = object_install::checked_object(&input)?;
    check(checked.reference == reference(0x11))?;
    check(checked.bytes == 16_908_304)?;
    check(checked.children == vec![reference(0x44), reference(0x22), reference(0x44)])?;
    check(input == checked)
}

#[test]
fn maximum_registration_keeps_fixed_wire_and_checksum() -> Result<(), IndexError> {
    // 44 header bytes + 4,096 ordered child slots of 32 bytes = 131,116.
    check(registration_codec::MAX_ENCODED_BYTES == 131_116)?;
    let input = object(16_908_304, vec![reference(0x33); 4_096]);
    let checked = object_install::checked_object(&input)?;
    check(checked.children.len() == 4_096)?;
    check(
        checked
            .children
            .iter()
            .all(|child| *child == reference(0x33)),
    )?;

    let encoded = registration_codec::encode(&checked)?;
    let mut golden = vec![0x11; 32];
    golden.extend_from_slice(&[0, 0, 0, 0, 1, 2, 0, 16]);
    golden.extend_from_slice(&[0, 0, 16, 0]);
    golden.extend(std::iter::repeat_n(0x33, 4_096 * 32));
    check(golden.len() == 131_116)?;
    check(encoded == golden)?;

    let mut hasher = Sha256::new();
    hasher.update(b"qidx.native.object.v1");
    hasher.update(&encoded);
    check(
        <[u8; 32]>::from(hasher.finalize())
            == [
                0xaa, 0x3b, 0xb9, 0xf1, 0x69, 0x19, 0xf3, 0xdd, 0x70, 0xe0, 0x54, 0x45, 0xb8, 0x4b,
                0xd6, 0x2f, 0x8a, 0xeb, 0xd6, 0x13, 0xb9, 0x7d, 0xbf, 0x4b, 0x03, 0xb4, 0x3c, 0x03,
                0x94, 0xc9, 0xb3, 0x6b,
            ],
    )
}
