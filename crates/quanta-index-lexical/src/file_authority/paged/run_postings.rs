//! Canonical File run postings. The 64-document segments remain immutable
//! source witnesses; one run dictionary routes grams to bounded pages of
//! logical IDs and their owning segment digests.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_core::{CoreError, RequestBudgetV1};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::Digest as _;

use crate::channel_payloads::{decode_cbor_exact, encode_cbor};
use crate::lexical_store::{
    AttemptLease, BlobRef, MAX_MAP_HEIGHT, MapCursor, MapEdit, MapRoot, ObjectKind, ObjectRef,
    ObjectStore,
};

use super::{
    BoundedVec, MAX_POSTING_PAGE_BYTES, MAX_RECORD_BYTES, MAX_SEGMENT_DOCS, PostingPage, Segment,
};

const PAGE_WIRE_VERSION: u8 = 1;
const ROOT_WIRE_VERSION: u8 = 1;
const GRAM_COUNT_WIRE_VERSION: u8 = 1;
const ID_BUCKET_BITS: u32 = 8;
const MAX_BUCKET_ROWS: usize = 1 << ID_BUCKET_BITS;
// Ciborium worst case for one (u64, [u8; 32]) tuple: tuple header 1,
// integer 9, byte-array header 2, and at most 2 bytes per array element.
const MAX_WIRE_ROW_BYTES: usize = 1 + 9 + 2 + 32 * 2;
const _: () = assert!(MAX_BUCKET_ROWS * MAX_WIRE_ROW_BYTES + 64 <= MAX_POSTING_PAGE_BYTES);

// The producer keeps a policy-bounded run reference list, but decodes only a
// bounded group of source cursors and one gram of posting rows at a time.
// These bounds are staging charges, not an allocator RSS prediction. The map
// decoder admits at most 96 leaf rows or 64 branch rows, keys of at most 256
// bytes, and a cursor retains at most 13 decoded pages. A 96 KiB frame charge
// covers those decoded rows, keys, and collection overhead with margin.
const NEW_RUN_SEGMENT_FAN_IN: usize = 64;
const NEW_RUN_PAGE_FAN_IN: usize = 16;
const DECODED_MAP_FRAME_CHARGE: u64 = 96 * 1024;
const MAP_BUILD_AND_CACHE_CHARGE: u64 = 8 * 1024 * 1024;
const CURSOR_AND_REF_CHARGE: u64 = 2 * 1024 * 1024;
const PARTIAL_ROOT_CHARGE: u64 = 1024 * 1024;
const SOURCE_ID_CHARGE: u64 = 4 * 1024 * 1024;
const BUCKET_ROW_CHARGE: usize = 2 * 1024 * 1024;
const _: () = assert!(
    NEW_RUN_SEGMENT_FAN_IN * MAX_SEGMENT_DOCS * std::mem::size_of::<RunPostingRow>()
        <= BUCKET_ROW_CHARGE
);

fn invalid(reason: &str) -> CoreError {
    CoreError::InvalidContract(format!("file run postings: {reason}"))
}

fn corrupt(reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE,
        message: format!("file run postings: {reason}"),
    }
}

fn valid_surface(surface: u8) -> bool {
    surface == 1 || surface == 2
}

/// The fixed bucket is derived from the worst-case 32 KiB canonical page,
/// independent of whether durable logical IDs are contiguous.
pub(crate) fn run_page_key(surface: u8, gram: [u8; 3], id: u64) -> Result<[u8; 12], CoreError> {
    if !valid_surface(surface) {
        return Err(invalid("unknown posting surface"));
    }
    let mut key = [0_u8; 12];
    key[0] = surface;
    key[1..4].copy_from_slice(&gram);
    key[4..].copy_from_slice(&(id >> ID_BUCKET_BITS).to_be_bytes());
    Ok(key)
}

pub(crate) fn run_page_prefix(surface: u8, gram: [u8; 3]) -> Result<[u8; 4], CoreError> {
    if !valid_surface(surface) {
        return Err(invalid("unknown posting surface"));
    }
    Ok([surface, gram[0], gram[1], gram[2]])
}

/// One global route per gram and run. A count is checked against the run's
/// page rows by the independent seal; it is a query estimate, not authority.
pub(crate) fn run_route_key(
    surface: u8,
    gram: [u8; 3],
    count: u64,
    run_id: u64,
) -> Result<[u8; 20], CoreError> {
    if !valid_surface(surface) || count == 0 || run_id == 0 {
        return Err(invalid("run route surface, count or ID differs"));
    }
    let mut key = [0_u8; 20];
    key[..4].copy_from_slice(&run_page_prefix(surface, gram)?);
    key[4..12].copy_from_slice(&count.to_be_bytes());
    key[12..].copy_from_slice(&run_id.to_be_bytes());
    Ok(key)
}

/// One checked counter per `(surface, gram)` in a run. This permits a delta
/// to update only affected route keys without scanning every posting bucket
/// in a potentially large run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RunGramCount {
    pub(crate) surface: u8,
    pub(crate) gram: [u8; 3],
    pub(crate) count: u64,
}

impl Serialize for RunGramCount {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (GRAM_COUNT_WIRE_VERSION, self.surface, self.gram, self.count).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RunGramCount {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (version, surface, gram, count) = <(u8, u8, [u8; 3], u64)>::deserialize(deserializer)?;
        if version != GRAM_COUNT_WIRE_VERSION {
            return Err(serde::de::Error::custom("run gram-count version differs"));
        }
        Ok(Self {
            surface,
            gram,
            count,
        })
    }
}

impl RunGramCount {
    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        if !valid_surface(self.surface) || self.count == 0 {
            return Err(invalid("run gram-count surface or count differs"));
        }
        Ok(())
    }

    pub(crate) fn key(&self) -> Result<[u8; 4], CoreError> {
        self.validate()?;
        run_page_prefix(self.surface, self.gram)
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        self.validate()?;
        let bytes = encode_cbor(self, "file run gram count")?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(invalid("run gram count exceeds record admission"));
        }
        Ok(bytes)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(corrupt("run gram count exceeds record admission"));
        }
        let count: Self = decode_cbor_exact(bytes)
            .map_err(|error| corrupt(&format!("run gram count decode: {error}")))?;
        count
            .validate()
            .map_err(|error| corrupt(&error.to_string()))?;
        if count.encode()? != bytes {
            return Err(corrupt("run gram count encoding is not canonical"));
        }
        Ok(count)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RunPostingRow {
    pub(crate) logical_id: u64,
    pub(crate) segment_sha256: [u8; 32],
}

impl Serialize for RunPostingRow {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (self.logical_id, self.segment_sha256).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RunPostingRow {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (logical_id, segment_sha256) = <(u64, [u8; 32])>::deserialize(deserializer)?;
        Ok(Self {
            logical_id,
            segment_sha256,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunPostingPage {
    pub(crate) surface: u8,
    pub(crate) gram: [u8; 3],
    pub(crate) bucket: u64,
    pub(crate) rows: Vec<RunPostingRow>,
}

impl Serialize for RunPostingPage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (
            PAGE_WIRE_VERSION,
            self.surface,
            self.gram,
            self.bucket,
            &self.rows,
        )
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RunPostingPage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (version, surface, gram, bucket, BoundedVec(rows)) =
            <(
                u8,
                u8,
                [u8; 3],
                u64,
                BoundedVec<RunPostingRow, MAX_BUCKET_ROWS>,
            )>::deserialize(deserializer)?;
        if version != PAGE_WIRE_VERSION {
            return Err(serde::de::Error::custom("run posting page version differs"));
        }
        Ok(Self {
            surface,
            gram,
            bucket,
            rows,
        })
    }
}

impl RunPostingPage {
    pub(crate) fn new(
        surface: u8,
        gram: [u8; 3],
        bucket: u64,
        rows: Vec<RunPostingRow>,
    ) -> Result<Self, CoreError> {
        let page = Self {
            surface,
            gram,
            bucket,
            rows,
        };
        page.validate()?;
        Ok(page)
    }

    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        if !valid_surface(self.surface) || self.rows.is_empty() || self.rows.len() > MAX_BUCKET_ROWS
        {
            return Err(invalid("page surface or row count differs"));
        }
        let mut prior = None;
        for row in &self.rows {
            if row.logical_id >> ID_BUCKET_BITS != self.bucket
                || prior.is_some_and(|id| id >= row.logical_id)
            {
                return Err(invalid("page bucket or logical-ID order differs"));
            }
            prior = Some(row.logical_id);
        }
        Ok(())
    }

    pub(crate) fn key(&self) -> Result<[u8; 12], CoreError> {
        self.validate()?;
        run_page_key(
            self.surface,
            self.gram,
            self.rows
                .first()
                .ok_or_else(|| invalid("empty page"))?
                .logical_id,
        )
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        self.validate()?;
        let bytes = encode_cbor(self, "file run posting page")?;
        if bytes.len() > MAX_POSTING_PAGE_BYTES {
            return Err(invalid("page exceeds encoded-byte admission"));
        }
        Ok(bytes)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        if bytes.len() > MAX_POSTING_PAGE_BYTES {
            return Err(corrupt("page exceeds encoded-byte admission"));
        }
        let page: Self =
            decode_cbor_exact(bytes).map_err(|error| corrupt(&format!("page decode: {error}")))?;
        page.validate()
            .map_err(|error| corrupt(&error.to_string()))?;
        if page.encode()? != bytes {
            return Err(corrupt("page encoding is not canonical"));
        }
        Ok(page)
    }
}

/// The run's page map is keyed by (surface, gram, ID bucket). It is the
/// complete posting authority for this run; leaf posting pages remain source
/// witnesses used by the independent seal and repair path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunPostingRoot {
    pub(crate) run_id: u64,
    pub(crate) pages: MapRoot,
    pub(crate) gram_counts: MapRoot,
    pub(crate) path_rows: u64,
    pub(crate) content_rows: u64,
    /// Encoded Record object widths for values in `pages` and `gram_counts`.
    /// `MapPage` structure and this root Record are accounted separately.
    pub(crate) page_record_encoded_bytes: u64,
    pub(crate) gram_count_record_encoded_bytes: u64,
}

