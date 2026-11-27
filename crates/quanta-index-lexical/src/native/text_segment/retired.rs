//! Root magics written by retired Text wire versions.
//!
//! A root carrying one of these magics was written by an earlier release of
//! the same layout. It is stale, not corrupt: readers and builders refuse it
//! with the typed `RebuildRequired`, which the product routes to a rebuild
//! instead of quarantine or integrity handling. The structural and
//! logical-commitment check below only proves that the bytes really are such
//! a root for this recipe and member; a retired root is never read for
//! content. Unknown magics and malformed retired bytes stay `CorruptObject`.

use super::wire::{self, TextCut};
use quanta_index_core::index::{Digest, IndexError, MapRoot, ObjectRef, SourceCut};
use sha2::{Digest as _, Sha256};

/// The retired root encodings, by the layout that wrote them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::native::text) enum RetiredRootLayout {
    /// `SegmentFull` root that bound all three `SourceCut` roots, frontiers included.
    SegmentFullSourceCut,
    /// Pre-release layered root. No encoding or commitment of it was ever
    /// published, so the recipe policy is its only checkable binding.
    LayeredUncommitted,
    /// Layered root that bound all three `SourceCut` roots, frontiers included.
    LayeredSourceCut,
}

/// Every retired Text root magic with the layout that wrote it.
pub(in crate::native::text) const RETIRED_ROOT_MAGICS: [(&[u8; 8], RetiredRootLayout); 3] = [
    (b"QTXROOT1", RetiredRootLayout::SegmentFullSourceCut),
    (b"QTXROOT2", RetiredRootLayout::LayeredUncommitted),
    (b"QTXROOT3", RetiredRootLayout::LayeredSourceCut),
];

const LAYERED_LOGICAL_DOMAIN: &[u8] = b"qidx.native.text.segment.layered.logical.v3\0";

/// The retired layout of `payload`, if it carries a retired root magic.
pub(in crate::native::text) fn retired_layout(payload: &[u8]) -> Option<RetiredRootLayout> {
    let magic = payload.get(..8)?;
    RETIRED_ROOT_MAGICS
        .iter()
        .find(|(retired, _)| retired.as_slice() == magic)
        .map(|(_, layout)| *layout)
}

/// The typed refusal for a retired root, or `None` for any other magic.
///
/// A well-formed retired root of this recipe (and of the expected member
/// commitment, when the caller knows it) is `RebuildRequired`; retired bytes
/// that do not authenticate are `CorruptObject`.
pub(in crate::native::text) fn refuse_retired_root(
    payload: &[u8],
    recipe: Digest,
    children: &[ObjectRef],
    expected_logical: Option<Digest>,
) -> Option<IndexError> {
    let layout = retired_layout(payload)?;
    Some(
        match authenticate(layout, payload, recipe, children, expected_logical) {
            Ok(()) => IndexError::RebuildRequired,
            Err(error) => error,
        },
    )
}

