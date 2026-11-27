//! Physical wire for layered Native Text.
//! Decoding borrows payloads; object identity, child contents, Source membership,
//! and live posting counts are verified by the reader, not by this wire parser.
//!
//! Root: `QTXROOT4`, policy:32, two optional `TextCut` refs (Source owners
//! and paths), optional path posting root, path document count:u64, path token
//! count:u64, partition count:u16,
//! then fixed rows (upperOwner:32, kind:u8, childRef:32, liveDocs:u32,
//! liveTokens:u64). The last upperOwner is all-FF. Partition intervals are
//! (previous upper, current upper], with the first interval starting at zero.
//! Kind 0 directly names one all-live V1-shaped segment; kind 1 names QTXPART2.
//! Children are `TextCut` refs in owners/paths order, then the present path
//! posting root, then exactly one child per partition row.
//!
//! A root binds only the Source surface its text is a function of. Source
//! frontiers record producer stream positions and never change indexed text,
//! so they are not part of the root, its children or its logical commitment.
//! A frontier-only Source advance therefore keeps the exact root identity.
//!
//! Partition: QTXPART2, policy:32, layer count:u16, then fixed rows
//! (segmentRef:32, firstOwner:32, lastOwner:32, physicalDocs:u16,
//! physicalTokens:u64, liveMask:u128 big-endian, liveDocs:u16,
//! liveTokens:u64). Children are exactly the layer refs in row order. A set
//! bit is a live local docID. The physical bound is 1..=128, including empty
//! indexed documents. The row order is stable oldest-to-newest layer order.
//! Neighboring partitions may reference the same physical segment. Each live
//! bit selects a document strictly inside its own partition interval. Opening
//! the segment document table verifies that fence and duplicate live owners.
//!
//! The wire caps only Storage's 4096 children and the existing 128-doc mask.
//! A funded reader may impose a smaller measured operational fan-in separately.
#![expect(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    reason = "all fixed wire offsets and child counts are checked before indexing"
)]

use super::wire;
use crate::native::text::TextStatistics;
use crate::native::text_path::PathIndex;
use quanta_index_core::index::{Digest, IndexError, MAX_RECLAIM_CHILDREN, MapRoot, ObjectRef};
use sha2::{Digest as _, Sha256};

pub(in crate::native::text) const POLICY: &[u8] =
    b"qidx.native.text.segment.layered.v3;all-source-paths";
pub(in crate::native::text) const ROOT_MAGIC: &[u8; 8] = b"QTXROOT4";
pub(in crate::native::text) const PART_MAGIC: &[u8; 8] = b"QTXPART2";
pub(in crate::native::text) const STORAGE_CHILDREN: usize = MAX_RECLAIM_CHILDREN;
pub(in crate::native::text) const ROOT_ROW_BYTES: usize = 32 + 1 + 32 + 4 + 8;
pub(in crate::native::text) const LAYER_ROW_BYTES: usize = 32 + 32 + 32 + 2 + 8 + 16 + 2 + 8;
const MAX_PAYLOAD: usize = 16 * 1024 * 1024;
const MAX_OWNER: [u8; 32] = [0xff; 32];

pub(in crate::native::text) use super::wire::TextCut;

pub(super) fn root_row_capacity(cut: TextCut, has_path_postings: bool) -> usize {
    STORAGE_CHILDREN - cut.edges().count() - usize::from(has_path_postings)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::native::text) enum PartitionKind {
    DirectAll,
    Manifest,
}

impl PartitionKind {
    fn tag(self) -> u8 {
        match self {
            Self::DirectAll => 0,
            Self::Manifest => 1,
        }
    }