impl Serialize for RunPostingRoot {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (
            ROOT_WIRE_VERSION,
            self.run_id,
            &self.pages,
            &self.gram_counts,
            self.path_rows,
            self.content_rows,
            self.page_record_encoded_bytes,
            self.gram_count_record_encoded_bytes,
        )
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RunPostingRoot {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (
            version,
            run_id,
            pages,
            gram_counts,
            path_rows,
            content_rows,
            page_record_encoded_bytes,
            gram_count_record_encoded_bytes,
        ) = <(u8, u64, MapRoot, MapRoot, u64, u64, u64, u64)>::deserialize(deserializer)?;
        if version != ROOT_WIRE_VERSION {
            return Err(serde::de::Error::custom("run posting root version differs"));
        }
        Ok(Self {
            run_id,
            pages,
            gram_counts,
            path_rows,
            content_rows,
            page_record_encoded_bytes,
            gram_count_record_encoded_bytes,
        })
    }
}

impl RunPostingRoot {
    pub(crate) fn load(
        store: &ObjectStore,
        reference: &BlobRef,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Self, CoreError> {
        if reference.root.kind != ObjectKind::Record {
            return Err(corrupt("run posting root object kind differs"));
        }
        let bytes = store.read_blob(
            reference,
            u64::try_from(MAX_RECORD_BYTES).map_err(|_width| invalid("record bound width"))?,
            budget,
        )?;
        Self::decode(&bytes)
    }

    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        let posting_rows = self
            .path_rows
            .checked_add(self.content_rows)
            .ok_or_else(|| invalid("run posting row count overflow"))?;
        if self.run_id == 0
            || self.pages.page.is_some() != (self.pages.entries != 0)
            || self.gram_counts.page.is_some() != (self.gram_counts.entries != 0)
            || (self.pages.entries == 0) != (posting_rows == 0)
            || (self.gram_counts.entries == 0) != (posting_rows == 0)
            || (self.pages.entries == 0) != (self.page_record_encoded_bytes == 0)
            || (self.gram_counts.entries == 0) != (self.gram_count_record_encoded_bytes == 0)
        {
            return Err(invalid("run ID, map root or posting counters differ"));
        }
        Ok(())
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        self.validate()?;
        let bytes = encode_cbor(self, "file run posting root")?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(invalid("run posting root exceeds record admission"));
        }
        Ok(bytes)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(corrupt("run posting root exceeds record admission"));
        }
        let root: Self = decode_cbor_exact(bytes)
            .map_err(|error| corrupt(&format!("run posting root decode: {error}")))?;
        root.validate()
            .map_err(|error| corrupt(&error.to_string()))?;
        if root.encode()? != bytes {
            return Err(corrupt("run posting root encoding is not canonical"));
        }
        Ok(root)
    }

    pub(crate) fn object_roots(&self) -> Vec<ObjectRef> {
        [&self.pages, &self.gram_counts]
            .into_iter()
            .filter_map(|root| root.page.as_ref().map(|page| page.object.clone()))
            .collect()
    }

    /// Logical encoded bundle for one run. Shared content-addressed values
    /// are charged once per owning run, not as unique physical pool blocks.
    pub(crate) fn bundle_encoded_bytes(
        &self,
        reference: &BlobRef,
        store: &ObjectStore,
    ) -> Result<u64, CoreError> {
        self.validate()?;
        let encoded = self.encode()?;
        if reference.root.kind != ObjectKind::Record
            || reference.bytes
                != u64::try_from(encoded.len()).map_err(|_width| invalid("root length width"))?
            || reference.sha256 != <[u8; 32]>::from(sha2::Sha256::digest(&encoded))
        {
            return Err(corrupt("run posting root reference differs"));
        }
        let page_structure = store.map_structure_encoded_bytes(&self.pages, None)?;
        let count_structure = store.map_structure_encoded_bytes(&self.gram_counts, None)?;
        reference
            .root
            .bytes
            .checked_add(self.page_record_encoded_bytes)
            .and_then(|sum| sum.checked_add(self.gram_count_record_encoded_bytes))
            .and_then(|sum| sum.checked_add(page_structure))
            .and_then(|sum| sum.checked_add(count_structure))
            .ok_or_else(|| invalid("run posting bundle bytes overflow"))
    }
}

/// Updates only changed posting buckets and their gram counters.
///
/// The caller supplies leaf-derived target pages. Independent seal
/// verification proves their rows against the changed physical leaves.
pub(crate) struct RunPostingPageEdit {
    pub(crate) prior: Option<(BlobRef, RunPostingPage)>,
    pub(crate) next: Option<RunPostingPage>,
}

pub(crate) fn apply_run_page_edits(
    store: &ObjectStore,
    lease: &AttemptLease,
    prior: Option<&RunPostingRoot>,
    run_id: u64,
    edits: BTreeMap<Vec<u8>, RunPostingPageEdit>,
) -> Result<(RunPostingRoot, BlobRef), CoreError> {
    if run_id == 0 || prior.is_some_and(|root| root.run_id != run_id) {
        return Err(invalid("run page update ID differs"));
    }
    let empty = RunPostingRoot {
        run_id,
        pages: MapRoot::default(),
        gram_counts: MapRoot::default(),
        path_rows: 0,
        content_rows: 0,
        page_record_encoded_bytes: 0,
        gram_count_record_encoded_bytes: 0,
    };
    let prior = prior.unwrap_or(&empty);
    prior.validate()?;
    let mut page_edits = Vec::new();
    let mut gram_changes = BTreeMap::<Vec<u8>, (u64, u64)>::new();
    let mut page_bytes = prior.page_record_encoded_bytes;
    let mut path_rows = prior.path_rows;
    let mut content_rows = prior.content_rows;
    for (key, edit) in edits {
        if key.len() != 12 {
            return Err(invalid("changed run posting bucket key width differs"));
        }
        let previous_ref = store.map_get(&prior.pages, &key, None)?;
        if previous_ref.as_ref() != edit.prior.as_ref().map(|(reference, _)| reference) {
            return Err(corrupt("old run posting page reference differs"));
        }
        let previous = edit.prior.as_ref().map(|(_, page)| page);
        if let Some((reference, page)) = &edit.prior {
            if reference.root.kind != ObjectKind::Record {
                return Err(corrupt("old run posting page kind differs"));
            }
            let encoded = page
                .encode()
                .map_err(|error| corrupt(&format!("old run posting page invalid: {error}")))?;
            if reference.bytes
                != u64::try_from(encoded.len()).map_err(|_width| corrupt("old page width"))?
                || reference.sha256 != <[u8; 32]>::from(sha2::Sha256::digest(&encoded))
            {
                return Err(corrupt("old run posting page bytes differ from reference"));
            }
        }
        if previous.is_some_and(|page| {
            page.key()
                .map_or(true, |actual| actual.as_slice() != key.as_slice())
        }) {
            return Err(corrupt("old run posting page key differs"));
        }
        let next = edit.next;
        if let Some(page) = &next
            && page.key()?.as_slice() != key.as_slice()
        {
            return Err(invalid("new run posting page key differs"));
        }
        let old_rows = previous.map_or(0, |page| page.rows.len());
        let new_rows = next.as_ref().map_or(0, |page| page.rows.len());
        if previous == next.as_ref() {
            return Err(invalid("declared run posting page edit is unchanged"));
        }
        let gram_prefix = key
            .get(..4)
            .ok_or_else(|| invalid("changed run posting gram key absent"))?;
        let count_delta = gram_changes.entry(gram_prefix.to_vec()).or_default();
        count_delta.0 = count_delta
            .0
            .checked_add(u64::try_from(old_rows).map_err(|_width| invalid("old row count width"))?)
            .ok_or_else(|| invalid("removed gram rows overflow"))?;
        count_delta.1 = count_delta
            .1
            .checked_add(u64::try_from(new_rows).map_err(|_width| invalid("new row count width"))?)
            .ok_or_else(|| invalid("added gram rows overflow"))?;
        let total = match key.first().copied() {
            Some(1) => &mut path_rows,
            Some(2) => &mut content_rows,
            _ => return Err(invalid("changed run posting surface differs")),
        };
        let old_rows = u64::try_from(old_rows).map_err(|_width| invalid("old row width"))?;
        let new_rows = u64::try_from(new_rows).map_err(|_width| invalid("new row width"))?;
        *total = total
            .checked_sub(old_rows)
            .and_then(|sum| sum.checked_add(new_rows))
            .ok_or_else(|| corrupt("run posting row total underflows or overflows"))?;
        if let Some(reference) = previous_ref {
            page_bytes = page_bytes
                .checked_sub(reference.root.bytes)
                .ok_or_else(|| corrupt("run posting Record ledger underflows"))?;
        }
        let new_ref = next
            .map(|page| store.put_blob(lease, ObjectKind::Record, &[], &page.encode()?))
            .transpose()?;
        if let Some(reference) = &new_ref {
            page_bytes = page_bytes
                .checked_add(reference.root.bytes)
                .ok_or_else(|| invalid("run posting Record ledger overflows"))?;
        }
        page_edits.push(Ok(MapEdit {
            key,
            value: new_ref,
        }));
    }
    let pages = if page_edits.is_empty() {
        prior.pages.clone()
    } else {
        store.map_apply(lease, &prior.pages, page_edits.into_iter())?
    };
    let mut gram_edits = Vec::new();
    let mut count_bytes = prior.gram_count_record_encoded_bytes;
    for (key, (removed, added)) in gram_changes {
        let old_ref = store.map_get(&prior.gram_counts, &key, None)?;
        let old_count = old_ref
            .as_ref()
            .map(|reference| {
                if reference.root.kind != ObjectKind::Record {
                    return Err(corrupt("old gram count kind differs"));
                }
                RunGramCount::decode(
                    &store.read_blob(
                        reference,
                        u64::try_from(MAX_RECORD_BYTES)
                            .map_err(|_width| invalid("count bound width"))?,
                        None,
                    )?,
                )
            })
            .transpose()?;
        if old_count.as_ref().is_some_and(|count| {
            count
                .key()
                .map_or(true, |actual| actual.as_slice() != key.as_slice())
        }) {
            return Err(corrupt("old run gram-count key differs"));
        }
        let count = old_count
            .map_or(0, |row| row.count)
            .checked_sub(removed)
            .and_then(|sum| sum.checked_add(added))
            .ok_or_else(|| corrupt("run gram-count underflows or overflows"))?;
        if let Some(reference) = old_ref {
            count_bytes = count_bytes
                .checked_sub(reference.root.bytes)
                .ok_or_else(|| corrupt("run gram-count Record ledger underflows"))?;
        }
        let new_ref = if count == 0 {
            None
        } else {
            let record = RunGramCount {
                surface: *key
                    .first()
                    .ok_or_else(|| corrupt("gram count key has no surface"))?,
                gram: key
                    .get(1..4)
                    .ok_or_else(|| corrupt("gram count key has no gram"))?
                    .try_into()
                    .map_err(|_width| corrupt("gram key width differs"))?,
                count,
            };
            let reference = store.put_blob(lease, ObjectKind::Record, &[], &record.encode()?)?;
            count_bytes = count_bytes
                .checked_add(reference.root.bytes)
                .ok_or_else(|| invalid("run gram-count Record ledger overflows"))?;
            Some(reference)
        };
        gram_edits.push(Ok(MapEdit {
            key,
            value: new_ref,
        }));
    }
    let gram_counts = if gram_edits.is_empty() {
        prior.gram_counts.clone()
    } else {
        store.map_apply(lease, &prior.gram_counts, gram_edits.into_iter())?
    };
    let root = RunPostingRoot {
        run_id,
        pages,
        gram_counts,
        path_rows,
        content_rows,
        page_record_encoded_bytes: page_bytes,
        gram_count_record_encoded_bytes: count_bytes,
    };
    let reference = store.put_blob(
        lease,
        ObjectKind::Record,
        &root.object_roots(),
        &root.encode()?,
    )?;
    Ok((root, reference))
}

