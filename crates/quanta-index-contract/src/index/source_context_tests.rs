//! Fixed SourceCut context fingerprints from the historical Kernel framing.
//! These values identify input bytes; they do not grant Source authority.

use super::{Digest, MapRoot, ObjectRef, SourceCut};

fn source(owners: Option<u8>, paths: Option<u8>, frontiers: Option<u8>) -> SourceCut {
    let root = |byte: Option<u8>| MapRoot(byte.map(|value| ObjectRef(Digest([value; 32]))));
    SourceCut {
        owners: root(owners),
        paths: root(paths),
        frontiers: root(frontiers),
    }
}

#[test]
fn empty_source_context_preserves_the_fixed_wire_and_kernel_gold() -> Result<(), serde_json::Error>
{
    let cut = source(None, None, None);
    assert_eq!(
        serde_json::to_string(&cut)?,
        r#"{"owners":null,"paths":null,"frontiers":null}"#
    );
    assert_eq!(
        cut.input_context_digest()?,
        Digest([
            0x8f, 0x28, 0xf5, 0x89, 0xfb, 0x1d, 0xad, 0x27, 0xb6, 0x64, 0xd8, 0x4b, 0xe1, 0xfe,
            0xcb, 0xc0, 0x7a, 0x4d, 0x88, 0xdc, 0x9a, 0x60, 0x65, 0x08, 0x7a, 0x63, 0xa7, 0x91,
            0x4f, 0x67, 0x3d, 0xa7,
        ])
    );
    Ok(())
}

#[test]
fn three_source_roots_preserve_the_fixed_wire_and_kernel_gold() -> Result<(), serde_json::Error> {
    let cut = source(Some(1), Some(2), Some(u8::MAX));
    assert_eq!(
        serde_json::to_string(&cut)?,
        r#"{"owners":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],"paths":[2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2],"frontiers":[255,255,255,255,255,255,255,255,255,255,255,255,255,255,255,255,255,255,255,255,255,255,255,255,255,255,255,255,255,255,255,255]}"#
    );
    assert_eq!(
        cut.input_context_digest()?,
        Digest([
            0xbd, 0x54, 0x7c, 0x94, 0x31, 0x6a, 0x41, 0x36, 0xf2, 0x2b, 0xe5, 0x6a, 0x8a, 0xe8,
            0xf3, 0x45, 0x18, 0x11, 0xee, 0xdc, 0x34, 0x86, 0x4f, 0xd4, 0xaf, 0x45, 0x05, 0x02,
            0x1f, 0x9d, 0xef, 0xbc,
        ])
    );
    Ok(())
}

#[test]
fn path_and_frontier_changes_change_context_with_the_same_owner() -> Result<(), serde_json::Error> {
    let original = source(Some(1), Some(2), Some(u8::MAX));
    let changed_paths = source(Some(1), Some(3), Some(u8::MAX));
    let changed_frontiers = source(Some(1), Some(2), Some(254));

    let original_digest = original.input_context_digest()?;
    let paths_digest = changed_paths.input_context_digest()?;
    let frontiers_digest = changed_frontiers.input_context_digest()?;

    assert_eq!(
        paths_digest,
        Digest([
            0x10, 0x91, 0x4b, 0x0f, 0x4b, 0x17, 0xf4, 0xa5, 0xbb, 0x86, 0xda, 0x8f, 0x37, 0x5f,
            0x57, 0x04, 0x47, 0x16, 0x08, 0x4a, 0x25, 0xcc, 0x29, 0x12, 0xbe, 0xd9, 0x30, 0xdb,
            0x33, 0x1e, 0x47, 0xaa,
        ])
    );
    assert_eq!(
        frontiers_digest,
        Digest([
            0x45, 0x1e, 0x6c, 0xc8, 0x2b, 0xaa, 0x02, 0x0b, 0x4e, 0xfd, 0x59, 0x38, 0x47, 0x5f,
            0xd2, 0x71, 0x10, 0x7f, 0xe5, 0x22, 0xa0, 0x7b, 0x5b, 0x49, 0xb7, 0x83, 0x07, 0x36,
            0xb0, 0x24, 0x6e, 0xfb,
        ])
    );
    assert_ne!(original_digest, paths_digest);
    assert_ne!(original_digest, frontiers_digest);
    assert_ne!(paths_digest, frontiers_digest);
    Ok(())
}