    fn from_tag(tag: u8) -> Result<Self, IndexError> {
        match tag {
            0 => Ok(Self::DirectAll),
            1 => Ok(Self::Manifest),
            _ => Err(wire::corrupt("V2 partition kind")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::native::text) struct RootRow {
    pub upper: [u8; 32],
    pub kind: PartitionKind,
    pub reference: ObjectRef,
    pub live_docs: u32,
    pub live_tokens: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::native::text) struct LayerRow {
    pub reference: ObjectRef,
    pub first: [u8; 32],
    pub last: [u8; 32],
    pub physical_docs: u16,
    pub physical_tokens: u64,
    pub live_mask: u128,
    pub live_docs: u16,
    pub live_tokens: u64,
}

#[derive(Clone, Copy)]
pub(in crate::native::text) struct RootView<'a> {
    pub cut: TextCut,
    pub path: PathIndex,
    pub count: usize,
    rows: &'a [u8],
    row_offset: usize,
    totals: (u64, u64),
}

impl RootView<'_> {
    pub(super) fn row(&self, index: usize) -> Result<RootRow, IndexError> {
        root_row(self.rows, index)
    }

    pub(super) const fn row_offset(&self) -> usize {
        self.row_offset
    }

    pub(super) const fn totals(&self) -> (u64, u64) {
        self.totals
    }
}

/// Address a row within the fixed table of an already authenticated root.
pub(in crate::native::text) fn root_row(rows: &[u8], index: usize) -> Result<RootRow, IndexError> {
    let start = index
        .checked_mul(ROOT_ROW_BYTES)
        .ok_or(IndexError::ResourceLimited)?;
    let end = start
        .checked_add(ROOT_ROW_BYTES)
        .ok_or(IndexError::ResourceLimited)?;
    let bytes = rows
        .get(start..end)
        .ok_or_else(|| wire::corrupt("V2 root row bounds"))?;
    read_root_row(&mut Cursor::new(bytes))
}

#[derive(Clone, Copy)]
pub(in crate::native::text) struct PartitionView<'a> {
    pub count: usize,
    rows: &'a [u8],
}

impl PartitionView<'_> {
    pub(super) fn layer(&self, index: usize) -> Result<LayerRow, IndexError> {
        let start = index
            .checked_mul(LAYER_ROW_BYTES)
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = self
            .rows
            .get(
                start
                    ..start
                        .checked_add(LAYER_ROW_BYTES)
                        .ok_or(IndexError::ResourceLimited)?,
            )
            .ok_or_else(|| wire::corrupt("V2 layer row bounds"))?;
        read_layer_row(&mut Cursor::new(bytes))
    }
}

fn checked_payload(fixed: usize, rows: usize, width: usize) -> Result<usize, IndexError> {
    let bytes = rows
        .checked_mul(width)
        .and_then(|n| fixed.checked_add(n))
        .ok_or(IndexError::ResourceLimited)?;
    if bytes > MAX_PAYLOAD {
        return Err(IndexError::ResourceLimited);
    }
    Ok(bytes)
}

fn valid_root_row(row: RootRow) -> Result<(), IndexError> {
    let live_docs = usize::try_from(row.live_docs).map_err(|_error| IndexError::ResourceLimited)?;
    if live_docs == 0 {
        return Err(wire::corrupt("V2 empty partition"));
    }
    if row.kind == PartitionKind::DirectAll && live_docs > wire::MAX_DOCS {
        return Err(wire::corrupt("V2 direct document bound"));
    }
    if row.kind == PartitionKind::Manifest && live_docs > STORAGE_CHILDREN * wire::MAX_DOCS {
        return Err(wire::corrupt("V2 manifest document bound"));
    }
    Ok(())
}

fn valid_layer_row(row: LayerRow) -> Result<(), IndexError> {
    let physical = usize::from(row.physical_docs);
    if physical == 0
        || physical > wire::MAX_DOCS
        || row.first > row.last
        || row.live_mask == 0
        || (physical < 128 && row.live_mask >> physical != 0)
        || row.live_mask.count_ones() != u32::from(row.live_docs)
        || row.live_tokens > row.physical_tokens
        || (row.live_docs == row.physical_docs && row.live_tokens != row.physical_tokens)
    {
        return Err(wire::corrupt("V2 layer count or mask"));
    }
    Ok(())
}

fn valid_layer_fence(
    row: LayerRow,
    lower_exclusive: Option<[u8; 32]>,
    upper_inclusive: [u8; 32],
) -> bool {
    if row.live_docs == row.physical_docs {
        return lower_exclusive.is_none_or(|lower| row.first > lower)
            && row.last <= upper_inclusive;
    }
    lower_exclusive.is_none_or(|lower| row.last > lower) && row.first <= upper_inclusive
}

fn read_root_row(cursor: &mut Cursor<'_>) -> Result<RootRow, IndexError> {
    let row = RootRow {
        upper: cursor.fixed()?,
        kind: PartitionKind::from_tag(cursor.byte()?)?,
        reference: cursor.reference()?,
        live_docs: cursor.u32()?,
        live_tokens: cursor.u64()?,
    };
    valid_root_row(row)?;
    Ok(row)
}