/// A bounded k-way iterator over one bucket from each selected run. It never
/// materializes the full run or a source-wide posting vector.
pub(crate) struct RunPostingUnion<'a> {
    inputs: &'a [&'a RunPostingPage],
    positions: Vec<usize>,
    prior_id: Option<u64>,
}

impl<'a> RunPostingUnion<'a> {
    pub(crate) fn new(inputs: &'a [&'a RunPostingPage]) -> Result<Self, CoreError> {
        let Some(first) = inputs.first() else {
            return Err(invalid("run posting union has no input"));
        };
        for page in inputs {
            page.validate()?;
            if page.surface != first.surface
                || page.gram != first.gram
                || page.bucket != first.bucket
            {
                return Err(invalid("run posting union key differs"));
            }
        }
        let mut positions = Vec::new();
        positions
            .try_reserve_exact(inputs.len())
            .map_err(|error| CoreError::Storage(format!("reserve run posting cursors: {error}")))?;
        positions.resize(inputs.len(), 0);
        Ok(Self {
            inputs,
            positions,
            prior_id: None,
        })
    }

    pub(crate) fn next_row(&mut self) -> Result<Option<RunPostingRow>, CoreError> {
        let mut selected = None;
        for (index, page) in self.inputs.iter().enumerate() {
            let position = *self
                .positions
                .get(index)
                .ok_or_else(|| corrupt("run posting cursor position absent"))?;
            let Some(row) = page.rows.get(position) else {
                continue;
            };
            if selected.is_none_or(|(_, current): (usize, RunPostingRow)| {
                row.logical_id < current.logical_id
            }) {
                selected = Some((index, *row));
            }
        }
        let Some((index, row)) = selected else {
            return Ok(None);
        };
        if self.prior_id.is_some_and(|prior| prior >= row.logical_id) {
            return Err(corrupt("run posting union repeats a logical ID"));
        }
        let position = self
            .positions
            .get_mut(index)
            .ok_or_else(|| corrupt("run posting selected cursor absent"))?;
        *position = position
            .checked_add(1)
            .ok_or_else(|| invalid("run posting cursor overflow"))?;
        self.prior_id = Some(row.logical_id);
        Ok(Some(row))
    }
}

pub(crate) fn merge_page_bucket(inputs: &[&RunPostingPage]) -> Result<RunPostingPage, CoreError> {
    let first = inputs
        .first()
        .ok_or_else(|| invalid("run posting merge has no input"))?;
    let mut union = RunPostingUnion::new(inputs)?;
    let mut rows = Vec::new();
    while let Some(row) = union.next_row()? {
        rows.try_reserve(1)
            .map_err(|error| CoreError::Storage(format!("reserve merged posting row: {error}")))?;
        rows.push(row);
    }
    RunPostingPage::new(first.surface, first.gram, first.bucket, rows)
}

/// Independent page oracle: compare the target against a keyed source census
/// rather than reusing the producer's cursor order or merge implementation.
#[cfg(test)]
pub(crate) fn verify_page_union(
    inputs: &[&RunPostingPage],
    target: &RunPostingPage,
) -> Result<(), CoreError> {
    target
        .validate()
        .map_err(|error| corrupt(&error.to_string()))?;
    let mut expected = BTreeMap::new();
    for page in inputs {
        page.validate()
            .map_err(|error| corrupt(&error.to_string()))?;
        if page.surface != target.surface
            || page.gram != target.gram
            || page.bucket != target.bucket
        {
            return Err(corrupt("source and target posting keys differ"));
        }
        for row in &page.rows {
            if expected
                .insert(row.logical_id, row.segment_sha256)
                .is_some()
            {
                return Err(corrupt("source runs repeat a logical ID"));
            }
        }
    }
    if expected.len() != target.rows.len()
        || target
            .rows
            .iter()
            .any(|row| expected.get(&row.logical_id) != Some(&row.segment_sha256))
    {
        return Err(corrupt(
            "target posting page differs from exact source union",
        ));
    }
    Ok(())
}

/// Apply one changed-leaf bucket without visiting any other gram or bucket.
/// A missing or mismatched retired locator is corruption, never a no-op.
pub(crate) fn apply_page_delta(
    surface: u8,
    gram: [u8; 3],
    bucket: u64,
    old: Option<&RunPostingPage>,
    removed: &[RunPostingRow],
    added: &[RunPostingRow],
) -> Result<Option<RunPostingPage>, CoreError> {
    if !valid_surface(surface) {
        return Err(invalid("delta surface differs"));
    }
    if let Some(page) = old {
        page.validate()?;
        if page.surface != surface || page.gram != gram || page.bucket != bucket {
            return Err(corrupt("delta base page key differs"));
        }
    }
    let mut rows = old.map_or_else(BTreeMap::new, |page| {
        page.rows
            .iter()
            .map(|row| (row.logical_id, row.segment_sha256))
            .collect::<BTreeMap<_, _>>()
    });
    let mut prior = None;
    for row in removed {
        if row.logical_id >> ID_BUCKET_BITS != bucket
            || prior.is_some_and(|id| id >= row.logical_id)
            || rows.remove(&row.logical_id) != Some(row.segment_sha256)
        {
            return Err(corrupt("retired posting row absent or differs"));
        }
        prior = Some(row.logical_id);
    }
    prior = None;
    for row in added {
        if row.logical_id >> ID_BUCKET_BITS != bucket
            || prior.is_some_and(|id| id >= row.logical_id)
            || rows.insert(row.logical_id, row.segment_sha256).is_some()
        {
            return Err(corrupt("added posting row repeats or differs"));
        }
        prior = Some(row.logical_id);
    }
    if rows.is_empty() {
        return Ok(None);
    }
    RunPostingPage::new(
        surface,
        gram,
        bucket,
        rows.into_iter()
            .map(|(logical_id, segment_sha256)| RunPostingRow {
                logical_id,
                segment_sha256,
            })
            .collect(),
    )
    .map(Some)
}

struct NewRunSegmentCursor<'a> {
    segment: Segment,
    digest: [u8; 32],
    terms: MapCursor<'a>,
    ready: Option<(Vec<u8>, BlobRef)>,
    cached: Option<(BlobRef, PostingPage)>,
}

/// Produces strictly increasing (surface, gram, bucket) pages from a bounded
/// group of immutable physical leaves. Only one gram's at most 64 * 64 member
/// rows is staged at a time.
struct NewRunPageStream<'a> {
    store: &'a ObjectStore,
    lease: &'a AttemptLease,
    sources: Vec<NewRunSegmentCursor<'a>>,
    surface: u8,
    pending_gram: Option<[u8; 3]>,
    pending_buckets: BTreeMap<u64, Vec<RunPostingRow>>,
    failed: bool,
}

impl<'a> NewRunPageStream<'a> {
    fn new(
        store: &'a ObjectStore,
        lease: &'a AttemptLease,
        references: &[&BlobRef],
        seen_ids: &mut BTreeSet<u64>,
    ) -> Result<Self, CoreError> {
        if references.is_empty() || references.len() > NEW_RUN_SEGMENT_FAN_IN {
            return Err(invalid("new run leaf group width differs"));
        }
        let mut sources = Vec::new();
        sources
            .try_reserve_exact(references.len())
            .map_err(|error| CoreError::Storage(format!("reserve new run cursors: {error}")))?;
        for reference in references {
            if reference.root.kind != ObjectKind::Segment {
                return Err(corrupt("new run leaf has wrong object kind"));
            }
            let segment = Segment::decode(&store.read_blob(
                reference,
                u64::try_from(MAX_RECORD_BYTES).map_err(|_width| invalid("record bound width"))?,
                None,
            )?)?;
            let max_ids = usize::try_from(super::super::policy().source_files)
                .map_err(|_width| invalid("source policy width"))?;
            for id in &segment.doc_ids {
                if seen_ids.len() >= max_ids || !seen_ids.insert(*id) {
                    return Err(corrupt("new run leaves repeat or exceed logical IDs"));
                }
            }
            let mut terms = store.map_cursor(&segment.path_terms, &[], None, None)?;
            let ready = terms.next()?;
            sources.push(NewRunSegmentCursor {
                segment,
                digest: reference.root.sha256,
                terms,
                ready,
                cached: None,
            });
        }
        Ok(Self {
            store,
            lease,
            sources,
            surface: 1,
            pending_gram: None,
            pending_buckets: BTreeMap::new(),
            failed: false,
        })
    }

