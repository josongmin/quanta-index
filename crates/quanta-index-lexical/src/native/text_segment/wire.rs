//! P0 wire contract. All fixed integers are big-endian. Object references are
//! raw digests. The containing object authenticates payload and ordered edges.
//!
//! Root: magic, recipe:32, two `TextCut` optional refs (0 or 1+ref, in
//! owners/paths order), count:u16, repeated `SegmentEdge`.
//! SegmentEdge: ref:32, firstOwner:32, lastOwner:32, docs:u16, tokens:u64.
//! Root children: present `TextCut` refs, then segment refs in row order.
//! Source frontiers are producer stream positions and never change indexed
//! text, so no Text root binds them; a frontier-only Source advance keeps the
//! exact root identity.
//!
//! Segment: magic, recipe:32, docs:u16, Doc rows, routes:u16, Route rows.
//! Doc: OwnerKey:32, source record ref:32, indexed token count:u64. The row
//! ordinal is the local docID. Source records are retained by the root's
//! `TextCut` owners map; the segment's children are exactly its posting pages.
//! Route: ref:32, firstTerm (len:u16 then full key), lastTerm (same).
//! Empty document rows are retained. An all-empty segment has no routes.
//!
//! Childless posting page: magic, repeated Fragment through authenticated EOF.
//! Fragment: key (len:u16 then full key), df:u16, bodyLen:u32, body.
//! Each body encodes exactly df documents. The first docID is absolute,
//! later docIDs are strictly positive deltas. Each docID is followed by TF
//! and then TF positions, with the first absolute and later positive deltas.
//! Body integers are minimal unsigned LEB128 limited to u32. All positions
//! for a document/term stay in one fragment. A term can continue on the next
//! page, with strictly increasing docIDs across consecutive fragments. Keys
//! are strictly increasing within a page and route ranges cannot overlap
//! except at a shared boundary key. The page's first/last keys equal its route.

use quanta_index_core::index::{Digest, IndexError, MapRoot, ObjectRef, SourceCut};
use sha2::{Digest as _, Sha256};

pub(in crate::native::text) const POLICY: &[u8] = b"qidx.native.text.segment.full.v1";
pub(in crate::native::text) const ROOT_MAGIC: &[u8; 8] = b"QTXROOT5";
pub(in crate::native::text) const SEGMENT_MAGIC: &[u8; 8] = b"QTXSEGM1";
pub(in crate::native::text) const PAGE_MAGIC: &[u8; 8] = b"QTXPOST1";
pub(in crate::native::text) const MAX_DOCS: usize = 128;
pub(in crate::native::text) const MAX_PAGES: usize = 1024;
pub(in crate::native::text) const MAX_SEGMENTS: usize = 4093;
pub(in crate::native::text) const PAGE_BYTES: usize = 64 * 1024;
pub(in crate::native::text) const MAX_KEY: usize = 1024;
pub(in crate::native::text) const DOC_BYTES: usize = 72;
pub(in crate::native::text) const ROOT_ROW_BYTES: usize = 106;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::native::text) struct Doc {
    pub owner: [u8; 32],
    pub source: ObjectRef,
    pub tokens: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::native::text) struct SegmentEdge {
    pub reference: ObjectRef,
    pub first: [u8; 32],
    pub last: [u8; 32],
    pub docs: u16,
    pub tokens: u64,
}

/// The Source surface bound by every segmented Text root: exactly the owner
/// and path maps that determine its indexed documents and path postings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::native::text) struct TextCut {
    pub owners: MapRoot,
    pub paths: MapRoot,
}

impl TextCut {
    /// Project a full `SourceCut` onto the surface a Text root binds.
    pub(in crate::native::text) const fn of(source: SourceCut) -> Self {
        Self {
            owners: source.owners,
            paths: source.paths,
        }
    }

    /// Present root edges in canonical owners/paths order.
    pub(in crate::native::text) fn edges(self) -> impl Iterator<Item = ObjectRef> {
        [self.owners.0, self.paths.0].into_iter().flatten()
    }
}

pub(in crate::native::text) fn logical_commitment(
    recipe: Digest,
    cut: TextCut,
    docs: u64,
    tokens: u64,
) -> Digest {
    let mut hash = Sha256::new();
    hash.update(b"qidx.native.text.segment.full.logical.v1\0");
    hash.update(recipe.0);
    match cut.owners.0 {
        Some(reference) => {
            hash.update([1]);
            hash.update(reference.0.0);
        }
        None => hash.update([0]),
    }
    hash.update(docs.to_be_bytes());
    hash.update(tokens.to_be_bytes());
    Digest(hash.finalize().into())
}