fn authenticate(
    layout: RetiredRootLayout,
    payload: &[u8],
    recipe: Digest,
    children: &[ObjectRef],
    expected_logical: Option<Digest>,
) -> Result<(), IndexError> {
    let mut cursor = Cursor {
        bytes: payload,
        offset: 8,
    };
    if cursor.fixed::<32>()? != recipe.0 {
        return Err(wire::corrupt("retired root policy"));
    }
    match layout {
        RetiredRootLayout::LayeredUncommitted => Ok(()),
        RetiredRootLayout::SegmentFullSourceCut => {
            let source = cursor.source()?;
            let count = usize::from(cursor.u16()?);
            let mut edges = Edges::new(children, source_edges(source))?;
            let (mut docs, mut tokens) = (0_u64, 0_u64);
            for _ in 0..count {
                // Row: segment ref, first and last owner, docs, tokens.
                edges.expect(cursor.reference()?)?;
                cursor.skip(32 + 32)?;
                docs = docs
                    .checked_add(u64::from(cursor.u16()?))
                    .ok_or(IndexError::ResourceLimited)?;
                tokens = tokens
                    .checked_add(cursor.u64()?)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            cursor.finish()?;
            edges.finish()?;
            let logical = wire::logical_commitment(recipe, TextCut::of(source), docs, tokens);
            check_logical(expected_logical, logical)
        }
        RetiredRootLayout::LayeredSourceCut => {
            let source = cursor.source()?;
            let postings = cursor.optional()?;
            let path_docs = cursor.u64()?;
            let path_tokens = cursor.u64()?;
            let count = usize::from(cursor.u16()?);
            let mut edges = Edges::new(children, source_edges(source).chain(postings.0))?;
            let (mut docs, mut tokens) = (0_u64, 0_u64);
            for _ in 0..count {
                // Row: upper owner, kind, partition ref, live docs, live tokens.
                cursor.skip(32 + 1)?;
                edges.expect(cursor.reference()?)?;
                docs = docs
                    .checked_add(u64::from(cursor.u32()?))
                    .ok_or(IndexError::ResourceLimited)?;
                tokens = tokens
                    .checked_add(cursor.u64()?)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            cursor.finish()?;
            edges.finish()?;
            let mut hash = Sha256::new();
            hash.update(LAYERED_LOGICAL_DOMAIN);
            hash.update(recipe.0);
            for root in [source.owners, source.paths, source.frontiers] {
                match root.0 {
                    Some(reference) => {
                        hash.update([1]);
                        hash.update(reference.0.0);
                    }
                    None => hash.update([0]),
                }
            }
            hash.update(docs.to_be_bytes());
            hash.update(tokens.to_be_bytes());
            hash.update(path_docs.to_be_bytes());
            hash.update(path_tokens.to_be_bytes());
            check_logical(expected_logical, Digest(hash.finalize().into()))
        }
    }
}

fn check_logical(expected: Option<Digest>, actual: Digest) -> Result<(), IndexError> {
    if expected.is_some_and(|expected| expected != actual) {
        return Err(wire::corrupt("retired root logical commitment"));
    }
    Ok(())
}

fn bound() -> IndexError {
    wire::corrupt("retired root bound")
}

fn source_edges(source: SourceCut) -> impl Iterator<Item = ObjectRef> {
    [source.owners.0, source.paths.0, source.frontiers.0]
        .into_iter()
        .flatten()
}

/// The registered children must be exactly the prefix edges, then one edge
/// per row, in order.
struct Edges<'a> {
    children: std::slice::Iter<'a, ObjectRef>,
}

impl<'a> Edges<'a> {
    fn new(
        children: &'a [ObjectRef],
        prefix: impl Iterator<Item = ObjectRef>,
    ) -> Result<Self, IndexError> {
        let mut edges = Self {
            children: children.iter(),
        };
        for expected in prefix {
            edges.expect(expected)?;
        }
        Ok(edges)
    }

    fn expect(&mut self, expected: ObjectRef) -> Result<(), IndexError> {
        if self.children.next() != Some(&expected) {
            return Err(wire::corrupt("retired root edges"));
        }
        Ok(())
    }

    fn finish(mut self) -> Result<(), IndexError> {
        if self.children.next().is_some() {
            return Err(wire::corrupt("retired root edges"));
        }
        Ok(())
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl Cursor<'_> {
    fn take(&mut self, len: usize) -> Result<&[u8], IndexError> {
        let end = self.offset.checked_add(len).ok_or_else(bound)?;
        let value = self.bytes.get(self.offset..end).ok_or_else(bound)?;
        self.offset = end;
        Ok(value)
    }

    fn skip(&mut self, len: usize) -> Result<(), IndexError> {
        self.take(len).map(|_skipped| ())
    }

    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], IndexError> {
        self.take(N)?.try_into().map_err(|_width| bound())
    }

    fn u16(&mut self) -> Result<u16, IndexError> {
        Ok(u16::from_be_bytes(self.fixed()?))
    }

    fn u32(&mut self) -> Result<u32, IndexError> {
        Ok(u32::from_be_bytes(self.fixed()?))
    }

    fn u64(&mut self) -> Result<u64, IndexError> {
        Ok(u64::from_be_bytes(self.fixed()?))
    }

    fn reference(&mut self) -> Result<ObjectRef, IndexError> {
        Ok(ObjectRef(Digest(self.fixed()?)))
    }

    fn optional(&mut self) -> Result<MapRoot, IndexError> {
        match self.fixed::<1>()? {
            [0] => Ok(MapRoot(None)),
            [1] => Ok(MapRoot(Some(self.reference()?))),
            _ => Err(wire::corrupt("retired root tag")),
        }
    }

    fn source(&mut self) -> Result<SourceCut, IndexError> {
        Ok(SourceCut {
            owners: self.optional()?,
            paths: self.optional()?,
            frontiers: self.optional()?,
        })
    }

    fn finish(&self) -> Result<(), IndexError> {
        if self.offset != self.bytes.len() {
            return Err(wire::corrupt("retired root length"));
        }
        Ok(())
    }
}

/// Byte-for-byte retired roots for tests, written from the retired layouts
/// independently of the authentication code above.
#[cfg(test)]
pub(super) mod fixtures {
    use super::*;

    pub(in crate::native::text) const OWNERS: ObjectRef = ObjectRef(Digest([0x11; 32]));
    pub(in crate::native::text) const FRONTIERS: ObjectRef = ObjectRef(Digest([0x33; 32]));
    pub(in crate::native::text) const SEGMENT: ObjectRef = ObjectRef(Digest([0x44; 32]));

    /// Registered children shared by both fixtures.
    pub(in crate::native::text) const CHILDREN: [ObjectRef; 3] = [OWNERS, FRONTIERS, SEGMENT];

    /// A retired `SegmentFull` root over owners + frontiers with one segment of
    /// 3 docs and 9 tokens, and its member logical commitment.
    pub(in crate::native::text) fn full_root(recipe: Digest) -> (Vec<u8>, Digest) {
        let mut bytes = b"QTXROOT1".to_vec();
        bytes.extend_from_slice(&recipe.0);
        bytes.push(1);
        bytes.extend_from_slice(&OWNERS.0.0);
        bytes.push(0);
        bytes.push(1);
        bytes.extend_from_slice(&FRONTIERS.0.0);
        bytes.extend_from_slice(&[0, 1]);
        bytes.extend_from_slice(&SEGMENT.0.0);
        bytes.extend_from_slice(&[0x10; 32]);
        bytes.extend_from_slice(&[0x20; 32]);
        bytes.extend_from_slice(&[0, 3]);
        bytes.extend_from_slice(&9_u64.to_be_bytes());
        let mut hash = Sha256::new();
        hash.update(b"qidx.native.text.segment.full.logical.v1\0");
        hash.update(recipe.0);
        hash.update([1]);
        hash.update(OWNERS.0.0);
        hash.update(3_u64.to_be_bytes());
        hash.update(9_u64.to_be_bytes());
        (bytes, Digest(hash.finalize().into()))
    }

    /// A retired layered root over owners + frontiers, no path postings, one
    /// direct row of 3 docs and 9 tokens, and its member logical commitment.
    pub(in crate::native::text) fn layered_root(recipe: Digest) -> (Vec<u8>, Digest) {
        let mut bytes = b"QTXROOT3".to_vec();
        bytes.extend_from_slice(&recipe.0);
        bytes.push(1);
        bytes.extend_from_slice(&OWNERS.0.0);
        bytes.push(0);
        bytes.push(1);
        bytes.extend_from_slice(&FRONTIERS.0.0);
        bytes.push(0);
        bytes.extend_from_slice(&3_u64.to_be_bytes());
        bytes.extend_from_slice(&0_u64.to_be_bytes());
        bytes.extend_from_slice(&[0, 1]);
        bytes.extend_from_slice(&[0xff; 32]);
        bytes.push(0);
        bytes.extend_from_slice(&SEGMENT.0.0);
        bytes.extend_from_slice(&3_u32.to_be_bytes());
        bytes.extend_from_slice(&9_u64.to_be_bytes());
        let mut hash = Sha256::new();
        hash.update(b"qidx.native.text.segment.layered.logical.v3\0");
        hash.update(recipe.0);
        hash.update([1]);
        hash.update(OWNERS.0.0);
        hash.update([0]);
        hash.update([1]);
        hash.update(FRONTIERS.0.0);
        hash.update(3_u64.to_be_bytes());
        hash.update(9_u64.to_be_bytes());
        hash.update(3_u64.to_be_bytes());
        hash.update(0_u64.to_be_bytes());
        (bytes, Digest(hash.finalize().into()))
    }

    /// The pre-release layered magic carries only its recipe policy.
    pub(in crate::native::text) fn uncommitted_root(recipe: Digest) -> Vec<u8> {
        let mut bytes = b"QTXROOT2".to_vec();
        bytes.extend_from_slice(&recipe.0);
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{CHILDREN, OWNERS, SEGMENT, full_root, layered_root, uncommitted_root};
    use super::*;

    const RECIPE: Digest = Digest(*b"0123456789abcdef0123456789abcdef");

    #[test]
    fn every_retired_magic_is_tabled_with_its_layout() {
        assert_eq!(
            RETIRED_ROOT_MAGICS.map(|(magic, layout)| (*magic, layout)),
            [
                (*b"QTXROOT1", RetiredRootLayout::SegmentFullSourceCut),
                (*b"QTXROOT2", RetiredRootLayout::LayeredUncommitted),
                (*b"QTXROOT3", RetiredRootLayout::LayeredSourceCut),
            ]
        );
        for current in [wire::ROOT_MAGIC, super::super::wire_v2::ROOT_MAGIC] {
            assert_eq!(retired_layout(current), None);
        }
    }

    #[test]
    fn retired_roots_refuse_as_rebuild_required_after_authentication() {
        for (payload, logical) in [full_root(RECIPE), layered_root(RECIPE)] {
            for expected in [None, Some(logical)] {
                assert_eq!(
                    refuse_retired_root(&payload, RECIPE, &CHILDREN, expected),
                    Some(IndexError::RebuildRequired)
                );
            }
            // Not this member's root, not this recipe, not these edges, or truncated.
            for refusal in [
                refuse_retired_root(&payload, RECIPE, &CHILDREN, Some(Digest([9; 32]))),
                refuse_retired_root(&payload, Digest([8; 32]), &CHILDREN, None),
                refuse_retired_root(&payload, RECIPE, &[OWNERS, SEGMENT], None),
                refuse_retired_root(
                    payload.split_last().map_or(&[][..], |(_, rest)| rest),
                    RECIPE,
                    &CHILDREN,
                    None,
                ),
            ] {
                assert!(matches!(refusal, Some(IndexError::CorruptObject(_))));
            }
        }
        let uncommitted = uncommitted_root(RECIPE);
        assert_eq!(
            refuse_retired_root(&uncommitted, RECIPE, &[], None),
            Some(IndexError::RebuildRequired)
        );
        assert!(matches!(
            refuse_retired_root(&uncommitted, Digest([8; 32]), &[], None),
            Some(IndexError::CorruptObject(_))
        ));
        for unknown in [b"QTXROOT9".as_slice(), b"XXXXXXXX", b"QTX"] {
            assert_eq!(refuse_retired_root(unknown, RECIPE, &[], None), None);
        }
    }
}