    fn emit_bucket(&mut self) -> Result<Option<MapEdit>, CoreError> {
        let Some((bucket, mut rows)) = self.pending_buckets.pop_first() else {
            self.pending_gram = None;
            return Ok(None);
        };
        let gram = self
            .pending_gram
            .ok_or_else(|| corrupt("new run bucket has no gram"))?;
        rows.sort_unstable_by_key(|row| row.logical_id);
        if rows.windows(2).any(|pair| {
            let [left, right] = pair else { return true };
            left.logical_id >= right.logical_id
        }) {
            return Err(corrupt("new run leaves repeat a posting logical ID"));
        }
        let page = RunPostingPage::new(self.surface, gram, bucket, rows)?;
        let key = page.key()?.to_vec();
        let reference =
            self.store
                .put_blob(self.lease, ObjectKind::Record, &[], &page.encode()?)?;
        Ok(Some(MapEdit {
            key,
            value: Some(reference),
        }))
    }

    fn next_edit(&mut self) -> Result<Option<MapEdit>, CoreError> {
        if !self.pending_buckets.is_empty() {
            return self.emit_bucket();
        }
        loop {
            let smallest = self
                .sources
                .iter()
                .filter_map(|source| source.ready.as_ref().map(|(key, _)| key.as_slice()))
                .min();
            let Some(key) = smallest else {
                if self.surface == 2 {
                    return Ok(None);
                }
                self.surface = 2;
                for source in &mut self.sources {
                    source.terms =
                        self.store
                            .map_cursor(&source.segment.content_terms, &[], None, None)?;
                    source.ready = source.terms.next()?;
                    source.cached = None;
                }
                continue;
            };
            let gram: [u8; 3] = key
                .try_into()
                .map_err(|_width| corrupt("new run leaf term key width differs"))?;
            for source in &mut self.sources {
                let selected = source
                    .ready
                    .as_ref()
                    .is_some_and(|(key, _)| key.as_slice() == gram.as_slice());
                if !selected {
                    continue;
                }
                let (_, page_ref) = source
                    .ready
                    .take()
                    .ok_or_else(|| corrupt("new run selected term disappeared"))?;
                if source
                    .cached
                    .as_ref()
                    .is_none_or(|(reference, _)| *reference != page_ref)
                {
                    if page_ref.root.kind != ObjectKind::FilePostings {
                        return Err(corrupt("new run leaf page kind differs"));
                    }
                    let page = PostingPage::decode(
                        &self.store.read_blob(
                            &page_ref,
                            u64::try_from(MAX_POSTING_PAGE_BYTES)
                                .map_err(|_width| invalid("posting bound width"))?,
                            None,
                        )?,
                        source.segment.doc_ids.len(),
                    )?;
                    source.cached = Some((page_ref, page));
                }
                let page = &source
                    .cached
                    .as_ref()
                    .ok_or_else(|| corrupt("new run leaf page absent"))?
                    .1;
                let position = page
                    .0
                    .binary_search_by_key(&gram, |entry| entry.0)
                    .map_err(|_missing| corrupt("new run leaf page omits routed gram"))?;
                let mask = page
                    .0
                    .get(position)
                    .ok_or_else(|| corrupt("new run leaf gram vanished"))?
                    .1;
                for (local, logical_id) in source.segment.doc_ids.iter().copied().enumerate() {
                    if mask & (1_u64 << local) != 0 {
                        self.pending_buckets
                            .entry(logical_id >> ID_BUCKET_BITS)
                            .or_default()
                            .push(RunPostingRow {
                                logical_id,
                                segment_sha256: source.digest,
                            });
                    }
                }
                source.ready = source.terms.next()?;
            }
            if self.pending_buckets.is_empty() {
                return Err(corrupt("new run leaf term has no posting IDs"));
            }
            self.pending_gram = Some(gram);
            return self.emit_bucket();
        }
    }
}

impl Iterator for NewRunPageStream<'_> {
    type Item = Result<MapEdit, CoreError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        match self.next_edit() {
            Ok(Some(edit)) => Some(Ok(edit)),
            Ok(None) => None,
            Err(error) => {
                self.failed = true;
                Some(Err(error))
            }
        }
    }
}

struct RunPageMergeStream<'a> {
    store: &'a ObjectStore,
    lease: &'a AttemptLease,
    cursors: Vec<MapCursor<'a>>,
    ready: Vec<Option<(Vec<u8>, BlobRef)>>,
    failed: bool,
    record_encoded_bytes: u64,
}

/// Minimum stream staging charge: one decoded leaf frame per input cursor.
///
/// The execution gate adds branch frames from each supplied map descriptor.
/// This charge does not include the caller-owned roots or shared store cache.
pub(crate) fn run_merge_stage_bytes(fan_in: usize) -> Result<u64, CoreError> {
    let count = u64::try_from(fan_in).map_err(|_width| invalid("run merge fan-in width"))?;
    let cursor_bytes = std::mem::size_of::<MapCursor<'_>>()
        .checked_add(std::mem::size_of::<Option<(Vec<u8>, BlobRef)>>())
        .ok_or_else(|| invalid("run merge cursor size overflow"))?;
    let cursor_bytes =
        u64::try_from(cursor_bytes).map_err(|_width| invalid("run merge cursor width"))?;
    let page_bytes = MAX_POSTING_PAGE_BYTES
        .checked_mul(3)
        .ok_or_else(|| invalid("run merge page charge overflow"))?;
    let page_bytes = u64::try_from(page_bytes).map_err(|_width| invalid("page charge width"))?;
    count
        .checked_mul(
            cursor_bytes
                .checked_add(DECODED_MAP_FRAME_CHARGE)
                .ok_or_else(|| invalid("run merge cursor charge overflow"))?,
        )
        .and_then(|bytes| bytes.checked_add(page_bytes))
        .ok_or_else(|| invalid("run merge staging charge overflow"))
}

fn run_merge_map_stage_bytes(pages: &[&MapRoot]) -> Result<u64, CoreError> {
    pages
        .iter()
        .try_fold(run_merge_stage_bytes(pages.len())?, |bytes, root| {
            let height = root.page.as_ref().map_or(0, |page| page.height);
            if height > MAX_MAP_HEIGHT {
                return Err(corrupt("run merge map height exceeds bound"));
            }
            u64::from(height)
                .checked_mul(DECODED_MAP_FRAME_CHARGE)
                .and_then(|frames| bytes.checked_add(frames))
                .ok_or_else(|| invalid("run merge decoded-frame charge overflow"))
        })
}

impl<'a> RunPageMergeStream<'a> {
    fn new(
        store: &'a ObjectStore,
        lease: &'a AttemptLease,
        inputs: &[RunPostingRoot],
        max_staging_bytes: u64,
    ) -> Result<Self, CoreError> {
        for root in inputs {
            root.validate()?;
        }
        let mut pages = Vec::new();
        pages
            .try_reserve_exact(inputs.len())
            .map_err(|error| CoreError::Storage(format!("reserve run page roots: {error}")))?;
        pages.extend(inputs.iter().map(|root| &root.pages));
        Self::new_page_roots(store, lease, &pages, max_staging_bytes)
    }

    fn new_page_roots(
        store: &'a ObjectStore,
        lease: &'a AttemptLease,
        pages: &[&MapRoot],
        max_staging_bytes: u64,
    ) -> Result<Self, CoreError> {
        let cursor_charge = run_merge_map_stage_bytes(pages)?;
        if cursor_charge > max_staging_bytes {
            return Err(invalid("run merge exceeds staging packet bound"));
        }
        let mut cursors = Vec::new();
        let mut ready = Vec::new();
        cursors
            .try_reserve_exact(pages.len())
            .map_err(|error| CoreError::Storage(format!("reserve run page cursors: {error}")))?;
        ready
            .try_reserve_exact(pages.len())
            .map_err(|error| CoreError::Storage(format!("reserve run page heads: {error}")))?;
        for &page_root in pages {
            let mut cursor = store.map_cursor(page_root, &[], None, None)?;
            ready.push(cursor.next()?);
            cursors.push(cursor);
        }
        Ok(Self {
            store,
            lease,
            cursors,
            ready,
            failed: false,
            record_encoded_bytes: 0,
        })
    }

    fn next_edit(&mut self) -> Result<Option<MapEdit>, CoreError> {
        let Some(key) = self
            .ready
            .iter()
            .filter_map(|row| row.as_ref().map(|(key, _)| key))
            .min()
            .cloned()
        else {
            return Ok(None);
        };
        let mut merged: Option<RunPostingPage> = None;
        for index in 0..self.ready.len() {
            if self
                .ready
                .get(index)
                .ok_or_else(|| corrupt("run page merge head absent"))?
                .as_ref()
                .is_some_and(|(candidate, _)| *candidate == key)
            {
                let (_, reference) = self
                    .ready
                    .get_mut(index)
                    .ok_or_else(|| corrupt("run page merge selected head absent"))?
                    .take()
                    .ok_or_else(|| corrupt("run page head disappeared"))?;
                if reference.root.kind != ObjectKind::Record {
                    return Err(corrupt("run page has wrong object kind"));
                }
                let page = RunPostingPage::decode(
                    &self.store.read_blob(
                        &reference,
                        u64::try_from(MAX_POSTING_PAGE_BYTES)
                            .map_err(|_width| corrupt("posting page limit width"))?,
                        None,
                    )?,
                )?;
                if page.key()?.as_slice() != key {
                    return Err(corrupt("run page map key differs"));
                }
                merged = Some(match merged {
                    Some(prior) => merge_page_bucket(&[&prior, &page])?,
                    None => page,
                });
                let next = self
                    .cursors
                    .get_mut(index)
                    .ok_or_else(|| corrupt("run page merge cursor absent"))?
                    .next()?;
                *self
                    .ready
                    .get_mut(index)
                    .ok_or_else(|| corrupt("run page merge head slot absent"))? = next;
            }
        }
        let merged = merged.ok_or_else(|| corrupt("run page merge has no selected page"))?;
        let reference =
            self.store
                .put_blob(self.lease, ObjectKind::Record, &[], &merged.encode()?)?;
        self.record_encoded_bytes = self
            .record_encoded_bytes
            .checked_add(reference.root.bytes)
            .ok_or_else(|| invalid("merged page Record bytes overflow"))?;
        Ok(Some(MapEdit {
            key,
            value: Some(reference),
        }))
    }
}

impl Iterator for RunPageMergeStream<'_> {
    type Item = Result<MapEdit, CoreError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        match self.next_edit() {
            Ok(Some(edit)) => Some(Ok(edit)),
            Ok(None) => None,
            Err(error) => {
                self.failed = true;
                Some(Err(error))
            }
        }
    }
}