fn read_layer_row(cursor: &mut Cursor<'_>) -> Result<LayerRow, IndexError> {
    let row = LayerRow {
        reference: cursor.reference()?,
        first: cursor.fixed()?,
        last: cursor.fixed()?,
        physical_docs: cursor.u16()?,
        physical_tokens: cursor.u64()?,
        live_mask: cursor.u128()?,
        live_docs: cursor.u16()?,
        live_tokens: cursor.u64()?,
    };
    valid_layer_row(row)?;
    Ok(row)
}

fn optional(cursor: &mut Cursor<'_>) -> Result<MapRoot, IndexError> {
    match cursor.byte()? {
        0 => Ok(MapRoot(None)),
        1 => Ok(MapRoot(Some(cursor.reference()?))),
        _ => Err(wire::corrupt("V2 optional root tag")),
    }
}

fn read_cut(cursor: &mut Cursor<'_>) -> Result<TextCut, IndexError> {
    Ok(TextCut {
        owners: optional(cursor)?,
        paths: optional(cursor)?,
    })
}

fn write_optional(out: &mut Vec<u8>, root: MapRoot) {
    match root.0 {
        Some(reference) => {
            out.push(1);
            out.extend_from_slice(&reference.0.0);
        }
        None => out.push(0),
    }
}

/// Parse and validate the complete root payload and exact registered child order.
/// This does not open child objects or prove Source/document membership.
pub(in crate::native::text) fn decode_root<'a>(
    payload: &'a [u8],
    expected_recipe: Digest,
    children: &[ObjectRef],
) -> Result<RootView<'a>, IndexError> {
    if payload.len() > MAX_PAYLOAD {
        return Err(wire::corrupt("V2 root payload bound"));
    }
    if let Some(refusal) =
        super::retired::refuse_retired_root(payload, expected_recipe, children, None)
    {
        return Err(refusal);
    }
    let mut cursor = Cursor::new(payload);
    if cursor.fixed::<8>()? != *ROOT_MAGIC || cursor.fixed::<32>()? != expected_recipe.0 {
        return Err(wire::corrupt("V2 root magic or policy"));
    }
    let cut = read_cut(&mut cursor)?;
    let path = PathIndex {
        postings: optional(&mut cursor)?,
        statistics: TextStatistics {
            docs: cursor.u64()?,
            indexed_tokens: cursor.u64()?,
        },
    };
    let count = usize::from(cursor.u16()?);
    let prefix = cut.edges().chain(path.postings.0);
    let prefix_count = cut.edges().count() + usize::from(path.postings.0.is_some());
    if count > root_row_capacity(cut, path.postings.0.is_some())
        || children.len() != prefix_count + count
        || !children[..prefix_count].iter().copied().eq(prefix)
    {
        return Err(wire::corrupt("V2 root child order or bound"));
    }
    let row_bytes = checked_payload(cursor.offset, count, ROOT_ROW_BYTES)?;
    if row_bytes != payload.len() {
        return Err(wire::corrupt("V2 root length"));
    }
    let row_offset = cursor.offset;
    let rows = cursor.take(count * ROOT_ROW_BYTES)?;
    let mut row_cursor = Cursor::new(rows);
    let mut previous = None;
    let mut docs = 0u64;
    let mut tokens = 0u64;
    for index in 0..count {
        let row = read_root_row(&mut row_cursor)?;
        if previous.is_some_and(|upper| row.upper <= upper)
            || children[prefix_count + index] != row.reference
        {
            return Err(wire::corrupt("V2 root fence or edge"));
        }
        previous = Some(row.upper);
        docs = docs
            .checked_add(u64::from(row.live_docs))
            .ok_or(IndexError::ResourceLimited)?;
        tokens = tokens
            .checked_add(row.live_tokens)
            .ok_or(IndexError::ResourceLimited)?;
    }
    if count != 0 && previous != Some(MAX_OWNER) {
        return Err(wire::corrupt("V2 final fence"));
    }
    path.validate(docs)?;
    Ok(RootView {
        cut,
        path,
        count,
        rows,
        row_offset,
        totals: (docs, tokens),
    })
}