pub(in crate::native::text) fn write_root(
    out: &mut Vec<u8>,
    recipe: Digest,
    cut: TextCut,
    segments: &[SegmentEdge],
) -> Result<(), IndexError> {
    if segments.len() > MAX_SEGMENTS {
        return Err(IndexError::ResourceLimited);
    }
    out.extend_from_slice(ROOT_MAGIC);
    out.extend_from_slice(&recipe.0);
    for root in [cut.owners, cut.paths] {
        match root.0 {
            Some(reference) => {
                out.push(1);
                out.extend_from_slice(&reference.0.0);
            }
            None => out.push(0),
        }
    }
    push_count(out, segments.len())?;
    for edge in segments {
        out.extend_from_slice(&edge.reference.0.0);
        out.extend_from_slice(&edge.first);
        out.extend_from_slice(&edge.last);
        out.extend_from_slice(&edge.docs.to_be_bytes());
        out.extend_from_slice(&edge.tokens.to_be_bytes());
    }
    Ok(())
}

pub(in crate::native::text) fn write_segment(
    out: &mut Vec<u8>,
    recipe: Digest,
    docs: &[Doc],
    route_count: usize,
    encoded_routes: &[u8],
) -> Result<(), IndexError> {
    if docs.is_empty() || docs.len() > MAX_DOCS || route_count > MAX_PAGES {
        return Err(IndexError::ResourceLimited);
    }
    out.extend_from_slice(SEGMENT_MAGIC);
    out.extend_from_slice(&recipe.0);
    push_count(out, docs.len())?;
    for doc in docs {
        out.extend_from_slice(&doc.owner);
        out.extend_from_slice(&doc.source.0.0);
        out.extend_from_slice(&doc.tokens.to_be_bytes());
    }
    push_count(out, route_count)?;
    out.extend_from_slice(encoded_routes);
    Ok(())
}

pub(in crate::native::text) fn write_route(
    out: &mut Vec<u8>,
    reference: ObjectRef,
    first: &[u8],
    last: &[u8],
) -> Result<(), IndexError> {
    out.extend_from_slice(&reference.0.0);
    push_key(out, first)?;
    push_key(out, last)
}

pub(in crate::native::text) fn push_key(out: &mut Vec<u8>, key: &[u8]) -> Result<(), IndexError> {
    let _canonical = crate::native::text_memory::decode_case_term_key(key)?;
    push_count(out, key.len())?;
    out.extend_from_slice(key);
    Ok(())
}

pub(in crate::native::text) fn push_count(
    out: &mut Vec<u8>,
    count: usize,
) -> Result<(), IndexError> {
    out.extend_from_slice(
        &u16::try_from(count)
            .map_err(|_error| IndexError::ResourceLimited)?
            .to_be_bytes(),
    );
    Ok(())
}

pub(in crate::native::text) fn push_varint(out: &mut Vec<u8>, mut value: u32) {
    while value >= 128 {
        let [low, ..] = value.to_le_bytes();
        out.push((low & 0x7f) | 0x80);
        value >>= 7;
    }
    let [low, ..] = value.to_le_bytes();
    out.push(low);
}

pub(in crate::native::text) fn corrupt(reason: &'static str) -> IndexError {
    IndexError::CorruptObject(format!("native text segment {reason}"))
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed bytes are independent test oracles"
)]
mod tests {
    use super::*;

    #[test]
    fn empty_root_wire_is_fixed() -> Result<(), IndexError> {
        let recipe = Digest(*b"0123456789abcdef0123456789abcdef");
        let cut = TextCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
        };
        let mut bytes = Vec::new();
        write_root(&mut bytes, recipe, cut, &[])?;
        assert_eq!(&bytes, b"QTXROOT50123456789abcdef0123456789abcdef\0\0\0\0");
        assert_eq!(cut.edges().count(), 0);
        Ok(())
    }

    #[test]
    fn text_cut_binds_owners_and_paths_but_not_frontiers() {
        let owners = MapRoot(Some(ObjectRef(Digest([0x11; 32]))));
        let paths = MapRoot(Some(ObjectRef(Digest([0x22; 32]))));
        let source = SourceCut {
            owners,
            paths,
            frontiers: MapRoot(None),
        };
        let advanced = SourceCut {
            frontiers: MapRoot(Some(ObjectRef(Digest([0x33; 32])))),
            ..source
        };
        assert_eq!(TextCut::of(source), TextCut::of(advanced));
        assert_eq!(
            TextCut::of(advanced).edges().collect::<Vec<_>>(),
            [ObjectRef(Digest([0x11; 32])), ObjectRef(Digest([0x22; 32]))]
        );
        for changed in [
            SourceCut {
                owners: MapRoot(None),
                ..source
            },
            SourceCut {
                paths: MapRoot(None),
                ..source
            },
        ] {
            assert_ne!(TextCut::of(changed), TextCut::of(source));
        }
    }

    #[test]
    fn varint_boundaries_match_the_wire() {
        let mut bytes = Vec::new();
        for value in [0, 127, 128, 16_384, u32::MAX] {
            push_varint(&mut bytes, value);
        }
        assert_eq!(
            bytes,
            [0, 127, 0x80, 1, 0x80, 0x80, 1, 0xff, 0xff, 0xff, 0xff, 15]
        );
    }
}