struct RunGramCountStream<'a> {
    store: &'a ObjectStore,
    lease: &'a AttemptLease,
    cursor: MapCursor<'a>,
    ready: Option<(Vec<u8>, BlobRef)>,
    path_rows: u64,
    content_rows: u64,
    failed: bool,
    page_record_encoded_bytes: u64,
    record_encoded_bytes: u64,
}

impl<'a> RunGramCountStream<'a> {
    fn new(
        store: &'a ObjectStore,
        lease: &'a AttemptLease,
        pages: &MapRoot,
    ) -> Result<Self, CoreError> {
        let mut cursor = store.map_cursor(pages, &[], None, None)?;
        let ready = cursor.next()?;
        Ok(Self {
            store,
            lease,
            cursor,
            ready,
            path_rows: 0,
            content_rows: 0,
            failed: false,
            page_record_encoded_bytes: 0,
            record_encoded_bytes: 0,
        })
    }

    fn next_edit(&mut self) -> Result<Option<MapEdit>, CoreError> {
        let Some((first_key, _)) = &self.ready else {
            return Ok(None);
        };
        let prefix: [u8; 4] = first_key
            .get(..4)
            .ok_or_else(|| corrupt("run page key lacks gram prefix"))?
            .try_into()
            .map_err(|_width| corrupt("run page gram prefix width differs"))?;
        let mut count = 0_u64;
        while self
            .ready
            .as_ref()
            .is_some_and(|(key, _)| key.get(..4) == Some(prefix.as_slice()))
        {
            let (key, reference) = self
                .ready
                .take()
                .ok_or_else(|| corrupt("run count page disappeared"))?;
            if key.len() != 12 || reference.root.kind != ObjectKind::Record {
                return Err(corrupt("run count page key or kind differs"));
            }
            let page = RunPostingPage::decode(
                &self.store.read_blob(
                    &reference,
                    u64::try_from(MAX_POSTING_PAGE_BYTES)
                        .map_err(|_width| corrupt("posting page limit width"))?,
                    None,
                )?,
            )?;
            if page.key()?.as_slice() != key {
                return Err(corrupt("run count page key differs"));
            }
            self.page_record_encoded_bytes = self
                .page_record_encoded_bytes
                .checked_add(reference.root.bytes)
                .ok_or_else(|| invalid("run page Record bytes overflow"))?;
            count = count
                .checked_add(
                    u64::try_from(page.rows.len()).map_err(|_width| invalid("page count width"))?,
                )
                .ok_or_else(|| invalid("run gram count overflow"))?;
            self.ready = self.cursor.next()?;
        }
        let total = if prefix[0] == 1 {
            &mut self.path_rows
        } else if prefix[0] == 2 {
            &mut self.content_rows
        } else {
            return Err(corrupt("run gram surface differs"));
        };
        *total = total
            .checked_add(count)
            .ok_or_else(|| invalid("run posting total overflow"))?;
        let gram = [prefix[1], prefix[2], prefix[3]];
        let record = RunGramCount {
            surface: prefix[0],
            gram,
            count,
        };
        let value = self
            .store
            .put_blob(self.lease, ObjectKind::Record, &[], &record.encode()?)?;
        self.record_encoded_bytes = self
            .record_encoded_bytes
            .checked_add(value.root.bytes)
            .ok_or_else(|| invalid("merged gram-count Record bytes overflow"))?;
        Ok(Some(MapEdit {
            key: prefix.to_vec(),
            value: Some(value),
        }))
    }
}

impl Iterator for RunGramCountStream<'_> {
    type Item = Result<MapEdit, CoreError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        match self.next_edit() {
            Ok(Some(edit)) => Some(Ok(edit)),
            Ok(None) => None,
            Err(error) => {
                self.failed = true;
                Some(Err(error))
            }
        }
    }
}

/// Charge bounded scratch for one fresh-run construction stage.
///
/// Conservative decoded-memory charge for fresh-run bootstrap.
///
/// The cap covers the simultaneously live source cursors, their decoded map
/// frames and cached physical posting pages, one gram's bounded bucket rows,
/// intermediate page roots, and map-build/cache overhead. A 64 KiB encoded
/// `MapPage` can decode into more than 64 KiB of Vec/row storage, so the frame
/// charge is deliberately based on decoded fanout and key bounds.
fn new_run_stage_charge() -> Result<u64, CoreError> {
    let record_bytes = u64::try_from(MAX_RECORD_BYTES).map_err(|_width| invalid("record width"))?;
    let posting_bytes =
        u64::try_from(MAX_POSTING_PAGE_BYTES).map_err(|_width| invalid("posting width"))?;
    let bucket_bytes =
        u64::try_from(BUCKET_ROW_CHARGE).map_err(|_width| invalid("bucket width"))?;
    let frames = u64::from(MAX_MAP_HEIGHT)
        .checked_add(1)
        .and_then(|levels| levels.checked_mul(DECODED_MAP_FRAME_CHARGE))
        .ok_or_else(|| invalid("new run cursor frame charge overflow"))?;
    let source = frames
        .checked_add(record_bytes)
        // One decoded cached page and one transient physical page payload.
        .and_then(|bytes| bytes.checked_add(posting_bytes.checked_mul(2)?))
        .ok_or_else(|| invalid("new run source charge overflow"))?;
    u64::try_from(NEW_RUN_SEGMENT_FAN_IN)
        .map_err(|_width| invalid("new run fan-in width"))?
        .checked_mul(source)
        .and_then(|bytes| bytes.checked_add(MAP_BUILD_AND_CACHE_CHARGE))
        .and_then(|bytes| bytes.checked_add(CURSOR_AND_REF_CHARGE))
        .and_then(|bytes| bytes.checked_add(PARTIAL_ROOT_CHARGE))
        .and_then(|bytes| bytes.checked_add(SOURCE_ID_CHARGE))
        .and_then(|bytes| bytes.checked_add(bucket_bytes))
        .ok_or_else(|| invalid("new run staging charge overflow"))
}

/// Build a fresh run from immutable leaves without repeated posting-map mutation.
///
/// Intermediate maps are attempt-local and never appear in the published run
/// registry or root; all writes retain the caller's lease.
pub(crate) fn bootstrap_new_run_postings(
    store: &ObjectStore,
    lease: &AttemptLease,
    run_id: u64,
    references: &[&BlobRef],
    max_staging_bytes: u64,
) -> Result<(RunPostingRoot, BlobRef), CoreError> {
    if run_id == 0 || references.is_empty() {
        return Err(invalid("new run ID or leaf set is empty"));
    }
    let reference_count =
        u64::try_from(references.len()).map_err(|_width| invalid("new run leaf count width"))?;
    if reference_count > super::super::policy().source_files {
        return Err(invalid("new run leaf set exceeds source policy"));
    }
    if new_run_stage_charge()? > max_staging_bytes {
        return Err(invalid("new run bootstrap exceeds staging admission"));
    }
    let mut level = Vec::new();
    let mut seen_ids = BTreeSet::new();
    for group in references.chunks(NEW_RUN_SEGMENT_FAN_IN) {
        let mut stream = NewRunPageStream::new(store, lease, group, &mut seen_ids)?;
        let pages = store.map_bootstrap(lease, &mut stream)?;
        level
            .try_reserve(1)
            .map_err(|error| CoreError::Storage(format!("reserve partial run map: {error}")))?;
        level.push(pages);
    }
    while level.len() > 1 {
        let mut next = Vec::new();
        for group in level.chunks(NEW_RUN_PAGE_FAN_IN) {
            let pages = if group.len() == 1 {
                group
                    .first()
                    .ok_or_else(|| corrupt("new run merge group absent"))?
                    .clone()
            } else {
                let mut page_roots = Vec::new();
                page_roots.try_reserve_exact(group.len()).map_err(|error| {
                    CoreError::Storage(format!("reserve partial run roots: {error}"))
                })?;
                page_roots.extend(group.iter());
                let mut stream = RunPageMergeStream::new_page_roots(
                    store,
                    lease,
                    &page_roots,
                    max_staging_bytes,
                )?;
                store.map_bootstrap(lease, &mut stream)?
            };
            next.try_reserve(1)
                .map_err(|error| CoreError::Storage(format!("reserve merged run map: {error}")))?;
            next.push(pages);
        }
        level = next;
    }
    let pages = level
        .pop()
        .ok_or_else(|| corrupt("new run produced no page map"))?;
    let mut counts = RunGramCountStream::new(store, lease, &pages)?;
    let gram_counts = store.map_bootstrap(lease, &mut counts)?;
    let root = RunPostingRoot {
        run_id,
        pages,
        gram_counts,
        path_rows: counts.path_rows,
        content_rows: counts.content_rows,
        page_record_encoded_bytes: counts.page_record_encoded_bytes,
        gram_count_record_encoded_bytes: counts.record_encoded_bytes,
    };
    let reference = store.put_blob(
        lease,
        ObjectKind::Record,
        &root.object_roots(),
        &root.encode()?,
    )?;
    Ok((root, reference))
}