/// Write a canonical root and return the live document and token totals.
pub(in crate::native::text) fn encode_root(
    out: &mut Vec<u8>,
    recipe: Digest,
    cut: TextCut,
    path: PathIndex,
    rows: &[RootRow],
) -> Result<(u64, u64), IndexError> {
    if !out.is_empty() {
        return Err(IndexError::InvalidInput(
            "V2 root output is not empty".into(),
        ));
    }
    let prefix_count = cut.edges().count() + usize::from(path.postings.0.is_some());
    if rows.len() > root_row_capacity(cut, path.postings.0.is_some()) {
        return Err(IndexError::ResourceLimited);
    }
    let fixed = 8 + 32 + 3 + prefix_count * 32 + 8 + 8 + 2;
    let _encoded_len = checked_payload(fixed, rows.len(), ROOT_ROW_BYTES)?;
    let mut previous = None;
    let mut docs = 0u64;
    let mut tokens = 0u64;
    for row in rows {
        valid_root_row(*row)?;
        if previous.is_some_and(|upper| row.upper <= upper) {
            return Err(wire::corrupt("V2 root fence"));
        }
        previous = Some(row.upper);
        docs = docs
            .checked_add(u64::from(row.live_docs))
            .ok_or(IndexError::ResourceLimited)?;
        tokens = tokens
            .checked_add(row.live_tokens)
            .ok_or(IndexError::ResourceLimited)?;
    }
    if !rows.is_empty() && previous != Some(MAX_OWNER) {
        return Err(wire::corrupt("V2 final fence"));
    }
    path.validate(docs)?;
    out.extend_from_slice(ROOT_MAGIC);
    out.extend_from_slice(&recipe.0);
    for root in [cut.owners, cut.paths, path.postings] {
        write_optional(out, root);
    }
    out.extend_from_slice(&path.statistics.docs.to_be_bytes());
    out.extend_from_slice(&path.statistics.indexed_tokens.to_be_bytes());
    wire::push_count(out, rows.len())?;
    for row in rows {
        out.extend_from_slice(&row.upper);
        out.push(row.kind.tag());
        out.extend_from_slice(&row.reference.0.0);
        out.extend_from_slice(&row.live_docs.to_be_bytes());
        out.extend_from_slice(&row.live_tokens.to_be_bytes());
    }
    Ok((docs, tokens))
}

/// Parse one bounded layer manifest. The enclosing root row supplies its
/// interval and live summary; all physical document tables still need checking.
pub(in crate::native::text) fn decode_partition<'a>(
    payload: &'a [u8],
    expected_recipe: Digest,
    lower_exclusive: Option<[u8; 32]>,
    upper_inclusive: [u8; 32],
    expected_live_docs: u32,
    expected_live_tokens: u64,
    children: &[ObjectRef],
) -> Result<PartitionView<'a>, IndexError> {
    if payload.len() > MAX_PAYLOAD {
        return Err(wire::corrupt("V2 partition payload bound"));
    }
    let mut cursor = Cursor::new(payload);
    if cursor.fixed::<8>()? != *PART_MAGIC || cursor.fixed::<32>()? != expected_recipe.0 {
        return Err(wire::corrupt("V2 partition magic or policy"));
    }
    let count = usize::from(cursor.u16()?);
    if count == 0
        || count > STORAGE_CHILDREN
        || children.len() != count
        || lower_exclusive.is_some_and(|lower| lower >= upper_inclusive)
    {
        return Err(wire::corrupt("V2 layer child count"));
    }
    let physical_doc_bound = count
        .checked_mul(wire::MAX_DOCS)
        .ok_or(IndexError::ResourceLimited)?;
    if usize::try_from(expected_live_docs).map_err(|_error| IndexError::ResourceLimited)?
        > physical_doc_bound
    {
        return Err(wire::corrupt("V2 manifest document bound"));
    }
    let row_bytes = checked_payload(cursor.offset, count, LAYER_ROW_BYTES)?;
    if row_bytes != payload.len() {
        return Err(wire::corrupt("V2 partition length"));
    }
    let rows = cursor.take(count * LAYER_ROW_BYTES)?;
    let mut row_cursor = Cursor::new(rows);
    let mut docs = 0u32;
    let mut tokens = 0u64;
    for index in 0..count {
        let row = read_layer_row(&mut row_cursor)?;
        if !valid_layer_fence(row, lower_exclusive, upper_inclusive)
            || children[index] != row.reference
        {
            return Err(wire::corrupt("V2 layer fence or edge"));
        }
        docs = docs
            .checked_add(u32::from(row.live_docs))
            .ok_or(IndexError::ResourceLimited)?;
        tokens = tokens
            .checked_add(row.live_tokens)
            .ok_or(IndexError::ResourceLimited)?;
    }
    if docs != expected_live_docs || tokens != expected_live_tokens {
        return Err(wire::corrupt("V2 partition live summary"));
    }
    Ok(PartitionView { count, rows })
}

/// Write a canonical partition manifest into an empty, caller-funded buffer.
pub(in crate::native::text) fn encode_partition(
    out: &mut Vec<u8>,
    recipe: Digest,
    lower_exclusive: Option<[u8; 32]>,
    upper_inclusive: [u8; 32],
    layers: &[LayerRow],
) -> Result<(u32, u64), IndexError> {
    if !out.is_empty() {
        return Err(IndexError::InvalidInput(
            "V2 partition output is not empty".into(),
        ));
    }
    if layers.is_empty() || layers.len() > STORAGE_CHILDREN {
        return Err(IndexError::ResourceLimited);
    }
    if lower_exclusive.is_some_and(|lower| lower >= upper_inclusive) {
        return Err(wire::corrupt("V2 partition fence order"));
    }
    let _encoded_len = checked_payload(8 + 32 + 2, layers.len(), LAYER_ROW_BYTES)?;
    let mut docs = 0u32;
    let mut tokens = 0u64;
    for row in layers {
        valid_layer_row(*row)?;
        if !valid_layer_fence(*row, lower_exclusive, upper_inclusive) {
            return Err(wire::corrupt("V2 layer fence"));
        }
        docs = docs
            .checked_add(u32::from(row.live_docs))
            .ok_or(IndexError::ResourceLimited)?;
        tokens = tokens
            .checked_add(row.live_tokens)
            .ok_or(IndexError::ResourceLimited)?;
    }
    out.extend_from_slice(PART_MAGIC);
    out.extend_from_slice(&recipe.0);
    wire::push_count(out, layers.len())?;
    for row in layers {
        out.extend_from_slice(&row.reference.0.0);
        out.extend_from_slice(&row.first);
        out.extend_from_slice(&row.last);
        out.extend_from_slice(&row.physical_docs.to_be_bytes());
        out.extend_from_slice(&row.physical_tokens.to_be_bytes());
        out.extend_from_slice(&row.live_mask.to_be_bytes());
        out.extend_from_slice(&row.live_docs.to_be_bytes());
        out.extend_from_slice(&row.live_tokens.to_be_bytes());
    }
    Ok((docs, tokens))
}

pub(in crate::native::text) fn logical_commitment(
    recipe: Digest,
    cut: TextCut,
    live_docs: u64,
    live_tokens: u64,
    path: PathIndex,
) -> Digest {
    let mut hash = Sha256::new();
    hash.update(b"qidx.native.text.segment.layered.logical.v4\0");
    hash.update(recipe.0);
    for root in [cut.owners, cut.paths] {
        match root.0 {
            Some(reference) => {
                hash.update([1]);
                hash.update(reference.0.0);
            }
            None => hash.update([0]),
        }
    }
    hash.update(live_docs.to_be_bytes());
    hash.update(live_tokens.to_be_bytes());
    hash.update(path.statistics.docs.to_be_bytes());
    hash.update(path.statistics.indexed_tokens.to_be_bytes());
    Digest(hash.finalize().into())
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], IndexError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or(IndexError::ResourceLimited)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| wire::corrupt("V2 truncated payload"))?;
        self.offset = end;
        Ok(value)
    }

    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], IndexError> {
        self.take(N)?
            .try_into()
            .map_err(|_| wire::corrupt("V2 fixed width"))
    }

    fn byte(&mut self) -> Result<u8, IndexError> {
        Ok(self.fixed::<1>()?[0])
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

    fn u128(&mut self) -> Result<u128, IndexError> {
        Ok(u128::from_be_bytes(self.fixed()?))
    }

    fn reference(&mut self) -> Result<ObjectRef, IndexError> {
        Ok(ObjectRef(Digest(self.fixed()?)))
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed bytes and explicit rejection assertions are independent test oracles"
)]
mod tests {
    use super::*;

    const RECIPE: Digest = Digest(*b"0123456789abcdef0123456789abcdef");
    const EMPTY_SOURCE: TextCut = TextCut {
        owners: MapRoot(None),
        paths: MapRoot(None),
    };
    const REF_A: ObjectRef = ObjectRef(Digest([0x11; 32]));
    const REF_B: ObjectRef = ObjectRef(Digest([0x22; 32]));

    fn path(postings: MapRoot, docs: u64, indexed_tokens: u64) -> PathIndex {
        PathIndex {
            postings,
            statistics: TextStatistics {
                docs,
                indexed_tokens,
            },
        }
    }