/// Merges authenticated posting dictionaries of complete old runs.
///
/// Leaf segments may be reused unchanged. Run-level page authority and route
/// counts are rebuilt through bounded page streams.
pub(crate) fn merge_run_posting_roots(
    store: &ObjectStore,
    lease: &AttemptLease,
    new_run_id: u64,
    inputs: &[RunPostingRoot],
    max_staging_bytes: u64,
) -> Result<(RunPostingRoot, BlobRef), CoreError> {
    if new_run_id == 0
        || inputs.len() < 2
        || inputs.windows(2).any(|pair| {
            let [left, right] = pair else { return true };
            left.run_id >= right.run_id
        })
        || inputs.iter().any(|root| root.run_id >= new_run_id)
    {
        return Err(invalid("run merge IDs are not sorted old cohorts"));
    }
    let mut page_stream = RunPageMergeStream::new(store, lease, inputs, max_staging_bytes)?;
    let pages = store.map_bootstrap(lease, &mut page_stream)?;
    let page_record_encoded_bytes = page_stream.record_encoded_bytes;
    drop(page_stream);
    let mut count_stream = RunGramCountStream::new(store, lease, &pages)?;
    let gram_counts = store.map_bootstrap(lease, &mut count_stream)?;
    let root = RunPostingRoot {
        run_id: new_run_id,
        pages,
        gram_counts,
        path_rows: count_stream.path_rows,
        content_rows: count_stream.content_rows,
        page_record_encoded_bytes,
        gram_count_record_encoded_bytes: count_stream.record_encoded_bytes,
    };
    let reference = store.put_blob(
        lease,
        ObjectKind::Record,
        &root.object_roots(),
        &root.encode()?,
    )?;
    Ok((root, reference))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexical_store::AttemptBinding;

    fn row(id: u64, digest: u8) -> RunPostingRow {
        RunPostingRow {
            logical_id: id,
            segment_sha256: [digest; 32],
        }
    }

    fn binding() -> AttemptBinding {
        AttemptBinding {
            root_incarnation: [1; 16],
            repository_sha256: [2; 32],
            revision_sha256: [3; 32],
            source_event_sha256: [4; 32],
            target_generation: 1,
            base_root_sha256: None,
            operations_sha256: [5; 32],
            format: 1,
        }
    }

    fn physical_segment(store: &ObjectStore, lease: &AttemptLease, logical_id: u64) -> BlobRef {
        physical_segment_rows(store, lease, vec![logical_id], 1)
    }

    fn physical_segment_rows(
        store: &ObjectStore,
        lease: &AttemptLease,
        doc_ids: Vec<u64>,
        content_mask: u64,
    ) -> BlobRef {
        let count = doc_ids.len();
        let path_mask = if count == 64 {
            u64::MAX
        } else {
            1_u64
                .checked_shl(u32::try_from(count).expect("source count width"))
                .and_then(|mask| mask.checked_sub(1))
                .expect("source mask width")
        };
        let path_page = PostingPage(vec![(*b"abc", path_mask)]);
        let content_page = PostingPage(vec![(*b"xyz", content_mask)]);
        let path_ref = store
            .put_blob(
                lease,
                ObjectKind::FilePostings,
                &[],
                &path_page.encode(count).expect("path page"),
            )
            .expect("path blob");
        let content_ref = store
            .put_blob(
                lease,
                ObjectKind::FilePostings,
                &[],
                &content_page.encode(count).expect("content page"),
            )
            .expect("content blob");
        let path_terms = store
            .map_bootstrap(
                lease,
                std::iter::once(Ok(MapEdit {
                    key: b"abc".to_vec(),
                    value: Some(path_ref),
                })),
            )
            .expect("path term map");
        let content_terms = store
            .map_bootstrap(
                lease,
                std::iter::once(Ok(MapEdit {
                    key: b"xyz".to_vec(),
                    value: Some(content_ref),
                })),
            )
            .expect("content term map");
        let segment = Segment {
            doc_ids,
            path_terms,
            content_terms,
            path_memberships: u64::try_from(count).expect("path count"),
            content_memberships: u64::from(content_mask.count_ones()),
        };
        let deps = [&segment.path_terms, &segment.content_terms]
            .into_iter()
            .filter_map(|root| root.page.as_ref().map(|page| page.object.clone()))
            .collect::<Vec<_>>();
        store
            .put_blob(
                lease,
                ObjectKind::Segment,
                &deps,
                &segment.encode().expect("segment bytes"),
            )
            .expect("segment blob")
    }

    #[test]
    fn run_merge_refuses_decoded_frames_before_loading_or_writing() {
        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let lease = store.begin_attempt(&binding(), &[]).expect("begin attempt");
        let segment = physical_segment(&store, &lease, 1);
        let (root, _) = bootstrap_new_run_postings(
            &store,
            &lease,
            1,
            &[&segment],
            super::super::super::policy().bucket_scratch_bytes,
        )
        .expect("run postings");
        let mut deep = root.pages.clone();
        // The otherwise valid descriptor points to an absent object. Admission
        // must refuse frame storage before attempting this independent read.
        let page = deep.page.as_mut().expect("page descriptor");
        page.height = 12;
        page.object.sha256 = [0x5a; 32];
        let before = lease.write_cost_snapshot().expect("before refused stream");
        let refused =
            RunPageMergeStream::new_page_roots(&store, &lease, &[&deep, &root.pages], 1_000_000);
        assert!(matches!(refused, Err(CoreError::InvalidContract(message))
            if message == "file run postings: run merge exceeds staging packet bound"));
        assert_eq!(lease.write_cost_snapshot().expect("after refusal"), before);
        let shallow = RunPageMergeStream::new_page_roots(
            &store,
            &lease,
            &[&root.pages, &root.pages],
            1_000_000,
        )
        .expect("same fan-in with shallow descriptors is admitted");
        drop(shallow);
        let page = deep.page.as_mut().expect("page descriptor");
        page.height = 13;
        assert!(matches!(run_merge_map_stage_bytes(&[&deep]),
            Err(CoreError::Typed { code, .. })
                if code == quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE));
    }

    #[test]
    fn new_run_bootstrap_preserves_full_masks_and_shuffled_interleaved_leaves() {
        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let lease = store.begin_attempt(&binding(), &[]).expect("begin attempt");
        // Four independently sorted leaves interleave in one full logical-ID
        // bucket. Odd local positions include bit 63 on every source leaf.
        let leaves = (0_u64..4)
            .map(|lane| {
                let ids = (0_u64..64).map(|local| local * 4 + lane + 256).collect();
                physical_segment_rows(&store, &lease, ids, 0xaaaa_aaaa_aaaa_aaaa)
            })
            .collect::<Vec<_>>();
        let reverse = leaves.iter().rev().collect::<Vec<_>>();
        let (root, reference) = bootstrap_new_run_postings(
            &store,
            &lease,
            1,
            &reverse,
            super::super::super::policy().bucket_scratch_bytes,
        )
        .expect("full-mask bootstrap");
        assert_eq!((root.path_rows, root.content_rows), (256, 128));
        assert_eq!((root.pages.entries, root.gram_counts.entries), (2, 2));
        let ordered = leaves.iter().collect::<Vec<_>>();
        let (_, ordered_ref) = bootstrap_new_run_postings(
            &store,
            &lease,
            1,
            &ordered,
            super::super::super::policy().bucket_scratch_bytes,
        )
        .expect("ordered bootstrap");
        assert_eq!(reference, ordered_ref);
        for (surface, gram) in [(1_u8, *b"abc"), (2_u8, *b"xyz")] {
            let key = run_page_key(surface, gram, 256).expect("fixed key");
            let page_ref = store
                .map_get(&root.pages, &key, None)
                .expect("map lookup")
                .expect("posting page");
            let page = RunPostingPage::decode(
                &store
                    .read_blob(
                        &page_ref,
                        u64::try_from(MAX_POSTING_PAGE_BYTES).expect("bound"),
                        None,
                    )
                    .expect("page bytes"),
            )
            .expect("page");
            let expected = (256_u64..=511)
                .filter(|id| surface == 1 || id & 4 != 0)
                .map(|id| RunPostingRow {
                    logical_id: id,
                    segment_sha256: leaves
                        .get(usize::try_from(id & 3).expect("lane width"))
                        .expect("source lane")
                        .root
                        .sha256,
                })
                .collect::<Vec<_>>();
            assert_eq!(page.rows, expected);
        }
    }

    #[test]
    fn new_run_bootstrap_crosses_chunk_and_bucket_with_exact_source_rows() {
        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let lease = store.begin_attempt(&binding(), &[]).expect("begin attempt");
        let leaves = (191_u64..=256)
            .map(|id| (id, physical_segment(&store, &lease, id)))
            .collect::<Vec<_>>();
        let references = leaves
            .iter()
            .map(|(_, reference)| reference)
            .collect::<Vec<_>>();
        let (root, reference) = bootstrap_new_run_postings(
            &store,
            &lease,
            1,
            &references,
            super::super::super::policy().bucket_scratch_bytes,
        )
        .expect("multi-pass new run");
        assert_eq!(root.pages.entries, 4);
        assert_eq!(root.gram_counts.entries, 2);
        assert_eq!(root.path_rows, 66);
        assert_eq!(root.content_rows, 66);
        assert_eq!(
            RunPostingRoot::load(&store, &reference, None).expect("reload"),
            root
        );
        for (surface, gram) in [(1_u8, *b"abc"), (2_u8, *b"xyz")] {
            for (first, last) in [(191_u64, 255_u64), (256_u64, 256_u64)] {
                let page_ref = store
                    .map_get(
                        &root.pages,
                        &run_page_key(surface, gram, first).expect("key"),
                        None,
                    )
                    .expect("page map")
                    .expect("page ref");
                let page = RunPostingPage::decode(
                    &store
                        .read_blob(
                            &page_ref,
                            u64::try_from(MAX_POSTING_PAGE_BYTES).expect("bound"),
                            None,
                        )
                        .expect("page bytes"),
                )
                .expect("page");
                let expected = leaves
                    .iter()
                    .filter(|(id, _)| *id >= first && *id <= last)
                    .map(|(id, source)| RunPostingRow {
                        logical_id: *id,
                        segment_sha256: source.root.sha256,
                    })
                    .collect::<Vec<_>>();
                assert_eq!(page.rows, expected);
            }
            let count_key = [surface, gram[0], gram[1], gram[2]];
            let count_ref = store
                .map_get(&root.gram_counts, &count_key, None)
                .expect("count map")
                .expect("count ref");
            let count = RunGramCount::decode(
                &store
                    .read_blob(
                        &count_ref,
                        u64::try_from(MAX_RECORD_BYTES).expect("bound"),
                        None,
                    )
                    .expect("count bytes"),
            )
            .expect("count");
            assert_eq!(count.count, 66);
        }
        let mut page_bytes = 0_u64;
        store
            .map_scan(&root.pages, &[], None, None, &mut |_key, value| {
                page_bytes = page_bytes
                    .checked_add(value.root.bytes)
                    .expect("page byte sum");
                Ok(())
            })
            .expect("page census");
        assert_eq!(root.page_record_encoded_bytes, page_bytes);
    }

    #[test]
    fn new_run_bootstrap_retires_partial_pages_without_collecting_retained_run() {
        use crate::lexical_gc::collect_shared_pool;

        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let original = store
            .begin_attempt(&binding(), &[])
            .expect("original attempt");
        let leaves = (191_u64..=256)
            .map(|id| (id, physical_segment(&store, &original, id)))
            .collect::<Vec<_>>();
        let references = leaves
            .iter()
            .map(|(_, reference)| reference)
            .collect::<Vec<_>>();
        let (root, final_ref) = bootstrap_new_run_postings(
            &store,
            &original,
            1,
            &references,
            super::super::super::policy().bucket_scratch_bytes,
        )
        .expect("two-partial bootstrap");
        assert_eq!((root.pages.entries, root.gram_counts.entries), (4, 2));
        let expected_partial_page = RunPostingPage::new(
            1,
            *b"abc",
            0,
            leaves
                .iter()
                .take(64)
                .map(|(id, reference)| RunPostingRow {
                    logical_id: *id,
                    segment_sha256: reference.root.sha256,
                })
                .collect(),
        )
        .expect("fixed partial page");
        let partial_bytes = expected_partial_page.encode().expect("partial bytes");
        let adopted = store.adopted_roots().expect("original adopted roots");
        let before = original
            .write_cost_snapshot()
            .expect("before fixed-page probe");
        let partial = store
            .put_blob(&original, ObjectKind::Record, &[], &partial_bytes)
            .expect("already-written fixed intermediate page");
        let after = original
            .write_cost_snapshot()
            .expect("after fixed-page probe");
        assert_eq!(after.new_objects, before.new_objects);
        assert_eq!(after.new_encoded_bytes, before.new_encoded_bytes);
        assert!(adopted.contains(&partial.root));
        let partial_len = u64::try_from(partial_bytes.len()).expect("partial length");
        assert_eq!(
            RunPostingPage::decode(
                &store
                    .read_blob(&partial, partial_len, None)
                    .expect("authenticated intermediate page")
            )
            .expect("intermediate posting page"),
            expected_partial_page
        );
        let mut protected = leaves
            .iter()
            .map(|(_, reference)| reference.root.clone())
            .collect::<Vec<_>>();
        protected.push(final_ref.root.clone());
        assert_eq!(protected.len(), 67);
        let wrapper = store
            .put_blob(
                &original,
                ObjectKind::Record,
                &protected,
                b"gc-retained-new-run-and-source-leaves-v1",
            )
            .expect("custody wrapper");
        let mut next_binding = binding();
        next_binding.target_generation = 2;
        let retained = store
            .begin_attempt(&next_binding, std::slice::from_ref(&wrapper.root))
            .expect("independent retained attempt");
        original
            .discard_unsealed()
            .expect("retire original attempt");
        let outcome =
            collect_shared_pool(&store, &[], store.gc_epoch().expect("current epoch"), None)
                .expect("actual shared-pool collection");
        assert!(matches!(
            outcome,
            crate::lexical_gc::SharedPoolGcOutcome::Completed { .. }
        ));
        assert!(store.read_blob(&partial, partial_len, None).is_err());
        assert_eq!(
            RunPostingRoot::load(&store, &final_ref, None).expect("retained final root"),
            root
        );
        let final_bucket_zero = store
            .map_get(
                &root.pages,
                &run_page_key(1, *b"abc", 191).expect("bucket key"),
                None,
            )
            .expect("retained page map")
            .expect("retained bucket zero");
        let page = RunPostingPage::decode(
            &store
                .read_blob(
                    &final_bucket_zero,
                    u64::try_from(MAX_POSTING_PAGE_BYTES).expect("posting bound"),
                    None,
                )
                .expect("retained page bytes"),
        )
        .expect("retained final page");
        let expected_final = leaves
            .iter()
            .take(65)
            .map(|(id, reference)| RunPostingRow {
                logical_id: *id,
                segment_sha256: reference.root.sha256,
            })
            .collect::<Vec<_>>();
        assert_eq!(page.rows, expected_final);
        assert_eq!((root.path_rows, root.content_rows), (66, 66));
        for (id, source) in &leaves {
            let bytes = store
                .read_blob(
                    source,
                    u64::try_from(MAX_RECORD_BYTES).expect("record bound"),
                    None,
                )
                .expect("protected source bytes");
            let segment = Segment::decode(&bytes).expect("protected source segment");
            assert_eq!(segment.doc_ids, vec![*id]);
        }
        retained
            .discard_unsealed()
            .expect("retire retained attempt");
    }

    #[test]
    fn new_run_bootstrap_crosses_second_merge_pass_with_exact_rows() {
        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let lease = store.begin_attempt(&binding(), &[]).expect("begin attempt");
        let first = physical_segment(&store, &lease, 1);
        let template = Segment::decode(
            &store
                .read_blob(
                    &first,
                    u64::try_from(MAX_RECORD_BYTES).expect("record bound"),
                    None,
                )
                .expect("first segment bytes"),
        )
        .expect("first segment");
        let dependencies = [&template.path_terms, &template.content_terms]
            .into_iter()
            .filter_map(|root| root.page.as_ref().map(|page| page.object.clone()))
            .collect::<Vec<_>>();
        let mut leaves = vec![(1_u64, first)];
        for id in 2_u64..=1025 {
            let mut segment = template.clone();
            segment.doc_ids = vec![id];
            let reference = store
                .put_blob(
                    &lease,
                    ObjectKind::Segment,
                    &dependencies,
                    &segment.encode().expect("segment bytes"),
                )
                .expect("segment blob");
            leaves.push((id, reference));
        }
        let references = leaves
            .iter()
            .map(|(_, reference)| reference)
            .collect::<Vec<_>>();
        let (root, _reference) = bootstrap_new_run_postings(
            &store,
            &lease,
            7,
            &references,
            super::super::super::policy().bucket_scratch_bytes,
        )
        .expect("seventeen partial maps require two merge rounds");
        assert_eq!(root.pages.entries, 10);
        assert_eq!(root.gram_counts.entries, 2);
        assert_eq!((root.path_rows, root.content_rows), (1025, 1025));
        let expected = leaves
            .iter()
            .map(|(id, reference)| (*id, reference.root.sha256))
            .collect::<BTreeMap<_, _>>();
        let mut seen = BTreeMap::<(u8, u64), [u8; 32]>::new();
        store
            .map_scan(&root.pages, &[], None, None, &mut |key, reference| {
                let page = RunPostingPage::decode(&store.read_blob(
                    reference,
                    u64::try_from(MAX_POSTING_PAGE_BYTES).expect("posting bound"),
                    None,
                )?)?;
                assert_eq!(page.key()?.as_slice(), key);
                for row in page.rows {
                    assert_eq!(Some(&row.segment_sha256), expected.get(&row.logical_id));
                    assert!(
                        seen.insert((page.surface, row.logical_id), row.segment_sha256)
                            .is_none()
                    );
                }
                Ok(())
            })
            .expect("complete page census");
        for surface in [1_u8, 2] {
            for id in 1_u64..=1025 {
                assert_eq!(seen.get(&(surface, id)), expected.get(&id));
            }
        }
        assert_eq!(seen.len(), 2050);
    }

    #[test]
    fn new_run_bootstrap_accepts_empty_leaf_term_maps() {
        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let lease = store.begin_attempt(&binding(), &[]).expect("begin attempt");
        let empty = Segment {
            doc_ids: vec![1],
            path_terms: MapRoot::default(),
            content_terms: MapRoot::default(),
            path_memberships: 0,
            content_memberships: 0,
        };
        let empty_ref = store
            .put_blob(
                &lease,
                ObjectKind::Segment,
                &[],
                &empty.encode().expect("empty segment bytes"),
            )
            .expect("empty segment blob");
        let (empty_root, _reference) = bootstrap_new_run_postings(
            &store,
            &lease,
            1,
            &[&empty_ref],
            super::super::super::policy().bucket_scratch_bytes,
        )
        .expect("empty run posting root");
        assert_eq!(
            (empty_root.pages.entries, empty_root.gram_counts.entries),
            (0, 0)
        );
        assert_eq!((empty_root.path_rows, empty_root.content_rows), (0, 0));
        let present_ref = physical_segment(&store, &lease, 2);
        let (mixed_root, _reference) = bootstrap_new_run_postings(
            &store,
            &lease,
            2,
            &[&empty_ref, &present_ref],
            super::super::super::policy().bucket_scratch_bytes,
        )
        .expect("mixed empty and present leaves");
        assert_eq!((mixed_root.path_rows, mixed_root.content_rows), (1, 1));
        assert_eq!(mixed_root.pages.entries, 2);
    }

    #[test]
    fn new_run_bootstrap_refuses_under_admission_before_writes() {
        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let lease = store.begin_attempt(&binding(), &[]).expect("begin attempt");
        let leaf = physical_segment(&store, &lease, 1);
        let before = lease.write_cost_snapshot().expect("before cost");
        let cap = new_run_stage_charge().expect("stage charge");
        let below = cap.checked_sub(1).expect("positive stage charge");
        assert!(bootstrap_new_run_postings(&store, &lease, 1, &[&leaf], below).is_err());
        assert_eq!(lease.write_cost_snapshot().expect("after cost"), before);
    }

    #[test]
    fn new_run_bootstrap_refuses_duplicate_ids_across_chunks() {
        use crate::lexical_gc::collect_shared_pool;

        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let keeper = store
            .begin_attempt(&binding(), &[])
            .expect("retained attempt");
        let retained = store
            .put_blob(&keeper, ObjectKind::Record, &[], b"retained-prior")
            .expect("retained prior object");
        let mut failed_binding = binding();
        failed_binding.target_generation = 2;
        let lease = store
            .begin_attempt(&failed_binding, &[])
            .expect("begin attempt");
        let mut leaves = (192_u64..=255)
            .map(|id| physical_segment(&store, &lease, id))
            .collect::<Vec<_>>();
        let duplicate = leaves.first().expect("first leaf").clone();
        leaves.push(duplicate);
        let references = leaves.iter().collect::<Vec<_>>();
        let before = lease.write_cost_snapshot().expect("before bootstrap");
        let error = bootstrap_new_run_postings(
            &store,
            &lease,
            1,
            &references,
            super::super::super::policy().bucket_scratch_bytes,
        )
        .expect_err("duplicate must refuse final run");
        assert!(matches!(error, CoreError::Typed { code, .. }
            if code == quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE));
        let after = lease
            .write_cost_snapshot()
            .expect("after refused bootstrap");
        assert!(after.new_objects > before.new_objects);
        let expected_partial = RunPostingPage::new(
            1,
            *b"abc",
            0,
            leaves
                .iter()
                .take(64)
                .enumerate()
                .map(|(index, leaf)| RunPostingRow {
                    logical_id: 192 + u64::try_from(index).expect("logical ID"),
                    segment_sha256: leaf.root.sha256,
                })
                .collect(),
        )
        .expect("fixed partial page")
        .encode()
        .expect("partial bytes");
        let partial = store
            .put_blob(&lease, ObjectKind::Record, &[], &expected_partial)
            .expect("already-installed intermediate");
        assert_eq!(
            lease
                .write_cost_snapshot()
                .expect("after lookup")
                .new_objects,
            after.new_objects
        );
        lease.discard_unsealed().expect("discard failed attempt");
        let outcome = collect_shared_pool(&store, &[], store.gc_epoch().expect("epoch"), None)
            .expect("collect failed-attempt objects");
        assert!(matches!(
            outcome,
            crate::lexical_gc::SharedPoolGcOutcome::Completed { .. }
        ));
        assert!(
            store
                .read_blob(
                    &partial,
                    u64::try_from(MAX_POSTING_PAGE_BYTES).expect("posting bound"),
                    None
                )
                .is_err()
        );
        for leaf in &leaves {
            assert!(
                store
                    .read_blob(
                        leaf,
                        u64::try_from(MAX_RECORD_BYTES).expect("segment bound"),
                        None
                    )
                    .is_err()
            );
        }
        assert_eq!(
            store
                .read_blob(&retained, 64, None)
                .expect("protected prior"),
            b"retained-prior"
        );
        keeper.discard_unsealed().expect("retire retained attempt");
    }

    #[test]
    fn authenticated_prior_page_is_reused_for_exact_delta_and_refuses_forgery() {
        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("open store");
        let lease = store.begin_attempt(&binding(), &[]).expect("begin attempt");
        let key = run_page_key(2, *b"abc", 1).expect("bucket key").to_vec();
        let old = RunPostingPage::new(2, *b"abc", 0, vec![row(1, 1), row(3, 1)]).expect("old page");
        let (base, _root_ref) = apply_run_page_edits(
            &store,
            &lease,
            None,
            1,
            BTreeMap::from([(
                key.clone(),
                RunPostingPageEdit {
                    prior: None,
                    next: Some(old),
                },
            )]),
        )
        .expect("initial page");
        assert_eq!(base.path_rows, 0);
        assert_eq!(base.content_rows, 2);
        let old_ref = store
            .map_get(&base.pages, &key, None)
            .expect("old page lookup")
            .expect("old page reference");
        let authenticated = RunPostingPage::decode(
            &store
                .read_blob(
                    &old_ref,
                    u64::try_from(MAX_POSTING_PAGE_BYTES).expect("bound"),
                    None,
                )
                .expect("old page bytes"),
        )
        .expect("old page decode");
        let next = apply_page_delta(
            2,
            *b"abc",
            0,
            Some(&authenticated),
            &[row(1, 1)],
            &[row(2, 2)],
        )
        .expect("delta")
        .expect("target page");
        let (target, _root_ref) = apply_run_page_edits(
            &store,
            &lease,
            Some(&base),
            1,
            BTreeMap::from([(
                key.clone(),
                RunPostingPageEdit {
                    prior: Some((old_ref.clone(), authenticated.clone())),
                    next: Some(next.clone()),
                },
            )]),
        )
        .expect("exact changed page");
        assert_eq!(target.path_rows, 0);
        assert_eq!(target.content_rows, 2);
        let target_ref = store
            .map_get(&target.pages, &key, None)
            .expect("target page lookup")
            .expect("target page reference");
        assert_eq!(target.page_record_encoded_bytes, target_ref.root.bytes);
        assert_eq!(
            RunPostingPage::decode(
                &store
                    .read_blob(
                        &target_ref,
                        u64::try_from(MAX_POSTING_PAGE_BYTES).expect("bound"),
                        None
                    )
                    .expect("target page bytes")
            )
            .expect("target page decode")
            .rows,
            vec![row(2, 2), row(3, 1)]
        );
        let count_ref = store
            .map_get(
                &target.gram_counts,
                &run_page_prefix(2, *b"abc").expect("gram key"),
                None,
            )
            .expect("gram count lookup")
            .expect("gram count reference");
        assert_eq!(
            RunGramCount::decode(
                &store
                    .read_blob(
                        &count_ref,
                        u64::try_from(MAX_RECORD_BYTES).expect("bound"),
                        None
                    )
                    .expect("gram count bytes")
            )
            .expect("gram count decode")
            .count,
            2
        );

        let forged =
            RunPostingPage::new(2, *b"abc", 0, vec![row(1, 9), row(3, 1)]).expect("forged page");
        let wrong_ref = store
            .put_blob(
                &lease,
                ObjectKind::Record,
                &[],
                &forged.encode().expect("forged bytes"),
            )
            .expect("forged reference");
        for (reference, page) in [(wrong_ref, authenticated), (old_ref, forged)] {
            assert!(
                apply_run_page_edits(
                    &store,
                    &lease,
                    Some(&base),
                    1,
                    BTreeMap::from([(
                        key.clone(),
                        RunPostingPageEdit {
                            prior: Some((reference, page)),
                            next: Some(next.clone()),
                        },
                    )]),
                )
                .is_err()
            );
        }
        let wrong_key_page =
            RunPostingPage::new(2, *b"xyz", 0, vec![row(1, 1), row(3, 1)]).expect("wrong-key page");
        let wrong_key_ref = store
            .put_blob(
                &lease,
                ObjectKind::Record,
                &[],
                &wrong_key_page.encode().expect("wrong-key bytes"),
            )
            .expect("wrong-key reference");
        let mut bad_base = base.clone();
        bad_base.pages = store
            .map_apply(
                &lease,
                &base.pages,
                std::iter::once(Ok(MapEdit {
                    key: key.clone(),
                    value: Some(wrong_key_ref.clone()),
                })),
            )
            .expect("forged map value");
        assert!(
            apply_run_page_edits(
                &store,
                &lease,
                Some(&bad_base),
                1,
                BTreeMap::from([(
                    key,
                    RunPostingPageEdit {
                        prior: Some((wrong_key_ref, wrong_key_page)),
                        next: Some(next),
                    },
                )]),
            )
            .is_err()
        );
    }

    #[test]
    fn fixed_keys_and_bucket_boundary() {
        assert_eq!(
            run_page_key(2, *b"abc", 255).expect("first bucket"),
            [2, b'a', b'b', b'c', 0, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            run_page_key(2, *b"abc", 256).expect("second bucket"),
            [2, b'a', b'b', b'c', 0, 0, 0, 0, 0, 0, 0, 1]
        );
        assert_eq!(
            run_route_key(1, *b"xyz", 3, 7).expect("route"),
            [
                1, b'x', b'y', b'z', 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 7,
            ]
        );
        let count = RunGramCount {
            surface: 2,
            gram: *b"abc",
            count: 513,
        };
        assert_eq!(count.key().expect("count key"), [2, b'a', b'b', b'c']);
        assert_eq!(
            RunGramCount::decode(&count.encode().expect("count bytes")).expect("count decode"),
            count
        );
        assert!(RunGramCount { count: 0, ..count }.encode().is_err());
    }

    #[test]
    fn canonical_page_and_exact_independent_union() {
        let first =
            RunPostingPage::new(2, *b"abc", 1, vec![row(256, 1), row(259, 1)]).expect("first page");
        let second = RunPostingPage::new(2, *b"abc", 1, vec![row(257, 2)]).expect("second page");
        let merged = merge_page_bucket(&[&first, &second]).expect("merged page");
        assert_eq!(merged.rows, vec![row(256, 1), row(257, 2), row(259, 1)]);
        verify_page_union(&[&first, &second], &merged).expect("independent union");
        let bytes = merged.encode().expect("encoded page");
        assert_eq!(
            RunPostingPage::decode(&bytes).expect("decoded page"),
            merged
        );
        let mut trailing = bytes;
        trailing.push(0);
        assert!(RunPostingPage::decode(&trailing).is_err());
        let missing = RunPostingPage::new(2, *b"abc", 1, vec![row(256, 1), row(259, 1)])
            .expect("missing row page");
        assert!(verify_page_union(&[&first, &second], &missing).is_err());
        let extra = RunPostingPage::new(
            2,
            *b"abc",
            1,
            vec![row(256, 1), row(257, 2), row(258, 3), row(259, 1)],
        )
        .expect("extra row page");
        assert!(verify_page_union(&[&first, &second], &extra).is_err());
    }

    #[test]
    fn duplicate_ids_and_wrong_locator_are_refused() {
        let first = RunPostingPage::new(1, *b"abc", 0, vec![row(1, 1)]).expect("first page");
        let duplicate =
            RunPostingPage::new(1, *b"abc", 0, vec![row(1, 2)]).expect("duplicate page");
        assert!(merge_page_bucket(&[&first, &duplicate]).is_err());
        let forged =
            RunPostingPage::new(1, *b"abc", 0, vec![row(1, 3)]).expect("forged locator page");
        assert!(verify_page_union(&[&first], &forged).is_err());
        assert!(RunPostingPage::new(1, *b"abc", 0, vec![row(1, 1), row(1, 1)]).is_err());
        assert!(RunPostingPage::new(1, *b"abc", 0, vec![row(256, 1)]).is_err());
    }

    #[test]
    fn changed_leaf_delta_updates_only_its_bucket() {
        let old =
            RunPostingPage::new(2, *b"abc", 0, vec![row(1, 1), row(3, 1)]).expect("base page");
        let target = apply_page_delta(2, *b"abc", 0, Some(&old), &[row(1, 1)], &[row(2, 2)])
            .expect("changed bucket")
            .expect("nonempty target");
        assert_eq!(target.rows, vec![row(2, 2), row(3, 1)]);
        assert!(apply_page_delta(2, *b"abc", 0, Some(&old), &[row(1, 2)], &[]).is_err());
        assert!(apply_page_delta(2, *b"abc", 0, Some(&old), &[], &[row(3, 2)]).is_err());
        assert!(apply_page_delta(2, *b"abc", 0, Some(&old), &[], &[row(256, 2)]).is_err());
        assert!(
            apply_page_delta(2, *b"abc", 0, Some(&old), &[row(1, 1), row(3, 1)], &[])
                .expect("all retired")
                .is_none()
        );
    }
}