    fn one_root() -> Result<Vec<u8>, IndexError> {
        let mut bytes = Vec::new();
        let totals = encode_root(
            &mut bytes,
            RECIPE,
            EMPTY_SOURCE,
            path(MapRoot(None), 1, 0),
            &[RootRow {
                upper: MAX_OWNER,
                kind: PartitionKind::DirectAll,
                reference: REF_A,
                live_docs: 1,
                live_tokens: 7,
            }],
        )?;
        assert_eq!(totals, (1, 7));
        Ok(bytes)
    }

    fn one_partition() -> Result<Vec<u8>, IndexError> {
        let mut bytes = Vec::new();
        let totals = encode_partition(
            &mut bytes,
            RECIPE,
            None,
            MAX_OWNER,
            &[LayerRow {
                reference: REF_B,
                first: [0x10; 32],
                last: [0x20; 32],
                physical_docs: 3,
                physical_tokens: 17,
                live_mask: 0b101,
                live_docs: 2,
                live_tokens: 11,
            }],
        )?;
        assert_eq!(totals, (2, 11));
        Ok(bytes)
    }

    #[test]
    fn fixed_empty_root_and_direct_row() -> Result<(), IndexError> {
        let mut empty = Vec::new();
        let totals = encode_root(&mut empty, RECIPE, EMPTY_SOURCE, PathIndex::default(), &[])?;
        assert_eq!(totals, (0, 0));
        assert_eq!(
            empty.as_slice(),
            b"QTXROOT40123456789abcdef0123456789abcdef\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"
        );
        let decoded = decode_root(&empty, RECIPE, &[])?;
        assert_eq!(decoded.count, 0);
        assert_eq!(decoded.totals(), (0, 0));

        let direct = one_root()?;
        assert_eq!(direct.len(), 61 + ROOT_ROW_BYTES);
        assert_eq!(&direct[..40], b"QTXROOT40123456789abcdef0123456789abcdef");
        assert_eq!(&direct[40..43], &[0; 3]);
        assert_eq!(&direct[43..51], &[0, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(&direct[51..59], &[0; 8]);
        assert_eq!(&direct[59..61], &[0, 1]);
        assert_eq!(&direct[61..93], &[0xff; 32]);
        assert_eq!(direct[93], 0);
        assert_eq!(&direct[94..126], &[0x11; 32]);
        assert_eq!(&direct[126..130], &[0, 0, 0, 1]);
        assert_eq!(&direct[130..138], &[0, 0, 0, 0, 0, 0, 0, 7]);
        let decoded = decode_root(&direct, RECIPE, &[REF_A])?;
        assert_eq!(decoded.row(0)?.reference, REF_A);
        assert_eq!(decoded.totals(), (1, 7));
        Ok(())
    }

    #[test]
    fn path_root_is_authenticated_before_partition_edges() -> Result<(), IndexError> {
        let mut bytes = Vec::new();
        let row = RootRow {
            upper: MAX_OWNER,
            kind: PartitionKind::DirectAll,
            reference: REF_A,
            live_docs: 1,
            live_tokens: 2,
        };
        let totals = encode_root(
            &mut bytes,
            RECIPE,
            EMPTY_SOURCE,
            path(MapRoot(Some(REF_B)), 1, 3),
            &[row],
        )?;
        assert_eq!(totals, (1, 2));
        let decoded = decode_root(&bytes, RECIPE, &[REF_B, REF_A])?;
        assert_eq!(decoded.path, path(MapRoot(Some(REF_B)), 1, 3));
        assert!(decode_root(&bytes, RECIPE, &[REF_A, REF_B]).is_err());
        Ok(())
    }

    #[test]
    fn path_only_root_commits_all_source_owners() -> Result<(), IndexError> {
        let index = path(MapRoot(Some(REF_B)), 2, 3);
        let mut bytes = Vec::new();
        assert_eq!(
            encode_root(&mut bytes, RECIPE, EMPTY_SOURCE, index, &[])?,
            (0, 0)
        );
        let decoded = decode_root(&bytes, RECIPE, &[REF_B])?;
        assert_eq!(decoded.count, 0);
        assert_eq!(decoded.path, index);
        assert_ne!(
            logical_commitment(RECIPE, EMPTY_SOURCE, 0, 0, index),
            logical_commitment(RECIPE, EMPTY_SOURCE, 0, 0, path(MapRoot(Some(REF_B)), 1, 3))
        );
        // A current-layout payload relabelled with a retired layered magic
        // does not authenticate as that retired root: it is corrupt, not stale.
        let mut relabelled = bytes.clone();
        relabelled[7] = b'3';
        assert!(matches!(
            decode_root(&relabelled, RECIPE, &[REF_B]),
            Err(IndexError::CorruptObject(_))
        ));
        assert!(
            encode_root(
                &mut Vec::new(),
                RECIPE,
                EMPTY_SOURCE,
                path(MapRoot(Some(REF_B)), 0, 3),
                &[],
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn path_root_reserves_one_registration_at_storage_bound() -> Result<(), IndexError> {
        let source = TextCut {
            owners: MapRoot(Some(REF_A)),
            paths: MapRoot(Some(REF_B)),
        };
        let path_root = ObjectRef(Digest([0x44; 32]));
        let mut rows: Vec<_> = (0..4094u16)
            .map(|index| {
                let mut upper = [0; 32];
                upper[..2].copy_from_slice(&index.to_be_bytes());
                RootRow {
                    upper,
                    kind: PartitionKind::DirectAll,
                    reference: ObjectRef(Digest(upper)),
                    live_docs: 1,
                    live_tokens: 1,
                }
            })
            .collect();
        rows[4093].upper = MAX_OWNER;
        let mut bytes = Vec::new();
        assert_eq!(
            encode_root(
                &mut bytes,
                RECIPE,
                source,
                path(MapRoot(None), 4094, 0),
                &rows
            )?,
            (4094, 4094)
        );
        bytes.clear();
        assert!(matches!(
            encode_root(
                &mut bytes,
                RECIPE,
                source,
                path(MapRoot(Some(path_root)), 4094, 1),
                &rows
            ),
            Err(IndexError::ResourceLimited)
        ));
        rows.truncate(4093);
        rows[4092].upper = MAX_OWNER;
        assert_eq!(
            encode_root(
                &mut bytes,
                RECIPE,
                source,
                path(MapRoot(Some(path_root)), 4093, 1),
                &rows
            )?,
            (4093, 4093)
        );
        let children: Vec<_> = source
            .edges()
            .chain([path_root])
            .chain(rows.iter().map(|row| row.reference))
            .collect();
        assert_eq!(children.len(), 4096);
        assert_eq!(decode_root(&bytes, RECIPE, &children)?.count, 4093);
        Ok(())
    }

    #[test]
    fn fixed_mask_and_summary() -> Result<(), IndexError> {
        let bytes = one_partition()?;
        assert_eq!(bytes.len(), 8 + 32 + 2 + LAYER_ROW_BYTES);
        assert_eq!(
            &bytes[..42],
            b"QTXPART20123456789abcdef0123456789abcdef\0\x01"
        );
        assert_eq!(&bytes[42..74], &[0x22; 32]);
        assert_eq!(&bytes[74..106], &[0x10; 32]);
        assert_eq!(&bytes[106..138], &[0x20; 32]);
        assert_eq!(&bytes[138..140], &[0, 3]);
        assert_eq!(&bytes[140..148], &[0, 0, 0, 0, 0, 0, 0, 17]);
        assert_eq!(
            &bytes[148..164],
            &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5]
        );
        assert_eq!(&bytes[164..166], &[0, 2]);
        assert_eq!(&bytes[166..174], &[0, 0, 0, 0, 0, 0, 0, 11]);
        let parsed = decode_partition(&bytes, RECIPE, None, MAX_OWNER, 2, 11, &[REF_B])?;
        assert_eq!(parsed.layer(0)?.live_mask, 0b101);
        Ok(())
    }

    #[test]
    fn adjacent_manifests_share_a_physical_segment() -> Result<(), IndexError> {
        let bytes = one_partition()?;
        let whole = decode_partition(&bytes, RECIPE, None, MAX_OWNER, 2, 11, &[REF_B])?.layer(0)?;
        let cut = [0x18; 32];
        let left = LayerRow {
            live_mask: 0b001,
            live_docs: 1,
            live_tokens: 3,
            ..whole
        };
        let right = LayerRow {
            live_mask: 0b100,
            live_docs: 1,
            live_tokens: 8,
            ..whole
        };
        let mut left_bytes = Vec::new();
        let mut right_bytes = Vec::new();
        let left_counts = encode_partition(&mut left_bytes, RECIPE, None, cut, &[left])?;
        let right_counts =
            encode_partition(&mut right_bytes, RECIPE, Some(cut), MAX_OWNER, &[right])?;
        assert_eq!((left_counts, right_counts), ((1, 3), (1, 8)));
        let left = decode_partition(&left_bytes, RECIPE, None, cut, 1, 3, &[REF_B])?.layer(0)?;
        let right = decode_partition(&right_bytes, RECIPE, Some(cut), MAX_OWNER, 1, 8, &[REF_B])?
            .layer(0)?;
        assert_eq!(left.reference, right.reference);
        assert_eq!(left.live_mask & right.live_mask, 0);
        assert_eq!(left.live_mask | right.live_mask, whole.live_mask);
        assert!(decode_partition(&right_bytes, RECIPE, Some(cut), cut, 1, 8, &[REF_B]).is_err());
        let all_live = LayerRow {
            live_mask: 0b111,
            live_docs: whole.physical_docs,
            live_tokens: whole.physical_tokens,
            ..whole
        };
        assert!(encode_partition(&mut Vec::new(), RECIPE, None, cut, &[all_live]).is_err());
        Ok(())
    }

    #[test]
    fn root_rejects_truncation_trailing_policy_edges_and_fences() -> Result<(), IndexError> {
        let base = one_root()?;
        for invalid in [
            base[..base.len() - 1].to_vec(),
            [base.as_slice(), &[0]].concat(),
            {
                let mut bytes = base.clone();
                bytes[0] = b'X';
                bytes
            },
            {
                let mut bytes = base.clone();
                bytes[8] ^= 1;
                bytes
            },
            {
                let mut bytes = base.clone();
                bytes[93] = 2;
                bytes
            },
            {
                let mut bytes = base.clone();
                bytes[61] = 0xfe;
                bytes
            },
            {
                let mut bytes = base.clone();
                bytes[126..130].copy_from_slice(&0u32.to_be_bytes());
                bytes
            },
        ] {
            assert!(decode_root(&invalid, RECIPE, &[REF_A]).is_err());
        }
        assert!(decode_root(&base, RECIPE, &[]).is_err());
        assert!(decode_root(&base, RECIPE, &[REF_B]).is_err());
        Ok(())
    }

    #[test]
    fn retired_root_magics_decode_as_rebuild_required_and_unknown_as_corrupt() {
        use super::super::retired::fixtures::{CHILDREN, layered_root, uncommitted_root};
        let (layered, _) = layered_root(RECIPE);
        assert!(matches!(
            decode_root(&layered, RECIPE, &CHILDREN),
            Err(IndexError::RebuildRequired)
        ));
        assert!(matches!(
            decode_root(&uncommitted_root(RECIPE), RECIPE, &[]),
            Err(IndexError::RebuildRequired)
        ));
        let mut unknown = layered;
        unknown[7] = b'9';
        assert!(matches!(
            decode_root(&unknown, RECIPE, &CHILDREN),
            Err(IndexError::CorruptObject(_))
        ));
    }

    #[test]
    fn partition_rejects_truncation_trailing_mask_counts_and_edges() -> Result<(), IndexError> {
        let base = one_partition()?;
        for invalid in [
            base[..base.len() - 1].to_vec(),
            [base.as_slice(), &[0]].concat(),
            {
                let mut bytes = base.clone();
                bytes[0] = b'X';
                bytes
            },
            {
                let mut bytes = base.clone();
                bytes[8] ^= 1;
                bytes
            },
            {
                let mut bytes = base.clone();
                bytes[148..164].copy_from_slice(&8u128.to_be_bytes());
                bytes
            },
            {
                let mut bytes = base.clone();
                bytes[164..166].copy_from_slice(&3u16.to_be_bytes());
                bytes
            },
            {
                let mut bytes = base.clone();
                bytes[166..174].copy_from_slice(&18u64.to_be_bytes());
                bytes
            },
        ] {
            assert!(decode_partition(&invalid, RECIPE, None, MAX_OWNER, 2, 11, &[REF_B]).is_err());
        }
        assert!(decode_partition(&base, RECIPE, None, MAX_OWNER, 2, 11, &[]).is_err());
        assert!(decode_partition(&base, RECIPE, None, MAX_OWNER, 2, 11, &[REF_A]).is_err());
        assert!(
            decode_partition(&base, RECIPE, Some([0x20; 32]), MAX_OWNER, 2, 11, &[REF_B]).is_err()
        );
        assert!(decode_partition(&base, RECIPE, None, MAX_OWNER, 1, 11, &[REF_B]).is_err());
        assert!(decode_partition(&base, RECIPE, None, MAX_OWNER, 129, 11, &[REF_B]).is_err());
        Ok(())
    }
}
